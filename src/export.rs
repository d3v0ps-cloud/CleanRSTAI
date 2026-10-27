//! Export settings and conversations to standalone JSON files.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::config::{KeyValue, McpTransportConfig, Settings};
use crate::types::Conversation;

const FORMAT_VERSION: u32 = 1;

fn envelope(kind: &str, key: &str, payload: Value) -> Value {
    json!({
        "app": "CleanRSTAI",
        "app_version": env!("CARGO_PKG_VERSION"),
        "format": kind,
        "format_version": FORMAT_VERSION,
        "exported_at": chrono::Utc::now().to_rfc3339(),
        key: payload,
    })
}

/// Blank out values that commonly hold credentials: provider API keys, provider and MCP HTTP
/// header values, and MCP stdio environment variable values. Keys/names are kept so the
/// structure can be re-filled after import.
pub fn redact_secrets(settings: &mut Settings) {
    for p in &mut settings.providers {
        p.api_key.clear();
        for h in &mut p.extra_headers {
            h.value.clear();
        }
    }
    for s in &mut settings.mcp_servers {
        match &mut s.transport {
            McpTransportConfig::Stdio { env, .. } => env.iter_mut().for_each(|kv| kv.value.clear()),
            McpTransportConfig::Http { headers, .. } => {
                headers.iter_mut().for_each(|kv| kv.value.clear())
            }
        }
    }
}

pub fn settings_document(settings: &Settings, include_secrets: bool) -> Value {
    let mut settings = settings.clone();
    if !include_secrets {
        redact_secrets(&mut settings);
    }
    let mut doc = envelope("settings", "settings", json!(settings));
    doc["includes_secrets"] = json!(include_secrets);
    doc
}

pub fn conversations_document(conversations: &[Conversation]) -> Value {
    envelope("conversations", "conversations", json!(conversations))
}

pub fn write_document(path: &Path, doc: &Value) -> Result<()> {
    // Private permissions: exports can contain API keys or private chats.
    crate::config::write_private(path, &serde_json::to_vec_pretty(doc)?)
}

/// Ask for a destination with the native save dialog. `None` if the user cancelled.
pub fn pick_save_path(default_name: &str) -> Option<std::path::PathBuf> {
    pick_save_path_as(default_name, "JSON", "json")
}

pub fn pick_save_path_as(
    default_name: &str,
    filter: &str,
    ext: &str,
) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Export")
        .set_file_name(default_name)
        .add_filter(filter, &[ext]);
    if let Some(dirs) = directories::UserDirs::new()
        && let Some(docs) = dirs.document_dir()
    {
        dialog = dialog.set_directory(docs);
    }
    dialog.save_file().map(|p| {
        if p.extension().is_none() {
            p.with_extension(ext)
        } else {
            p
        }
    })
}

pub fn default_file_name(kind: &str) -> String {
    format!(
        "cleanrstai-{kind}-{}.json",
        chrono::Local::now().format("%Y-%m-%d")
    )
}

/// A file name derived from a chat title.
pub fn safe_file_name(title: &str, ext: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    let base = if cleaned.is_empty() { "chat" } else { cleaned };
    format!("{}.{ext}", base.chars().take(80).collect::<String>())
}

/// Render one conversation as a Markdown document. Images are embedded as data URLs.
pub fn conversation_markdown(c: &Conversation) -> String {
    use crate::types::Role;
    use std::fmt::Write as _;

    fn fence(text: &str) -> String {
        // Use a fence longer than any backtick run in the text.
        let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
        "`".repeat((longest + 1).max(3))
    }
    fn attachments(out: &mut String, list: &[crate::attach::Attachment]) {
        for a in list {
            if let Some(url) = a.data_url().filter(|_| a.is_image()) {
                let _ = writeln!(out, "![{}]({url})\n", a.name);
            } else if let Some(t) = &a.text {
                let f = fence(t);
                let _ = writeln!(out, "**{}**\n\n{f}\n{}\n{f}\n", a.name, t.trim_end());
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", c.title);
    let _ = writeln!(
        out,
        "_Exported from CleanRSTAI on {} · started {}_\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        c.created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
    );
    for (idx, m) in c.messages.iter().enumerate() {
        match m.role {
            Role::User => {
                let _ = writeln!(out, "## You\n\n{}\n", m.content.trim_end());
                attachments(&mut out, &m.attachments);
            }
            Role::Assistant => {
                let _ = writeln!(out, "## Assistant\n");
                if let Some(r) = m.reasoning.as_deref().filter(|r| !r.trim().is_empty()) {
                    let _ = writeln!(
                        out,
                        "<details><summary>Thinking</summary>\n\n{}\n\n</details>\n",
                        r.trim_end()
                    );
                }
                if !m.content.is_empty() {
                    let _ = writeln!(out, "{}\n", m.content.trim_end());
                }
                let results = crate::types::results_after(&c.messages, idx);
                for call in &m.tool_calls {
                    let args = serde_json::from_str::<Value>(&call.arguments)
                        .ok()
                        .and_then(|v| serde_json::to_string_pretty(&v).ok())
                        .unwrap_or_else(|| call.arguments.clone());
                    let _ = writeln!(
                        out,
                        "**Tool call:** `{}`\n\n```json\n{args}\n```\n",
                        call.name
                    );
                    if let Some(r) = results.get(call.id.as_str()) {
                        let f = fence(&r.content);
                        let label = if r.is_error {
                            "Result (error)"
                        } else {
                            "Result"
                        };
                        let _ = writeln!(out, "{label}:\n\n{f}\n{}\n{f}\n", r.content.trim_end());
                        attachments(&mut out, &r.attachments);
                    }
                }
            }
            Role::Tool | Role::System => {}
        }
    }
    out
}

/// Escape the few characters that are special in HTML text and attribute values.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render the conversation's Markdown to the HTML body, dropping markup that could run code
/// or load content in the shared file. A reply, a fetched page or an attachment could carry a
/// `<script>`, an event handler, an `<iframe>` or a `javascript:` link, so instead of regexing
/// the rendered text (which a crafted tag can defeat) the parser's event stream is filtered:
/// raw HTML is dropped outright, and every link and image destination is checked against a
/// scheme allow-list before it is emitted. Code blocks and ordinary text are escaped by the
/// renderer, so only real markup reaches the output.
fn render_body(markdown: &str) -> String {
    use pulldown_cmark::{CowStr, Event, Tag};

    let options = crate::artifacts::markdown_options();
    let parser = pulldown_cmark::Parser::new_ext(markdown, options);

    // Filter the event stream, then hand the whole result to the renderer in one pass: the
    // renderer consumes an image's inner text from the same iterator to build its `alt`, so the
    // events it sees must be a single, complete sequence. Raw HTML is dropped; an unsafe link or
    // image destination is rewritten to an inert `#`, so the tag still renders but can neither
    // run code nor load anything.
    let events: Vec<Event<'static>> = parser
        .filter_map(|event| match event {
            // The only raw HTML kept is the app's own <details> wrapper.
            Event::Html(ref html) if trusted_details_fragment(html) => Some(event.into_static()),
            // Any other raw HTML — <script>, <img onerror>, <a href="javascript:">, <iframe> …
            // — is dropped, so it can neither run nor load anything.
            Event::Html(_) | Event::InlineHtml(_) => None,
            // An unsafe link or image destination is rewritten to an inert `#`.
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) if !url_is_safe(&dest_url, false) => Some(
                Event::Start(Tag::Link {
                    link_type,
                    dest_url: CowStr::Borrowed("#"),
                    title,
                    id,
                })
                .into_static(),
            ),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) if !url_is_safe(&dest_url, true) => Some(
                Event::Start(Tag::Image {
                    link_type,
                    dest_url: CowStr::Borrowed("#"),
                    title,
                    id,
                })
                .into_static(),
            ),
            other => Some(other.into_static()),
        })
        .collect();

    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events.into_iter());
    out
}

/// The only raw HTML fragments the Markdown carries are the `<details>`/`</details>` pair that
/// wraps the model's reasoning. Everything else is untrusted and dropped.
fn trusted_details_fragment(html: &str) -> bool {
    let t = html.trim();
    t == "<details><summary>Thinking</summary>" || t == "</details>"
}

/// Whether a link or image destination may be emitted. Links may use `http(s):` or `mailto:`;
/// images may only be embedded `data:image/…` URLs of the raster types the app itself attaches
/// (PNG, JPEG, GIF, WebP). Everything else — `javascript:`, `file:`, `data:text/html`,
/// executable `data:image/svg+xml`, and any remote image — is dropped, so the shared file never
/// reaches the network. Leading control characters are stripped first, matching how a browser
/// reads an `href`, so they cannot hide a scheme.
fn url_is_safe(dest_url: &str, is_image: bool) -> bool {
    let u: String = dest_url
        .chars()
        .skip_while(|c| c.is_ascii_control() || c.is_ascii_whitespace())
        .collect();
    let lower = u.to_ascii_lowercase();
    // The scheme is the run before the first `:`, provided that run holds no `/`, `?` or `#`
    // (a `:` after any of those is not a scheme separator, as in `a/b:c`).
    let scheme = u
        .find(':')
        .filter(|&i| !u[..i].contains(['/', '?', '#']))
        .map_or("", |i| &u[..i]);
    let scheme = scheme.to_ascii_lowercase();
    if is_image {
        // Only embedded raster images, exactly the types the app attaches. The media type is
        // the run after `data:` up to the first `;` or `,`, so a longer type (or
        // `data:image/svg+xml`) cannot slip in.
        const IMAGE_TYPES: &[&str] = &[
            "image/png",
            "image/jpeg",
            "image/jpg",
            "image/gif",
            "image/webp",
        ];
        let Some(rest) = lower.strip_prefix("data:") else {
            return false;
        };
        let media_type = rest.split_once([';', ',']).map_or(rest, |(t, _)| t);
        IMAGE_TYPES.contains(&media_type)
    } else if scheme.is_empty() {
        // No scheme: a relative link is fine, but a protocol-relative `//host/…` would reach
        // the network.
        !u.starts_with("//")
    } else {
        matches!(scheme.as_str(), "http" | "https" | "mailto")
    }
}

/// A self-contained HTML document, styled like the app, for sharing one chat. It reuses the
/// Markdown rendering so both exports carry the same data; the result needs no network, fonts
/// or scripts to open.
pub fn conversation_html(c: &Conversation) -> String {
    let markdown = conversation_markdown(c);
    let body = render_body(&markdown);
    // A quiet, centred column with the app's warm palette, in light and dark.
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
:root {{
  --paper:#FAF9F5; --raised:#FFFFFF; --ink:#1F1E1D; --ink2:#3D3C38;
  --pencil:#6F6C64; --rule:#E3E0D7; --code:#F3F1EA; --signal:#9E5A38;
}}
@media (prefers-color-scheme: dark) {{
  :root {{
    --paper:#151515; --raised:#20201F; --ink:#F0EFED; --ink2:#B9B9AF;
    --pencil:#8A8984; --rule:#292929; --code:#1C1C1B; --signal:#BC815F;
  }}
}}
* {{ box-sizing:border-box; }}
body {{
  margin:0; background:var(--paper); color:var(--ink);
  font:16px/1.65 -apple-system,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
  -webkit-font-smoothing:antialiased;
}}
main {{ max-width:760px; margin:0 auto; padding:48px 24px 96px; }}
h1 {{ font-size:28px; line-height:1.25; margin:0 0 4px; }}
h2 {{
  font-size:13px; font-weight:600; letter-spacing:.05em; text-transform:uppercase;
  color:var(--pencil); margin:36px 0 10px;
}}
p {{ margin:0 0 14px; }}
a {{ color:var(--signal); }}
hr {{ border:0; border-top:1px solid var(--rule); margin:28px 0; }}
blockquote {{ margin:0 0 14px; padding:2px 0 2px 14px; border-left:3px solid var(--rule); color:var(--ink2); }}
code {{
  font:13px/1.5 ui-monospace,"SF Mono",Menlo,Consolas,monospace;
  background:var(--code); border-radius:4px; padding:1px 5px;
}}
pre {{
  background:var(--code); border:1px solid var(--rule); border-radius:8px;
  padding:12px 14px; overflow-x:auto; margin:0 0 14px;
}}
pre code {{ background:none; padding:0; font-size:13px; }}
img {{ max-width:100%; border-radius:8px; }}
table {{ border-collapse:collapse; width:100%; margin:0 0 14px; font-size:15px; }}
th,td {{ border:1px solid var(--rule); padding:6px 10px; text-align:left; }}
th {{ color:var(--ink2); }}
details {{ margin:0 0 14px; }}
summary {{ cursor:pointer; color:var(--pencil); font-size:14px; }}
</style>
</head>
<body>
<main>
{body}
</main>
</body>
</html>
"#,
        title = esc(&c.title),
        body = body
    )
}

/// Ask for a file to import with the native open dialog. `None` if the user cancelled.
pub fn pick_open_path() -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Import")
        .add_filter("JSON", &["json"]);
    if let Some(dirs) = directories::UserDirs::new()
        && let Some(docs) = dirs.document_dir()
    {
        dialog = dialog.set_directory(docs);
    }
    dialog.pick_file()
}

/// The contents of an export file.
pub enum Imported {
    Settings(Box<Settings>),
    Conversations(Vec<Conversation>),
}

/// Parse and validate an export produced by [`settings_document`] or [`conversations_document`].
pub fn parse_import(bytes: &[u8]) -> Result<Imported> {
    let doc: Value = serde_json::from_slice(bytes).context("not a valid JSON file")?;
    if doc.get("app").and_then(Value::as_str) != Some("CleanRSTAI") {
        bail!("not a CleanRSTAI export");
    }
    let version = doc
        .get("format_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if version == 0 || version > u64::from(FORMAT_VERSION) {
        bail!(
            "unsupported export format version {version}; this app reads up to {FORMAT_VERSION} \
             (was it made by a newer CleanRSTAI?)"
        );
    }
    match doc.get("format").and_then(Value::as_str) {
        Some("settings") => {
            let mut settings: Settings = serde_json::from_value(
                doc.get("settings")
                    .cloned()
                    .context("export has no settings")?,
            )
            .context("settings in the export are invalid")?;
            settings.normalize();
            Ok(Imported::Settings(Box::new(settings)))
        }
        Some("conversations") => Ok(Imported::Conversations(
            serde_json::from_value(
                doc.get("conversations")
                    .cloned()
                    .context("export has no conversations")?,
            )
            .context("conversations in the export are invalid")?,
        )),
        other => bail!("unknown export format {other:?}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportMode {
    /// Add imported providers and MCP servers, updating ones with the same ID; keep
    /// everything else, including general preferences.
    Merge,
    /// Use the imported settings as they are.
    Replace,
}

/// Fill blank secret values in `incoming` from `current` where they describe the same
/// provider / server / key, so re-importing a redacted export doesn't wipe saved keys.
fn keep_existing_secrets(current: &Settings, incoming: &mut Settings) {
    fn fill(dst: &mut [KeyValue], src: &[KeyValue]) {
        for kv in dst.iter_mut().filter(|kv| kv.value.is_empty()) {
            if let Some(old) = src.iter().find(|o| o.key == kv.key) {
                kv.value = old.value.clone();
            }
        }
    }
    for p in &mut incoming.providers {
        if let Some(old) = current.providers.iter().find(|o| o.id == p.id) {
            if p.api_key.is_empty() {
                p.api_key = old.api_key.clone();
            }
            fill(&mut p.extra_headers, &old.extra_headers);
        }
    }
    for s in &mut incoming.mcp_servers {
        let Some(old) = current.mcp_servers.iter().find(|o| o.id == s.id) else {
            continue;
        };
        match (&mut s.transport, &old.transport) {
            (McpTransportConfig::Stdio { env, .. }, McpTransportConfig::Stdio { env: old, .. }) => {
                fill(env, old)
            }
            (
                McpTransportConfig::Http { headers, .. },
                McpTransportConfig::Http { headers: old, .. },
            ) => fill(headers, old),
            _ => {}
        }
    }
}

/// Values that are still blank after import and probably need filling in.
pub fn missing_secrets(settings: &Settings) -> Vec<String> {
    let mut out = Vec::new();
    for p in &settings.providers {
        let local = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"]
            .iter()
            .any(|h| p.base_url.contains(h));
        if p.api_key.is_empty() && !local {
            out.push(format!("API key for provider \"{}\"", p.name));
        }
        for h in p.extra_headers.iter().filter(|h| h.value.is_empty()) {
            out.push(format!("header {} for provider \"{}\"", h.key, p.name));
        }
    }
    for s in &settings.mcp_servers {
        let (what, list) = match &s.transport {
            McpTransportConfig::Stdio { env, .. } => ("environment variable", env),
            McpTransportConfig::Http { headers, .. } => ("header", headers),
        };
        for kv in list.iter().filter(|kv| kv.value.is_empty()) {
            out.push(format!("{what} {} for MCP server \"{}\"", kv.key, s.name));
        }
    }
    out
}

/// Combine imported settings with the current ones.
pub fn apply_settings_import(
    current: &Settings,
    mut incoming: Settings,
    mode: ImportMode,
) -> Settings {
    keep_existing_secrets(current, &mut incoming);
    let mut out = match mode {
        ImportMode::Replace => incoming,
        ImportMode::Merge => {
            let mut merged = current.clone();
            for p in incoming.providers {
                match merged.providers.iter_mut().find(|o| o.id == p.id) {
                    Some(o) => *o = p,
                    None => merged.providers.push(p),
                }
            }
            for s in incoming.mcp_servers {
                match merged.mcp_servers.iter_mut().find(|o| o.id == s.id) {
                    Some(o) => *o = s,
                    None => merged.mcp_servers.push(s),
                }
            }
            merged
        }
    };
    out.normalize();
    out
}

/// Conversations from `incoming` whose IDs aren't already present, plus how many were skipped.
pub fn new_conversations(
    existing: &[Conversation],
    incoming: Vec<Conversation>,
) -> (Vec<Conversation>, usize) {
    let total = incoming.len();
    let mut seen: std::collections::HashSet<uuid::Uuid> = existing.iter().map(|c| c.id).collect();
    let fresh: Vec<Conversation> = incoming.into_iter().filter(|c| seen.insert(c.id)).collect();
    let skipped = total - fresh.len();
    (fresh, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{KeyValue, McpServerConfig, ProviderConfig};
    use crate::types::{ChatMessage, Role};

    fn sample() -> Settings {
        Settings {
            providers: vec![ProviderConfig {
                api_key: "sk-secret".into(),
                extra_headers: vec![KeyValue {
                    key: "X-Org".into(),
                    value: "org-secret".into(),
                }],
                ..Default::default()
            }],
            mcp_servers: vec![
                McpServerConfig {
                    transport: McpTransportConfig::Stdio {
                        command: "npx".into(),
                        args: vec!["-y".into()],
                        env: vec![KeyValue {
                            key: "GITHUB_TOKEN".into(),
                            value: "ghp_x".into(),
                        }],
                        cwd: String::new(),
                    },
                    ..Default::default()
                },
                McpServerConfig {
                    transport: McpTransportConfig::Http {
                        url: "https://example.com/mcp".into(),
                        headers: vec![KeyValue {
                            key: "Authorization".into(),
                            value: "Bearer y".into(),
                        }],
                        oauth_client_id: String::new(),
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn redacts_secrets_but_keeps_structure() {
        let doc = settings_document(&sample(), false);
        let text = doc.to_string();
        for secret in ["sk-secret", "org-secret", "ghp_x", "Bearer y"] {
            assert!(!text.contains(secret), "{secret} leaked");
        }
        assert_eq!(doc["includes_secrets"], false);
        assert!(text.contains("GITHUB_TOKEN") && text.contains("Authorization"));
        // The export round-trips into Settings.
        let back: Settings = serde_json::from_value(doc["settings"].clone()).unwrap();
        assert_eq!(back.mcp_servers.len(), 2);
    }

    #[test]
    fn keeps_secrets_when_asked() {
        let doc = settings_document(&sample(), true);
        assert!(doc.to_string().contains("sk-secret"));
        assert_eq!(doc["format"], "settings");
    }

    #[test]
    fn exports_conversations_and_writes_file() {
        let mut c = Conversation::new();
        c.messages.push(ChatMessage::new(Role::User, "hello"));
        let doc = conversations_document(&[c]);
        assert_eq!(doc["format"], "conversations");
        assert_eq!(doc["conversations"][0]["messages"][0]["content"], "hello");

        let dir = std::env::temp_dir().join(format!("cleanrstai-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("out.json");
        write_document(&path, &doc).unwrap();
        let read: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read["conversations"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn bytes(v: &Value) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }

    #[test]
    fn redacted_reimport_keeps_existing_secrets() {
        let current = sample();
        let Imported::Settings(incoming) =
            parse_import(&bytes(&settings_document(&current, false))).unwrap()
        else {
            panic!("expected settings")
        };
        assert!(
            !missing_secrets(&incoming).is_empty(),
            "redacted export has blanks"
        );
        let out = apply_settings_import(&current, *incoming, ImportMode::Replace);
        assert_eq!(
            out, current,
            "blank secrets refilled from the matching entries"
        );
        assert!(missing_secrets(&out).is_empty());
    }

    #[test]
    fn import_on_fresh_install_reports_missing_secrets() {
        let Imported::Settings(incoming) =
            parse_import(&bytes(&settings_document(&sample(), false))).unwrap()
        else {
            panic!()
        };
        let out = apply_settings_import(&Settings::default(), *incoming, ImportMode::Replace);
        let missing = missing_secrets(&out).join("; ");
        assert!(missing.contains("API key"), "{missing}");
        assert!(missing.contains("GITHUB_TOKEN"), "{missing}");
        assert!(missing.contains("Authorization"), "{missing}");
    }

    #[test]
    fn merge_adds_and_updates_but_keeps_preferences() {
        let mut current = sample();
        current.general.system_prompt = "mine".into();
        let mut incoming = current.clone();
        incoming.general.system_prompt = "theirs".into();
        incoming.providers[0].model = "new-model".into();
        incoming.providers.push(ProviderConfig {
            name: "Extra".into(),
            ..Default::default()
        });
        let out = apply_settings_import(&current, incoming.clone(), ImportMode::Merge);
        assert_eq!(out.general.system_prompt, "mine");
        assert_eq!(out.providers.len(), 2);
        assert_eq!(out.providers[0].model, "new-model");
        assert_eq!(
            out.mcp_servers.len(),
            2,
            "same IDs are updated, not duplicated"
        );
        let out = apply_settings_import(&current, incoming, ImportMode::Replace);
        assert_eq!(out.general.system_prompt, "theirs");
    }

    #[test]
    fn rejects_foreign_or_future_files() {
        assert!(parse_import(b"not json").is_err());
        assert!(parse_import(&bytes(&json!({"app": "Other", "format": "settings"}))).is_err());
        let mut doc = settings_document(&sample(), false);
        doc["format_version"] = json!(99);
        let err = parse_import(&bytes(&doc)).err().unwrap().to_string();
        assert!(err.contains("newer"), "{err}");
        doc["format_version"] = json!(1);
        doc["format"] = json!("mystery");
        assert!(parse_import(&bytes(&doc)).is_err());
    }

    #[test]
    fn conversation_import_skips_existing_ids() {
        let a = Conversation::new();
        let b = Conversation::new();
        let doc = conversations_document(&[a.clone(), b.clone()]);
        let Imported::Conversations(list) = parse_import(&bytes(&doc)).unwrap() else {
            panic!()
        };
        let (fresh, skipped) = new_conversations(&[a], list);
        assert_eq!(skipped, 1);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, b.id);
    }

    #[test]
    fn markdown_export_includes_everything() {
        use crate::types::{ChatMessage, Role, ToolCall};
        let mut c = Conversation::new();
        c.title = "My / chat?".into();
        let mut u = ChatMessage::new(Role::User, "Look");
        u.attachments = vec![crate::attach::Attachment::image(
            "p.png".into(),
            "image/png".into(),
            b"x",
        )];
        let mut a = ChatMessage::new(Role::Assistant, "Done ```weird``` fences");
        a.reasoning = Some("hmm".into());
        a.tool_calls = vec![ToolCall {
            id: "1".into(),
            name: "srv__t".into(),
            arguments: "{\"a\":1}".into(),
        }];
        let t = ChatMessage::tool_result(&a.tool_calls[0], "has ``` inside", false);
        c.messages = vec![u, a, t];
        let md = conversation_markdown(&c);
        assert!(md.starts_with("# My / chat?"));
        assert!(md.contains("![p.png](data:image/png;base64,eA==)"));
        assert!(md.contains("<summary>Thinking</summary>"));
        assert!(md.contains("**Tool call:** `srv__t`"));
        assert!(md.contains("````\nhas ``` inside\n````"), "{md}");
        assert_eq!(safe_file_name(&c.title, "md"), "My - chat-.md");
        assert_eq!(safe_file_name("..", "md"), "chat.md");
    }

    #[test]
    fn html_export_is_self_contained() {
        use crate::types::{ChatMessage, Role};
        let mut c = Conversation::new();
        c.title = "My chat & friends".into();
        c.messages = vec![
            ChatMessage::new(Role::User, "Look at **this**"),
            ChatMessage::new(Role::Assistant, "Here you go: `code`"),
        ];
        let html = conversation_html(&c);
        // A standalone document that opens without a network, fonts or scripts.
        assert!(html.starts_with("<!doctype html>"), "{html}");
        assert!(
            html.contains("<title>My chat &amp; friends</title>"),
            "{html}"
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(
            !html.contains("http://") && !html.contains("https://"),
            "{html}"
        );
        // The same data as the Markdown export, rendered to HTML.
        assert!(html.contains("<h1>My chat &amp; friends</h1>"), "{html}");
        assert!(html.contains("<strong>this</strong>"), "{html}");
        assert!(html.contains("<code>code</code>"), "{html}");
        assert!(html.contains("<h2>You</h2>"), "{html}");
        assert!(html.contains("<h2>Assistant</h2>"), "{html}");
    }

    #[test]
    fn html_export_neutralises_dangerous_markup() {
        use crate::types::{ChatMessage, Role};
        let payloads = [
            // Raw HTML blocks and inline tags are dropped outright.
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(2)>",
            // Handler with no whitespace before it (defeats `\son…` regexes).
            "<div><img src=\"x\"/onerror=\"alert(1)\"></div>",
            "<img src=\"x\"onerror=\"alert(1)\">",
            // Tag stripping that rebuilds an earlier-checked tag.
            "<div><ifr<svg>ame srcdoc=\"&lt;script&gt;alert(1)&lt;/script&gt;\"></iframe></div>",
            "<iframe srcdoc=\"&lt;script&gt;alert(1)&lt;/script&gt;\"></iframe>",
            // Raw anchor with a dangerous scheme.
            "<a href=\"javascript:alert(1)\">x</a>",
            // Markdown links with dangerous or obfuscated schemes.
            "[x](javascript:alert(1))",
            "[x](&#106;avascript:alert(1))",
            "[x](javascript&colon;alert(1))",
            "[x](java&#9;script:alert(1))",
            "[x](\x01javascript:alert(1))",
            // Protocol-relative links reach the network.
            "[x](//evil.example/x)",
            // Remote and executable images are dropped (the file must stay offline).
            "![x](https://evil.example/x.png)",
            "<img src=\"https://evil.example/x.png\" onerror=\"alert(1)\">",
            "![x](data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+)",
            "![x](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)",
        ];
        let mut c = Conversation::new();
        c.messages = vec![
            ChatMessage::new(Role::User, "hi"),
            ChatMessage::new(Role::Assistant, payloads.join("\n\n")),
        ];
        let html = conversation_html(&c);
        // No executable markup survives: raw HTML is dropped (only the app's own <details>
        // wrapper is kept), and no rendered link or image carries a dangerous scheme, handler,
        // or remote source.
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<iframe"), "{html}");
        assert!(!html.contains("srcdoc"), "{html}");
        assert!(!html.contains("href=\"javascript"), "{html}");
        assert!(!html.contains("src=\"javascript"), "{html}");
        assert!(!html.contains("src=\"https://evil"), "{html}");
        assert!(!html.contains("src=\"data:image/svg"), "{html}");
        assert!(!html.contains("src=\"data:text/html"), "{html}");
        assert!(!html.contains("href=\"//evil"), "{html}");
        // The no-space handler payload is not recognised as a tag, so it is emitted as escaped
        // text the browser will display literally, not execute.
        assert!(html.contains("&lt;img src=\"x\"onerror"), "{html}");
        // Ordinary content and the app's own heading survive.
        assert!(html.contains("<h2>Assistant</h2>"), "{html}");
    }

    #[test]
    fn html_export_keeps_safe_links_and_embedded_images() {
        use crate::types::{ChatMessage, Role};
        let mut c = Conversation::new();
        c.messages = vec![ChatMessage::new(
            Role::Assistant,
            "[docs](https://docs.example/x) [me](mailto:a@b.c) \
             ![p](data:image/png;base64,AAAA) [rel](x/y.md)",
        )];
        let html = conversation_html(&c);
        assert!(html.contains("href=\"https://docs.example/x\""), "{html}");
        assert!(html.contains("href=\"mailto:a@b.c\""), "{html}");
        assert!(
            html.contains("src=\"data:image/png;base64,AAAA\""),
            "{html}"
        );
        assert!(html.contains("href=\"x/y.md\""), "{html}");
    }

    #[test]
    fn markdown_export_pairs_results_with_their_own_turn() {
        use crate::types::{ChatMessage, Role, ToolCall};
        // Two turns whose calls share the id `call_0`, as some local servers produce.
        let call = ToolCall {
            id: "call_0".into(),
            name: "x__echo".into(),
            arguments: "{}".into(),
        };
        let mut c = Conversation::new();
        let mut a = ChatMessage::new(Role::Assistant, "");
        a.tool_calls = vec![call.clone()];
        c.messages = vec![
            a.clone(),
            ChatMessage::tool_result(&call, "first result", false),
            a,
            ChatMessage::tool_result(&call, "second result", false),
        ];
        let md = conversation_markdown(&c);
        let first = md.find("first result").expect("first result exported");
        let second = md.find("second result").expect("second result exported");
        assert!(first < second, "{md}");
        assert_eq!(md.matches("second result").count(), 1, "{md}");
    }
}

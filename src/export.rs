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
    let results: std::collections::HashMap<&str, &crate::types::ChatMessage> = c
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
        .collect();
    for m in &c.messages {
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
}

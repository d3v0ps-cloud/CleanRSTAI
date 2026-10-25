//! Artifacts: the substantial outputs in replies (web pages, SVGs, diagrams, documents and
//! longer code) that open in a side panel beside the chat.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::types::{ChatMessage, Role};

/// Code blocks shorter than this stay in the reply only; drawings, pages and documents are
/// artifacts at any length.
const MIN_CODE_LINES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Html,
    Svg,
    Mermaid,
    Markdown,
    Code,
}

impl Kind {
    fn of(lang: &str) -> Self {
        match lang.to_ascii_lowercase().as_str() {
            "html" | "htm" | "xhtml" => Self::Html,
            "svg" => Self::Svg,
            "mermaid" => Self::Mermaid,
            "markdown" | "md" => Self::Markdown,
            _ => Self::Code,
        }
    }

    /// Whether the panel can show it rendered, not just as source.
    pub fn has_preview(self) -> bool {
        matches!(self, Self::Svg | Self::Mermaid | Self::Markdown)
    }

    /// Whether "Open in browser" makes sense for it.
    pub fn opens_in_browser(self) -> bool {
        matches!(self, Self::Html | Self::Svg)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// Index of the reply in the conversation's messages.
    pub message: usize,
    /// Position among that reply's artifacts.
    pub index: usize,
    pub kind: Kind,
    /// The code block's language, as written ("" if none).
    pub lang: String,
    pub title: String,
    pub source: String,
    /// Where the code block is in the reply's text.
    pub range: Range<usize>,
}

impl Artifact {
    pub fn lines(&self) -> usize {
        self.source.lines().count()
    }

    /// A file name to save it under.
    pub fn file_name(&self) -> String {
        let stem: String = self
            .title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase();
        let stem = if stem.is_empty() { "artifact" } else { &stem };
        // A title that is already a file name keeps its own extension.
        if self.title.contains('.') && !self.title.contains(' ') {
            return self.title.clone();
        }
        format!("{stem}.{}", extension(&self.lang, self.kind))
    }

    /// Artifacts with the same title and kind are versions of one another.
    pub fn same_thing(&self, other: &Artifact) -> bool {
        self.kind == other.kind && self.title == other.title
    }
}

/// Every artifact in a conversation's replies, oldest first.
pub fn in_conversation(messages: &[ChatMessage]) -> Vec<Artifact> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::Assistant)
        .flat_map(|(i, m)| in_reply(i, &m.content))
        .collect()
}

/// The artifacts in one reply: closed top-level fenced code blocks that qualify. A block that
/// is still streaming in (no closing fence yet) isn't one.
pub fn in_reply(message: usize, text: &str) -> Vec<Artifact> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(String, String, bool, usize)> = None;
    for (event, range) in Parser::new_ext(text, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if depth == 0 => {
                depth += 1;
                let closed = fence_is_closed(&text[range.clone()]);
                open = Some((info.to_string(), String::new(), closed, range.start));
            }
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::CodeBlock) => {
                depth = depth.saturating_sub(1);
                if let Some((info, source, true, start)) = open.take() {
                    let lang = info.split_whitespace().next().unwrap_or("").to_owned();
                    let kind = Kind::of(&lang);
                    if kind == Kind::Code && source.lines().count() < MIN_CODE_LINES {
                        continue;
                    }
                    let title = title(&info, &lang, kind, &source);
                    out.push(Artifact {
                        message,
                        index: out.len(),
                        kind,
                        lang,
                        title,
                        source,
                        range: start..range.end,
                    });
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(t) => {
                if let Some((_, source, ..)) = &mut open {
                    source.push_str(&t);
                }
            }
            _ => {}
        }
    }
    out
}

/// How replies are parsed, here and when they are shown.
pub fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}

/// Whether a fenced code block's text ends with its closing fence.
pub fn fence_is_closed(block: &str) -> bool {
    let mut lines = block.trim_end().lines();
    let Some(first) = lines.next() else {
        return false;
    };
    let opener = first.trim_start();
    let fence_char = opener.chars().next().unwrap_or('`');
    let fence_len = opener.chars().take_while(|&c| c == fence_char).count();
    lines.next_back().is_some_and(|last| {
        let last = last.trim();
        last.len() >= fence_len && last.chars().all(|c| c == fence_char)
    })
}

/// A name for the artifact: a file name or `title="…"` in the fence's info string, a page's
/// `<title>`, a document's first heading, or else what it is.
fn title(info: &str, lang: &str, kind: Kind, source: &str) -> String {
    let rest = info.trim_start()[lang.len()..].trim();
    if let Some(t) = rest
        .split_once("title=")
        .map(|(_, t)| t.trim_matches(|c| c == '"' || c == '\''))
        .filter(|t| !t.is_empty())
    {
        return t.to_owned();
    }
    if let Some(file) = rest
        .split_whitespace()
        .find(|w| w.contains('.') && !w.contains('='))
    {
        return file.rsplit(['/', '\\']).next().unwrap_or(file).to_owned();
    }
    let found = match kind {
        Kind::Html => tag_text(source, "title"),
        Kind::Svg => tag_text(source, "title"),
        Kind::Markdown => source
            .lines()
            .find_map(|l| l.trim().strip_prefix('#'))
            .map(|h| h.trim_start_matches('#').trim().to_owned()),
        _ => None,
    };
    if let Some(t) = found.filter(|t| !t.is_empty()) {
        return t.chars().take(80).collect();
    }
    match kind {
        Kind::Html => "Web page".into(),
        Kind::Svg => "SVG image".into(),
        Kind::Mermaid => "Diagram".into(),
        Kind::Markdown => "Document".into(),
        Kind::Code => match language_name(lang) {
            Some(name) => format!("{name} code"),
            None => "Code".into(),
        },
    }
}

/// The text inside the first `<tag>…</tag>`, if any.
fn tag_text(source: &str, tag: &str) -> Option<String> {
    let lower = source.to_ascii_lowercase();
    let open = lower.find(&format!("<{tag}"))?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find(&format!("</{tag}"))?;
    Some(
        source[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn language_name(lang: &str) -> Option<&'static str> {
    Some(match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "Rust",
        "python" | "py" => "Python",
        "javascript" | "js" | "jsx" => "JavaScript",
        "typescript" | "ts" | "tsx" => "TypeScript",
        "go" | "golang" => "Go",
        "java" => "Java",
        "kotlin" | "kt" => "Kotlin",
        "swift" => "Swift",
        "c" => "C",
        "cpp" | "c++" | "cc" => "C++",
        "csharp" | "cs" | "c#" => "C#",
        "ruby" | "rb" => "Ruby",
        "php" => "PHP",
        "bash" | "sh" | "shell" | "zsh" => "Shell",
        "powershell" | "ps1" => "PowerShell",
        "sql" => "SQL",
        "css" => "CSS",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "lua" => "Lua",
        "dockerfile" | "docker" => "Dockerfile",
        _ => return None,
    })
}

fn extension(lang: &str, kind: Kind) -> &'static str {
    match kind {
        Kind::Html => return "html",
        Kind::Svg => return "svg",
        Kind::Mermaid => return "mmd",
        Kind::Markdown => return "md",
        Kind::Code => {}
    }
    match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "javascript" | "js" => "js",
        "jsx" => "jsx",
        "typescript" | "ts" => "ts",
        "tsx" => "tsx",
        "go" | "golang" => "go",
        "java" => "java",
        "kotlin" | "kt" => "kt",
        "swift" => "swift",
        "c" => "c",
        "cpp" | "c++" | "cc" => "cpp",
        "csharp" | "cs" | "c#" => "cs",
        "ruby" | "rb" => "rb",
        "php" => "php",
        "bash" | "sh" | "shell" | "zsh" => "sh",
        "powershell" | "ps1" => "ps1",
        "sql" => "sql",
        "css" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "lua" => "lua",
        _ => "txt",
    }
}

/// Write `artifact` to a temporary file and open it with the system's default app (the web
/// browser for pages and SVGs).
pub fn open_in_browser(artifact: &Artifact) -> Result<std::path::PathBuf, String> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let dir = std::env::temp_dir().join("CleanRSTAI-artifacts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut h = DefaultHasher::new();
    artifact.source.hash(&mut h);
    let path = dir.join(format!(
        "{:016x}.{}",
        h.finish(),
        extension(&artifact.lang, artifact.kind)
    ));
    std::fs::write(&path, &artifact.source).map_err(|e| e.to_string())?;
    open::that_detached(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(lines: usize) -> String {
        (0..lines).map(|i| format!("let x{i} = {i};\n")).collect()
    }

    #[test]
    fn finds_pages_drawings_documents_and_long_code() {
        let text = format!(
            "Here:\n\n```html\n<html><head><title>My  Page</title></head></html>\n```\n\n\
             ```rust\nfn short() {{}}\n```\n\n```rust src/main.rs\n{}```\n\n\
             ```svg\n<svg/>\n```\n\n```mermaid\nflowchart LR\n```\n\n\
             ```markdown\n# Plan\n\ntext\n```\n\n```python\n{}```\n",
            code(12),
            code(10)
        );
        let found = in_reply(3, &text);
        let titles: Vec<_> = found.iter().map(|a| (a.kind, a.title.as_str())).collect();
        assert_eq!(
            titles,
            [
                (Kind::Html, "My Page"),
                (Kind::Code, "main.rs"),
                (Kind::Svg, "SVG image"),
                (Kind::Mermaid, "Diagram"),
                (Kind::Markdown, "Plan"),
                (Kind::Code, "Python code"),
            ]
        );
        assert!(found.iter().all(|a| a.message == 3));
        assert_eq!(found[1].index, 1);
        assert_eq!(found[1].lines(), 12);
        assert_eq!(found[1].file_name(), "main.rs");
        assert_eq!(found[0].file_name(), "my-page.html");
        assert_eq!(found[5].file_name(), "python-code.py");
        assert!(text[found[2].range.clone()].starts_with("```svg"));
        assert!(text[found[2].range.clone()].trim_end().ends_with("```"));
    }

    #[test]
    fn skips_unclosed_and_nested_blocks() {
        let streaming = "```html\n<html>\n";
        assert!(in_reply(0, streaming).is_empty());
        let nested = "- item\n\n  ```html\n  <p>x</p>\n  ```\n";
        assert!(in_reply(0, nested).is_empty());
    }

    #[test]
    fn titles_from_the_info_string() {
        let a = &in_reply(0, "```html title=\"Landing page\"\n<p/>\n```\n")[0];
        assert_eq!(a.title, "Landing page");
        assert_eq!(a.file_name(), "landing-page.html");
    }

    #[test]
    fn same_title_means_another_version() {
        let first = &in_reply(0, "```svg\n<svg><title>Logo</title></svg>\n```\n")[0];
        let second = &in_reply(2, "```svg\n<svg><title>Logo</title><g/></svg>\n```\n")[0];
        let other = &in_reply(2, "```svg\n<svg><title>Icon</title></svg>\n```\n")[0];
        assert!(first.same_thing(second));
        assert!(!first.same_thing(other));
    }
}

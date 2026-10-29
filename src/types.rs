//! Chat data model and on-disk conversation storage.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON-encoded arguments, exactly as produced by the model.
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For tool results: the function name that produced it (display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Model "thinking" output, if the endpoint provides it. Never sent back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    /// Speed and token stats for the request that produced this assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<crate::stats::RequestStats>,
    /// Images and text files attached by the user, or images returned by a tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::attach::Attachment>,
    /// For a `delegate` tool result: what each subagent did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<crate::subagent::SubagentReport>,
    /// A user prompt whose generation was cancelled. It stays visible so it can be edited and
    /// resent, but is never sent to the model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancelled: bool,
    /// For a command that ran in the Docker sandbox: where.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxInfo>,
    /// For a workspace `write_file` or `edit_file` result: the change, and how to undo it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<FileEdit>,
    /// A compaction marker (a `System` message whose content is the summary): the messages
    /// before it are no longer sent to the model, only the summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionInfo>,
    /// For an assistant message from a native API: items that API needs back on the next
    /// request (Anthropic's signed thinking blocks, the Responses API's reasoning items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeItems>,
}

/// Provider-specific output kept so it can be sent back, only to the API that made it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NativeItems {
    pub api: crate::config::ProviderApi,
    pub items: Vec<serde_json::Value>,
}

/// One file changed by `write_file` or `edit_file`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileEdit {
    /// The file, relative to the workspace.
    pub path: String,
    /// The file's full path, for undo.
    pub abs: PathBuf,
    /// The change as a unified diff (empty when diffs are off).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diff: String,
    /// The file didn't exist before.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub created: bool,
    /// The previous content, while a checkpoint is kept (`None` for a new file, or once the
    /// checkpoint is dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// A checkpoint is kept: the edit can be undone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint: bool,
    /// SHA-256 of the content the edit wrote, to notice later changes before undoing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub after_sha256: String,
    /// The edit was undone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undone: bool,
}

/// What a compaction replaced with its summary.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompactionInfo {
    /// Messages the summary stands for (this compaction's, not earlier ones).
    pub messages: usize,
    /// Estimated tokens of the request before compacting.
    #[serde(default)]
    pub tokens_before: u64,
    /// Started by hand ("Compact now") rather than automatically.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manual: bool,
}

/// Where the latest compaction marker is, if any.
pub fn last_compaction(messages: &[ChatMessage]) -> Option<usize> {
    messages.iter().rposition(|m| m.compaction.is_some())
}

/// The Docker sandbox container a command ran in.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SandboxInfo {
    /// The image the container runs.
    pub image: String,
    /// The container's name.
    pub container: String,
}

/// The tool results answering the calls of the assistant message at `idx`: the tool
/// messages right after it. (Matching by id across the whole chat would mix up turns from
/// servers that reuse ids like `call_0`.)
pub fn results_after(
    messages: &[ChatMessage],
    idx: usize,
) -> std::collections::HashMap<&str, &ChatMessage> {
    messages[idx + 1..]
        .iter()
        .take_while(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
        .collect()
}

/// The messages a fork up to message `idx` keeps: everything through `idx`, and for an
/// assistant message with tool calls, the tool results that directly follow it, so the
/// fork never ends on a call without its results. Empty if `idx` is out of range.
///
/// The fork shares the original's workspace folder, so its copied file edits must not be
/// undoable from the fork: their checkpoints are dropped (`checkpoint` false, `before` None)
/// while the diff is kept for the tool card.
pub fn fork_messages(messages: &[ChatMessage], idx: usize) -> Vec<ChatMessage> {
    let Some(msg) = messages.get(idx) else {
        return Vec::new();
    };
    let mut end = idx + 1;
    if msg.role == Role::Assistant && !msg.tool_calls.is_empty() {
        end += messages[end..]
            .iter()
            .take_while(|m| m.role == Role::Tool)
            .count();
    }
    let mut kept = messages[..end].to_vec();
    for m in &mut kept {
        if let Some(edit) = &mut m.edit {
            edit.checkpoint = false;
            edit.before = None;
        }
    }
    kept
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            is_error: false,
            stats: None,
            attachments: Vec::new(),
            subagents: Vec::new(),
            cancelled: false,
            sandbox: None,
            edit: None,
            compaction: None,
            native: None,
        }
    }

    pub fn tool_result(call: &ToolCall, content: impl Into<String>, is_error: bool) -> Self {
        Self {
            tool_call_id: Some(call.id.clone()),
            tool_name: Some(call.name.clone()),
            is_error,
            ..Self::new(Role::Tool, content)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<ChatMessage>,
    /// Pinned chats stay at the top of the sidebar.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Tools and MCP servers switched on or off for this chat only.
    #[serde(default, skip_serializing_if = "crate::tools::ToolOverrides::is_empty")]
    pub tool_overrides: crate::tools::ToolOverrides,
    /// The local folder this chat works in, with the workspace tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<std::path::PathBuf>,
    /// Reasoning and sampling options for this chat, over the provider's and the model's.
    #[serde(
        default,
        skip_serializing_if = "crate::model_options::ModelOptions::is_empty"
    )]
    pub model_options: crate::model_options::ModelOptions,
    /// How this chat's tool calls are approved, and whether its commands use the sandbox.
    #[serde(
        default,
        skip_serializing_if = "crate::approval::ChatPermissions::is_default"
    )]
    pub permissions: crate::approval::ChatPermissions,
}

impl Conversation {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: "New chat".into(),
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
            pinned: false,
            tool_overrides: Default::default(),
            workspace: None,
            model_options: Default::default(),
            permissions: Default::default(),
        }
    }

    /// A new chat that starts in `mode` (Settings' default for new chats).
    pub fn with_permission_mode(mode: crate::approval::ApprovalMode) -> Self {
        let mut c = Self::new();
        c.permissions.mode = mode;
        c
    }

    fn dir() -> PathBuf {
        crate::config::data_dir().join("conversations")
    }

    fn path(id: Uuid) -> PathBuf {
        Self::dir().join(format!("{id}.json"))
    }

    pub fn load_all() -> Vec<Conversation> {
        let mut out: Vec<Conversation> = std::fs::read_dir(Self::dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                serde_json::from_str(&text)
                    .inspect_err(|err| log::warn!("skipping {}: {err}", e.path().display()))
                    .ok()
            })
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        out
    }

    pub fn save(&self) {
        let result = serde_json::to_vec_pretty(self)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| crate::config::write_private(&Self::path(self.id), &bytes));
        if let Err(e) = result {
            log::error!("failed to save conversation {}: {e}", self.id);
        }
    }

    pub fn delete(id: Uuid) {
        let _ = std::fs::remove_file(Self::path(id));
    }

    /// Derive a title from the first user message.
    pub fn auto_title(&mut self) {
        if self.title != "New chat" {
            return;
        }
        if let Some(first) = self.messages.iter().find(|m| m.role == Role::User) {
            let line = first
                .content
                .lines()
                .next()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .or_else(|| first.attachments.first().map(|a| a.name.as_str()))
                .unwrap_or("");
            let mut title: String = line.chars().take(48).collect();
            if line.chars().count() > 48 {
                title.push('…');
            }
            if !title.is_empty() {
                self.title = title;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chat_takes_the_default_permission_mode() {
        use crate::approval::ApprovalMode;
        assert_eq!(Conversation::new().permissions.mode, ApprovalMode::Default);
        let c = Conversation::with_permission_mode(ApprovalMode::AutoWorkspace);
        assert_eq!(c.permissions.mode, ApprovalMode::AutoWorkspace);
        assert!(!c.permissions.no_sandbox);
    }

    #[test]
    fn auto_title_from_attachment() {
        let mut c = Conversation::new();
        let mut m = ChatMessage::new(Role::User, "");
        m.attachments.push(crate::attach::Attachment {
            name: "report.pdf.txt".into(),
            mime: "text/plain".into(),
            text: Some("x".into()),
            data: None,
        });
        c.messages.push(m);
        c.auto_title();
        assert_eq!(c.title, "report.pdf.txt");
    }

    #[test]
    fn a_message_without_a_sandbox_loads_and_one_with_it_round_trips() {
        let old = r#"{"role":"tool","content":"Exit code: 0","tool_call_id":"c1"}"#;
        let m: ChatMessage = serde_json::from_str(old).unwrap();
        assert_eq!(m.sandbox, None);
        assert!(!serde_json::to_string(&m).unwrap().contains("sandbox"));

        let mut m = ChatMessage::new(Role::Tool, "Exit code: 0");
        m.sandbox = Some(SandboxInfo {
            image: "python:3.13-slim".into(),
            container: "cleanrstai-3f2a".into(),
        });
        let json = serde_json::to_string(&m).unwrap();
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn fork_at_a_user_message_keeps_up_to_it() {
        let messages = vec![
            ChatMessage::new(Role::User, "one"),
            ChatMessage::new(Role::Assistant, "two"),
            ChatMessage::new(Role::User, "three"),
            ChatMessage::new(Role::Assistant, "four"),
        ];
        let forked = fork_messages(&messages, 2);
        assert_eq!(forked, messages[..=2]);
    }

    #[test]
    fn fork_at_a_tool_call_keeps_its_results() {
        let mut call = ChatMessage::new(Role::Assistant, "");
        call.tool_calls.push(ToolCall {
            id: "c1".into(),
            name: "time".into(),
            arguments: "{}".into(),
        });
        let mut result = ChatMessage::new(Role::Tool, "12:00");
        result.tool_call_id = Some("c1".into());
        let messages = vec![
            ChatMessage::new(Role::User, "what time is it?"),
            call.clone(),
            result.clone(),
            ChatMessage::new(Role::Assistant, "It is noon."),
            ChatMessage::new(Role::User, "thanks"),
        ];
        let forked = fork_messages(&messages, 1);
        assert_eq!(forked, vec![messages[0].clone(), call, result]);
    }

    #[test]
    fn fork_at_the_last_message_keeps_everything() {
        let messages = vec![
            ChatMessage::new(Role::User, "hi"),
            ChatMessage::new(Role::Assistant, "hello"),
        ];
        assert_eq!(fork_messages(&messages, 1), messages);
        assert!(fork_messages(&messages, 2).is_empty());
    }

    #[test]
    fn a_fork_keeps_file_edit_diffs_but_not_their_checkpoints() {
        let mut call = ChatMessage::new(Role::Assistant, "");
        call.tool_calls.push(ToolCall {
            id: "c1".into(),
            name: "write_file".into(),
            arguments: "{}".into(),
        });
        let mut result = ChatMessage::new(Role::Tool, "Wrote a.txt");
        result.tool_call_id = Some("c1".into());
        result.edit = Some(FileEdit {
            path: "a.txt".into(),
            abs: PathBuf::from("/work/a.txt"),
            diff: "-old\n+new\n".into(),
            created: false,
            before: Some("old\n".into()),
            checkpoint: true,
            after_sha256: "abc".into(),
            undone: false,
        });
        let messages = vec![ChatMessage::new(Role::User, "edit it"), call, result];
        let forked = fork_messages(&messages, 1);
        let edit = forked[2].edit.as_ref().expect("the edit is copied");
        assert!(!edit.checkpoint);
        assert_eq!(edit.before, None);
        assert_eq!(edit.diff, "-old\n+new\n");
        // The original is untouched.
        assert!(messages[2].edit.as_ref().unwrap().checkpoint);
    }
}

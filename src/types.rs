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
        }
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
            let line = first.content.lines().next().unwrap_or("").trim();
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

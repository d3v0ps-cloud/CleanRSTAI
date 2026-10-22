//! Export settings and conversations to standalone JSON files.

use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use crate::config::{McpTransportConfig, Settings};
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
    let mut dialog = rfd::FileDialog::new()
        .set_title("Export")
        .set_file_name(default_name)
        .add_filter("JSON", &["json"]);
    if let Some(dirs) = directories::UserDirs::new()
        && let Some(docs) = dirs.document_dir()
    {
        dialog = dialog.set_directory(docs);
    }
    dialog.save_file().map(|p| {
        if p.extension().is_none() {
            p.with_extension("json")
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
}

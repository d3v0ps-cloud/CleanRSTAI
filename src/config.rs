//! Persistent application settings. Everything here is editable from the UI.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::secrets::SecretStore;

pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("net", "bukn", "CleanRSTAI")
}

pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cleanrstai"))
}

pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cleanrstai"))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// What kind of server a provider is, so the app can use its extra stats.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any OpenAI-compatible API: standard `usage` plus client-side timing.
    #[default]
    Generic,
    /// llama.cpp `llama-server`: native prefill/decode timings and context size.
    LlamaCpp,
    /// LiteLLM proxy: served model and cost.
    LiteLlm,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 3] = [Self::Generic, Self::LlamaCpp, Self::LiteLlm];

    pub fn label(self) -> &'static str {
        match self {
            Self::Generic => "Generic OpenAI-compatible",
            Self::LlamaCpp => "llama.cpp server",
            Self::LiteLlm => "LiteLLM proxy",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Generic => "Token usage from the API; speeds measured by the app.",
            Self::LlamaCpp => "Prefill/decode speeds and context size reported by llama.cpp.",
            Self::LiteLlm => "Shows the model that served each request and its cost.",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    /// Base URL of the OpenAI-compatible API, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Models discovered via `GET /models`, cached for the model picker.
    pub models: Vec<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    pub extra_headers: Vec<KeyValue>,
    pub timeout_secs: u64,
    pub kind: ProviderKind,
    /// Ask for token usage in streamed responses (`stream_options.include_usage`).
    pub request_usage: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            models: Vec::new(),
            temperature: None,
            max_tokens: None,
            stream: true,
            extra_headers: Vec::new(),
            timeout_secs: 300,
            kind: ProviderKind::Generic,
            request_usage: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<KeyValue>,
        #[serde(default)]
        cwd: String,
    },
    /// MCP "Streamable HTTP" transport.
    Http {
        url: String,
        #[serde(default)]
        headers: Vec<KeyValue>,
    },
}

impl Default for McpTransportConfig {
    fn default() -> Self {
        Self::Stdio {
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransportConfig,
    /// Tools the user switched off; they are not offered to the model.
    pub disabled_tools: Vec<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "new-server".into(),
            enabled: true,
            transport: McpTransportConfig::default(),
            disabled_tools: Vec::new(),
        }
    }
}

impl McpServerConfig {
    /// Whether a change from `other` requires reconnecting.
    pub fn needs_reconnect(&self, other: &McpServerConfig) -> bool {
        self.enabled != other.enabled || self.transport != other.transport
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeneralSettings {
    pub system_prompt: String,
    pub theme: Theme,
    pub zoom: f32,
    /// Enter sends, Shift+Enter inserts a newline. When false, Ctrl/Cmd+Enter sends.
    pub send_with_enter: bool,
    /// Run MCP tool calls without asking first.
    pub auto_approve_tools: bool,
    pub max_tool_rounds: u32,
    pub tool_timeout_secs: u64,
    /// Show prefill/decode speed and token counts under each response.
    pub show_stats: bool,
    /// Keep API keys and other secrets in the OS keychain instead of settings.json.
    pub use_keychain: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            system_prompt: "You are a helpful assistant.".into(),
            theme: Theme::System,
            zoom: 1.0,
            send_with_enter: true,
            auto_approve_tools: false,
            max_tool_rounds: 20,
            tool_timeout_secs: 120,
            show_stats: true,
            use_keychain: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub providers: Vec<ProviderConfig>,
    pub active_provider: usize,
    pub mcp_servers: Vec<McpServerConfig>,
    pub general: GeneralSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            providers: vec![ProviderConfig::default()],
            active_provider: 0,
            mcp_servers: Vec::new(),
            general: GeneralSettings::default(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    /// Load settings, filling secrets back in from the OS keychain.
    pub fn load() -> LoadedSettings {
        Self::load_from(&Self::path(), &crate::secrets::OsKeychain)
    }

    pub fn load_from(path: &std::path::Path, store: &dyn SecretStore) -> LoadedSettings {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                return LoadedSettings {
                    settings: Self::default(),
                    notice: None,
                    needs_migration: false,
                };
            }
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&text)
            .and_then(|v| Ok((serde_json::from_value::<Settings>(v.clone())?, v)));
        let (mut settings, raw) = match parsed {
            Ok(x) => x,
            Err(e) => {
                log::error!("Failed to parse {}: {e}", path.display());
                // Keep the broken file around rather than silently overwriting it later.
                let _ = std::fs::copy(path, path.with_extension("json.bak"));
                return LoadedSettings {
                    settings: Self::default(),
                    notice: Some(format!(
                        "settings.json could not be read ({e}); started with defaults. The old file was kept as settings.json.bak."
                    )),
                    needs_migration: false,
                };
            }
        };
        settings.normalize();
        let in_keychain = raw
            .get(KEY_IN_KEYCHAIN)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut notice = None;
        if in_keychain {
            let failed = crate::secrets::internalize(&mut settings, store, &stored_accounts(&raw));
            if !failed.is_empty() {
                notice = Some(format!(
                    "{} secret(s) couldn't be read from the system keychain; re-enter them in Settings.",
                    failed.len()
                ));
            }
        }
        let needs_migration = !in_keychain
            && settings.general.use_keychain
            && crate::secrets::has_any_secret(&settings);
        LoadedSettings {
            settings,
            notice,
            needs_migration,
        }
    }

    /// Save settings. Returns a warning if secrets had to stay in the file.
    pub fn save(&self) -> Result<Option<String>> {
        self.save_to(&Self::path(), &crate::secrets::OsKeychain)
    }

    pub fn save_to(
        &self,
        path: &std::path::Path,
        store: &dyn SecretStore,
    ) -> Result<Option<String>> {
        let previous = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| stored_accounts(&v))
            .unwrap_or_default();

        let mut on_disk = self.clone();
        let mut accounts = Vec::new();
        let mut warning = None;
        if self.general.use_keychain {
            match crate::secrets::externalize(&mut on_disk, store) {
                Ok(a) => accounts = a,
                Err(e) => {
                    warning = Some(format!(
                        "Couldn't use the system keychain ({e}); secrets are saved in settings.json instead. Turn off \"Store API keys and secrets in the system keychain\" in Settings → General to stop this message."
                    ));
                    on_disk = self.clone();
                }
            }
        }
        let mut doc = serde_json::to_value(&on_disk)?;
        doc[KEY_IN_KEYCHAIN] = serde_json::json!(self.general.use_keychain && warning.is_none());
        doc[KEY_ACCOUNTS] = serde_json::json!(accounts);
        write_private(path, serde_json::to_string_pretty(&doc)?.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        crate::secrets::remove_stale(store, &previous, &accounts);
        Ok(warning)
    }

    pub fn normalize(&mut self) {
        if self.providers.is_empty() {
            self.providers.push(ProviderConfig::default());
        }
        if self.active_provider >= self.providers.len() {
            self.active_provider = 0;
        }
        self.general.zoom = self.general.zoom.clamp(0.5, 3.0);
    }

    pub fn provider(&self) -> &ProviderConfig {
        &self.providers[self.active_provider.min(self.providers.len() - 1)]
    }

    pub fn provider_mut(&mut self) -> &mut ProviderConfig {
        let idx = self.active_provider.min(self.providers.len() - 1);
        &mut self.providers[idx]
    }
}

const KEY_IN_KEYCHAIN: &str = "secrets_in_keychain";
const KEY_ACCOUNTS: &str = "keychain_accounts";

fn stored_accounts(raw: &serde_json::Value) -> Vec<String> {
    raw.get(KEY_ACCOUNTS)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub struct LoadedSettings {
    pub settings: Settings,
    /// Something the user should know (keychain read failures, unreadable file).
    pub notice: Option<String>,
    /// Secrets are still in plain text but should move to the keychain; save once to migrate.
    pub needs_migration: bool,
}

/// Write a file atomically; on Unix restrict it to the current user since it may hold API keys.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Parse a Claude Desktop / VS Code style `{"mcpServers": {...}}` JSON blob.
pub fn import_mcp_json(text: &str) -> Result<Vec<McpServerConfig>> {
    let value: serde_json::Value = serde_json::from_str(text).context("invalid JSON")?;
    let servers = value
        .get("mcpServers")
        .or_else(|| value.get("servers"))
        .unwrap_or(&value)
        .as_object()
        .context("expected an object of servers (e.g. {\"mcpServers\": {...}})")?;

    let kv = |v: Option<&serde_json::Value>| -> Vec<KeyValue> {
        v.and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| KeyValue {
                        key: k.clone(),
                        value: v
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut out = Vec::new();
    for (name, cfg) in servers {
        let transport = if let Some(command) = cfg.get("command").and_then(|c| c.as_str()) {
            McpTransportConfig::Stdio {
                command: command.to_owned(),
                args: cfg
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                env: kv(cfg.get("env")),
                cwd: cfg
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            }
        } else if let Some(url) = cfg.get("url").and_then(|u| u.as_str()) {
            McpTransportConfig::Http {
                url: url.to_owned(),
                headers: kv(cfg.get("headers")),
            }
        } else {
            anyhow::bail!("server '{name}' has neither 'command' nor 'url'");
        };
        out.push(McpServerConfig {
            name: name.clone(),
            enabled: !cfg
                .get("disabled")
                .and_then(|d| d.as_bool())
                .unwrap_or(false),
            transport,
            ..Default::default()
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_claude_desktop_format() {
        let json = r#"{"mcpServers": {
            "fs": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"], "env": {"A": "1"}},
            "remote": {"url": "https://example.com/mcp", "headers": {"Authorization": "Bearer x"}}
        }}"#;
        let servers = import_mcp_json(json).unwrap();
        assert_eq!(servers.len(), 2);
        let fs = servers.iter().find(|s| s.name == "fs").unwrap();
        match &fs.transport {
            McpTransportConfig::Stdio {
                command, args, env, ..
            } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
                assert_eq!(env[0].key, "A");
            }
            _ => panic!("expected stdio"),
        }
        let remote = servers.iter().find(|s| s.name == "remote").unwrap();
        assert!(matches!(remote.transport, McpTransportConfig::Http { .. }));
    }

    #[test]
    fn keychain_save_load_migrate_and_fallback() {
        use crate::secrets::tests::{MemoryStore, sample};
        let dir = std::env::temp_dir().join(format!("cleanrstai-cfg-{}", uuid::Uuid::new_v4()));
        let path = dir.join("settings.json");
        let store = MemoryStore::default();

        // A legacy plain-text file needs migrating.
        let legacy = sample();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
        let loaded = Settings::load_from(&path, &store);
        assert!(loaded.needs_migration);
        assert_eq!(loaded.settings, legacy);

        // Saving moves secrets out of the file; loading puts them back.
        assert_eq!(legacy.save_to(&path, &store).unwrap(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("sk-1") && text.contains("\"secrets_in_keychain\": true"),
            "{text}"
        );
        let loaded = Settings::load_from(&path, &store);
        assert!(!loaded.needs_migration && loaded.notice.is_none());
        assert_eq!(loaded.settings, legacy);

        // Removing a provider deletes its keychain entry.
        let mut fewer = legacy.clone();
        fewer.providers = vec![ProviderConfig::default()];
        fewer.save_to(&path, &store).unwrap();
        assert!(
            !store
                .items
                .lock()
                .unwrap()
                .contains_key("provider:p1:api_key")
        );

        // A missing keychain entry is reported.
        legacy.save_to(&path, &store).unwrap();
        store.items.lock().unwrap().clear();
        assert!(Settings::load_from(&path, &store).notice.is_some());

        // A failing keychain falls back to plain text with a warning.
        let broken = MemoryStore {
            fail: true,
            ..Default::default()
        };
        assert!(legacy.save_to(&path, &broken).unwrap().is_some());
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-1"));

        // Turning the keychain off writes plain text and clears stored entries.
        let mut off = legacy.clone();
        off.general.use_keychain = false;
        off.save_to(&path, &store).unwrap(); // previous save had no accounts
        legacy.save_to(&path, &store).unwrap();
        assert!(!store.items.lock().unwrap().is_empty());
        off.save_to(&path, &store).unwrap();
        assert!(store.items.lock().unwrap().is_empty());
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_roundtrip_with_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert_eq!(s.general.zoom, 1.5);
        assert!(s.general.send_with_enter);
        assert_eq!(s.providers.len(), 1);
    }
}

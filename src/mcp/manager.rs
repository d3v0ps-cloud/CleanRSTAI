//! Owns the live MCP connections and keeps them in sync with the settings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use super::{McpClient, McpTool, ToolOutput};
use crate::config::McpServerConfig;
use crate::llm::ToolSpec;

#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    Disabled,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Clone)]
pub struct ServerView {
    pub id: String,
    pub status: ServerStatus,
    pub tools: Vec<McpTool>,
    pub server_info: String,
    pub stderr: Vec<String>,
}

struct Entry {
    config: McpServerConfig,
    status: ServerStatus,
    tools: Vec<McpTool>,
    client: Option<Arc<McpClient>>,
    generation: u64,
    task: Option<JoinHandle<()>>,
}

impl Entry {
    fn stop(&mut self, rt: &Handle) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        if let Some(c) = self.client.take() {
            rt.spawn(async move { c.shutdown().await });
        }
        self.tools.clear();
        self.generation += 1;
    }
}

pub struct McpManager {
    entries: Mutex<Vec<Entry>>,
    rt: Handle,
    http: reqwest::Client,
    on_change: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// Tools offered to the model for one request, plus how to route each call.
#[derive(Default, Clone)]
pub struct ToolRouter {
    pub specs: Vec<ToolSpec>,
    routes: HashMap<String, (Arc<McpClient>, String)>,
}

impl ToolRouter {
    pub async fn call(
        &self,
        function: &str,
        arguments: &str,
        timeout: Duration,
    ) -> Result<ToolOutput> {
        let (client, tool) = self
            .routes
            .get(function)
            .ok_or_else(|| anyhow!("unknown tool '{function}' (is its server still connected?)"))?;
        let args: Value = if arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments).context("tool arguments are not valid JSON")?
        };
        client.call_tool(tool, args, timeout).await
    }
}

/// OpenAI function names must match `^[a-zA-Z0-9_-]{1,64}$`.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// JSON schema the model APIs accept: an object, with `properties` present.
fn normalize_schema(schema: &Value) -> Value {
    let mut s = if schema.is_object() {
        schema.clone()
    } else {
        json!({})
    };
    if s.get("type").is_none() {
        s["type"] = json!("object");
    }
    if s.get("properties").is_none() {
        s["properties"] = json!({});
    }
    s
}

impl McpManager {
    pub fn new(rt: Handle, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(Vec::new()),
            rt,
            http,
            on_change: Mutex::new(None),
        })
    }

    pub fn set_on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_change.lock().unwrap() = Some(Box::new(f));
    }

    fn changed(&self) {
        if let Some(f) = self.on_change.lock().unwrap().as_ref() {
            f();
        }
    }

    /// Reconcile live connections with `configs`, (re)connecting only what changed.
    pub fn sync(self: &Arc<Self>, configs: &[McpServerConfig]) {
        let mut to_connect = Vec::new();
        {
            let mut entries = self.entries.lock().unwrap();
            let mut old: Vec<Entry> = std::mem::take(&mut *entries);
            for cfg in configs {
                let entry = match old.iter().position(|e| e.config.id == cfg.id) {
                    Some(pos) => {
                        let mut e = old.swap_remove(pos);
                        let reconnect = e.config.needs_reconnect(cfg)
                            || (cfg.enabled && matches!(e.status, ServerStatus::Disabled));
                        e.config = cfg.clone();
                        if reconnect {
                            e.stop(&self.rt);
                            e.status = ServerStatus::Disabled;
                            if cfg.enabled {
                                to_connect.push(cfg.id.clone());
                            }
                        }
                        e
                    }
                    None => {
                        if cfg.enabled {
                            to_connect.push(cfg.id.clone());
                        }
                        Entry {
                            config: cfg.clone(),
                            status: ServerStatus::Disabled,
                            tools: Vec::new(),
                            client: None,
                            generation: 0,
                            task: None,
                        }
                    }
                };
                entries.push(entry);
            }
            for mut removed in old {
                removed.stop(&self.rt);
            }
        }
        for id in to_connect {
            self.connect(&id);
        }
        self.changed();
    }

    pub fn connect(self: &Arc<Self>, id: &str) {
        let mut entries = self.entries.lock().unwrap();
        let Some(e) = entries.iter_mut().find(|e| e.config.id == id) else {
            return;
        };
        e.stop(&self.rt);
        if !e.config.enabled {
            e.status = ServerStatus::Disabled;
            return;
        }
        e.status = ServerStatus::Connecting;
        let generation = e.generation;
        let transport = e.config.transport.clone();
        let (this, id, http) = (self.clone(), id.to_owned(), self.http.clone());
        e.task = Some(self.rt.spawn(async move {
            let result = async {
                let client = McpClient::connect(&transport, http).await?;
                let tools = client.list_tools().await.context("listing tools")?;
                Ok::<_, anyhow::Error>((Arc::new(client), tools))
            }
            .await;
            {
                let mut entries = this.entries.lock().unwrap();
                if let Some(e) = entries
                    .iter_mut()
                    .find(|e| e.config.id == id && e.generation == generation)
                {
                    e.task = None;
                    match result {
                        Ok((client, tools)) => {
                            e.status = ServerStatus::Connected;
                            e.tools = tools;
                            e.client = Some(client);
                        }
                        Err(err) => {
                            log::warn!("MCP server '{}' failed: {err:#}", e.config.name);
                            e.status = ServerStatus::Error(format!("{err:#}"));
                        }
                    }
                }
            }
            this.changed();
        }));
    }

    /// Re-fetch the tool list of a connected server.
    pub fn refresh_tools(self: &Arc<Self>, id: &str) {
        let client = {
            let entries = self.entries.lock().unwrap();
            entries
                .iter()
                .find(|e| e.config.id == id)
                .and_then(|e| e.client.clone().map(|c| (c, e.generation)))
        };
        let Some((client, generation)) = client else {
            return self.connect(id);
        };
        let (this, id) = (self.clone(), id.to_owned());
        self.rt.spawn(async move {
            let result = client.list_tools().await;
            if let Some(e) = this
                .entries
                .lock()
                .unwrap()
                .iter_mut()
                .find(|e| e.config.id == id && e.generation == generation)
            {
                match result {
                    Ok(tools) => e.tools = tools,
                    Err(err) => e.status = ServerStatus::Error(format!("{err:#}")),
                }
            }
            this.changed();
        });
    }

    pub fn snapshot(&self) -> Vec<ServerView> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| ServerView {
                id: e.config.id.clone(),
                status: e.status.clone(),
                tools: e.tools.clone(),
                server_info: e
                    .client
                    .as_ref()
                    .map(|c| {
                        format!("{} {}", c.server_name, c.server_version)
                            .trim()
                            .to_owned()
                    })
                    .unwrap_or_default(),
                stderr: e
                    .client
                    .as_ref()
                    .map(|c| c.stderr_tail())
                    .unwrap_or_default(),
            })
            .collect()
    }

    pub fn connected_tool_count(&self) -> (usize, usize) {
        let entries = self.entries.lock().unwrap();
        let servers = entries.iter().filter(|e| e.client.is_some()).count();
        let tools = entries
            .iter()
            .filter(|e| e.client.is_some())
            .map(|e| {
                e.tools
                    .iter()
                    .filter(|t| !e.config.disabled_tools.contains(&t.name))
                    .count()
            })
            .sum();
        (servers, tools)
    }

    pub fn router(&self) -> ToolRouter {
        let entries = self.entries.lock().unwrap();
        let mut router = ToolRouter::default();
        for e in entries.iter() {
            let Some(client) = &e.client else { continue };
            let prefix = sanitize(&e.config.name);
            for tool in &e.tools {
                if e.config.disabled_tools.contains(&tool.name) {
                    continue;
                }
                let base: String = format!("{prefix}__{}", sanitize(&tool.name))
                    .chars()
                    .take(64)
                    .collect();
                let mut name = base.clone();
                let mut n = 2;
                while router.routes.contains_key(&name) {
                    let suffix = format!("_{n}");
                    name = format!("{}{suffix}", &base[..base.len().min(64 - suffix.len())]);
                    n += 1;
                }
                router.specs.push(ToolSpec {
                    name: name.clone(),
                    description: if tool.description.is_empty() {
                        format!("Tool '{}' from MCP server '{}'", tool.name, e.config.name)
                    } else {
                        tool.description.clone()
                    },
                    parameters: normalize_schema(&tool.input_schema),
                });
                router
                    .routes
                    .insert(name, (client.clone(), tool.name.clone()));
            }
        }
        router
    }

    /// Display label for a routed function name (`server__tool` → `server › tool`).
    pub fn display_name(function: &str) -> String {
        match function.split_once("__") {
            Some((server, tool)) => format!("{server} › {tool}"),
            None => function.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_and_schema() {
        assert_eq!(sanitize("my server.v2"), "my_server_v2");
        let s = normalize_schema(&json!({"type": "object"}));
        assert_eq!(s["properties"], json!({}));
        let s = normalize_schema(&Value::Null);
        assert_eq!(s["type"], "object");
    }
}

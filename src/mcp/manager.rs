//! Owns the live MCP connections and keeps them in sync with the settings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use serde_json::{Value, json};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use super::{McpClient, McpPrompt, McpResource, McpTool, NotifyFn, ToolOutput};
use crate::config::McpServerConfig;
use crate::llm::ToolSpec;

#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    Disabled,
    Connecting,
    Connected,
    /// The server needs an OAuth sign-in before it can be used.
    NeedsSignIn,
    /// Waiting for the user to finish signing in in their browser.
    SigningIn,
    Error(String),
}

#[derive(Clone)]
pub struct ServerView {
    pub id: String,
    pub status: ServerStatus,
    pub tools: Vec<McpTool>,
    pub resources: Vec<McpResource>,
    pub prompts: Vec<McpPrompt>,
    /// Holds OAuth tokens for this (HTTP) server.
    pub signed_in: bool,
    pub server_info: String,
    pub stderr: Vec<String>,
}

struct Entry {
    config: McpServerConfig,
    status: ServerStatus,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    client: Option<Arc<McpClient>>,
    /// `WWW-Authenticate` from the server's last 401, used to start sign-in.
    auth_hint: Option<String>,
    signed_in: bool,
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
        self.resources.clear();
        self.prompts.clear();
        self.generation += 1;
    }
}

pub type UrlOpener = Arc<dyn Fn(&str) + Send + Sync>;

pub struct McpManager {
    entries: Mutex<Vec<Entry>>,
    rt: Handle,
    http: reqwest::Client,
    on_change: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Where OAuth tokens are kept (keychain, or a private file without one).
    tokens: super::oauth::Store,
    open_url: Mutex<UrlOpener>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum List {
    Tools,
    Resources,
    Prompts,
    All,
}

/// Tools offered to the model for one request, plus how to route each call.
#[derive(Default, Clone)]
pub struct ToolRouter {
    pub specs: Vec<ToolSpec>,
    routes: HashMap<String, (Arc<McpClient>, String)>,
    /// Function name → (server id, MCP tool name).
    origins: HashMap<String, (String, String)>,
}

impl ToolRouter {
    /// Keep only the tools for which `keep(server_id, tool_name)` is true.
    pub fn retain(&mut self, keep: impl Fn(&str, &str) -> bool) {
        let origins = &self.origins;
        let dropped: Vec<String> = self
            .specs
            .iter()
            .filter(|s| {
                origins
                    .get(&s.name)
                    .is_none_or(|(server, tool)| !keep(server, tool))
            })
            .map(|s| s.name.clone())
            .collect();
        self.specs.retain(|s| !dropped.contains(&s.name));
        for name in dropped {
            self.routes.remove(&name);
            self.origins.remove(&name);
        }
    }

    /// The server id and MCP tool name behind a routed function name.
    pub fn origin(&self, function: &str) -> Option<(&str, &str)> {
        self.origins
            .get(function)
            .map(|(server, tool)| (server.as_str(), tool.as_str()))
    }

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
        Self::with_token_store(rt, http, crate::secrets::durable_store())
    }

    pub fn with_token_store(
        rt: Handle,
        http: reqwest::Client,
        tokens: super::oauth::Store,
    ) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(Vec::new()),
            rt,
            http,
            on_change: Mutex::new(None),
            tokens,
            open_url: Mutex::new(Arc::new(|url: &str| {
                if let Err(e) = open::that_detached(url) {
                    log::error!("couldn't open the browser: {e}");
                }
            })),
        })
    }

    /// Replace how sign-in URLs are opened (tests drive the flow without a browser).
    #[cfg(test)]
    pub fn set_url_opener(&self, f: UrlOpener) {
        *self.open_url.lock().unwrap() = f;
    }

    /// Run the browser sign-in for a Streamable HTTP server, then reconnect.
    pub fn sign_in(self: &Arc<Self>, id: &str) {
        let (url, client_id, hint, generation) = {
            let mut entries = self.entries.lock().unwrap();
            let Some(e) = entries.iter_mut().find(|e| e.config.id == id) else {
                return;
            };
            let crate::config::McpTransportConfig::Http {
                url,
                oauth_client_id,
                ..
            } = &e.config.transport
            else {
                return;
            };
            let (url, client_id) = (url.clone(), oauth_client_id.clone());
            e.stop(&self.rt);
            e.status = ServerStatus::SigningIn;
            (url, client_id, e.auth_hint.clone(), e.generation)
        };
        self.changed();
        let (this, task_id) = (self.clone(), id.to_owned());
        let opener = self.open_url.lock().unwrap().clone();
        let task = self.rt.spawn(async move {
            let result = super::oauth::sign_in(
                &this.http,
                &url,
                hint.as_deref(),
                Some(client_id.as_str()),
                |u| opener(u),
            )
            .await
            .and_then(|t| super::oauth::save_tokens(this.tokens.as_ref(), &task_id, &t));
            let id = task_id;
            match result {
                Ok(()) => this.connect(&id),
                Err(err) => {
                    if let Some(e) = this
                        .entries
                        .lock()
                        .unwrap()
                        .iter_mut()
                        .find(|e| e.config.id == id && e.generation == generation)
                    {
                        e.status = ServerStatus::Error(format!("Sign-in failed: {err:#}"));
                        e.task = None;
                    }
                    this.changed();
                }
            }
        });
        if let Some(e) = self
            .entries
            .lock()
            .unwrap()
            .iter_mut()
            .find(|e| e.config.id == id)
        {
            e.task = Some(task);
        }
    }

    /// Forget the sign-in tokens for a server and reconnect (which will ask to sign in).
    pub fn sign_out(self: &Arc<Self>, id: &str) {
        super::oauth::delete_tokens(self.tokens.as_ref(), id);
        self.connect(id);
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
                            resources: Vec::new(),
                            prompts: Vec::new(),
                            client: None,
                            auth_hint: None,
                            signed_in: false,
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
        let on_notify = self.notify_handler(&id, generation);
        let auth = matches!(transport, crate::config::McpTransportConfig::Http { .. }).then(|| {
            Arc::new(super::oauth::Auth::new(
                &id,
                self.tokens.clone(),
                self.http.clone(),
            ))
        });
        e.signed_in = auth.as_ref().is_some_and(|a| a.signed_in);
        e.task = Some(self.rt.spawn(async move {
            let result = async {
                let client = McpClient::connect(&transport, http, on_notify, auth).await?;
                let tools = client.list_tools().await.context("listing tools")?;
                // Resources and prompts are optional extras; don't fail the connection on them.
                let resources = client.list_resources().await.unwrap_or_else(|e| {
                    log::warn!("listing resources: {e:#}");
                    Vec::new()
                });
                let prompts = client.list_prompts().await.unwrap_or_else(|e| {
                    log::warn!("listing prompts: {e:#}");
                    Vec::new()
                });
                Ok::<_, anyhow::Error>((Arc::new(client), tools, resources, prompts))
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
                        Ok((client, tools, resources, prompts)) => {
                            e.status = ServerStatus::Connected;
                            e.tools = tools;
                            e.resources = resources;
                            e.prompts = prompts;
                            e.client = Some(client);
                        }
                        Err(err) if super::oauth::is_auth_required(&err) => {
                            e.auth_hint = err
                                .chain()
                                .find_map(|c| c.downcast_ref::<super::oauth::AuthRequired>())
                                .and_then(|a| a.www_authenticate.clone());
                            e.status = ServerStatus::NeedsSignIn;
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

    /// React to `notifications/*/list_changed` from a server by re-fetching that list.
    fn notify_handler(self: &Arc<Self>, id: &str, generation: u64) -> NotifyFn {
        let this = Arc::downgrade(self);
        let id = id.to_owned();
        Arc::new(move |method: &str| {
            let list = match method {
                "notifications/tools/list_changed" => List::Tools,
                "notifications/resources/list_changed" => List::Resources,
                "notifications/prompts/list_changed" => List::Prompts,
                _ => return,
            };
            if let Some(this) = this.upgrade() {
                this.refresh(&id, generation, list);
            }
        })
    }

    /// Re-fetch tools, resources and prompts of a connected server.
    pub fn refresh_tools(self: &Arc<Self>, id: &str) {
        let generation = {
            let entries = self.entries.lock().unwrap();
            entries
                .iter()
                .find(|e| e.config.id == id && e.client.is_some())
                .map(|e| e.generation)
        };
        match generation {
            Some(g) => self.refresh(id, g, List::All),
            None => self.connect(id),
        }
    }

    fn refresh(self: &Arc<Self>, id: &str, generation: u64, list: List) {
        let client = {
            let entries = self.entries.lock().unwrap();
            entries
                .iter()
                .find(|e| e.config.id == id && e.generation == generation)
                .and_then(|e| e.client.clone())
        };
        let Some(client) = client else { return };
        let (this, id) = (self.clone(), id.to_owned());
        self.rt.spawn(async move {
            let want = |l: List| list == List::All || list == l;
            let tools = if want(List::Tools) {
                Some(client.list_tools().await)
            } else {
                None
            };
            let resources = if want(List::Resources) {
                Some(client.list_resources().await)
            } else {
                None
            };
            let prompts = if want(List::Prompts) {
                Some(client.list_prompts().await)
            } else {
                None
            };
            if let Some(e) = this
                .entries
                .lock()
                .unwrap()
                .iter_mut()
                .find(|e| e.config.id == id && e.generation == generation)
            {
                match tools {
                    Some(Ok(t)) => e.tools = t,
                    Some(Err(err)) => e.status = ServerStatus::Error(format!("{err:#}")),
                    None => {}
                }
                if let Some(Ok(r)) = resources {
                    e.resources = r;
                }
                if let Some(Ok(p)) = prompts {
                    e.prompts = p;
                }
            }
            this.changed();
        });
    }

    /// The live client for a server, for one-off calls (reading a resource, getting a prompt).
    pub fn client(&self, id: &str) -> Option<Arc<McpClient>> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.config.id == id)
            .and_then(|e| e.client.clone())
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
                resources: e.resources.clone(),
                prompts: e.prompts.clone(),
                signed_in: e.signed_in,
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

    /// Every tool of every connected server, including ones switched off in Settings;
    /// callers filter with [`ToolRouter::retain`].
    pub fn router(&self) -> ToolRouter {
        let entries = self.entries.lock().unwrap();
        let mut router = ToolRouter::default();
        for e in entries.iter() {
            let Some(client) = &e.client else { continue };
            let prefix = sanitize(&e.config.name);
            for tool in &e.tools {
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
                    .origins
                    .insert(name.clone(), (e.config.id.clone(), tool.name.clone()));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn http_list_changed_notification_refreshes_tools() {
        use crate::testutil::{Response, serve};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let lists = Arc::new(AtomicUsize::new(0));
        let seen = lists.clone();
        let (base, _) = serve(move |r| {
            if r.method == "GET" {
                // The server-push stream: announce that the tool list changed.
                return Response::with_type(
                    200,
                    "text/event-stream",
                    "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n".into(),
                );
            }
            let req = r.json();
            let id = req["id"].clone();
            match req["method"].as_str().unwrap_or("") {
                "initialize" => Response::json(json!({"jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {"listChanged": true}},
                    "serverInfo": {"name": "mock", "version": "1"}}})),
                "tools/list" => {
                    let n = seen.fetch_add(1, Ordering::SeqCst);
                    let name = if n == 0 { "old_tool" } else { "new_tool" };
                    Response::json(json!({"jsonrpc": "2.0", "id": id,
                        "result": {"tools": [{"name": name, "inputSchema": {"type": "object"}}]}}))
                }
                _ => Response::status(202),
            }
        })
        .await;
        let mcp = McpManager::new(Handle::current(), crate::llm::build_http_client());
        mcp.sync(&[McpServerConfig {
            id: "s".into(),
            name: "mock".into(),
            transport: crate::config::McpTransportConfig::Http {
                url: format!("{base}/mcp"),
                headers: vec![],
                oauth_client_id: String::new(),
            },
            ..Default::default()
        }]);
        for _ in 0..100 {
            let v = mcp.snapshot();
            if v[0].tools.iter().any(|t| t.name == "new_tool") {
                assert!(lists.load(Ordering::SeqCst) >= 2);
                return;
            }
            if let ServerStatus::Error(e) = &v[0].status {
                panic!("{e}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("tool list was not refreshed: {:?}", mcp.snapshot()[0].tools);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oauth_sign_in_refresh_and_sign_out() {
        use crate::secrets::tests::MemoryStore;
        use crate::testutil::{Response, serve};
        use base64::Engine as _;
        use sha2::Digest as _;
        use std::collections::HashSet;

        #[derive(Default)]
        struct State {
            base: String,
            challenge: String,
            valid: HashSet<String>,
            refresh_ok: bool,
            registered: usize,
            resource: String,
        }
        let state: Arc<Mutex<State>> = Arc::default();
        state.lock().unwrap().refresh_ok = true;
        let st = state.clone();
        let form = |body: &[u8]| -> HashMap<String, String> {
            reqwest::Url::parse(&format!("http://x/?{}", String::from_utf8_lossy(body)))
                .unwrap()
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        };
        let (base, _) = serve(move |r| {
            let mut s = st.lock().unwrap();
            let base = s.base.clone();
            let path = r.path.split('?').next().unwrap_or("");
            match (r.method.as_str(), path) {
                ("GET", "/.well-known/oauth-protected-resource" | "/.well-known/oauth-protected-resource/mcp") => {
                    Response::json(json!({"resource": format!("{base}/mcp"), "authorization_servers": [base]}))
                }
                ("GET", "/.well-known/oauth-authorization-server") => Response::json(json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                    "code_challenge_methods_supported": ["S256"],
                })),
                ("POST", "/register") => {
                    s.registered += 1;
                    let uris = r.json()["redirect_uris"].clone();
                    assert!(uris[0].as_str().unwrap().starts_with("http://127.0.0.1:"));
                    Response::json(json!({"client_id": "cid"}))
                }
                ("GET", "/authorize") => {
                    let q: HashMap<String, String> = reqwest::Url::parse(&format!("http://x{}", r.path))
                        .unwrap()
                        .query_pairs()
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect();
                    assert_eq!(q["client_id"], "cid");
                    assert_eq!(q["code_challenge_method"], "S256");
                    s.challenge = q["code_challenge"].clone();
                    s.resource = q["resource"].clone();
                    let loc = format!("{}?code=code-1&state={}", q["redirect_uri"], q["state"]);
                    Response::status(302).header("location", &loc)
                }
                ("POST", "/token") => {
                    let f = form(&r.body);
                    match f["grant_type"].as_str() {
                        "authorization_code" => {
                            let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD
                                .encode(sha2::Sha256::digest(f["code_verifier"].as_bytes()));
                            assert_eq!(expect, s.challenge, "PKCE verifier matches challenge");
                            assert_eq!(f["code"], "code-1");
                            assert_eq!(f["resource"], s.resource);
                            s.valid.insert("at-1".into());
                            Response::json(json!({"access_token": "at-1", "refresh_token": "rt-1",
                                "expires_in": 3600, "token_type": "Bearer"}))
                        }
                        "refresh_token" if s.refresh_ok && f["refresh_token"] == "rt-1" => {
                            s.valid.insert("at-2".into());
                            Response::json(json!({"access_token": "at-2", "expires_in": 3600, "token_type": "Bearer"}))
                        }
                        _ => Response::with_type(400, "application/json", json!({"error": "invalid_grant"}).to_string()),
                    }
                }
                ("POST", "/mcp") => {
                    let ok = r
                        .header("authorization")
                        .and_then(|h| h.strip_prefix("Bearer "))
                        .is_some_and(|t| s.valid.contains(t));
                    if !ok {
                        return Response::status(401).header(
                            "www-authenticate",
                            &format!(r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#),
                        );
                    }
                    let req = r.json();
                    let id = req["id"].clone();
                    match req["method"].as_str().unwrap_or("") {
                        "initialize" => Response::json(json!({"jsonrpc": "2.0", "id": id, "result": {
                            "protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                            "serverInfo": {"name": "secure", "version": "1"}}})),
                        "tools/list" => Response::json(json!({"jsonrpc": "2.0", "id": id,
                            "result": {"tools": [{"name": "secret_tool", "inputSchema": {"type": "object"}}]}})),
                        _ => Response::status(202),
                    }
                }
                _ => Response::status(405),
            }
        })
        .await;
        state.lock().unwrap().base = base.clone();

        let store: Arc<MemoryStore> = Arc::default();
        let mcp = McpManager::with_token_store(
            Handle::current(),
            crate::llm::build_http_client(),
            store.clone(),
        );
        // Stand-in for the browser: follow the authorization redirect to our loopback listener.
        mcp.set_url_opener(Arc::new(|url: &str| {
            let url = url.to_owned();
            tokio::spawn(async move {
                let http = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap();
                let resp = http.get(&url).send().await.unwrap();
                let loc = resp.headers()["location"].to_str().unwrap().to_owned();
                http.get(&loc).send().await.unwrap();
            });
        }));
        let cfg = McpServerConfig {
            id: "sec".into(),
            name: "secure".into(),
            transport: crate::config::McpTransportConfig::Http {
                url: format!("{base}/mcp"),
                headers: vec![],
                oauth_client_id: String::new(),
            },
            ..Default::default()
        };
        let wait_for = |want: ServerStatus| {
            let mcp = mcp.clone();
            async move {
                for _ in 0..200 {
                    let v = mcp.snapshot().remove(0);
                    if v.status == want {
                        return v;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                panic!("expected {want:?}, got {:?}", mcp.snapshot()[0].status);
            }
        };
        let stored = |s: &MemoryStore| -> Option<super::super::oauth::Tokens> {
            s.items
                .lock()
                .unwrap()
                .get("mcp:sec:oauth")
                .map(|j| serde_json::from_str(j).unwrap())
        };

        // 1. No token: the server asks for sign-in.
        mcp.sync(std::slice::from_ref(&cfg));
        wait_for(ServerStatus::NeedsSignIn).await;

        // 2. Sign in → discovery, registration, PKCE, code exchange → connected.
        mcp.sign_in("sec");
        let v = wait_for(ServerStatus::Connected).await;
        assert_eq!(v.tools[0].name, "secret_tool");
        assert!(v.signed_in);
        let t = stored(&store).expect("tokens saved");
        assert_eq!(t.access_token, "at-1");
        assert_eq!(t.resource, format!("{base}/mcp"));
        assert_eq!(state.lock().unwrap().registered, 1);

        // 3. An expired token is refreshed before use.
        let mut expired = t.clone();
        expired.expires_at = Some(0);
        super::super::oauth::save_tokens(store.as_ref(), "sec", &expired).unwrap();
        mcp.connect("sec");
        wait_for(ServerStatus::Connected).await;
        assert_eq!(stored(&store).unwrap().access_token, "at-2");
        assert_eq!(
            stored(&store).unwrap().refresh_token.as_deref(),
            Some("rt-1"),
            "refresh token kept"
        );

        // 4. Revoked tokens and a failing refresh → back to sign-in required.
        {
            let mut s = state.lock().unwrap();
            s.valid.clear();
            s.refresh_ok = false;
        }
        mcp.connect("sec");
        wait_for(ServerStatus::NeedsSignIn).await;

        // 5. Sign out forgets the tokens.
        mcp.sign_out("sec");
        wait_for(ServerStatus::NeedsSignIn).await;
        assert!(stored(&store).is_none());
    }
}

//! Model Context Protocol client (JSON-RPC 2.0) over stdio or Streamable HTTP.

pub mod http;
pub mod manager;
pub mod oauth;
pub mod stdio;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config::McpTransportConfig;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

const INIT_TIMEOUT: Duration = Duration::from_secs(120);

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": "CleanRSTAI", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// The protocol version the server chose in its `initialize` result.
fn negotiated_version(init: &Value) -> &str {
    init.get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_VERSION)
}

/// Called with the method name of every notification the server sends.
pub type NotifyFn = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpPrompt {
    pub name: String,
    pub description: String,
    pub arguments: Vec<PromptArgument>,
}

/// A message produced by `prompts/get`.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptMessage {
    pub role: crate::types::Role,
    pub text: String,
    pub attachments: Vec<crate::attach::Attachment>,
}

#[derive(Clone, Debug)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    /// Images the tool returned, shown in the chat.
    pub images: Vec<crate::attach::Attachment>,
}

enum Transport {
    Stdio(Box<stdio::StdioTransport>),
    Http(http::HttpTransport),
}

pub struct McpClient {
    transport: Transport,
    next_id: AtomicU64,
    pub server_name: String,
    pub server_version: String,
    pub instructions: Option<String>,
    /// The server's `capabilities` from `initialize`.
    pub capabilities: Value,
    /// Held while an expired HTTP session is being replaced by a new one.
    renewing: tokio::sync::Mutex<()>,
}

/// Turn a JSON-RPC response into its `result`, or an error.
pub(crate) fn into_result(resp: Value) -> Result<Value> {
    if let Some(err) = resp.get("error") {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        bail!("MCP error {code}: {msg}");
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

/// Reply for requests the server sends us. We advertise no client capabilities, so
/// only `ping` is supported.
pub(crate) fn reply_to_server_request(req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    match req.get("method").and_then(|m| m.as_str()) {
        Some("ping") => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        _ => json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": "Method not found"}}),
    }
}

/// Extract the numeric id of a JSON-RPC response (we only ever send numeric ids).
pub(crate) fn response_id(msg: &Value) -> Option<u64> {
    if msg.get("method").is_some() {
        return None;
    }
    match msg.get("id")? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

impl McpClient {
    pub async fn connect(
        cfg: &McpTransportConfig,
        http: reqwest::Client,
        on_notify: NotifyFn,
        auth: Option<Arc<oauth::Auth>>,
    ) -> Result<Self> {
        let transport = match cfg {
            McpTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => Transport::Stdio(Box::new(
                stdio::StdioTransport::spawn(command, args, env, cwd, on_notify).await?,
            )),
            McpTransportConfig::Http { url, headers, .. } => Transport::Http(
                http::HttpTransport::new(http, url, headers, on_notify, auth)?,
            ),
        };
        let mut client = Self {
            transport,
            next_id: AtomicU64::new(1),
            server_name: String::new(),
            server_version: String::new(),
            instructions: None,
            capabilities: Value::Null,
            renewing: tokio::sync::Mutex::new(()),
        };

        let init = client
            .request("initialize", initialize_params(), INIT_TIMEOUT)
            .await
            .map_err(|e| client.annotate(e))
            .context("initialize failed")?;

        client.server_name = init
            .pointer("/serverInfo/name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        client.server_version = init
            .pointer("/serverInfo/version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        client.capabilities = init.get("capabilities").cloned().unwrap_or(Value::Null);
        client.instructions = init
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if let Transport::Http(h) = &client.transport {
            h.set_protocol_version(negotiated_version(&init));
        }

        client
            .notify("notifications/initialized", None)
            .await
            .context("sending initialized notification")?;
        if let Transport::Http(h) = &client.transport {
            h.start_listening();
        }
        Ok(client)
    }

    /// Attach recent stderr output to an error, which usually explains why a stdio server died.
    fn annotate(&self, e: anyhow::Error) -> anyhow::Error {
        let tail = self.stderr_tail();
        if tail.is_empty() {
            e
        } else {
            let lines: Vec<&str> = tail
                .iter()
                .rev()
                .take(8)
                .rev()
                .map(String::as_str)
                .collect();
            e.context(format!("server stderr:\n{}", lines.join("\n")))
        }
    }

    /// Send a request. When a Streamable HTTP server has forgotten our session, start a new
    /// one (as the spec asks) and send the request once more.
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let Transport::Http(h) = &self.transport else {
            return self.request_once(method, params, timeout).await;
        };
        let session = h.session_id();
        match self.request_once(method, params.clone(), timeout).await {
            Err(e) if e.chain().any(|c| c.is::<http::SessionExpired>()) => {
                self.renew_session(h, session)
                    .await
                    .context("the MCP session expired and a new one couldn't be started")?;
                self.request_once(method, params, timeout).await
            }
            r => r,
        }
    }

    /// Replace the expired session `stale` with a new one, unless a concurrent request
    /// already did.
    async fn renew_session(&self, h: &http::HttpTransport, stale: Option<String>) -> Result<()> {
        let _guard = self.renewing.lock().await;
        if h.session_id() != stale {
            return Ok(());
        }
        log::info!(
            "MCP session of '{}' expired; starting a new one",
            self.server_name
        );
        h.clear_session();
        let init = self
            .request_once("initialize", initialize_params(), INIT_TIMEOUT)
            .await?;
        h.set_protocol_version(negotiated_version(&init));
        self.notify("notifications/initialized", None).await
    }

    async fn request_once(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let fut = async {
            match &self.transport {
                Transport::Stdio(t) => t.request(id, &msg).await,
                Transport::Http(t) => t.request(id, &msg).await,
            }
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(resp) => into_result(resp?),
            Err(_) => {
                if let Transport::Stdio(t) = &self.transport {
                    t.forget(id);
                }
                let _ = self
                    .notify(
                        "notifications/cancelled",
                        Some(json!({"requestId": id, "reason": "timeout"})),
                    )
                    .await;
                Err(anyhow!("'{method}' timed out after {}s", timeout.as_secs()))
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let mut msg = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        match &self.transport {
            Transport::Stdio(t) => t.send(&msg).await,
            Transport::Http(t) => t.notify(&msg).await,
        }
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities
            .get(capability)
            .is_some_and(|c| !c.is_null())
    }

    /// Fetch every page of a paginated list method (`tools/list`, `resources/list`, …).
    async fn list_all(&self, method: &str, key: &str) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let res = self
                .request(method, params, Duration::from_secs(60))
                .await?;
            items.extend(
                res.get(key)
                    .and_then(|t| t.as_array())
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            cursor = res
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                return Ok(items);
            }
        }
    }

    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        if !self.supports("resources") {
            return Ok(Vec::new());
        }
        let items = self.list_all("resources/list", "resources").await?;
        Ok(items
            .iter()
            .filter_map(|r| {
                let uri = r.get("uri")?.as_str()?.to_owned();
                Some(McpResource {
                    name: str_field(r, "title")
                        .or_else(|| str_field(r, "name"))
                        .unwrap_or_else(|| uri.clone()),
                    description: str_field(r, "description").unwrap_or_default(),
                    mime_type: str_field(r, "mimeType"),
                    uri,
                })
            })
            .collect())
    }

    /// Read a resource as attachments: text contents become text files, blobs with an
    /// image type become images, and other blobs (PDFs, Office documents) their text.
    pub async fn read_resource(&self, uri: &str) -> Result<Vec<crate::attach::Attachment>> {
        let res = self
            .request(
                "resources/read",
                json!({"uri": uri}),
                Duration::from_secs(60),
            )
            .await?;
        let mut out = Vec::new();
        for c in res
            .get("contents")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(a) = content_to_attachment(c) {
                out.push(a);
            }
        }
        if out.is_empty() {
            bail!("the resource has no text or image content");
        }
        Ok(out)
    }

    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        if !self.supports("prompts") {
            return Ok(Vec::new());
        }
        let items = self.list_all("prompts/list", "prompts").await?;
        Ok(items
            .iter()
            .filter_map(|p| {
                Some(McpPrompt {
                    name: p.get("name")?.as_str()?.to_owned(),
                    description: str_field(p, "description")
                        .or_else(|| str_field(p, "title"))
                        .unwrap_or_default(),
                    arguments: p
                        .get("arguments")
                        .and_then(|a| a.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|a| {
                            Some(PromptArgument {
                                name: a.get("name")?.as_str()?.to_owned(),
                                description: str_field(a, "description").unwrap_or_default(),
                                required: a
                                    .get("required")
                                    .and_then(|r| r.as_bool())
                                    .unwrap_or(false),
                            })
                        })
                        .collect(),
                })
            })
            .collect())
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: &std::collections::BTreeMap<String, String>,
    ) -> Result<Vec<PromptMessage>> {
        let res = self
            .request(
                "prompts/get",
                json!({"name": name, "arguments": arguments}),
                Duration::from_secs(60),
            )
            .await?;
        Ok(prompt_messages(&res))
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let res = self
                .request("tools/list", params, Duration::from_secs(60))
                .await?;
            for t in res
                .get("tools")
                .and_then(|t| t.as_array())
                .into_iter()
                .flatten()
            {
                let Some(name) = t.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                tools.push(McpTool {
                    name: name.to_owned(),
                    description: t
                        .get("description")
                        .and_then(|d| d.as_str())
                        .or_else(|| t.get("title").and_then(|d| d.as_str()))
                        .unwrap_or_default()
                        .to_owned(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                });
            }
            cursor = res
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: Value,
        timeout: Duration,
    ) -> Result<ToolOutput> {
        let res = self
            .request(
                "tools/call",
                json!({"name": name, "arguments": args}),
                timeout,
            )
            .await?;
        Ok(format_tool_result(&res))
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        match &self.transport {
            Transport::Stdio(t) => t.stderr_lines(),
            Transport::Http(_) => Vec::new(),
        }
    }

    pub async fn shutdown(&self) {
        if let Transport::Http(t) = &self.transport {
            t.close().await;
        }
    }
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
}

/// Turn an MCP resource-contents object (`{uri, mimeType, text | blob}`) into an attachment.
fn content_to_attachment(c: &Value) -> Option<crate::attach::Attachment> {
    let uri = c.get("uri").and_then(|u| u.as_str()).unwrap_or("resource");
    let name = uri
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(uri)
        .to_owned();
    let mime = str_field(c, "mimeType");
    if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
        return Some(crate::attach::Attachment {
            name,
            mime: mime.unwrap_or_else(|| "text/plain".into()),
            text: Some(text.to_owned()),
            data: None,
        });
    }
    let blob = c.get("blob").and_then(|b| b.as_str())?;
    if let Some(mime) = mime.filter(|m| m.starts_with("image/")) {
        return Some(crate::attach::Attachment {
            name,
            mime,
            text: None,
            data: Some(blob.to_owned()),
        });
    }
    // Other blobs (PDFs, Office documents, text in another encoding) as their text.
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .ok()?;
    crate::attach::Attachment::from_bytes(name, bytes).ok()
}

/// Convert a `prompts/get` result into messages.
pub fn prompt_messages(res: &Value) -> Vec<PromptMessage> {
    use crate::types::Role;
    let mut out: Vec<PromptMessage> = Vec::new();
    for m in res
        .get("messages")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
    {
        let role = match m.get("role").and_then(|r| r.as_str()) {
            Some("assistant") => Role::Assistant,
            _ => Role::User,
        };
        let c = m.get("content").cloned().unwrap_or(Value::Null);
        let mut msg = PromptMessage {
            role,
            text: String::new(),
            attachments: Vec::new(),
        };
        match c.get("type").and_then(|t| t.as_str()) {
            Some("text") => msg.text = str_field(&c, "text").unwrap_or_default(),
            Some("image") => {
                if let (Some(data), Some(mime)) = (str_field(&c, "data"), str_field(&c, "mimeType"))
                {
                    msg.attachments.push(crate::attach::Attachment {
                        name: format!("image.{}", mime.rsplit('/').next().unwrap_or("png")),
                        mime,
                        text: None,
                        data: Some(data),
                    });
                }
            }
            Some("resource") => {
                if let Some(a) = c.get("resource").and_then(content_to_attachment) {
                    msg.attachments.push(a);
                }
            }
            _ => {}
        }
        // Merge consecutive messages from the same role.
        match out.last_mut() {
            Some(last) if last.role == msg.role => {
                if !msg.text.is_empty() {
                    if !last.text.is_empty() {
                        last.text.push_str("\n\n");
                    }
                    last.text.push_str(&msg.text);
                }
                last.attachments.extend(msg.attachments);
            }
            _ => out.push(msg),
        }
    }
    out
}

pub fn format_tool_result(res: &Value) -> ToolOutput {
    let is_error = res
        .get("isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    let mut parts = Vec::new();
    let mut images = Vec::new();
    for item in res
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        let ty = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let part = match ty {
            "text" => item
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_owned(),
            "image" => {
                let mime = item
                    .get("mimeType")
                    .and_then(|m| m.as_str())
                    .unwrap_or("image/png");
                let name = format!(
                    "image-{}.{}",
                    images.len() + 1,
                    mime.rsplit('/').next().unwrap_or("png")
                );
                if let Some(data) = item.get("data").and_then(|d| d.as_str()) {
                    images.push(crate::attach::Attachment {
                        name: name.clone(),
                        mime: mime.to_owned(),
                        text: None,
                        data: Some(data.to_owned()),
                    });
                }
                format!("[image: {name}]")
            }
            "audio" => format!(
                "[{ty}: {}]",
                item.get("mimeType")
                    .and_then(|m| m.as_str())
                    .unwrap_or("binary")
            ),
            "resource" => {
                let r = item.get("resource").cloned().unwrap_or(Value::Null);
                match r.get("text").and_then(|t| t.as_str()) {
                    Some(t) => t.to_owned(),
                    None => format!(
                        "[resource: {}]",
                        r.get("uri").and_then(|u| u.as_str()).unwrap_or("?")
                    ),
                }
            }
            "resource_link" => format!(
                "[resource link: {}]",
                item.get("uri").and_then(|u| u.as_str()).unwrap_or("?")
            ),
            _ => item.to_string(),
        };
        parts.push(part);
    }
    if parts.is_empty()
        && let Some(s) = res.get("structuredContent")
    {
        parts.push(s.to_string());
    }
    ToolOutput {
        text: parts.join("\n"),
        is_error,
        images,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_a_new_http_session_when_the_old_one_expires() {
        use crate::testutil::{Response, serve};
        use std::sync::atomic::AtomicUsize;
        let inits = Arc::new(AtomicUsize::new(0));
        let n = inits.clone();
        let (base, log) = serve(move |r| {
            if r.method == "GET" {
                return Response::status(405);
            }
            let req = r.json();
            let id = req["id"].clone();
            let session = r.header("mcp-session-id").unwrap_or_default().to_owned();
            match req["method"].as_str().unwrap_or("") {
                "initialize" => {
                    let k = n.fetch_add(1, Ordering::SeqCst) + 1;
                    Response::json(json!({"jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": PROTOCOL_VERSION, "capabilities": {"tools": {}},
                        "serverInfo": {"name": "mock", "version": "1"}}}))
                    .header("mcp-session-id", &format!("s{k}"))
                }
                // The server restarted and only knows the second session.
                "tools/call" if session != "s2" => Response::status(404),
                "tools/call" => Response::json(json!({"jsonrpc": "2.0", "id": id,
                    "result": {"content": [{"type": "text", "text": "ok"}]}})),
                _ => Response::status(202),
            }
        })
        .await;
        let cfg = McpTransportConfig::Http {
            url: base,
            headers: Vec::new(),
            oauth_client_id: String::new(),
        };
        let client = McpClient::connect(&cfg, reqwest::Client::new(), Arc::new(|_| {}), None)
            .await
            .unwrap();
        let out = client
            .call_tool("echo", json!({}), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(out.text, "ok");
        assert_eq!(inits.load(Ordering::SeqCst), 2);
        // The new session was announced with `notifications/initialized` before the retry.
        let methods: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .map(|r| r.json()["method"].as_str().unwrap_or_default().to_owned())
            .collect();
        let tail = &methods[methods.len() - 4..];
        assert_eq!(
            tail,
            [
                "tools/call",
                "initialize",
                "notifications/initialized",
                "tools/call"
            ]
        );
    }

    #[test]
    fn formats_mixed_content() {
        let out = format_tool_result(&json!({
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "image", "data": "...", "mimeType": "image/png"},
                {"type": "resource", "resource": {"uri": "file:///a", "text": "body"}}
            ],
            "isError": true
        }));
        assert!(out.is_error);
        assert_eq!(out.text, "hello\n[image: image-1.png]\nbody");
        assert_eq!(out.images.len(), 1);
        assert_eq!(out.images[0].mime, "image/png");
        assert_eq!(out.images[0].data.as_deref(), Some("..."));
    }

    #[test]
    fn resource_blobs_become_images_or_document_text() {
        use base64::Engine as _;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let docx = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/documents/report.docx"
        ))
        .unwrap();
        let doc = content_to_attachment(&json!({
            "uri": "file:///reports/report.docx",
            "mimeType": "application/octet-stream",
            "blob": b64(&docx),
        }))
        .unwrap();
        assert_eq!(doc.name, "report.docx");
        assert!(doc.text.unwrap().contains("ZEBRA-7"));

        let img = content_to_attachment(&json!({
            "uri": "file:///a.png", "mimeType": "image/png", "blob": "iVBOR"
        }))
        .unwrap();
        assert_eq!(img.data.as_deref(), Some("iVBOR"));

        // Unrecognised binary data is still left out.
        assert!(
            content_to_attachment(&json!({
                "uri": "file:///a.bin", "blob": b64(&[0, 1, 2, 3])
            }))
            .is_none()
        );
    }

    #[test]
    fn response_ids() {
        assert_eq!(response_id(&json!({"id": 3, "result": {}})), Some(3));
        assert_eq!(response_id(&json!({"id": "4", "result": {}})), Some(4));
        assert_eq!(response_id(&json!({"id": 3, "method": "ping"})), None);
    }

    #[test]
    fn converts_prompt_messages() {
        let msgs = prompt_messages(&json!({"messages": [
            {"role": "user", "content": {"type": "text", "text": "Explain"}},
            {"role": "user", "content": {"type": "resource",
                "resource": {"uri": "file:///src/main.rs", "mimeType": "text/x-rust", "text": "fn main(){}"}}},
            {"role": "assistant", "content": {"type": "text", "text": "Sure"}},
            {"role": "user", "content": {"type": "image", "data": "AAAA", "mimeType": "image/png"}}
        ]}));
        assert_eq!(msgs.len(), 3, "consecutive user messages merge");
        assert_eq!(msgs[0].text, "Explain");
        assert_eq!(msgs[0].attachments[0].name, "main.rs");
        assert_eq!(msgs[0].attachments[0].text.as_deref(), Some("fn main(){}"));
        assert_eq!(msgs[1].role, crate::types::Role::Assistant);
        assert!(msgs[2].attachments[0].is_image());
    }
}

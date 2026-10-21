//! Model Context Protocol client (JSON-RPC 2.0) over stdio or Streamable HTTP.

pub mod http;
pub mod manager;
pub mod stdio;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config::McpTransportConfig;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

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
}

enum Transport {
    Stdio(stdio::StdioTransport),
    Http(http::HttpTransport),
}

pub struct McpClient {
    transport: Transport,
    next_id: AtomicU64,
    pub server_name: String,
    pub server_version: String,
    pub instructions: Option<String>,
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
    pub async fn connect(cfg: &McpTransportConfig, http: reqwest::Client) -> Result<Self> {
        let transport = match cfg {
            McpTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => Transport::Stdio(stdio::StdioTransport::spawn(command, args, env, cwd).await?),
            McpTransportConfig::Http { url, headers } => {
                Transport::Http(http::HttpTransport::new(http, url, headers)?)
            }
        };
        let mut client = Self {
            transport,
            next_id: AtomicU64::new(1),
            server_name: String::new(),
            server_version: String::new(),
            instructions: None,
        };

        let init = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "CleanRSTAI", "version": env!("CARGO_PKG_VERSION")},
                }),
                Duration::from_secs(120),
            )
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
        client.instructions = init
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if let Transport::Http(h) = &client.transport {
            let version = init
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_VERSION);
            h.set_protocol_version(version);
        }

        client
            .notify("notifications/initialized", None)
            .await
            .context("sending initialized notification")?;
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

    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
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

pub fn format_tool_result(res: &Value) -> ToolOutput {
    let is_error = res
        .get("isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    let mut parts = Vec::new();
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
            "image" | "audio" => format!(
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(out.text, "hello\n[image: image/png]\nbody");
    }

    #[test]
    fn response_ids() {
        assert_eq!(response_id(&json!({"id": 3, "result": {}})), Some(3));
        assert_eq!(response_id(&json!({"id": "4", "result": {}})), Some(4));
        assert_eq!(response_id(&json!({"id": 3, "method": "ping"})), None);
    }
}

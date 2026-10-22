//! MCP "Streamable HTTP" transport: every message is POSTed to a single endpoint; the
//! server answers with either a JSON body or an SSE stream carrying the response.

use std::sync::Mutex;

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::config::KeyValue;
use crate::sse::SseParser;

const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";

pub struct HttpTransport {
    http: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
}

impl HttpTransport {
    pub fn new(http: reqwest::Client, url: &str, headers: &[KeyValue]) -> Result<Self> {
        let url = url.trim();
        if url.is_empty() {
            bail!("no URL configured");
        }
        reqwest::Url::parse(url).with_context(|| format!("invalid URL '{url}'"))?;
        let mut map = HeaderMap::new();
        for kv in headers.iter().filter(|kv| !kv.key.trim().is_empty()) {
            let name = HeaderName::from_bytes(kv.key.trim().as_bytes())
                .with_context(|| format!("invalid header name '{}'", kv.key))?;
            let value = HeaderValue::from_str(kv.value.trim())
                .with_context(|| format!("invalid value for header '{}'", kv.key))?;
            map.insert(name, value);
        }
        Ok(Self {
            http,
            url: url.to_owned(),
            headers: map,
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(None),
        })
    }

    pub fn set_protocol_version(&self, v: &str) {
        *self.protocol_version.lock().unwrap() = Some(v.to_owned());
    }

    fn with_session(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req.headers(self.headers.clone());
        if let Some(sid) = self.session_id.lock().unwrap().clone() {
            req = req.header(SESSION_HEADER, sid);
        }
        if let Some(v) = self.protocol_version.lock().unwrap().clone() {
            req = req.header(VERSION_HEADER, v);
        }
        req
    }

    async fn post(&self, msg: &Value) -> Result<reqwest::Response> {
        let req = self
            .http
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(msg);
        let resp = self
            .with_session(req)
            .send()
            .await
            .with_context(|| format!("POST {}", self.url))?;
        if let Some(sid) = resp
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().unwrap() = Some(sid.to_owned());
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::NOT_FOUND && self.session_id.lock().unwrap().is_some()
            {
                bail!("MCP session expired (HTTP 404); reconnect the server");
            }
            bail!("HTTP {status}: {}", body.trim());
        }
        Ok(resp)
    }

    pub async fn notify(&self, msg: &Value) -> Result<()> {
        self.post(msg).await.map(|_| ())
    }

    pub async fn request(&self, id: u64, msg: &Value) -> Result<Value> {
        let resp = self.post(msg).await?;
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if content_type.contains("text/event-stream") {
            let mut parser = SseParser::default();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                for ev in parser.feed(&chunk.context("SSE stream interrupted")?) {
                    if let Some(found) = self.handle_message(&ev.data, id).await {
                        return Ok(found);
                    }
                }
            }
            if let Some(ev) = parser.finish()
                && let Some(found) = self.handle_message(&ev.data, id).await
            {
                return Ok(found);
            }
            bail!("server closed the SSE stream without a response");
        }

        let body = resp.text().await?;
        if body.trim().is_empty() {
            bail!("empty response from server");
        }
        let v: Value = serde_json::from_str(&body).context("invalid JSON response")?;
        let found = match v {
            Value::Array(items) => items
                .into_iter()
                .find(|m| super::response_id(m) == Some(id)),
            other => Some(other),
        };
        found.context("response did not contain a reply to our request")
    }

    /// Handle one message from an SSE stream; returns it if it is the response we wait for.
    async fn handle_message(&self, data: &str, id: u64) -> Option<Value> {
        let msg: Value = serde_json::from_str(data.trim()).ok()?;
        if super::response_id(&msg) == Some(id) {
            return Some(msg);
        }
        if msg.get("id").is_some() && msg.get("method").is_some() {
            let reply = super::reply_to_server_request(&msg);
            if let Err(e) = self.post(&reply).await {
                log::warn!("failed to answer server request: {e}");
            }
        }
        None
    }

    pub async fn close(&self) {
        let has_session = self.session_id.lock().unwrap().is_some();
        if has_session {
            let req = self.with_session(self.http.delete(&self.url));
            let _ = req.send().await;
        }
    }
}

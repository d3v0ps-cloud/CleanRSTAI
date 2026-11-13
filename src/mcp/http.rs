//! MCP "Streamable HTTP" transport: every message is POSTed to a single endpoint; the
//! server answers with either a JSON body or an SSE stream carrying the response. A
//! background GET stream, when the server offers one, carries server-initiated messages.

use crate::sync::LockExt as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tokio::task::JoinHandle;

use super::NotifyFn;
use crate::config::KeyValue;
use crate::sse::SseParser;

const SESSION_HEADER: &str = "mcp-session-id";

/// The server answered 404 to a request carrying our session id: it has forgotten the
/// session, and the client must start a new one.
#[derive(Debug)]
pub struct SessionExpired;

impl std::fmt::Display for SessionExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP session expired (HTTP 404)")
    }
}

impl std::error::Error for SessionExpired {}
const VERSION_HEADER: &str = "mcp-protocol-version";
/// The notification stream is opened again after this wait, doubled for each failure in a row
/// up to `RETRY_MAX`.
#[cfg(not(test))]
const RETRY_BASE: Duration = Duration::from_secs(1);
#[cfg(test)]
const RETRY_BASE: Duration = Duration::from_millis(1);
const RETRY_MAX: Duration = RETRY_BASE.saturating_mul(60);

/// The wait before opening the notification stream again after `failures` failures in a row.
fn retry_delay(failures: u32) -> Duration {
    RETRY_BASE
        .saturating_mul(1 << failures.min(6))
        .min(RETRY_MAX)
}

struct Inner {
    http: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    on_notify: NotifyFn,
    auth: Option<Arc<super::oauth::Auth>>,
}

pub struct HttpTransport {
    inner: Arc<Inner>,
    listener: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        if let Some(t) = self.listener.locked().take() {
            t.abort();
        }
    }
}

/// A header left as the settings page fills it in for a new server (`Authorization: Bearer `
/// with no token, or an empty value): not sent, so a server that signs in with OAuth instead
/// isn't sent a broken credential.
fn unfilled(kv: &KeyValue) -> bool {
    let value = kv.value.trim();
    value.is_empty()
        || (kv.key.trim().eq_ignore_ascii_case("authorization")
            && ["bearer", "basic", "token"]
                .iter()
                .any(|scheme| value.eq_ignore_ascii_case(scheme)))
}

impl HttpTransport {
    pub fn new(
        http: reqwest::Client,
        url: &str,
        headers: &[KeyValue],
        on_notify: NotifyFn,
        auth: Option<Arc<super::oauth::Auth>>,
    ) -> Result<Self> {
        let url = url.trim();
        if url.is_empty() {
            bail!("no URL configured");
        }
        reqwest::Url::parse(url).with_context(|| format!("invalid URL '{url}'"))?;
        let mut map = HeaderMap::new();
        for kv in headers
            .iter()
            .filter(|kv| !kv.key.trim().is_empty() && !unfilled(kv))
        {
            let name = HeaderName::from_bytes(kv.key.trim().as_bytes())
                .with_context(|| format!("invalid header name '{}'", kv.key))?;
            let value = HeaderValue::from_str(kv.value.trim())
                .with_context(|| format!("invalid value for header '{}'", kv.key))?;
            map.insert(name, value);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                url: url.to_owned(),
                headers: map,
                session_id: Mutex::new(None),
                protocol_version: Mutex::new(None),
                on_notify,
                auth,
            }),
            listener: Mutex::new(None),
        })
    }

    pub fn set_protocol_version(&self, v: &str) {
        *self.inner.protocol_version.locked() = Some(v.to_owned());
    }

    pub fn session_id(&self) -> Option<String> {
        self.inner.session_id.locked().clone()
    }

    /// Forget the session id, so the next `initialize` starts a new session.
    pub fn clear_session(&self) {
        *self.inner.session_id.locked() = None;
    }

    /// Open the optional GET stream for server-initiated notifications and requests.
    /// Servers that don't offer it answer 405 (or something other than an event stream),
    /// which simply ends the listener. Other failures (including 401 while a token is
    /// refreshed, 404 while an expired session is renewed, or a server that is restarting)
    /// are retried for as long as the connection lasts, waiting at most a minute between
    /// tries, so `list_changed` notifications arrive again once the server is back.
    pub fn start_listening(&self) {
        let inner = self.inner.clone();
        let task = tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                match inner.listen_once().await {
                    Ok(Listen::NotSupported) => return,
                    Ok(Listen::Ended) => failures = 0,
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        log::debug!("MCP notification stream: {e:#}");
                    }
                }
                tokio::time::sleep(retry_delay(failures)).await;
            }
        });
        *self.listener.locked() = Some(task);
    }

    pub async fn notify(&self, msg: &Value) -> Result<()> {
        self.inner.post(msg).await.map(|_| ())
    }

    pub async fn request(&self, id: u64, msg: &Value) -> Result<Value> {
        self.inner.request(id, msg).await
    }

    pub async fn close(&self) {
        self.inner.close().await
    }
}

enum Listen {
    NotSupported,
    Ended,
}

impl Inner {
    fn with_session(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req.headers(self.headers.clone());
        if let Some(sid) = self.session_id.locked().clone() {
            req = req.header(SESSION_HEADER, sid);
        }
        if let Some(v) = self.protocol_version.locked().clone() {
            req = req.header(VERSION_HEADER, v);
        }
        req
    }

    async fn bearer(&self) -> Option<String> {
        match &self.auth {
            Some(a) => a.bearer().await,
            None => None,
        }
    }

    async fn post(&self, msg: &Value) -> Result<reqwest::Response> {
        let mut retried = false;
        loop {
            let token = self.bearer().await;
            let resp = self.post_once(msg, token.as_deref()).await?;
            if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
                return self.check(resp).await;
            }
            let www_authenticate = resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            // One retry after refreshing an access token the server rejected.
            if !retried
                && let Some(auth) = &self.auth
                && token.is_some()
                && auth.refresh_after_rejection(token.as_deref()).await
            {
                retried = true;
                continue;
            }
            return Err(super::oauth::AuthRequired { www_authenticate }.into());
        }
    }

    async fn post_once(&self, msg: &Value, token: Option<&str>) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(msg);
        req = self.with_session(req);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send()
            .await
            .with_context(|| format!("POST {}", self.url))
    }

    async fn check(&self, resp: reqwest::Response) -> Result<reqwest::Response> {
        if let Some(sid) = resp
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.locked() = Some(sid.to_owned());
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::NOT_FOUND && self.session_id.locked().is_some() {
                return Err(SessionExpired.into());
            }
            bail!("HTTP {status}: {}", body.trim());
        }
        Ok(resp)
    }

    async fn request(&self, id: u64, msg: &Value) -> Result<Value> {
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
                    if let Some(found) = self.handle_message(&ev.data, Some(id)).await {
                        return Ok(found);
                    }
                }
            }
            if let Some(ev) = parser.finish()
                && let Some(found) = self.handle_message(&ev.data, Some(id)).await
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

    /// Handle one message from an SSE stream: answer server requests, pass notifications
    /// on, and return the message if it is the response `id` we wait for.
    async fn handle_message(&self, data: &str, id: Option<u64>) -> Option<Value> {
        let msg: Value = serde_json::from_str(data.trim()).ok()?;
        if id.is_some() && super::response_id(&msg) == id {
            return Some(msg);
        }
        match (msg.get("id"), msg.get("method").and_then(|m| m.as_str())) {
            (Some(_), Some(_)) => {
                let reply = super::reply_to_server_request(&msg);
                if let Err(e) = self.post(&reply).await {
                    log::warn!("failed to answer server request: {e}");
                }
            }
            (None, Some(method)) => (self.on_notify)(method),
            _ => {}
        }
        None
    }

    async fn listen_once(&self) -> Result<Listen> {
        let mut req = self
            .with_session(self.http.get(&self.url))
            .header(ACCEPT, "text/event-stream");
        let token = self.bearer().await;
        if let Some(t) = &token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(Listen::NotSupported);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED
            && let (Some(auth), Some(t)) = (&self.auth, token.as_deref())
        {
            auth.refresh_after_rejection(Some(t)).await;
        }
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        let is_sse = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("text/event-stream"));
        if !is_sse {
            return Ok(Listen::NotSupported);
        }
        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            for ev in parser.feed(&chunk?) {
                self.handle_message(&ev.data, None).await;
            }
        }
        Ok(Listen::Ended)
    }

    async fn close(&self) {
        let has_session = self.session_id.locked().is_some();
        if has_session {
            let mut req = self.with_session(self.http.delete(&self.url));
            if let Some(t) = self.bearer().await {
                req = req.bearer_auth(t);
            }
            let _ = req.send().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn retry_waits_double_up_to_a_cap() {
        assert_eq!(retry_delay(0), RETRY_BASE);
        assert_eq!(retry_delay(1), RETRY_BASE * 2);
        assert_eq!(retry_delay(3), RETRY_BASE * 8);
        assert_eq!(retry_delay(6), RETRY_MAX);
        assert_eq!(retry_delay(u32::MAX), RETRY_MAX);
    }

    /// A server that is down for a while (many failed tries in a row) still gets its
    /// notification stream back once it answers again.
    #[tokio::test]
    async fn notification_stream_comes_back_after_many_failures() {
        use crate::testutil::{Response, serve};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let gets = Arc::new(AtomicUsize::new(0));
        let seen = gets.clone();
        let (base, _) = serve(move |r| {
            if r.method != "GET" {
                return Response::status(202);
            }
            if seen.fetch_add(1, Ordering::SeqCst) < 8 {
                return Response::status(503);
            }
            Response::with_type(
                200,
                "text/event-stream",
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
            )
        })
        .await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let on_notify: NotifyFn = Arc::new(move |method: &str| {
            let _ = tx.send(method.to_owned());
        });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let transport =
            HttpTransport::new(http, &format!("{base}/mcp"), &[], on_notify, None).unwrap();
        transport.start_listening();
        let method = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no notification after the server came back");
        assert_eq!(method.as_deref(), Some("notifications/tools/list_changed"));
        assert!(gets.load(Ordering::SeqCst) > 8);
    }

    #[test]
    fn a_bearer_header_without_a_token_is_not_sent() {
        assert!(unfilled(&kv("Authorization", "Bearer ")));
        assert!(unfilled(&kv("authorization", " bearer")));
        assert!(unfilled(&kv("X-Api-Key", "  ")));
        assert!(!unfilled(&kv("Authorization", "Bearer abc")));
        assert!(!unfilled(&kv("X-Mode", "Bearer")));
    }
}

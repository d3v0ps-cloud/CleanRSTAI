//! A tiny HTTP/1.1 server for tests: one request per connection, handled by a closure.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(v: serde_json::Value) -> Self {
        Self::with_type(200, "application/json", v.to_string())
    }

    pub fn sse(chunks: &[serde_json::Value]) -> Self {
        let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        Self::with_type(200, "text/event-stream", body)
    }

    pub fn with_type(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), content_type.into())],
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Self::with_type(status, "text/plain", Vec::new())
    }

    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

/// Start a server on a free port. Returns its base URL (`http://127.0.0.1:port`) and a log
/// of every request received.
pub async fn serve(
    handler: impl Fn(&Request) -> Response + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Request>>> = Arc::default();
    let handler = Arc::new(handler);
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let (handler, seen) = (handler.clone(), seen.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (head_end, len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(h) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..h]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.trim()
                                    .eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0usize);
                        if buf.len() >= h + 4 + len {
                            break (h, len);
                        }
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let mut first = lines.next().unwrap_or("").split_whitespace();
                let req = Request {
                    method: first.next().unwrap_or("").to_owned(),
                    path: first.next().unwrap_or("").to_owned(),
                    headers: lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                        .collect(),
                    body: buf[head_end + 4..head_end + 4 + len].to_vec(),
                };
                let resp = handler(&req);
                seen.lock().unwrap().push(req);
                let mut out = format!(
                    "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n",
                    resp.status,
                    reqwest::StatusCode::from_u16(resp.status)
                        .ok()
                        .and_then(|s| s.canonical_reason())
                        .unwrap_or("Unknown"),
                    resp.body.len()
                );
                for (k, v) in &resp.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let mut out = out.into_bytes();
                out.extend_from_slice(&resp.body);
                let _ = sock.write_all(&out).await;
            });
        }
    });
    (format!("http://{addr}"), log)
}

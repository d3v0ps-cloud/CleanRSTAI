//! Client for OpenAI-compatible `/chat/completions` endpoints (OpenAI, Ollama, LM Studio,
//! vLLM, llama.cpp server, OpenRouter, Groq, ...).

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

use crate::config::{ProviderConfig, ProviderKind};
use crate::sse::SseParser;
use crate::stats::{Collector, RequestStats};
use crate::types::{ChatMessage, Role, ToolCall};

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub enum Delta<'a> {
    Content(&'a str),
    Reasoning(&'a str),
}

#[derive(Debug, Default)]
pub struct AssistantTurn {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub stats: RequestStats,
}

pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("CleanRSTAI/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("failed to build HTTP client")
}

fn endpoint(p: &ProviderConfig, path: &str) -> String {
    format!("{}/{}", p.base_url.trim().trim_end_matches('/'), path)
}

fn authorize(mut req: reqwest::RequestBuilder, p: &ProviderConfig) -> reqwest::RequestBuilder {
    if !p.api_key.trim().is_empty() {
        req = req.bearer_auth(p.api_key.trim());
    }
    for h in &p.extra_headers {
        if !h.key.trim().is_empty() {
            req = req.header(h.key.trim(), h.value.as_str());
        }
    }
    req
}

async fn error_from_response(resp: reqwest::Response) -> anyhow::Error {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("error"))
                .or_else(|| v.get("message"))
                .map(|m| {
                    m.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| m.to_string())
                })
        })
        .unwrap_or(body);
    anyhow::anyhow!("HTTP {status}: {}", detail.trim())
}

/// Server root, without the trailing `/v1` (where llama.cpp and LiteLLM serve extra endpoints).
fn origin(p: &ProviderConfig) -> String {
    let base = p.base_url.trim().trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base).to_owned()
}

/// llama.cpp's `/props`: returns the context size if this is a llama.cpp server. Asked
/// once per server and model, not on every generation; failures are asked again.
pub async fn llama_cpp_context_size(http: &reqwest::Client, p: &ProviderConfig) -> Option<u64> {
    static CACHE: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), u64>>,
    > = std::sync::LazyLock::new(Default::default);
    let key = (origin(p), p.model.clone());
    if let Some(n) = CACHE.lock().unwrap().get(&key) {
        return Some(*n);
    }
    let v = llama_cpp_props(http, p).await?;
    let n = v
        .pointer("/default_generation_settings/n_ctx")
        .or_else(|| v.get("n_ctx"))
        .and_then(Value::as_u64)?;
    CACHE.lock().unwrap().insert(key, n);
    Some(n)
}

/// llama.cpp's `/slots`: how many of the server's parallel slots are in use. Asked at the
/// start of a generation, like the context size, not on every request.
pub async fn llama_cpp_slot_usage(
    http: &reqwest::Client,
    p: &ProviderConfig,
) -> Option<crate::stats::SlotUsage> {
    let resp = authorize(http.get(format!("{}/slots", origin(p))), p)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let slots: Vec<Value> = resp.json().await.ok()?;
    let total = slots.len() as u64;
    let busy = slots
        .iter()
        .filter(|s| {
            // Current servers report `is_processing`; older ones a numeric `state` (0 = idle).
            s.get("is_processing")
                .and_then(Value::as_bool)
                .or_else(|| s.get("state").and_then(Value::as_u64).map(|st| st != 0))
                .unwrap_or(false)
        })
        .count() as u64;
    Some(crate::stats::SlotUsage { busy, total })
}

async fn llama_cpp_props(http: &reqwest::Client, p: &ProviderConfig) -> Option<Value> {
    let resp = authorize(http.get(format!("{}/props", origin(p))), p)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    (v.get("default_generation_settings").is_some() || v.get("build_info").is_some()).then_some(v)
}

/// Work out which kind of server a provider points at.
pub async fn detect_kind(http: &reqwest::Client, p: &ProviderConfig) -> ProviderKind {
    if llama_cpp_props(http, p).await.is_some() {
        return ProviderKind::LlamaCpp;
    }
    // LiteLLM tags its responses with `x-litellm-*` headers and has a liveness endpoint.
    let litellm_headers = authorize(http.get(endpoint(p, "models")), p)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .is_ok_and(|r| {
            r.headers()
                .keys()
                .any(|k| k.as_str().starts_with("x-litellm"))
        });
    if litellm_headers {
        return ProviderKind::LiteLlm;
    }
    let liveliness = http
        .get(format!("{}/health/liveliness", origin(p)))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;
    if let Ok(r) = liveliness
        && r.status().is_success()
        && r.text()
            .await
            .is_ok_and(|t| t.to_ascii_lowercase().contains("alive"))
    {
        return ProviderKind::LiteLlm;
    }
    ProviderKind::Generic
}

pub async fn list_models(http: &reqwest::Client, p: &ProviderConfig) -> Result<Vec<String>> {
    let resp = authorize(http.get(endpoint(p, "models")), p)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }
    let v: Value = resp.json().await?;
    let list = v
        .get("data")
        .or_else(|| v.get("models"))
        .and_then(|d| d.as_array())
        .context("unexpected /models response")?;
    let mut ids: Vec<String> = list
        .iter()
        .filter_map(|m| {
            m.get("id")
                .or_else(|| m.get("name"))
                .and_then(|i| i.as_str())
                .map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Convert stored history into the wire format, repairing any tool calls that never
/// received a result (e.g. the user hit Stop mid-call) so the API doesn't reject it.
pub fn to_api_messages(system_prompt: &str, history: &[ChatMessage]) -> Vec<Value> {
    let mut out = Vec::with_capacity(history.len() + 1);
    if !system_prompt.trim().is_empty() {
        out.push(json!({"role": "system", "content": system_prompt}));
    }
    let mut i = 0;
    while i < history.len() {
        let m = &history[i];
        match m.role {
            Role::System => out.push(json!({"role": "system", "content": m.content})),
            Role::User => {
                let text = crate::attach::text_with_files(&m.content, &m.attachments);
                let images: Vec<Value> = m
                    .attachments
                    .iter()
                    .filter(|a| a.is_image())
                    .filter_map(|a| a.data_url())
                    .map(|url| json!({"type": "image_url", "image_url": {"url": url}}))
                    .collect();
                if images.is_empty() {
                    out.push(json!({"role": "user", "content": text}));
                } else {
                    let mut parts = vec![json!({"type": "text", "text": text})];
                    parts.extend(images);
                    out.push(json!({"role": "user", "content": parts}));
                }
            }
            Role::Assistant if m.tool_calls.is_empty() => {
                if !m.content.is_empty() {
                    out.push(json!({"role": "assistant", "content": m.content}));
                }
            }
            Role::Assistant => {
                let calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|c| {
                        json!({"id": c.id, "type": "function",
                               "function": {"name": c.name, "arguments": c.arguments}})
                    })
                    .collect();
                out.push(json!({
                    "role": "assistant",
                    "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                    "tool_calls": calls,
                }));
                // Gather the tool results that follow, then add placeholders for missing ones.
                let mut j = i + 1;
                let mut answered = Vec::new();
                while j < history.len() && history[j].role == Role::Tool {
                    let t = &history[j];
                    if let Some(id) = &t.tool_call_id {
                        answered.push(id.clone());
                        out.push(json!({"role": "tool", "tool_call_id": id, "content": t.content}));
                    }
                    j += 1;
                }
                for c in &m.tool_calls {
                    if !answered.contains(&c.id) {
                        out.push(json!({"role": "tool", "tool_call_id": c.id,
                                        "content": "Tool call was cancelled."}));
                    }
                }
                i = j;
                continue;
            }
            // Orphan tool results (shouldn't happen) are dropped to keep the request valid.
            Role::Tool => {}
        }
        i += 1;
    }
    out
}

#[derive(Default)]
struct ToolCallBuilder {
    calls: Vec<ToolCall>,
}

impl ToolCallBuilder {
    fn apply(&mut self, delta: &Value) {
        let Some(list) = delta.get("tool_calls").and_then(|t| t.as_array()) else {
            return;
        };
        for (pos, tc) in list.iter().enumerate() {
            let id = tc
                .get("id")
                .and_then(|i| i.as_str())
                .filter(|s| !s.is_empty());
            let idx = match tc.get("index").and_then(|i| i.as_u64()) {
                Some(i) => i as usize,
                // Some servers omit `index`; match by id, else treat as a new call.
                None => id
                    .and_then(|id| self.calls.iter().position(|c| c.id == id))
                    .unwrap_or(self.calls.len().max(pos)),
            };
            while self.calls.len() <= idx {
                self.calls.push(ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
            }
            let call = &mut self.calls[idx];
            if let Some(id) = id {
                call.id = id.to_owned();
            }
            if let Some(f) = tc.get("function") {
                if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                    call.name.push_str(n);
                }
                match f.get("arguments") {
                    Some(Value::String(a)) => call.arguments.push_str(a),
                    // Non-standard servers may send an already-parsed object.
                    Some(v @ Value::Object(_)) => call.arguments = v.to_string(),
                    _ => {}
                }
            }
        }
    }

    fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .enumerate()
            .map(|(i, mut c)| {
                if c.id.is_empty() {
                    c.id = format!("call_{}_{i}", uuid::Uuid::new_v4().simple());
                }
                if c.arguments.trim().is_empty() {
                    c.arguments = "{}".into();
                }
                c
            })
            .collect()
    }
}

fn reasoning_of(v: &Value) -> Option<&str> {
    v.get("reasoning_content")
        .or_else(|| v.get("reasoning"))
        .and_then(|r| r.as_str())
}

pub async fn chat_completion(
    http: &reqwest::Client,
    p: &ProviderConfig,
    messages: Vec<Value>,
    tools: &[ToolSpec],
    mut on_delta: impl FnMut(Delta<'_>),
    mut on_stats: impl FnMut(RequestStats),
) -> Result<AssistantTurn> {
    if p.model.trim().is_empty() {
        bail!("No model selected. Choose one in Settings → Providers.");
    }
    let mut body = json!({
        "model": p.model.trim(),
        "messages": messages,
        "stream": p.stream,
    });
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters}})
            })
            .collect();
    }
    if let Some(t) = p.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(m) = p.max_tokens {
        body["max_tokens"] = json!(m);
    }
    if p.stream && p.request_usage {
        body["stream_options"] = json!({"include_usage": true});
    }

    let mut stats = Collector::default();

    let resp = authorize(http.post(endpoint(p, "chat/completions")), p)
        .timeout(std::time::Duration::from_secs(p.timeout_secs.max(10)))
        .json(&body)
        .send()
        .await
        .context("request failed")?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }
    stats.header_cost_usd = resp
        .headers()
        .get("x-litellm-response-cost")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok());

    let mut turn = AssistantTurn::default();
    let mut builder = ToolCallBuilder::default();
    // Live stats are pushed at most a few times a second, not on every chunk.
    let mut last_live = std::time::Instant::now();

    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));

    if !is_sse {
        let v: Value = resp.json().await.context("invalid JSON response")?;
        if let Some(err) = v.get("error") {
            bail!(
                "{}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or(&err.to_string())
            );
        }
        let choice = v.pointer("/choices/0").context("response has no choices")?;
        let msg = choice.get("message").cloned().unwrap_or(Value::Null);
        stats.timer.token();
        stats.observe(&v);
        if let Some(r) = reasoning_of(&msg) {
            on_delta(Delta::Reasoning(r));
            turn.reasoning.push_str(r);
        }
        if let Some(c) = msg.get("content").and_then(|c| c.as_str()) {
            on_delta(Delta::Content(c));
            turn.content.push_str(c);
        }
        builder.apply(&msg);
        turn.finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(str::to_owned);
        turn.tool_calls = builder.finish();
        turn.stats = stats.finish(&p.model);
        return Ok(turn);
    }

    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    let mut done = false;
    let mut handle = |data: &str,
                      turn: &mut AssistantTurn,
                      builder: &mut ToolCallBuilder,
                      stats: &mut Collector|
     -> Result<bool> {
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(true);
        }
        if data.is_empty() {
            return Ok(false);
        }
        let v: Value = serde_json::from_str(data).with_context(|| format!("bad chunk: {data}"))?;
        if let Some(err) = v.get("error") {
            bail!(
                "{}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or(&err.to_string())
            );
        }
        stats.observe(&v);
        if let Some(choice) = v.pointer("/choices/0") {
            if let Some(delta) = choice.get("delta") {
                let produced = reasoning_of(delta).is_some_and(|r| !r.is_empty())
                    || delta
                        .get("content")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| !c.is_empty())
                    || delta.get("tool_calls").is_some_and(|t| !t.is_null());
                if produced {
                    stats.timer.token();
                    if last_live.elapsed() >= std::time::Duration::from_millis(100) {
                        last_live = std::time::Instant::now();
                        on_stats(stats.snapshot(&p.model));
                    }
                }
                if let Some(r) = reasoning_of(delta).filter(|r| !r.is_empty()) {
                    on_delta(Delta::Reasoning(r));
                    turn.reasoning.push_str(r);
                }
                if let Some(c) = delta
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                {
                    on_delta(Delta::Content(c));
                    turn.content.push_str(c);
                }
                builder.apply(delta);
            }
            if let Some(f) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                turn.finish_reason = Some(f.to_owned());
            }
        }
        Ok(false)
    };

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("stream interrupted")?;
        for ev in parser.feed(&chunk) {
            if handle(&ev.data, &mut turn, &mut builder, &mut stats)? {
                done = true;
                break;
            }
        }
        if done {
            break;
        }
    }
    if !done && let Some(ev) = parser.finish() {
        handle(&ev.data, &mut turn, &mut builder, &mut stats)?;
    }
    turn.tool_calls = builder.finish();
    turn.stats = stats.finish(&p.model);
    Ok(turn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_streamed_tool_calls() {
        let mut b = ToolCallBuilder::default();
        b.apply(&json!({"tool_calls": [{"index": 0, "id": "a", "function": {"name": "get_", "arguments": ""}}]}));
        b.apply(&json!({"tool_calls": [{"index": 0, "function": {"name": "weather", "arguments": "{\"ci"}}]}));
        b.apply(&json!({"tool_calls": [{"index": 0, "function": {"arguments": "ty\":1}"}}]}));
        b.apply(&json!({"tool_calls": [{"index": 1, "id": "b", "function": {"name": "x"}}]}));
        let calls = b.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, "{\"city\":1}");
        assert_eq!(calls[1].arguments, "{}");
    }

    #[test]
    fn repairs_unanswered_tool_calls() {
        let mut a = ChatMessage::new(Role::Assistant, "");
        a.tool_calls = vec![
            ToolCall {
                id: "1".into(),
                name: "f".into(),
                arguments: "{}".into(),
            },
            ToolCall {
                id: "2".into(),
                name: "g".into(),
                arguments: "{}".into(),
            },
        ];
        let t = ChatMessage::tool_result(&a.tool_calls[0], "ok", false);
        let history = vec![
            ChatMessage::new(Role::User, "hi"),
            a,
            t,
            ChatMessage::new(Role::User, "again"),
        ];
        let msgs = to_api_messages("sys", &history);
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(
            roles,
            ["system", "user", "assistant", "tool", "tool", "user"]
        );
        assert_eq!(msgs[4]["tool_call_id"], "2");
        assert!(msgs[2]["content"].is_null());
    }
}

#[cfg(test)]
mod server_tests {
    //! Against mock llama.cpp / LiteLLM / generic servers over real HTTP.
    use super::*;
    use crate::stats::StatsSource;
    use crate::testutil::{Response, serve};

    fn provider(base: &str, kind: ProviderKind) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{base}/v1"),
            model: "m".into(),
            kind,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn llama_cpp_detection_and_server_timings() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/props") => Response::json(json!({
                "default_generation_settings": {"n_ctx": 8192}, "build_info": "b1"})),
            ("POST", "/v1/chat/completions") => Response::sse(&[
                json!({"choices": [{"delta": {"content": "Hel"}}]}),
                json!({"choices": [{"delta": {"content": "lo"}, "finish_reason": "stop"}],
                       "timings": {"prompt_n": 120, "prompt_ms": 60.0, "prompt_per_second": 2000.0,
                                   "predicted_n": 2, "predicted_ms": 40.0, "predicted_per_second": 50.0}}),
            ]),
            _ => Response::status(404),
        })
        .await;
        let http = build_http_client();
        let p = provider(&base, ProviderKind::Generic);
        assert_eq!(detect_kind(&http, &p).await, ProviderKind::LlamaCpp);
        assert_eq!(llama_cpp_context_size(&http, &p).await, Some(8192));
        // Asked once: the second call is answered from the cache.
        let props = |log: &std::sync::Mutex<Vec<crate::testutil::Request>>| {
            log.lock()
                .unwrap()
                .iter()
                .filter(|r| r.path == "/props")
                .count()
        };
        let before = props(&log);
        assert_eq!(llama_cpp_context_size(&http, &p).await, Some(8192));
        assert_eq!(props(&log), before);

        let turn = chat_completion(&http, &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap();
        assert_eq!(turn.content, "Hello");
        let s = turn.stats;
        assert_eq!(s.source, StatsSource::Server);
        assert_eq!((s.prefill_tps, s.decode_tps), (Some(2000.0), Some(50.0)));
        assert_eq!((s.prompt_tokens, s.completion_tokens), (Some(120), Some(2)));

        let log = log.lock().unwrap();
        let chat = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(chat.json()["stream_options"]["include_usage"], true);
    }

    #[tokio::test]
    async fn llama_cpp_slot_usage_report() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/slots") => Response::json(json!([
                {"id": 0, "is_processing": true, "n_ctx": 4096},
                {"id": 1, "is_processing": false, "n_ctx": 4096},
                {"id": 2, "state": 1},
                {"id": 3, "state": 0},
            ])),
            _ => Response::status(404),
        })
        .await;
        let http = build_http_client();
        let p = provider(&base, ProviderKind::LlamaCpp);
        assert_eq!(
            llama_cpp_slot_usage(&http, &p).await,
            Some(crate::stats::SlotUsage { busy: 2, total: 4 })
        );
        assert!(log.lock().unwrap().iter().any(|r| r.path == "/slots"));
        // A 404 (not a llama.cpp slot endpoint) yields nothing rather than panicking.
        let (base, _) = serve(|_| Response::status(404)).await;
        assert_eq!(
            llama_cpp_slot_usage(&http, &provider(&base, ProviderKind::LlamaCpp)).await,
            None
        );
    }

    #[tokio::test]
    async fn litellm_detection_cost_and_served_model() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/v1/models") => Response::json(json!({"data": [{"id": "alias"}]}))
                .header("x-litellm-version", "1.0.0"),
            ("POST", "/v1/chat/completions") => Response::sse(&[
                json!({"model": "azure/gpt-x", "choices": [{"delta": {"content": "a"}}]}),
                json!({"model": "azure/gpt-x", "choices": [{"delta": {"content": "b"}}]}),
                json!({"model": "azure/gpt-x", "choices": [],
                       "usage": {"prompt_tokens": 10, "completion_tokens": 2}}),
            ])
            .header("x-litellm-response-cost", "0.0042"),
            _ => Response::status(404),
        })
        .await;
        let http = build_http_client();
        let p = ProviderConfig {
            model: "alias".into(),
            api_key: "sk-test".into(),
            ..provider(&base, ProviderKind::Generic)
        };
        assert_eq!(detect_kind(&http, &p).await, ProviderKind::LiteLlm);
        let s = chat_completion(&http, &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap()
            .stats;
        assert_eq!(s.source, StatsSource::Measured);
        assert_eq!(s.cost_usd, Some(0.0042));
        assert_eq!(s.served_model.as_deref(), Some("azure/gpt-x"));
        assert_eq!((s.prompt_tokens, s.completion_tokens), (Some(10), Some(2)));
        assert!(!s.tokens_estimated);
        let log = log.lock().unwrap();
        let chat = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(chat.header("authorization"), Some("Bearer sk-test"));
    }

    #[tokio::test]
    async fn generic_server_without_usage_and_opt_out() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("POST", "/v1/chat/completions") => Response::sse(&[
                json!({"choices": [{"delta": {"content": "x"}}]}),
                json!({"choices": [{"delta": {"content": "y"}}]}),
            ]),
            _ => Response::status(404),
        })
        .await;
        let http = build_http_client();
        let mut p = provider(&base, ProviderKind::Generic);
        assert_eq!(detect_kind(&http, &p).await, ProviderKind::Generic);
        p.request_usage = false;
        let s = chat_completion(&http, &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap()
            .stats;
        assert!(s.tokens_estimated);
        assert_eq!(s.completion_tokens, Some(2));
        let log = log.lock().unwrap();
        let chat = log.iter().find(|r| r.method == "POST").unwrap();
        assert!(chat.json().get("stream_options").is_none());
    }

    #[test]
    fn user_images_become_content_parts() {
        let mut m = ChatMessage::new(Role::User, "What is this?");
        m.attachments = vec![
            crate::attach::Attachment::image("a.png".into(), "image/png".into(), b"xyz"),
            crate::attach::Attachment {
                name: "n.txt".into(),
                mime: "text/plain".into(),
                text: Some("notes".into()),
                data: None,
            },
        ];
        let msgs = to_api_messages("", &[m]);
        let parts = msgs[0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert!(
            parts[0]["text"]
                .as_str()
                .unwrap()
                .contains("<file name=\"n.txt\">")
        );
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,eHl6");
        // Text-only stays a plain string for servers that don't accept content arrays.
        let msgs = to_api_messages("", &[ChatMessage::new(Role::User, "hi")]);
        assert_eq!(msgs[0]["content"], "hi");
    }
}

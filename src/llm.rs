//! Client for OpenAI-compatible `/chat/completions` endpoints (OpenAI, Ollama, LM Studio,
//! vLLM, llama.cpp server, OpenRouter, Groq, ...).

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::sse::SseParser;
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
            Role::User => out.push(json!({"role": "user", "content": m.content})),
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

    let resp = authorize(http.post(endpoint(p, "chat/completions")), p)
        .timeout(std::time::Duration::from_secs(p.timeout_secs.max(10)))
        .json(&body)
        .send()
        .await
        .context("request failed")?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }

    let mut turn = AssistantTurn::default();
    let mut builder = ToolCallBuilder::default();

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
        return Ok(turn);
    }

    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    let mut done = false;
    let mut handle =
        |data: &str, turn: &mut AssistantTurn, builder: &mut ToolCallBuilder| -> Result<bool> {
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(true);
            }
            if data.is_empty() {
                return Ok(false);
            }
            let v: Value =
                serde_json::from_str(data).with_context(|| format!("bad chunk: {data}"))?;
            if let Some(err) = v.get("error") {
                bail!(
                    "{}",
                    err.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or(&err.to_string())
                );
            }
            if let Some(choice) = v.pointer("/choices/0") {
                if let Some(delta) = choice.get("delta") {
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
            if handle(&ev.data, &mut turn, &mut builder)? {
                done = true;
                break;
            }
        }
        if done {
            break;
        }
    }
    if !done && let Some(ev) = parser.finish() {
        handle(&ev.data, &mut turn, &mut builder)?;
    }
    turn.tool_calls = builder.finish();
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

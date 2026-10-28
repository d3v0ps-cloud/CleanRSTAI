//! Native APIs beside OpenAI-compatible Chat Completions: Anthropic's Messages API and
//! OpenAI's Responses API. Both take the Chat Completions messages `llm::to_api_messages`
//! builds and convert them, and both report the same `AssistantTurn` and stats.

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use serde_json::{Map, Value, json};

use crate::config::{ProviderApi, ProviderConfig};
use crate::llm::{AssistantTurn, Delta, ToolSpec, authorize, endpoint, error_from_response};
use crate::model_options::{ModelOptions, ReasoningEffort};
use crate::sse::SseParser;
use crate::stats::{Collector, RequestStats};
use crate::types::{NativeItems, ToolCall};

/// The key `to_api_messages` stores an assistant message's native items under.
pub const NATIVE_KEY: &str = "native";

/// Max tokens for the Anthropic API, which requires one, when none is set.
const ANTHROPIC_MAX_TOKENS: u32 = 8192;

/// What both APIs report while a reply streams in.
struct Sink<'a> {
    on_delta: &'a mut (dyn FnMut(Delta<'_>) + Send),
    on_stats: &'a mut (dyn FnMut(RequestStats) + Send),
    stats: Collector,
    last_live: std::time::Instant,
    model: String,
    turn: AssistantTurn,
}

impl Sink<'_> {
    fn produced(&mut self) {
        self.stats.timer.token();
        if self.last_live.elapsed() >= std::time::Duration::from_millis(100) {
            self.last_live = std::time::Instant::now();
            (self.on_stats)(self.stats.snapshot(&self.model));
        }
    }

    fn content(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.produced();
        (self.on_delta)(Delta::Content(s));
        self.turn.content.push_str(s);
    }

    fn reasoning(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.produced();
        (self.on_delta)(Delta::Reasoning(s));
        self.turn.reasoning.push_str(s);
    }

    /// Token usage in the OpenAI form the stats read.
    fn usage(&mut self, prompt: u64, completion: Option<u64>, cached: Option<u64>) {
        let mut u = json!({"prompt_tokens": prompt});
        if let Some(c) = completion {
            u["completion_tokens"] = json!(c);
        }
        if let Some(c) = cached {
            u["prompt_tokens_details"] = json!({"cached_tokens": c});
        }
        self.stats.observe(&json!({"usage": u}));
    }
}

/// One streamed (or whole) response, event by event.
trait Reader {
    /// Handle one event; true when the response is complete.
    fn event(&mut self, v: &Value, sink: &mut Sink<'_>) -> Result<bool>;
    /// Handle a non-streamed response body.
    fn whole(&mut self, v: &Value, sink: &mut Sink<'_>) -> Result<()>;
    fn finish(self, sink: &mut Sink<'_>);
}

fn error_message(v: &Value) -> Option<String> {
    let e = v.get("error").filter(|e| !e.is_null())?;
    Some(
        e.get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| e.to_string()),
    )
}

async fn send(
    http: &reqwest::Client,
    p: &ProviderConfig,
    path: &str,
    body: &Value,
    mut reader: impl Reader,
    on_delta: &mut (dyn FnMut(Delta<'_>) + Send),
    on_stats: &mut (dyn FnMut(RequestStats) + Send),
) -> Result<AssistantTurn> {
    let resp = authorize(http.post(endpoint(p, path)), p)
        .timeout(std::time::Duration::from_secs(p.timeout_secs.max(10)))
        .json(body)
        .send()
        .await
        .context("request failed")?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }
    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));
    let mut sink = Sink {
        on_delta,
        on_stats,
        stats: Collector::default(),
        last_live: std::time::Instant::now(),
        model: p.model.clone(),
        turn: AssistantTurn::default(),
    };
    if !is_sse {
        let v: Value = resp.json().await.context("invalid JSON response")?;
        if let Some(e) = error_message(&v) {
            bail!("{e}");
        }
        sink.stats.timer.token();
        reader.whole(&v, &mut sink)?;
    } else {
        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        let mut handle = |data: &str, sink: &mut Sink<'_>| -> Result<bool> {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                return Ok(data == "[DONE]");
            }
            let v: Value =
                serde_json::from_str(data).with_context(|| format!("bad chunk: {data}"))?;
            reader.event(&v, sink)
        };
        let mut done = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("stream interrupted")?;
            for ev in parser.feed(&chunk) {
                if handle(&ev.data, &mut sink)? {
                    done = true;
                    break;
                }
            }
            if done {
                break;
            }
        }
        if !done && let Some(ev) = parser.finish() {
            handle(&ev.data, &mut sink)?;
        }
    }
    reader.finish(&mut sink);
    let mut turn = std::mem::take(&mut sink.turn);
    turn.stats = sink.stats.finish(&p.model);
    Ok(turn)
}

/// The native items an assistant message carries, if they came from `api`.
fn native_items(m: &Value, api: ProviderApi) -> Vec<Value> {
    m.get(NATIVE_KEY)
        .and_then(|n| serde_json::from_value::<NativeItems>(n.clone()).ok())
        .filter(|n| n.api == api)
        .map(|n| n.items)
        .unwrap_or_default()
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn arguments(call: &Value) -> (String, String, String) {
    let s = |k: &str| {
        call.pointer(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    (s("/id"), s("/function/name"), s("/function/arguments"))
}

// ---------------------------------------------------------------------------------------
// Anthropic Messages API

/// Convert Chat Completions messages into Anthropic's system blocks and messages.
fn anthropic_messages(messages: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut system = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    let push = |out: &mut Vec<Value>, role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(last) if last["role"] == role => {
                last["content"].as_array_mut().unwrap().extend(blocks);
            }
            _ => out.push(json!({"role": role, "content": blocks})),
        }
    };
    for m in messages {
        let role = m["role"].as_str().unwrap_or_default();
        match role {
            "system" => {
                let text = text_of(&m["content"]);
                if !text.trim().is_empty() {
                    system.push(json!({"type": "text", "text": text}));
                }
            }
            "user" => {
                let mut blocks = Vec::new();
                match &m["content"] {
                    Value::Array(parts) => {
                        for part in parts {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                if !t.is_empty() {
                                    blocks.push(json!({"type": "text", "text": t}));
                                }
                            } else if let Some(url) =
                                part.pointer("/image_url/url").and_then(Value::as_str)
                            {
                                blocks.push(anthropic_image(url));
                            }
                        }
                    }
                    c => {
                        let t = text_of(c);
                        if !t.is_empty() {
                            blocks.push(json!({"type": "text", "text": t}));
                        }
                    }
                }
                push(&mut out, "user", blocks);
            }
            "assistant" => {
                let mut blocks = native_items(m, ProviderApi::Anthropic);
                let text = text_of(&m["content"]);
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    let (id, name, args) = arguments(call);
                    let input = serde_json::from_str::<Value>(&args)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    blocks
                        .push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                }
                push(&mut out, "assistant", blocks);
            }
            "tool" => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": m["tool_call_id"],
                    "content": text_of(&m["content"]),
                });
                push(&mut out, "user", vec![block]);
            }
            _ => {}
        }
    }
    (system, out)
}

fn anthropic_image(url: &str) -> Value {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((meta, data)) = rest.split_once(',')
    {
        let media_type = meta.trim_end_matches(";base64");
        return json!({"type": "image", "source":
            {"type": "base64", "media_type": media_type, "data": data}});
    }
    json!({"type": "image", "source": {"type": "url", "url": url}})
}

fn cache_last(blocks: &mut [Value]) {
    if let Some(b) = blocks.last_mut()
        && !matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking"))
    {
        b["cache_control"] = json!({"type": "ephemeral"});
    }
}

pub(crate) fn anthropic_body(
    p: &ProviderConfig,
    messages: &[Value],
    tools: &[ToolSpec],
    o: &ModelOptions,
) -> Value {
    let (mut system, mut msgs) = anthropic_messages(messages);
    let budget = o.budget().map(|b| b.max(1024));
    let mut max_tokens = o.max_tokens.unwrap_or(ANTHROPIC_MAX_TOKENS);
    if let Some(b) = budget
        && max_tokens <= b
    {
        // The budget counts towards max_tokens and must stay below it.
        max_tokens = b + ANTHROPIC_MAX_TOKENS.min(4096);
    }
    let mut tool_list: Vec<Value> = tools
        .iter()
        .map(
            |t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}),
        )
        .collect();
    if p.prompt_caching {
        cache_last(&mut system);
        cache_last(&mut tool_list);
        if let Some(last) = msgs.last_mut()
            && let Some(blocks) = last["content"].as_array_mut()
        {
            cache_last(blocks);
        }
    }
    let mut body = json!({
        "model": p.model.trim(),
        "max_tokens": max_tokens,
        "messages": msgs,
        "stream": p.stream,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !tool_list.is_empty() {
        body["tools"] = json!(tool_list);
    }
    match budget {
        // Extended thinking doesn't allow changing the temperature or top-p.
        Some(b) => body["thinking"] = json!({"type": "enabled", "budget_tokens": b}),
        None => {
            if let Some(t) = o.temperature {
                body["temperature"] = json!(t);
            }
            if let Some(t) = o.top_p {
                body["top_p"] = json!(t);
            }
        }
    }
    let stops = o.stops();
    if !stops.is_empty() {
        body["stop_sequences"] = json!(stops);
    }
    body
}

enum Block {
    Text,
    Thinking {
        thinking: String,
        signature: String,
    },
    Redacted(Value),
    ToolUse {
        id: String,
        name: String,
        json: String,
        input: Value,
    },
    Other,
}

#[derive(Default)]
struct AnthropicReader {
    blocks: Vec<Block>,
    input_tokens: u64,
    cache_read: u64,
    cache_write: u64,
    output_tokens: Option<u64>,
}

impl AnthropicReader {
    fn start(&mut self, index: usize, b: &Value, sink: &mut Sink<'_>) {
        let s = |k: &str| b[k].as_str().unwrap_or_default().to_owned();
        let block = match b["type"].as_str() {
            Some("text") => {
                sink.content(b["text"].as_str().unwrap_or_default());
                Block::Text
            }
            Some("thinking") => {
                sink.reasoning(b["thinking"].as_str().unwrap_or_default());
                Block::Thinking {
                    thinking: s("thinking"),
                    signature: s("signature"),
                }
            }
            Some("redacted_thinking") => Block::Redacted(b.clone()),
            Some("tool_use") => {
                sink.produced();
                Block::ToolUse {
                    id: s("id"),
                    name: s("name"),
                    json: String::new(),
                    input: b["input"].clone(),
                }
            }
            _ => Block::Other,
        };
        while self.blocks.len() < index {
            self.blocks.push(Block::Other);
        }
        if index < self.blocks.len() {
            self.blocks[index] = block;
        } else {
            self.blocks.push(block);
        }
    }

    fn usage(&mut self, u: &Value, sink: &mut Sink<'_>) {
        let n = |k: &str| u.get(k).and_then(Value::as_u64);
        if let Some(i) = n("input_tokens") {
            self.input_tokens = i;
        }
        if let Some(c) = n("cache_read_input_tokens") {
            self.cache_read = c;
        }
        if let Some(c) = n("cache_creation_input_tokens") {
            self.cache_write = c;
        }
        if let Some(o) = n("output_tokens") {
            self.output_tokens = Some(o);
        }
        sink.usage(
            self.input_tokens + self.cache_read + self.cache_write,
            self.output_tokens,
            Some(self.cache_read).filter(|&c| c > 0),
        );
    }

    fn stop(&mut self, reason: Option<&str>, sink: &mut Sink<'_>) {
        if let Some(r) = reason {
            sink.turn.finish_reason = Some(
                match r {
                    "max_tokens" => "length",
                    "tool_use" => "tool_calls",
                    "end_turn" | "stop_sequence" => "stop",
                    other => other,
                }
                .to_owned(),
            );
        }
    }
}

impl Reader for AnthropicReader {
    fn event(&mut self, v: &Value, sink: &mut Sink<'_>) -> Result<bool> {
        let index = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str() {
            Some("error") => bail!(
                "{}",
                error_message(v).unwrap_or_else(|| "the server reported an error".into())
            ),
            Some("message_start") => {
                if let Some(m) = v.pointer("/message/model") {
                    sink.stats.observe(&json!({"model": m}));
                }
                if let Some(u) = v.pointer("/message/usage") {
                    self.usage(u, sink);
                }
            }
            Some("content_block_start") => self.start(index, &v["content_block"], sink),
            Some("content_block_delta") => {
                let d = &v["delta"];
                let text = |k: &str| d[k].as_str().unwrap_or_default();
                match (d["type"].as_str(), self.blocks.get_mut(index)) {
                    (Some("text_delta"), _) => sink.content(text("text")),
                    (Some("thinking_delta"), Some(Block::Thinking { thinking, .. })) => {
                        thinking.push_str(text("thinking"));
                        sink.reasoning(text("thinking"));
                    }
                    (Some("thinking_delta"), _) => sink.reasoning(text("thinking")),
                    (Some("signature_delta"), Some(Block::Thinking { signature, .. })) => {
                        signature.push_str(text("signature"));
                    }
                    (Some("input_json_delta"), Some(Block::ToolUse { json, .. })) => {
                        json.push_str(text("partial_json"));
                        sink.produced();
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                self.stop(
                    v.pointer("/delta/stop_reason").and_then(Value::as_str),
                    sink,
                );
                if let Some(u) = v.get("usage") {
                    self.usage(u, sink);
                }
            }
            Some("message_stop") => return Ok(true),
            _ => {}
        }
        Ok(false)
    }

    fn whole(&mut self, v: &Value, sink: &mut Sink<'_>) -> Result<()> {
        if let Some(m) = v.get("model") {
            sink.stats.observe(&json!({"model": m}));
        }
        for (i, b) in v["content"].as_array().into_iter().flatten().enumerate() {
            self.start(i, b, sink);
        }
        self.stop(v["stop_reason"].as_str(), sink);
        if let Some(u) = v.get("usage") {
            self.usage(u, sink);
        }
        Ok(())
    }

    fn finish(self, sink: &mut Sink<'_>) {
        let mut items = Vec::new();
        for (i, b) in self.blocks.into_iter().enumerate() {
            match b {
                Block::Thinking {
                    thinking,
                    signature,
                } => items.push(json!({"type": "thinking", "thinking": thinking,
                                      "signature": signature})),
                Block::Redacted(v) => items.push(v),
                Block::ToolUse {
                    id,
                    name,
                    json,
                    input,
                } => {
                    let arguments = if !json.trim().is_empty() {
                        json
                    } else if input.as_object().is_some_and(|o| !o.is_empty()) {
                        input.to_string()
                    } else {
                        "{}".into()
                    };
                    let id = if id.is_empty() {
                        format!("toolu_{i}_{}", uuid::Uuid::new_v4().simple())
                    } else {
                        id
                    };
                    sink.turn.tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                Block::Text | Block::Other => {}
            }
        }
        if !items.is_empty() {
            sink.turn.native = Some(NativeItems {
                api: ProviderApi::Anthropic,
                items,
            });
        }
    }
}

pub async fn anthropic(
    http: &reqwest::Client,
    p: &ProviderConfig,
    messages: Vec<Value>,
    tools: &[ToolSpec],
    on_delta: &mut (dyn FnMut(Delta<'_>) + Send),
    on_stats: &mut (dyn FnMut(RequestStats) + Send),
) -> Result<AssistantTurn> {
    let body = anthropic_body(p, &messages, tools, &p.resolved_options());
    send(
        http,
        p,
        "messages",
        &body,
        AnthropicReader::default(),
        on_delta,
        on_stats,
    )
    .await
}

// ---------------------------------------------------------------------------------------
// OpenAI Responses API

/// Convert Chat Completions messages into Responses `instructions` and input items.
fn responses_input(messages: &[Value]) -> (String, Vec<Value>) {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for m in messages {
        match m["role"].as_str().unwrap_or_default() {
            "system" => instructions.push(text_of(&m["content"])),
            "user" => {
                let parts: Vec<Value> = match &m["content"] {
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|part| {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                Some(json!({"type": "input_text", "text": t}))
                            } else {
                                part.pointer("/image_url/url")
                                    .and_then(Value::as_str)
                                    .map(|u| json!({"type": "input_image", "image_url": u}))
                            }
                        })
                        .collect(),
                    c => vec![json!({"type": "input_text", "text": text_of(c)})],
                };
                input.push(json!({"role": "user", "content": parts}));
            }
            "assistant" => {
                input.extend(native_items(m, ProviderApi::Responses));
                let text = text_of(&m["content"]);
                if !text.is_empty() {
                    input.push(json!({"role": "assistant", "content": text}));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    let (id, name, args) = arguments(call);
                    input.push(json!({"type": "function_call", "call_id": id, "name": name,
                                      "arguments": args}));
                }
            }
            "tool" => input.push(json!({
                "type": "function_call_output",
                "call_id": m["tool_call_id"],
                "output": text_of(&m["content"]),
            })),
            _ => {}
        }
    }
    let instructions = instructions
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (instructions, input)
}

pub(crate) fn responses_body(
    p: &ProviderConfig,
    messages: &[Value],
    tools: &[ToolSpec],
    o: &ModelOptions,
) -> Value {
    let (instructions, input) = responses_input(messages);
    let mut body = json!({
        "model": p.model.trim(),
        "input": input,
        "stream": p.stream,
        // Nothing is kept on the server; reasoning comes back encrypted instead.
        "store": false,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| {
                json!({"type": "function", "name": t.name, "description": t.description,
                       "parameters": t.parameters})
            })
            .collect();
    }
    if let Some(t) = o.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = o.top_p {
        body["top_p"] = json!(t);
    }
    if let Some(m) = o.max_tokens {
        body["max_output_tokens"] = json!(m);
    }
    match o.reasoning_effort {
        Some(ReasoningEffort::Off) => body["reasoning"] = json!({"effort": "none"}),
        Some(e) => {
            body["reasoning"] = json!({"effort": e.wire(), "summary": "auto"});
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
        None => {}
    }
    body
}

#[derive(Default)]
struct ResponsesReader {
    /// Function calls by output index, filled in as their arguments stream.
    calls: Vec<(usize, ToolCall)>,
    reasoning_items: Vec<Value>,
    /// Final items have been read (from `output_item.done` or the whole response).
    final_items: bool,
}

impl ResponsesReader {
    fn call_at(&mut self, index: usize) -> &mut ToolCall {
        let pos = match self.calls.iter().position(|(i, _)| *i == index) {
            Some(pos) => pos,
            None => {
                self.calls.push((
                    index,
                    ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    },
                ));
                self.calls.len() - 1
            }
        };
        &mut self.calls[pos].1
    }

    /// A finished output item.
    fn item(&mut self, index: usize, item: &Value, sink: &mut Sink<'_>, whole: bool) {
        let s = |k: &str| item[k].as_str().unwrap_or_default().to_owned();
        match item["type"].as_str() {
            Some("function_call") => {
                let call = self.call_at(index);
                call.id = s("call_id");
                call.name = s("name");
                call.arguments = s("arguments");
            }
            Some("reasoning") => {
                if whole {
                    for part in item["summary"].as_array().into_iter().flatten() {
                        sink.reasoning(part["text"].as_str().unwrap_or_default());
                    }
                }
                // Without its encrypted content the item can't be sent back (nothing is stored).
                if item["encrypted_content"].as_str().is_some() {
                    let mut kept = Map::new();
                    for k in ["type", "id", "summary", "encrypted_content"] {
                        if let Some(v) = item.get(k) {
                            kept.insert(k.into(), v.clone());
                        }
                    }
                    self.reasoning_items.push(Value::Object(kept));
                }
            }
            Some("message") if whole => {
                for part in item["content"].as_array().into_iter().flatten() {
                    let text = part["text"]
                        .as_str()
                        .or_else(|| part["refusal"].as_str())
                        .unwrap_or_default();
                    sink.content(text);
                }
            }
            _ => {}
        }
    }

    fn response(&mut self, r: &Value, sink: &mut Sink<'_>, whole: bool) {
        if let Some(m) = r.get("model") {
            sink.stats.observe(&json!({"model": m}));
        }
        if let Some(u) = r.get("usage").filter(|u| u.is_object()) {
            sink.usage(
                u["input_tokens"].as_u64().unwrap_or(0),
                u["output_tokens"].as_u64(),
                u.pointer("/input_tokens_details/cached_tokens")
                    .and_then(Value::as_u64),
            );
        }
        if !self.final_items || whole {
            let content_seen = !sink.turn.content.is_empty();
            for (i, item) in r["output"].as_array().into_iter().flatten().enumerate() {
                // Text already streamed isn't added twice.
                let whole = whole || (!content_seen && item["type"] == "message");
                self.item(i, item, sink, whole);
            }
        }
        let incomplete = r
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str);
        sink.turn.finish_reason = Some(
            match (r["status"].as_str(), incomplete) {
                (Some("incomplete"), Some("max_output_tokens")) => "length",
                (Some("incomplete"), Some(other)) => other,
                _ if !self.calls.is_empty() => "tool_calls",
                _ => "stop",
            }
            .to_owned(),
        );
    }
}

impl Reader for ResponsesReader {
    fn event(&mut self, v: &Value, sink: &mut Sink<'_>) -> Result<bool> {
        let index = v["output_index"].as_u64().unwrap_or(0) as usize;
        let delta = v["delta"].as_str().unwrap_or_default();
        match v["type"].as_str().unwrap_or_default() {
            "error" => bail!(
                "{}",
                error_message(v)
                    .or_else(|| v["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "the server reported an error".into())
            ),
            "response.failed" => bail!(
                "{}",
                error_message(&v["response"]).unwrap_or_else(|| "the response failed".into())
            ),
            "response.output_text.delta" | "response.refusal.delta" => sink.content(delta),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                sink.reasoning(delta)
            }
            "response.reasoning_summary_part.added"
                if v["summary_index"].as_u64().unwrap_or(0) > 0 =>
            {
                sink.reasoning("\n\n")
            }
            "response.output_item.added" if v["item"]["type"] == "function_call" => {
                let item = &v["item"];
                let call = self.call_at(index);
                call.id = item["call_id"].as_str().unwrap_or_default().to_owned();
                call.name = item["name"].as_str().unwrap_or_default().to_owned();
                sink.produced();
            }
            "response.function_call_arguments.delta" => {
                self.call_at(index).arguments.push_str(delta);
                sink.produced();
            }
            "response.output_item.done" => {
                self.final_items = true;
                self.item(index, &v["item"], sink, false);
            }
            "response.completed" | "response.incomplete" => {
                self.response(&v["response"], sink, false);
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn whole(&mut self, v: &Value, sink: &mut Sink<'_>) -> Result<()> {
        if v["status"] == "failed" {
            bail!(
                "{}",
                error_message(v).unwrap_or_else(|| "the response failed".into())
            );
        }
        self.response(v, sink, true);
        Ok(())
    }

    fn finish(mut self, sink: &mut Sink<'_>) {
        self.calls.sort_by_key(|(i, _)| *i);
        for (i, (_, mut call)) in self.calls.into_iter().enumerate() {
            if call.name.is_empty() {
                continue;
            }
            if call.id.is_empty() {
                call.id = format!("call_{i}_{}", uuid::Uuid::new_v4().simple());
            }
            if call.arguments.trim().is_empty() {
                call.arguments = "{}".into();
            }
            sink.turn.tool_calls.push(call);
        }
        if !self.reasoning_items.is_empty() {
            sink.turn.native = Some(NativeItems {
                api: ProviderApi::Responses,
                items: self.reasoning_items,
            });
        }
    }
}

pub async fn responses(
    http: &reqwest::Client,
    p: &ProviderConfig,
    messages: Vec<Value>,
    tools: &[ToolSpec],
    on_delta: &mut (dyn FnMut(Delta<'_>) + Send),
    on_stats: &mut (dyn FnMut(RequestStats) + Send),
) -> Result<AssistantTurn> {
    let body = responses_body(p, &messages, tools, &p.resolved_options());
    send(
        http,
        p,
        "responses",
        &body,
        ResponsesReader::default(),
        on_delta,
        on_stats,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{build_http_client, chat_completion, to_api_messages};
    use crate::testutil::{Response, serve};
    use crate::types::{ChatMessage, Role};

    fn provider(base: &str, api: ProviderApi) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{base}/v1"),
            api_key: "key".into(),
            model: "m".into(),
            api,
            ..Default::default()
        }
    }

    /// A chat with a tool round whose assistant message carries native items.
    fn history(api: ProviderApi) -> Vec<ChatMessage> {
        let mut a = ChatMessage::new(Role::Assistant, "Looking.");
        a.tool_calls = vec![ToolCall {
            id: "c1".into(),
            name: "get".into(),
            arguments: r#"{"q":1}"#.into(),
        }];
        a.native = Some(NativeItems {
            api,
            items: vec![json!({"type": "thinking", "thinking": "hm", "signature": "sig"})],
        });
        let t = ChatMessage::tool_result(&a.tool_calls[0], "42", false);
        vec![ChatMessage::new(Role::User, "hi"), a, t]
    }

    #[test]
    fn anthropic_request_shape() {
        let p = ProviderConfig {
            model: "claude".into(),
            api: ProviderApi::Anthropic,
            options: ModelOptions {
                reasoning_effort: Some(ReasoningEffort::Low),
                temperature: Some(0.3),
                stop: vec!["STOP".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let msgs = to_api_messages("sys", &history(ProviderApi::Anthropic));
        let tools = [ToolSpec {
            name: "get".into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
        }];
        let body = anthropic_body(&p, &msgs, &tools, &p.resolved_options());
        assert_eq!(body["system"][0]["text"], "sys");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[1]["content"][0]["type"], "thinking");
        assert_eq!(m[1]["content"][0]["signature"], "sig");
        assert_eq!(m[1]["content"][2]["input"]["q"], 1);
        assert_eq!(m[2]["role"], "user");
        assert_eq!(m[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(m[2]["content"][0]["cache_control"]["type"], "ephemeral");
        // Thinking on: a budget from the effort, no temperature, room for the answer.
        assert_eq!(body["thinking"]["budget_tokens"], 2048);
        assert!(body.get("temperature").is_none());
        assert!(body["max_tokens"].as_u64().unwrap() > 2048);
        assert_eq!(body["stop_sequences"], json!(["STOP"]));

        // Items from another API are left out.
        let msgs = to_api_messages("", &history(ProviderApi::Responses));
        let body = anthropic_body(&p, &msgs, &[], &ModelOptions::default());
        assert_eq!(body["messages"][1]["content"][0]["type"], "text");
        assert!(body.get("system").is_none());
    }

    #[test]
    fn anthropic_images() {
        assert_eq!(
            anthropic_image("data:image/png;base64,QUJD"),
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                               "data": "QUJD"}})
        );
    }

    #[test]
    fn responses_request_shape() {
        let p = ProviderConfig {
            model: "gpt".into(),
            api: ProviderApi::Responses,
            ..Default::default()
        };
        let mut h = history(ProviderApi::Responses);
        h[1].native.as_mut().unwrap().items =
            vec![json!({"type": "reasoning", "id": "rs_1", "encrypted_content": "x"})];
        let msgs = to_api_messages("sys", &h);
        let o = ModelOptions {
            reasoning_effort: Some(ReasoningEffort::High),
            max_tokens: Some(500),
            ..Default::default()
        };
        let body = responses_body(&p, &msgs, &[], &o);
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
        assert_eq!(body["max_output_tokens"], 500);
        let types: Vec<&str> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["type"].as_str().or(i["role"].as_str()).unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "user",
                "reasoning",
                "assistant",
                "function_call",
                "function_call_output"
            ]
        );
    }

    #[tokio::test]
    async fn anthropic_stream_with_thinking_and_tool_use() {
        let (base, log) = serve(|_| {
            Response::sse(&[
                json!({"type": "message_start", "message": {"model": "claude-x",
                       "usage": {"input_tokens": 10, "cache_read_input_tokens": 90,
                                 "output_tokens": 1}}}),
                json!({"type": "content_block_start", "index": 0,
                       "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
                json!({"type": "content_block_delta", "index": 0,
                       "delta": {"type": "thinking_delta", "thinking": "Let me see."}}),
                json!({"type": "content_block_delta", "index": 0,
                       "delta": {"type": "signature_delta", "signature": "abc"}}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "content_block_start", "index": 1,
                       "content_block": {"type": "text", "text": ""}}),
                json!({"type": "content_block_delta", "index": 1,
                       "delta": {"type": "text_delta", "text": "Checking."}}),
                json!({"type": "content_block_start", "index": 2,
                       "content_block": {"type": "tool_use", "id": "tu1", "name": "get",
                                         "input": {}}}),
                json!({"type": "content_block_delta", "index": 2,
                       "delta": {"type": "input_json_delta", "partial_json": "{\"q\":"}}),
                json!({"type": "content_block_delta", "index": 2,
                       "delta": {"type": "input_json_delta", "partial_json": "2}"}}),
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                       "usage": {"output_tokens": 30}}),
                json!({"type": "message_stop"}),
            ])
        })
        .await;
        let http = build_http_client();
        let p = provider(&base, ProviderApi::Anthropic);
        let turn = chat_completion(&http, &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap();
        assert_eq!(turn.content, "Checking.");
        assert_eq!(turn.reasoning, "Let me see.");
        assert_eq!(turn.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "tu1");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"q":2}"#);
        let native = turn.native.unwrap();
        assert_eq!(native.api, ProviderApi::Anthropic);
        assert_eq!(native.items[0]["signature"], "abc");
        assert_eq!(turn.stats.prompt_tokens, Some(100));
        assert_eq!(turn.stats.cached_tokens, Some(90));
        assert_eq!(turn.stats.completion_tokens, Some(30));
        assert_eq!(turn.stats.served_model.as_deref(), Some("claude-x"));

        let log = log.lock().unwrap();
        let req = &log[0];
        assert_eq!(req.path, "/v1/messages");
        assert_eq!(req.header("x-api-key"), Some("key"));
        assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
        assert!(req.header("authorization").is_none());
    }

    #[tokio::test]
    async fn anthropic_whole_response() {
        let (base, _) = serve(|_| {
            Response::json(json!({
                "content": [{"type": "text", "text": "Hi"},
                            {"type": "tool_use", "id": "t", "name": "f", "input": {"a": 1}}],
                "stop_reason": "max_tokens",
                "usage": {"input_tokens": 5, "output_tokens": 2},
            }))
        })
        .await;
        let mut p = provider(&base, ProviderApi::Anthropic);
        p.stream = false;
        let turn = chat_completion(&build_http_client(), &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap();
        assert_eq!(turn.content, "Hi");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"a":1}"#);
        assert_eq!(turn.finish_reason.as_deref(), Some("length"));
        assert!(turn.native.is_none());
    }

    #[tokio::test]
    async fn responses_stream_with_reasoning_and_function_call() {
        let (base, log) = serve(|_| {
            Response::sse(&[
                json!({"type": "response.created", "response": {"status": "in_progress"}}),
                json!({"type": "response.reasoning_summary_text.delta", "output_index": 0,
                       "delta": "Thinking"}),
                json!({"type": "response.output_item.done", "output_index": 0,
                       "item": {"type": "reasoning", "id": "rs_1", "status": "completed",
                                "summary": [{"type": "summary_text", "text": "Thinking"}],
                                "encrypted_content": "enc"}}),
                json!({"type": "response.output_text.delta", "output_index": 1,
                       "delta": "Sure."}),
                json!({"type": "response.output_item.added", "output_index": 2,
                       "item": {"type": "function_call", "call_id": "call_9", "name": "get",
                                "arguments": ""}}),
                json!({"type": "response.function_call_arguments.delta", "output_index": 2,
                       "delta": "{\"q\":"}),
                json!({"type": "response.function_call_arguments.delta", "output_index": 2,
                       "delta": "3}"}),
                json!({"type": "response.completed", "response": {
                    "status": "completed", "model": "gpt-x",
                    "usage": {"input_tokens": 50, "output_tokens": 7,
                              "input_tokens_details": {"cached_tokens": 20}}}}),
            ])
        })
        .await;
        let mut p = provider(&base, ProviderApi::Responses);
        p.options.reasoning_effort = Some(ReasoningEffort::Medium);
        let turn = chat_completion(&build_http_client(), &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap();
        assert_eq!(turn.content, "Sure.");
        assert_eq!(turn.reasoning, "Thinking");
        assert_eq!(turn.tool_calls[0].id, "call_9");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"q":3}"#);
        assert_eq!(turn.finish_reason.as_deref(), Some("tool_calls"));
        let native = turn.native.unwrap();
        assert_eq!(native.items[0]["encrypted_content"], "enc");
        assert!(native.items[0].get("status").is_none());
        assert_eq!(
            (turn.stats.prompt_tokens, turn.stats.cached_tokens),
            (Some(50), Some(20))
        );

        let log = log.lock().unwrap();
        assert_eq!(log[0].path, "/v1/responses");
        assert_eq!(log[0].header("authorization"), Some("Bearer key"));
        assert_eq!(log[0].json()["reasoning"]["effort"], "medium");
    }

    #[tokio::test]
    async fn responses_whole_and_errors() {
        let (base, _) = serve(|r| {
            if r.json()["model"] == "bad" {
                return Response::json(json!({"status": "failed",
                                             "error": {"message": "nope"}}));
            }
            Response::json(json!({
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"},
                "output": [{"type": "message", "role": "assistant",
                            "content": [{"type": "output_text", "text": "Partial"}]}],
            }))
        })
        .await;
        let mut p = provider(&base, ProviderApi::Responses);
        p.stream = false;
        let http = build_http_client();
        let turn = chat_completion(&http, &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap();
        assert_eq!(turn.content, "Partial");
        assert_eq!(turn.finish_reason.as_deref(), Some("length"));
        p.model = "bad".into();
        let err = chat_completion(&http, &p, vec![], &[], |_| {}, |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[tokio::test]
    async fn chat_completions_leave_native_items_out() {
        let (base, log) = serve(|_| {
            Response::sse(&[json!({"choices": [{"delta": {"content": "ok"},
                                                "finish_reason": "stop"}]})])
        })
        .await;
        let mut p = provider(&base, ProviderApi::ChatCompletions);
        p.options.top_p = Some(0.4);
        p.model_options.insert(
            "m".into(),
            ModelOptions {
                top_p: Some(0.9),
                ..Default::default()
            },
        );
        let msgs = to_api_messages("", &history(ProviderApi::Anthropic));
        assert!(msgs[1].get(NATIVE_KEY).is_some());
        chat_completion(&build_http_client(), &p, msgs, &[], |_| {}, |_| {})
            .await
            .unwrap();
        let body = log.lock().unwrap()[0].json();
        assert!(body["messages"][1].get(NATIVE_KEY).is_none());
        // The model's own option wins over the provider's.
        assert_eq!(body["top_p"], json!(0.9f32));
    }
}

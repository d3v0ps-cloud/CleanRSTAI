//! Client for OpenAI-compatible `/chat/completions` endpoints (OpenAI, Ollama, LM Studio,
//! vLLM, llama.cpp server, OpenRouter, Groq, ...).

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

use crate::config::{ProviderApi, ProviderConfig, ProviderKind};
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
    /// Items a native API needs back with the next request.
    pub native: Option<crate::types::NativeItems>,
    /// Pages the provider's own search cited.
    pub sources: Vec<crate::types::Source>,
    /// The provider's own search ran for this reply.
    pub searched: bool,
}

pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("CleanRSTAI/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("failed to build HTTP client")
}

pub(crate) fn endpoint(p: &ProviderConfig, path: &str) -> String {
    format!("{}/{}", p.base_url.trim().trim_end_matches('/'), path)
}

pub(crate) fn authorize(
    mut req: reqwest::RequestBuilder,
    p: &ProviderConfig,
) -> reqwest::RequestBuilder {
    if p.api == ProviderApi::Anthropic {
        req = req.header("anthropic-version", "2023-06-01");
        if !p.api_key.trim().is_empty() {
            req = req.header("x-api-key", p.api_key.trim());
        }
    } else if !p.api_key.trim().is_empty() {
        req = req.bearer_auth(p.api_key.trim());
    }
    for h in &p.extra_headers {
        if !h.key.trim().is_empty() {
            req = req.header(h.key.trim(), h.value.as_str());
        }
    }
    req
}

pub(crate) async fn error_from_response(resp: reqwest::Response) -> anyhow::Error {
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
pub(crate) fn origin(p: &ProviderConfig) -> String {
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

/// What a llama.cpp server is doing with the provider's model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlamaStatus {
    Ready,
    /// `/health` answers 503 while the model loads.
    Loading,
    /// Unloaded after idling (`--sleep-idle-seconds`); the next request reloads it.
    Asleep,
}

/// The model a llama.cpp server runs and how it is doing, for the details beside the model
/// picker. Fields are None when the server doesn't report them.
#[derive(Clone, Debug, PartialEq)]
pub struct LlamaServerInfo {
    pub status: LlamaStatus,
    /// The `.gguf` file (`/props` `model_path`).
    pub model_path: Option<String>,
    pub build: Option<String>,
    /// Context size of one slot, and the number of slots (`--parallel`).
    pub n_ctx: Option<u64>,
    pub total_slots: Option<u64>,
    pub vision: Option<bool>,
    /// From `/v1/models` `meta`.
    pub n_params: Option<u64>,
    pub size_bytes: Option<u64>,
    pub n_ctx_train: Option<u64>,
    /// From `/metrics`, when the server runs with `--metrics`.
    pub metrics: Option<LlamaMetrics>,
}

/// A few of llama.cpp's Prometheus metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LlamaMetrics {
    pub requests_processing: Option<f64>,
    pub requests_deferred: Option<f64>,
    /// Speculative decoding: draft tokens accepted out of drafted, when it is on.
    pub draft_tokens: Option<f64>,
    pub draft_accepted: Option<f64>,
}

impl LlamaMetrics {
    /// Read the metrics we show from Prometheus text (`llamacpp:name value` lines).
    pub fn parse(text: &str) -> Self {
        let mut m = Self::default();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
                continue;
            };
            let Ok(value) = value.parse::<f64>() else {
                continue;
            };
            match name {
                "llamacpp:requests_processing" => m.requests_processing = Some(value),
                "llamacpp:requests_deferred" => m.requests_deferred = Some(value),
                "llamacpp:spec_decode_num_draft_tokens_total" => m.draft_tokens = Some(value),
                "llamacpp:spec_decode_num_accepted_tokens_total" => m.draft_accepted = Some(value),
                _ => {}
            }
        }
        m
    }

    /// Share of drafted tokens the model accepted, when speculative decoding has run.
    pub fn draft_acceptance(&self) -> Option<f64> {
        let drafted = self.draft_tokens.filter(|&d| d > 0.0)?;
        Some(self.draft_accepted.unwrap_or(0.0) / drafted)
    }
}

/// One model a llama.cpp router (one server, several models) can serve, from `/models`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterModel {
    pub id: String,
    /// `loaded`, `unloaded`, `loading`, `sleeping`, `downloading` or `incomplete`.
    pub status: String,
    /// The last load failed.
    pub failed: bool,
}

/// The models of a llama.cpp router and their load state. None for a single-model server,
/// whose `/models` entries carry no `status`, and for anything else.
pub async fn llama_cpp_router_models(
    http: &reqwest::Client,
    p: &ProviderConfig,
) -> Option<Vec<RouterModel>> {
    let resp = authorize(http.get(format!("{}/models", origin(p))), p)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    let models: Vec<RouterModel> = v
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|m| {
            let status = m.get("status")?;
            Some(RouterModel {
                id: m.get("id")?.as_str()?.to_owned(),
                status: status.get("value")?.as_str()?.to_owned(),
                failed: status.get("failed").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect();
    (!models.is_empty()).then_some(models)
}

/// Ask a llama.cpp router to load (or unload) one of its models.
pub async fn llama_cpp_set_loaded(
    http: &reqwest::Client,
    p: &ProviderConfig,
    model: &str,
    load: bool,
) -> Result<()> {
    let path = if load { "models/load" } else { "models/unload" };
    let resp = authorize(http.post(format!("{}/{path}", origin(p))), p)
        .timeout(std::time::Duration::from_secs(30))
        .json(&json!({"model": model}))
        .send()
        .await
        .context("couldn't reach the server")?;
    if !resp.status().is_success() {
        return Err(error_from_response(resp).await);
    }
    Ok(())
}

/// Ask a llama.cpp server about the provider's model: `/health`, `/props`, `/v1/models`
/// and, when on, `/metrics`, together. None when it doesn't answer like llama.cpp.
pub async fn llama_cpp_server_info(
    http: &reqwest::Client,
    p: &ProviderConfig,
) -> Option<LlamaServerInfo> {
    let json = |req: reqwest::RequestBuilder| async move {
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<Value>().await.ok()
    };
    let (health, props, models, metrics) = tokio::join!(
        llama_cpp_get(http, p, "health").send(),
        json(llama_cpp_get(http, p, "props")),
        json(
            authorize(http.get(endpoint(p, "models")), p)
                .timeout(std::time::Duration::from_secs(5))
        ),
        async {
            let resp = llama_cpp_get(http, p, "metrics").send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            resp.text().await.ok()
        },
    );
    let loading = health.is_ok_and(|r| r.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE);
    // While a single-model server loads, /props may not answer yet.
    let props = props.filter(|v| {
        v.get("default_generation_settings").is_some() || v.get("build_info").is_some()
    });
    if props.is_none() && !loading {
        return None;
    }
    let props = props.unwrap_or(Value::Null);
    let model = p.model.trim();
    let meta = models.as_ref().and_then(|v| {
        let list = v.get("data")?.as_array()?;
        // A single-model server lists one model, whatever its id; a router lists them all.
        let entry = list
            .iter()
            .find(|m| m.get("id").and_then(Value::as_str) == Some(model))
            .or_else(|| (list.len() == 1).then(|| &list[0]))?;
        entry.get("meta").filter(|m| m.is_object())
    });
    let meta_n = |k: &str| meta.and_then(|m| m.get(k)).and_then(Value::as_u64);
    let status = if loading {
        LlamaStatus::Loading
    } else if props.get("is_sleeping").and_then(Value::as_bool) == Some(true) {
        LlamaStatus::Asleep
    } else {
        LlamaStatus::Ready
    };
    Some(LlamaServerInfo {
        status,
        model_path: props
            .get("model_path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        build: props
            .get("build_info")
            .and_then(Value::as_str)
            .map(str::to_owned),
        n_ctx: props
            .pointer("/default_generation_settings/n_ctx")
            .and_then(Value::as_u64),
        total_slots: props.get("total_slots").and_then(Value::as_u64),
        vision: props.pointer("/modalities/vision").and_then(Value::as_bool),
        n_params: meta_n("n_params"),
        size_bytes: meta_n("size"),
        n_ctx_train: meta_n("n_ctx_train"),
        metrics: metrics.map(|t| LlamaMetrics::parse(&t)),
    })
}

/// llama.cpp's `/slots`: how many of the server's parallel slots are in use. Asked at the
/// start of a generation, like the context size, not on every request.
pub async fn llama_cpp_slot_usage(
    http: &reqwest::Client,
    p: &ProviderConfig,
) -> Option<crate::stats::SlotUsage> {
    let resp = llama_cpp_get(http, p, "slots").send().await.ok()?;
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

/// A GET to one of llama.cpp's own endpoints (`/props`, `/slots`). In router mode (one
/// server, several models) these need `?model=`; a single-model server ignores it.
/// `autoload=false` keeps the router from loading a model just to answer.
fn llama_cpp_get(
    http: &reqwest::Client,
    p: &ProviderConfig,
    path: &str,
) -> reqwest::RequestBuilder {
    let mut req = http.get(format!("{}/{path}", origin(p)));
    let model = p.model.trim();
    if !model.is_empty() {
        req = req.query(&[("model", model), ("autoload", "false")]);
    }
    authorize(req, p).timeout(std::time::Duration::from_secs(5))
}

async fn llama_cpp_props(http: &reqwest::Client, p: &ProviderConfig) -> Option<Value> {
    let resp = llama_cpp_get(http, p, "props").send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    (v.get("default_generation_settings").is_some() || v.get("build_info").is_some()).then_some(v)
}

/// What a LiteLLM proxy lists for the provider's model in `/model/info`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiteLlmModel {
    /// Per-token prices, to estimate the cost of streamed replies (LiteLLM sends the cost of
    /// non-streamed ones only).
    pub prices: Option<crate::stats::ModelPrices>,
    /// The context window, for context use.
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub provider: Option<String>,
    pub vision: Option<bool>,
    pub tools: Option<bool>,
    pub reasoning: Option<bool>,
    pub pdf: Option<bool>,
}

/// The provider's model in a LiteLLM proxy's `/model/info`, which any key may read for its
/// own models. Asked once per proxy and model; failures are asked again.
pub async fn litellm_model_info(
    http: &reqwest::Client,
    p: &ProviderConfig,
) -> Option<LiteLlmModel> {
    static CACHE: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<(String, String), LiteLlmModel>>,
    > = std::sync::LazyLock::new(Default::default);
    let model = p.model.trim();
    let key = (origin(p), model.to_owned());
    if let Some(info) = CACHE.lock().unwrap().get(&key) {
        return Some(info.clone());
    }
    let resp = authorize(http.get(format!("{}/model/info", origin(p))), p)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    // One entry per deployment of a model name; take each field from the first that has it.
    let infos: Vec<&Value> = v
        .get("data")?
        .as_array()?
        .iter()
        .filter(|d| d.get("model_name").and_then(Value::as_str) == Some(model))
        .filter_map(|d| d.get("model_info"))
        .collect();
    if infos.is_empty() {
        return None;
    }
    let first = |k: &str| infos.iter().find_map(|i| i.get(k).filter(|v| !v.is_null()));
    let num = |k: &str| first(k).and_then(Value::as_u64);
    let flag = |k: &str| first(k).and_then(Value::as_bool);
    let info = LiteLlmModel {
        prices: infos.iter().find_map(|i| {
            let price = |k: &str| i.get(k).and_then(Value::as_f64);
            Some(crate::stats::ModelPrices {
                input: price("input_cost_per_token")?,
                output: price("output_cost_per_token")?,
                cache_read: price("cache_read_input_token_cost"),
            })
        }),
        max_input_tokens: num("max_input_tokens").or_else(|| num("max_tokens")),
        max_output_tokens: num("max_output_tokens"),
        provider: first("litellm_provider")
            .and_then(Value::as_str)
            .map(str::to_owned),
        vision: flag("supports_vision"),
        tools: flag("supports_function_calling"),
        reasoning: flag("supports_reasoning"),
        pdf: flag("supports_pdf_input"),
    };
    CACHE.lock().unwrap().insert(key, info.clone());
    Some(info)
}

/// A LiteLLM key's spend and budget, from `/key/info` (each key may read its own).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiteLlmKey {
    pub spend: Option<f64>,
    pub max_budget: Option<f64>,
}

pub async fn litellm_key_info(http: &reqwest::Client, p: &ProviderConfig) -> Option<LiteLlmKey> {
    let resp = authorize(http.get(format!("{}/key/info", origin(p))), p)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    let info = v.get("info").unwrap_or(&v);
    let key = LiteLlmKey {
        spend: info.get("spend").and_then(Value::as_f64),
        max_budget: info.get("max_budget").and_then(Value::as_f64),
    };
    (key.spend.is_some() || key.max_budget.is_some()).then_some(key)
}

/// What a LiteLLM proxy says about one request in its `x-litellm-*` response headers. None
/// when there are none (not LiteLLM).
pub fn litellm_details(headers: &reqwest::header::HeaderMap) -> Option<crate::stats::ProxyDetails> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "None")
    };
    if !headers.keys().any(|k| k.as_str().starts_with("x-litellm")) {
        return None;
    }
    let f = |name: &str| get(name).and_then(|v| v.parse::<f64>().ok());
    let n = |name: &str| {
        get(name)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v as u64)
    };
    // Per-key limits are sent per model group: x-litellm-key-remaining-requests-<group>.
    let prefixed = |prefix: &str| {
        headers.iter().find_map(|(k, v)| {
            k.as_str()
                .starts_with(prefix)
                .then(|| v.to_str().ok()?.trim().parse::<f64>().ok())
                .flatten()
                .map(|v| v as u64)
        })
    };
    let mut cost_breakdown = Vec::new();
    for (part, header) in [
        ("input", "x-litellm-response-cost-input"),
        ("output", "x-litellm-response-cost-output"),
        ("cache read", "x-litellm-response-cost-cache-read"),
        ("cache write", "x-litellm-response-cost-cache-creation"),
        ("reasoning", "x-litellm-response-cost-reasoning"),
        ("tools", "x-litellm-response-cost-tool-usage"),
    ] {
        if let Some(c) = f(header).filter(|&c| c > 0.0) {
            cost_breakdown.push((part.to_owned(), c));
        }
    }
    Some(crate::stats::ProxyDetails {
        deployment: get("x-litellm-model-name").map(str::to_owned),
        api_base: get("x-litellm-model-api-base").map(str::to_owned),
        retries: n("x-litellm-attempted-retries").filter(|&r| r > 0),
        fallbacks: n("x-litellm-attempted-fallbacks").filter(|&r| r > 0),
        key_spend: f("x-litellm-key-spend"),
        key_budget: f("x-litellm-key-max-budget"),
        remaining_requests: n("x-ratelimit-remaining-requests")
            .or_else(|| prefixed("x-litellm-key-remaining-requests-")),
        remaining_tokens: n("x-ratelimit-remaining-tokens")
            .or_else(|| prefixed("x-litellm-key-remaining-tokens-")),
        cost_breakdown,
        cache_used: get("x-litellm-cache-key").is_some(),
        overhead_ms: f("x-litellm-overhead-duration-ms"),
    })
}

/// Work out which kind of server a provider points at.
pub async fn detect_kind(http: &reqwest::Client, p: &ProviderConfig) -> ProviderKind {
    if llama_cpp_props(http, p).await.is_some() {
        return ProviderKind::LlamaCpp;
    }
    if let Some(kind) = crate::servers::detect(http, p).await {
        return kind;
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
                if !m.content.is_empty() || m.native.is_some() {
                    let mut v = json!({"role": "assistant", "content": m.content});
                    add_native(&mut v, m);
                    out.push(v);
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
                let mut v = json!({
                    "role": "assistant",
                    "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                    "tool_calls": calls,
                });
                add_native(&mut v, m);
                out.push(v);
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

/// Keep a native API's items with the message; only that API reads them, and
/// `chat_completion` removes them before sending to any other.
fn add_native(v: &mut Value, m: &ChatMessage) {
    if let Some(n) = &m.native
        && let Ok(n) = serde_json::to_value(n)
    {
        v[crate::native::NATIVE_KEY] = n;
    }
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
    mut on_delta: impl FnMut(Delta<'_>) + Send,
    mut on_stats: impl FnMut(RequestStats) + Send,
) -> Result<AssistantTurn> {
    if p.model.trim().is_empty() {
        bail!("No model selected. Choose one in Settings → Providers.");
    }
    match p.api {
        ProviderApi::Anthropic => {
            return crate::native::anthropic(
                http,
                p,
                messages,
                tools,
                &mut on_delta,
                &mut on_stats,
            )
            .await;
        }
        ProviderApi::Responses => {
            return crate::native::responses(
                http,
                p,
                messages,
                tools,
                &mut on_delta,
                &mut on_stats,
            )
            .await;
        }
        ProviderApi::ChatCompletions => {}
    }
    let mut messages = messages;
    for m in &mut messages {
        if let Some(o) = m.as_object_mut() {
            o.remove(crate::native::NATIVE_KEY);
        }
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
    p.resolved_options()
        .apply_chat_completions(&mut body, p.kind);
    // llama.cpp reports how far it has got reading the prompt, which can take minutes on
    // a long one. Other servers may refuse fields they don't know, so only for llama.cpp.
    // OpenRouter reports the cost and cached tokens when asked to account for usage.
    if p.kind == ProviderKind::OpenRouter {
        body["usage"] = json!({"include": true});
    }
    // OpenRouter's web plugin searches for the model and cites the pages it used.
    let web_plugin = p.native_search.filter(|_| {
        crate::native::native_search_support(p) == Some(crate::native::NativeKind::OpenRouter)
    });
    if let Some(max_results) = web_plugin {
        body["plugins"] = json!([{"id": "web", "max_results": max_results}]);
    }
    if p.stream && p.kind == ProviderKind::LlamaCpp {
        body["return_progress"] = json!(true);
        // Its own prefill/decode speeds in every chunk, for the live readout.
        body["timings_per_token"] = json!(true);
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
    // LiteLLM's cost. Some versions leave the header out when it equals the cost before
    // discounts, which they send as `-original`.
    stats.header_cost_usd = [
        "x-litellm-response-cost",
        "x-litellm-response-cost-original",
    ]
    .iter()
    .find_map(|h| {
        resp.headers()
            .get(*h)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
    });
    stats.proxy = litellm_details(resp.headers());

    let mut turn = AssistantTurn {
        searched: web_plugin.is_some(),
        ..Default::default()
    };
    let mut builder = ToolCallBuilder::default();
    // Live stats are pushed at most a few times a second, not on every chunk.
    let mut last_live = std::time::Instant::now();
    // The first prompt progress goes out at once, so the bar shows from the start.
    let mut progress_shown = false;

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
        if let Some(a) = msg.get("annotations") {
            crate::native::add_annotations(&mut turn.sources, a);
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
        if v.get("prompt_progress").is_some()
            && (!progress_shown || last_live.elapsed() >= std::time::Duration::from_millis(100))
        {
            progress_shown = true;
            last_live = std::time::Instant::now();
            on_stats(stats.snapshot(&p.model));
        }
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
                if let Some(a) = delta.get("annotations") {
                    crate::native::add_annotations(&mut turn.sources, a);
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
            ("GET", "/props?model=m&autoload=false") => Response::json(json!({
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
                .filter(|r| r.path.starts_with("/props"))
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
    async fn llama_cpp_prompt_progress() {
        let progress = |processed: u64| {
            json!({"choices": [{"delta": {"role": "assistant", "content": null}}],
                   "prompt_progress": {"total": 4096, "cache": 0, "processed": processed,
                                       "time_ms": processed as f64}})
        };
        let (base, log) = serve(move |_| {
            Response::sse(&[
                progress(0),
                progress(2048),
                json!({"choices": [{"delta": {"content": "Hi"}, "finish_reason": "stop"}]}),
            ])
        })
        .await;
        let http = build_http_client();
        let mut live = Vec::new();
        let turn = chat_completion(
            &http,
            &provider(&base, ProviderKind::LlamaCpp),
            vec![],
            &[],
            |_| {},
            |s| live.push(s.prompt_progress),
        )
        .await
        .unwrap();
        assert_eq!(turn.content, "Hi");
        assert_eq!(turn.stats.prompt_progress, None);
        assert_eq!(
            live[0].map(|p| p.processed),
            Some(0),
            "first update goes out at once"
        );
        assert_eq!(log.lock().unwrap()[0].json()["return_progress"], true);
        assert_eq!(log.lock().unwrap()[0].json()["timings_per_token"], true);

        // Not asked of other servers, which may refuse fields they don't know.
        let (base, log) = serve(|_| Response::sse(&[json!({"choices": [{"delta": {}}]})])).await;
        chat_completion(
            &http,
            &provider(&base, ProviderKind::Generic),
            vec![],
            &[],
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
        let body = log.lock().unwrap()[0].json();
        assert!(body.get("return_progress").is_none());
        assert!(body.get("timings_per_token").is_none());
    }

    #[tokio::test]
    async fn llama_cpp_server_info_report() {
        let (base, _) =
            serve(
                |r| match r.path.split_once('?').map_or(r.path.as_str(), |(p, _)| p) {
                    "/health" => Response::json(json!({"status": "ok"})),
                    "/props" => Response::json(json!({
                "default_generation_settings": {"n_ctx": 32768},
                "total_slots": 2,
                "model_path": "/models/Qwen-27B-Q4_K_M.gguf",
                "modalities": {"vision": true},
                "build_info": "b6500-abc1234",
                "is_sleeping": false})),
                    "/v1/models" => Response::json(json!({"data": [{"id": "whatever", "meta": {
                "n_params": 27_000_000_000_u64, "size": 16_300_000_000_u64,
                "n_ctx_train": 262_144}}]})),
                    "/metrics" => Response::with_type(
                        200,
                        "text/plain",
                        "# HELP llamacpp:requests_processing x\n\
                 llamacpp:requests_processing 1\n\
                 llamacpp:requests_deferred 2\n\
                 llamacpp:spec_decode_num_draft_tokens_total 200\n\
                 llamacpp:spec_decode_num_accepted_tokens_total 150\n",
                    ),
                    _ => Response::status(404),
                },
            )
            .await;
        let http = build_http_client();
        let info = llama_cpp_server_info(&http, &provider(&base, ProviderKind::LlamaCpp))
            .await
            .unwrap();
        assert_eq!(info.status, LlamaStatus::Ready);
        assert_eq!(
            info.model_path.as_deref(),
            Some("/models/Qwen-27B-Q4_K_M.gguf")
        );
        assert_eq!((info.n_ctx, info.total_slots), (Some(32768), Some(2)));
        assert_eq!(info.vision, Some(true));
        // A single-model server's one entry counts, whatever its id.
        assert_eq!(info.n_params, Some(27_000_000_000));
        assert_eq!(
            (info.size_bytes, info.n_ctx_train),
            (Some(16_300_000_000), Some(262_144))
        );
        let m = info.metrics.unwrap();
        assert_eq!(
            (m.requests_processing, m.requests_deferred),
            (Some(1.0), Some(2.0))
        );
        assert_eq!(m.draft_acceptance(), Some(0.75));
    }

    #[tokio::test]
    async fn llama_cpp_server_info_loading_and_asleep() {
        // Loading: /health answers 503, and /props may not answer yet.
        let (base, _) = serve(|_| Response::status(503)).await;
        let http = build_http_client();
        let info = llama_cpp_server_info(&http, &provider(&base, ProviderKind::LlamaCpp))
            .await
            .unwrap();
        assert_eq!(info.status, LlamaStatus::Loading);
        assert_eq!(
            info.metrics, None,
            "/metrics is off unless the server runs --metrics"
        );

        let (base, _) =
            serve(
                |r| match r.path.split_once('?').map_or(r.path.as_str(), |(p, _)| p) {
                    "/health" => Response::json(json!({"status": "ok"})),
                    "/props" => Response::json(json!({"build_info": "b1", "is_sleeping": true})),
                    _ => Response::status(404),
                },
            )
            .await;
        let info = llama_cpp_server_info(&http, &provider(&base, ProviderKind::LlamaCpp))
            .await
            .unwrap();
        assert_eq!(info.status, LlamaStatus::Asleep);

        // Not llama.cpp at all.
        let (base, _) = serve(|_| Response::status(404)).await;
        assert_eq!(
            llama_cpp_server_info(&http, &provider(&base, ProviderKind::LlamaCpp)).await,
            None
        );
    }

    #[tokio::test]
    async fn llama_cpp_router_models_and_loading() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/models") => Response::json(json!({"data": [
                {"id": "qwen", "status": {"value": "loaded", "args": ["llama-server"]}},
                {"id": "gemma", "status": {"value": "unloaded", "failed": true}},
                {"id": "tiny", "status": {"value": "sleeping"}},
            ]})),
            ("POST", "/models/load") => Response::json(json!({"success": true})),
            ("POST", "/models/unload") => Response::with_type(
                400,
                "application/json",
                json!({"error": {"message": "model is not loaded"}}).to_string(),
            ),
            _ => Response::status(404),
        })
        .await;
        let http = build_http_client();
        let p = provider(&base, ProviderKind::LlamaCpp);
        let models = llama_cpp_router_models(&http, &p).await.unwrap();
        let summary: Vec<(&str, &str, bool)> = models
            .iter()
            .map(|m| (m.id.as_str(), m.status.as_str(), m.failed))
            .collect();
        assert_eq!(
            summary,
            [
                ("qwen", "loaded", false),
                ("gemma", "unloaded", true),
                ("tiny", "sleeping", false)
            ]
        );

        llama_cpp_set_loaded(&http, &p, "gemma", true)
            .await
            .unwrap();
        let err = llama_cpp_set_loaded(&http, &p, "gemma", false)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("model is not loaded"),
            "{err:#}"
        );
        let load = log
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.path == "/models/load")
            .unwrap()
            .json();
        assert_eq!(load, json!({"model": "gemma"}));

        // A single-model server's /models entries have no status: not a router.
        let (base, _) = serve(|_| Response::json(json!({"data": [{"id": "m"}]}))).await;
        assert_eq!(
            llama_cpp_router_models(&http, &provider(&base, ProviderKind::LlamaCpp)).await,
            None
        );
    }

    #[tokio::test]
    async fn llama_cpp_router_mode_needs_the_model() {
        // A router answers /props only for a named model, like llama.cpp's own router.
        let (base, _) = serve(|r| match r.path.split_once('?') {
            Some(("/props", q)) if q.contains("model=qwen%2Fq4") => Response::json(json!({
                "default_generation_settings": {"n_ctx": 32768}, "build_info": "b2"})),
            _ => Response::status(400),
        })
        .await;
        let http = build_http_client();
        let mut p = provider(&base, ProviderKind::Generic);
        p.model = "qwen/q4".into();
        assert_eq!(detect_kind(&http, &p).await, ProviderKind::LlamaCpp);
        assert_eq!(llama_cpp_context_size(&http, &p).await, Some(32768));
    }

    #[tokio::test]
    async fn llama_cpp_slot_usage_report() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/slots?model=m&autoload=false") => Response::json(json!([
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
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|r| r.path.starts_with("/slots"))
        );
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
    async fn open_router_usage_and_provider() {
        let (base, log) = serve(|_| {
            Response::sse(&[
                json!({"provider": "Anthropic", "choices": [{"delta": {"content": "a"}}]}),
                json!({"provider": "Anthropic", "choices": [],
                       "usage": {"prompt_tokens": 10, "completion_tokens": 1, "cost": 0.0001}}),
            ])
        })
        .await;
        let http = build_http_client();
        let s = chat_completion(
            &http,
            &provider(&base, ProviderKind::OpenRouter),
            vec![],
            &[],
            |_| {},
            |_| {},
        )
        .await
        .unwrap()
        .stats;
        assert_eq!(s.upstream.as_deref(), Some("Anthropic"));
        assert_eq!(s.cost_usd, Some(0.0001));
        assert_eq!(
            log.lock().unwrap()[0].json()["usage"],
            json!({"include": true})
        );
    }

    #[tokio::test]
    async fn litellm_cost_without_the_main_header() {
        // Some LiteLLM versions send only the cost before discounts.
        let (base, _) = serve(|_| {
            Response::sse(&[json!({"choices": [{"delta": {"content": "a"}}]})])
                .header("x-litellm-response-cost-original", "0.0021")
        })
        .await;
        let http = build_http_client();
        let s = chat_completion(
            &http,
            &provider(&base, ProviderKind::LiteLlm),
            vec![],
            &[],
            |_| {},
            |_| {},
        )
        .await
        .unwrap()
        .stats;
        assert_eq!(s.cost_usd, Some(0.0021));
    }

    #[tokio::test]
    async fn litellm_model_info_and_key_info() {
        let (base, log) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/model/info") => Response::json(json!({"data": [
                {"model_name": "other", "model_info": {
                    "input_cost_per_token": 1.0, "output_cost_per_token": 1.0}},
                // Fields come from the first deployment of the model that has them.
                {"model_name": "m", "model_info": {"id": "a", "max_input_tokens": 128000,
                    "litellm_provider": "azure", "supports_vision": true}},
                {"model_name": "m", "model_info": {
                    "input_cost_per_token": 0.000003, "output_cost_per_token": 0.000015,
                    "cache_read_input_token_cost": 0.0000003, "max_input_tokens": 1,
                    "supports_function_calling": true}},
            ]})),
            ("GET", "/key/info") => Response::json(json!({"key": "sk-…",
                "info": {"spend": 4.2, "max_budget": 50.0}})),
            _ => Response::status(404),
        })
        .await;
        let http = build_http_client();
        let p = provider(&base, ProviderKind::LiteLlm);
        let want = LiteLlmModel {
            prices: Some(crate::stats::ModelPrices {
                input: 0.000003,
                output: 0.000015,
                cache_read: Some(0.0000003),
            }),
            max_input_tokens: Some(128_000),
            provider: Some("azure".into()),
            vision: Some(true),
            tools: Some(true),
            ..Default::default()
        };
        assert_eq!(litellm_model_info(&http, &p).await, Some(want.clone()));
        // Asked once: the second call is answered from the cache.
        assert_eq!(litellm_model_info(&http, &p).await, Some(want));
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|r| r.path == "/model/info")
                .count(),
            1
        );
        assert_eq!(
            litellm_key_info(&http, &p).await,
            Some(LiteLlmKey {
                spend: Some(4.2),
                max_budget: Some(50.0)
            })
        );

        // A proxy without these endpoints (or a model it doesn't list) gives nothing.
        let (base, _) = serve(|_| Response::status(404)).await;
        let p = provider(&base, ProviderKind::LiteLlm);
        assert_eq!(litellm_model_info(&http, &p).await, None);
        assert_eq!(litellm_key_info(&http, &p).await, None);
    }

    #[test]
    fn litellm_response_headers() {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in [
            ("x-litellm-model-name", "azure/gpt-4o"),
            ("x-litellm-model-api-base", "https://x.openai.azure.com"),
            ("x-litellm-attempted-retries", "2"),
            ("x-litellm-attempted-fallbacks", "0"),
            ("x-litellm-key-spend", "4.25"),
            ("x-litellm-key-max-budget", "None"),
            ("x-litellm-key-remaining-requests-gpt-4o", "99"),
            ("x-ratelimit-remaining-tokens", "40000"),
            ("x-litellm-response-cost-input", "0.001"),
            ("x-litellm-response-cost-output", "0.002"),
            ("x-litellm-response-cost-reasoning", "0"),
            ("x-litellm-cache-key", "abc"),
            ("x-litellm-overhead-duration-ms", "12.5"),
        ] {
            h.insert(k, v.parse().unwrap());
        }
        let d = litellm_details(&h).unwrap();
        assert_eq!(d.deployment.as_deref(), Some("azure/gpt-4o"));
        assert_eq!(d.api_base.as_deref(), Some("https://x.openai.azure.com"));
        assert_eq!(
            (d.retries, d.fallbacks),
            (Some(2), None),
            "zero isn't shown"
        );
        assert_eq!(
            (d.key_spend, d.key_budget),
            (Some(4.25), None),
            "\"None\" is no budget"
        );
        assert_eq!(
            (d.remaining_requests, d.remaining_tokens),
            (Some(99), Some(40_000))
        );
        assert_eq!(
            d.cost_breakdown,
            [("input".to_owned(), 0.001), ("output".to_owned(), 0.002)]
        );
        assert!(d.cache_used);
        assert_eq!(d.overhead_ms, Some(12.5));
        // Not LiteLLM: nothing.
        assert_eq!(litellm_details(&reqwest::header::HeaderMap::new()), None);
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

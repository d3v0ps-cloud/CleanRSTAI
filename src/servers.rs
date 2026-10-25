//! What Ollama, LM Studio, vLLM and OpenRouter report beyond the OpenAI-compatible API: how
//! to recognise them, the context size the model actually runs with (for context use), and
//! the details shown beside the model picker. llama.cpp and LiteLLM, which have more to
//! show, live in `llm.rs`.

use std::time::Duration;

use serde_json::Value;

use crate::config::{ProviderConfig, ProviderKind};
use crate::llm::{authorize, endpoint, origin};
use crate::stats::{fmt_bytes, fmt_cost, fmt_tokens};

/// What a server reports about the provider's model, for the hover beside the model picker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerDetails {
    /// The server's name, as the hover's heading.
    pub title: &'static str,
    /// Label and value, in order; rows the server doesn't report are left out.
    pub rows: Vec<(String, String)>,
    /// A short state worth showing beside the picker, such as "Not loaded".
    pub status: Option<String>,
    /// The context size the model runs with, for context use.
    pub context_size: Option<u64>,
}

impl ServerDetails {
    fn row(&mut self, label: &str, value: impl Into<String>) {
        self.rows.push((label.to_owned(), value.into()));
    }
}

async fn get_json(req: reqwest::RequestBuilder) -> Option<Value> {
    let resp = req.timeout(Duration::from_secs(5)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

async fn get_text(req: reqwest::RequestBuilder) -> Option<String> {
    let resp = req.timeout(Duration::from_secs(5)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
}

/// Recognise Ollama, LM Studio, vLLM or OpenRouter, for Detect. None for anything else.
pub async fn detect(http: &reqwest::Client, p: &ProviderConfig) -> Option<ProviderKind> {
    if p.base_url.contains("openrouter.ai") {
        return Some(ProviderKind::OpenRouter);
    }
    let base = origin(p);
    let (ollama, lm_studio, models) = tokio::join!(
        get_json(http.get(format!("{base}/api/version"))),
        get_json(authorize(http.get(format!("{base}/api/v0/models")), p)),
        get_json(authorize(http.get(endpoint(p, "models")), p)),
    );
    if ollama.is_some_and(|v| v.get("version").is_some_and(Value::is_string)) {
        return Some(ProviderKind::Ollama);
    }
    let first = |v: &Option<Value>| v.as_ref()?.pointer("/data/0").cloned();
    if first(&lm_studio).is_some_and(|m| m.get("state").is_some()) {
        return Some(ProviderKind::LmStudio);
    }
    if first(&models).is_some_and(|m| m.get("owned_by").and_then(Value::as_str) == Some("vllm")) {
        return Some(ProviderKind::Vllm);
    }
    None
}

/// What the provider's server reports about its model. None for other kinds, or when the
/// server doesn't answer like its kind.
pub async fn details(http: &reqwest::Client, p: &ProviderConfig) -> Option<ServerDetails> {
    match p.kind {
        ProviderKind::Ollama => ollama(http, p).await,
        ProviderKind::LmStudio => lm_studio(http, p).await,
        ProviderKind::Vllm => vllm(http, p).await,
        ProviderKind::OpenRouter => open_router(http, p).await,
        _ => None,
    }
}

/// The context size the model runs with, for context use. OpenRouter's model list is
/// large, so its answers are kept per model; the local servers are asked each time, since
/// a model can be reloaded with another size.
pub async fn context_size(http: &reqwest::Client, p: &ProviderConfig) -> Option<u64> {
    if p.kind == ProviderKind::OpenRouter {
        static CACHE: std::sync::LazyLock<
            std::sync::Mutex<std::collections::HashMap<(String, String), u64>>,
        > = std::sync::LazyLock::new(Default::default);
        let key = (origin(p), p.model.trim().to_owned());
        if let Some(n) = CACHE.lock().unwrap().get(&key) {
            return Some(*n);
        }
        let n = open_router_model(http, p)
            .await?
            .get("context_length")
            .and_then(Value::as_u64)?;
        CACHE.lock().unwrap().insert(key, n);
        return Some(n);
    }
    details(http, p).await?.context_size
}

/// The model's entry in a list, by `id` (or Ollama's `name`/`model`).
fn find_model<'a>(list: &'a Value, model: &str) -> Option<&'a Value> {
    list.as_array()?.iter().find(|m| {
        ["id", "name", "model"]
            .iter()
            .any(|k| m.get(*k).and_then(Value::as_str) == Some(model))
    })
}

// ---------------------------------------------------------------- Ollama

async fn ollama(http: &reqwest::Client, p: &ProviderConfig) -> Option<ServerDetails> {
    let base = origin(p);
    let model = p.model.trim();
    let (version, ps, show) = tokio::join!(
        get_json(http.get(format!("{base}/api/version"))),
        get_json(http.get(format!("{base}/api/ps"))),
        get_json(
            http.post(format!("{base}/api/show"))
                .json(&serde_json::json!({"model": model}))
        ),
    );
    let version = version?.get("version")?.as_str()?.to_owned();
    let mut d = ServerDetails {
        title: "Ollama",
        ..Default::default()
    };
    let running = ps
        .as_ref()
        .and_then(|v| find_model(v.get("models")?, model));
    let info = running
        .and_then(|m| m.get("details"))
        .or_else(|| show.as_ref()?.get("details"));
    let text = |v: Option<&Value>, k: &str| {
        v.and_then(|v| v.get(k))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    d.row(
        "Status",
        if running.is_some() {
            "Loaded"
        } else {
            "Not loaded"
        },
    );
    if running.is_none() {
        d.status = Some("Not loaded".into());
    }
    if let Some(s) = text(info, "parameter_size") {
        d.row("Parameters", s);
    }
    if let Some(s) = text(info, "quantization_level") {
        d.row("Quantization", s);
    }
    if let Some(s) = text(info, "family") {
        d.row("Family", s);
    }
    if let Some(m) = running {
        let n = |k: &str| m.get(k).and_then(Value::as_u64);
        if let Some(size) = n("size") {
            d.row("Size", fmt_bytes(size));
            if let Some(vram) = n("size_vram") {
                d.row(
                    "In GPU memory",
                    format!("{:.0}%", vram as f64 * 100.0 / size.max(1) as f64),
                );
            }
        }
        // Newer versions report the context size the model was loaded with.
        d.context_size = n("context_length");
    }
    // Otherwise a size set in the Modelfile (`PARAMETER num_ctx 8192`).
    if d.context_size.is_none() {
        d.context_size = show
            .as_ref()
            .and_then(|s| num_ctx(s.get("parameters")?.as_str()?));
    }
    if let Some(n) = d.context_size {
        d.row("Context", fmt_tokens(n));
    }
    if let Some(info) = show.as_ref().and_then(|s| s.get("model_info")) {
        let trained = info.as_object().and_then(|o| {
            o.iter()
                .find(|(k, _)| k.ends_with(".context_length"))
                .and_then(|(_, v)| v.as_u64())
        });
        if let Some(n) = trained {
            d.row("Trained context", fmt_tokens(n));
        }
    }
    if let Some(caps) = show
        .as_ref()
        .and_then(|s| s.get("capabilities")?.as_array().cloned())
    {
        let caps: Vec<&str> = caps.iter().filter_map(Value::as_str).collect();
        if !caps.is_empty() {
            d.row("Capabilities", caps.join(", "));
        }
    }
    d.row("Version", version);
    Some(d)
}

/// `num_ctx` from Ollama's `parameters` text (`num_ctx    8192` lines).
fn num_ctx(parameters: &str) -> Option<u64> {
    parameters.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        (parts.next()? == "num_ctx").then(|| parts.next()?.parse().ok())?
    })
}

// ---------------------------------------------------------------- LM Studio

async fn lm_studio(http: &reqwest::Client, p: &ProviderConfig) -> Option<ServerDetails> {
    let list = get_json(authorize(
        http.get(format!("{}/api/v0/models", origin(p))),
        p,
    ))
    .await?;
    let m = find_model(list.get("data")?, p.model.trim())?;
    let text = |k: &str| {
        m.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let n = |k: &str| m.get(k).and_then(Value::as_u64);
    let mut d = ServerDetails {
        title: "LM Studio",
        ..Default::default()
    };
    let loaded = text("state").as_deref() == Some("loaded");
    d.row("Status", if loaded { "Loaded" } else { "Not loaded" });
    if !loaded {
        d.status = Some("Not loaded".into());
    }
    for (label, key) in [
        ("Type", "type"),
        ("Architecture", "arch"),
        ("Quantization", "quantization"),
        ("Format", "compatibility_type"),
        ("Publisher", "publisher"),
    ] {
        if let Some(v) = text(key) {
            d.row(label, v);
        }
    }
    // The window the loaded model serves, which can be well below its maximum.
    d.context_size = n("loaded_context_length").filter(|_| loaded);
    if let Some(c) = d.context_size {
        d.row("Context", fmt_tokens(c));
    }
    if let Some(c) = n("max_context_length") {
        d.row("Max context", fmt_tokens(c));
    }
    Some(d)
}

// ---------------------------------------------------------------- vLLM

async fn vllm(http: &reqwest::Client, p: &ProviderConfig) -> Option<ServerDetails> {
    let base = origin(p);
    let (models, version, metrics) = tokio::join!(
        get_json(authorize(http.get(endpoint(p, "models")), p)),
        get_json(authorize(http.get(format!("{base}/version")), p)),
        get_text(authorize(http.get(format!("{base}/metrics")), p)),
    );
    let m = models
        .as_ref()
        .and_then(|v| find_model(v.get("data")?, p.model.trim()))?;
    let mut d = ServerDetails {
        title: "vLLM",
        context_size: m.get("max_model_len").and_then(Value::as_u64),
        ..Default::default()
    };
    if let Some(n) = d.context_size {
        d.row("Context", fmt_tokens(n));
    }
    if let Some(root) = m
        .get("root")
        .and_then(Value::as_str)
        .filter(|r| *r != p.model.trim())
    {
        d.row("Weights", root.to_owned());
    }
    if let Some(m) = metrics.as_deref().map(VllmMetrics::parse) {
        if let (Some(run), Some(wait)) = (m.running, m.waiting) {
            d.row("Requests", format!("{run:.0} running · {wait:.0} waiting"));
        }
        if let Some(kv) = m.kv_cache {
            d.row("KV cache in use", format!("{:.0}%", kv * 100.0));
        }
        if let Some(rate) = m.prefix_hit_rate() {
            d.row("Prefix cache hits", format!("{:.0}%", rate * 100.0));
        }
    }
    if let Some(v) = version.as_ref().and_then(|v| v.get("version")?.as_str()) {
        d.row("Version", v.to_owned());
    }
    Some(d)
}

/// A few of vLLM's Prometheus metrics, summed over their labels (engines, models).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VllmMetrics {
    pub running: Option<f64>,
    pub waiting: Option<f64>,
    /// Share of the KV cache in use, from 0 to 1.
    pub kv_cache: Option<f64>,
    pub prefix_queries: Option<f64>,
    pub prefix_hits: Option<f64>,
}

impl VllmMetrics {
    pub fn parse(text: &str) -> Self {
        let mut m = Self::default();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            // `name{labels} value`, or `name value`.
            let Some((name, value)) = line.rsplit_once(' ') else {
                continue;
            };
            let Ok(value) = value.parse::<f64>() else {
                continue;
            };
            let name = name.split('{').next().unwrap_or(name).trim();
            let slot = match name {
                "vllm:num_requests_running" => &mut m.running,
                "vllm:num_requests_waiting" => &mut m.waiting,
                // Renamed from gpu_cache_usage_perc in newer versions.
                "vllm:kv_cache_usage_perc" | "vllm:gpu_cache_usage_perc" => &mut m.kv_cache,
                "vllm:prefix_cache_queries_total" | "vllm:gpu_prefix_cache_queries_total" => {
                    &mut m.prefix_queries
                }
                "vllm:prefix_cache_hits_total" | "vllm:gpu_prefix_cache_hits_total" => {
                    &mut m.prefix_hits
                }
                _ => continue,
            };
            *slot.get_or_insert(0.0) += value;
        }
        m
    }

    /// Share of prompt tokens found in the prefix cache, once there have been queries.
    pub fn prefix_hit_rate(&self) -> Option<f64> {
        let queries = self.prefix_queries.filter(|&q| q > 0.0)?;
        Some(self.prefix_hits.unwrap_or(0.0) / queries)
    }
}

// ---------------------------------------------------------------- OpenRouter

/// The model's entry in OpenRouter's `/models` list.
async fn open_router_model(http: &reqwest::Client, p: &ProviderConfig) -> Option<Value> {
    let list = get_json(authorize(http.get(endpoint(p, "models")), p)).await?;
    find_model(list.get("data")?, p.model.trim()).cloned()
}

async fn open_router(http: &reqwest::Client, p: &ProviderConfig) -> Option<ServerDetails> {
    let (model, key) = tokio::join!(
        open_router_model(http, p),
        get_json(authorize(http.get(endpoint(p, "key")), p)),
    );
    if model.is_none() && key.is_none() {
        return None;
    }
    let mut d = ServerDetails {
        title: "OpenRouter",
        ..Default::default()
    };
    if let Some(m) = &model {
        d.context_size = m.get("context_length").and_then(Value::as_u64);
        if let Some(n) = d.context_size {
            d.row("Context", fmt_tokens(n));
        }
        if let Some(n) = m
            .pointer("/top_provider/max_completion_tokens")
            .and_then(Value::as_u64)
        {
            d.row("Max reply", fmt_tokens(n));
        }
        // Prices are strings, per token.
        let price = |k: &str| {
            m.pointer(&format!("/pricing/{k}"))
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<f64>().ok())
        };
        if let (Some(i), Some(o)) = (price("prompt"), price("completion")) {
            d.row(
                "Price per 1M",
                format!("{} in · {} out", fmt_cost(i * 1e6), fmt_cost(o * 1e6)),
            );
        }
        if let Some(inputs) = m
            .pointer("/architecture/input_modalities")
            .and_then(Value::as_array)
        {
            let inputs: Vec<&str> = inputs.iter().filter_map(Value::as_str).collect();
            if !inputs.is_empty() {
                d.row("Reads", inputs.join(", "));
            }
        }
    }
    if let Some(k) = key.as_ref().and_then(|k| k.get("data")) {
        let f = |name: &str| k.get(name).and_then(Value::as_f64);
        match (f("usage"), f("limit")) {
            (Some(used), Some(limit)) => d.row(
                "Key spend",
                format!("{} of {}", fmt_cost(used), fmt_cost(limit)),
            ),
            (Some(used), None) => d.row("Key spend", fmt_cost(used)),
            _ => {}
        }
        if let Some(left) = f("limit_remaining") {
            d.row("Key credit left", fmt_cost(left));
        }
        if k.get("is_free_tier").and_then(Value::as_bool) == Some(true) {
            d.row("Tier", "Free");
        }
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};
    use serde_json::json;

    fn provider(base: &str, kind: ProviderKind, model: &str) -> ProviderConfig {
        ProviderConfig {
            base_url: format!("{base}/v1"),
            model: model.into(),
            kind,
            ..Default::default()
        }
    }

    fn path(r: &crate::testutil::Request) -> &str {
        r.path.split_once('?').map_or(r.path.as_str(), |(p, _)| p)
    }

    #[tokio::test]
    async fn detects_each_server() {
        let http = reqwest::Client::new();
        let ollama = serve(|r| match path(r) {
            "/api/version" => Response::json(json!({"version": "0.12.3"})),
            _ => Response::status(404),
        })
        .await
        .0;
        let lm_studio = serve(|r| match path(r) {
            "/api/v0/models" => {
                Response::json(json!({"data": [{"id": "qwen", "state": "loaded"}]}))
            }
            _ => Response::status(404),
        })
        .await
        .0;
        let vllm = serve(|r| match path(r) {
            "/v1/models" => Response::json(json!({"data": [{"id": "m", "owned_by": "vllm"}]})),
            _ => Response::status(404),
        })
        .await
        .0;
        let other = serve(|r| match path(r) {
            "/v1/models" => Response::json(json!({"data": [{"id": "m", "owned_by": "openai"}]})),
            _ => Response::status(404),
        })
        .await
        .0;
        let kind = |base: &str| provider(base, ProviderKind::Generic, "m");
        assert_eq!(
            detect(&http, &kind(&ollama)).await,
            Some(ProviderKind::Ollama)
        );
        assert_eq!(
            detect(&http, &kind(&lm_studio)).await,
            Some(ProviderKind::LmStudio)
        );
        assert_eq!(detect(&http, &kind(&vllm)).await, Some(ProviderKind::Vllm));
        assert_eq!(detect(&http, &kind(&other)).await, None);
        let mut or = kind(&other);
        or.base_url = "https://openrouter.ai/api/v1".into();
        assert_eq!(detect(&http, &or).await, Some(ProviderKind::OpenRouter));
    }

    #[tokio::test]
    async fn ollama_details_and_context() {
        let (base, _) = serve(|r| match path(r) {
            "/api/version" => Response::json(json!({"version": "0.12.3"})),
            "/api/ps" => Response::json(json!({"models": [{
                "name": "qwen3:8b", "model": "qwen3:8b", "size": 6_000_000_000_u64,
                "size_vram": 4_500_000_000_u64, "context_length": 16384,
                "details": {"family": "qwen3", "parameter_size": "8.2B",
                            "quantization_level": "Q4_K_M"}}]})),
            "/api/show" => Response::json(json!({
                "parameters": "num_ctx 8192\nstop \"<|im_end|>\"",
                "model_info": {"general.architecture": "qwen3", "qwen3.context_length": 40960},
                "capabilities": ["completion", "tools", "thinking"]})),
            _ => Response::status(404),
        })
        .await;
        let http = reqwest::Client::new();
        let d = details(&http, &provider(&base, ProviderKind::Ollama, "qwen3:8b"))
            .await
            .unwrap();
        assert_eq!(d.status, None, "loaded");
        // The loaded size wins over the Modelfile's num_ctx.
        assert_eq!(d.context_size, Some(16384));
        let rows: std::collections::HashMap<_, _> = d.rows.iter().cloned().collect();
        assert_eq!(rows["Parameters"], "8.2B");
        assert_eq!(rows["Quantization"], "Q4_K_M");
        assert_eq!(rows["In GPU memory"], "75%");
        assert_eq!(rows["Trained context"], "41.0k");
        assert_eq!(rows["Capabilities"], "completion, tools, thinking");
        assert_eq!(rows["Version"], "0.12.3");

        // Not loaded: say so, and fall back to the Modelfile's num_ctx.
        let d = details(&http, &provider(&base, ProviderKind::Ollama, "gemma3:4b"))
            .await
            .unwrap();
        assert_eq!(d.status.as_deref(), Some("Not loaded"));
        assert_eq!(d.context_size, Some(8192));
        assert_eq!(num_ctx("num_keep 24\nnum_ctx    32768"), Some(32768));
        assert_eq!(num_ctx("stop x"), None);
    }

    #[tokio::test]
    async fn lm_studio_details_and_context() {
        let (base, _) = serve(|r| match path(r) {
            "/api/v0/models" => Response::json(json!({"object": "list", "data": [
                {"id": "qwen", "type": "llm", "arch": "qwen3", "quantization": "Q4_K_M",
                 "compatibility_type": "gguf", "publisher": "lmstudio-community",
                 "state": "loaded", "max_context_length": 262144,
                 "loaded_context_length": 81920},
                {"id": "gemma", "state": "not-loaded", "max_context_length": 131072}]})),
            _ => Response::status(404),
        })
        .await;
        let http = reqwest::Client::new();
        let d = details(&http, &provider(&base, ProviderKind::LmStudio, "qwen"))
            .await
            .unwrap();
        // The loaded window, not the model's maximum.
        assert_eq!(d.context_size, Some(81920));
        assert!(d.rows.contains(&("Quantization".into(), "Q4_K_M".into())));
        assert!(d.rows.contains(&("Max context".into(), "262.1k".into())));
        let d = details(&http, &provider(&base, ProviderKind::LmStudio, "gemma"))
            .await
            .unwrap();
        assert_eq!(
            (d.status.as_deref(), d.context_size),
            (Some("Not loaded"), None)
        );
    }

    #[tokio::test]
    async fn vllm_details_and_metrics() {
        let (base, _) = serve(|r| match path(r) {
            "/v1/models" => Response::json(json!({"data": [{"id": "qwen", "owned_by": "vllm",
                "root": "Qwen/Qwen3-8B", "max_model_len": 32768}]})),
            "/version" => Response::json(json!({"version": "0.11.0"})),
            "/metrics" => Response::with_type(
                200,
                "text/plain",
                "# HELP vllm:num_requests_running x\n\
                 vllm:num_requests_running{engine=\"0\",model_name=\"qwen\"} 2.0\n\
                 vllm:num_requests_waiting{engine=\"0\",model_name=\"qwen\"} 1.0\n\
                 vllm:kv_cache_usage_perc{engine=\"0\",model_name=\"qwen\"} 0.42\n\
                 vllm:prefix_cache_queries_total{engine=\"0\",model_name=\"qwen\"} 1000.0\n\
                 vllm:prefix_cache_hits_total{engine=\"0\",model_name=\"qwen\"} 250.0\n",
            ),
            _ => Response::status(404),
        })
        .await;
        let http = reqwest::Client::new();
        let p = provider(&base, ProviderKind::Vllm, "qwen");
        let d = details(&http, &p).await.unwrap();
        assert_eq!(d.context_size, Some(32768));
        assert_eq!(context_size(&http, &p).await, Some(32768));
        let rows: std::collections::HashMap<_, _> = d.rows.iter().cloned().collect();
        assert_eq!(rows["Requests"], "2 running · 1 waiting");
        assert_eq!(rows["KV cache in use"], "42%");
        assert_eq!(rows["Prefix cache hits"], "25%");
        assert_eq!(rows["Weights"], "Qwen/Qwen3-8B");
        assert_eq!(rows["Version"], "0.11.0");
    }

    #[tokio::test]
    async fn open_router_details_and_context() {
        let (base, log) = serve(|r| match path(r) {
            "/v1/models" => Response::json(json!({"data": [{"id": "anthropic/claude-x",
                "context_length": 200000, "top_provider": {"max_completion_tokens": 64000},
                "pricing": {"prompt": "0.000003", "completion": "0.000015"},
                "architecture": {"input_modalities": ["text", "image"]}}]})),
            "/v1/key" => Response::json(json!({"data": {"usage": 25.5, "limit": 100.0,
                "limit_remaining": 74.5, "is_free_tier": false}})),
            _ => Response::status(404),
        })
        .await;
        let http = reqwest::Client::new();
        let p = provider(&base, ProviderKind::OpenRouter, "anthropic/claude-x");
        let d = details(&http, &p).await.unwrap();
        let rows: std::collections::HashMap<_, _> = d.rows.iter().cloned().collect();
        assert_eq!(rows["Context"], "200.0k");
        assert_eq!(rows["Price per 1M"], "$3.00 in · $15.00 out");
        assert_eq!(rows["Reads"], "text, image");
        assert_eq!(rows["Key spend"], "$25.50 of $100.00");
        assert_eq!(rows["Key credit left"], "$74.50");
        // The context size is kept per model: the large list is asked once.
        assert_eq!(context_size(&http, &p).await, Some(200_000));
        assert_eq!(context_size(&http, &p).await, Some(200_000));
        let lists = log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| path(r) == "/v1/models")
            .count();
        assert_eq!(
            lists, 2,
            "one for the details, one for the first context lookup"
        );
    }

    #[test]
    fn vllm_metrics_older_names() {
        let m = VllmMetrics::parse(
            "vllm:gpu_cache_usage_perc{model_name=\"a\"} 0.1\n\
             vllm:gpu_cache_usage_perc{model_name=\"b\"} 0.2\n\
             vllm:gpu_prefix_cache_queries_total 0\n",
        );
        assert!(
            (m.kv_cache.unwrap() - 0.3).abs() < 1e-9,
            "summed over labels"
        );
        assert_eq!(m.prefix_hit_rate(), None, "no queries yet");
    }
}

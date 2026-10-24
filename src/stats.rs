//! Per-request performance stats: prefill/decode speed, time to first token, token counts.
//!
//! Figures come from the server when it reports them (llama.cpp `timings`, `usage`, LiteLLM
//! cost headers) and are otherwise measured client-side from stream timing.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum StatsSource {
    /// Timed by this app from the response stream (includes network latency).
    #[default]
    Measured,
    /// Reported by the inference server itself.
    Server,
}

/// Occupancy of a llama.cpp server's parallel slots (in use / total).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlotUsage {
    pub busy: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RequestStats {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    /// Completion tokens were estimated from the number of streamed chunks.
    pub tokens_estimated: bool,
    pub ttft_ms: Option<f64>,
    pub total_ms: f64,
    pub prefill_tps: Option<f64>,
    pub decode_tps: Option<f64>,
    pub source: StatsSource,
    /// The model that actually served the request, if it differs from the one requested.
    pub served_model: Option<String>,
    pub cost_usd: Option<f64>,
    /// Context window size, when the server exposes it (llama.cpp).
    pub context_size: Option<u64>,
    /// Slot occupancy, when the server exposes it (llama.cpp).
    pub slot_usage: Option<SlotUsage>,
}

/// Timestamps and counters collected while a response streams in.
pub struct Timer {
    start: Instant,
    first_token: Option<Instant>,
    last_token: Option<Instant>,
    token_chunks: u64,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            first_token: None,
            last_token: None,
            token_chunks: 0,
        }
    }

    /// Record a chunk that carried generated output (content, reasoning or tool-call text).
    pub fn token(&mut self) {
        let now = Instant::now();
        self.first_token.get_or_insert(now);
        self.last_token = Some(now);
        self.token_chunks += 1;
    }

    fn ms(a: Instant, b: Instant) -> f64 {
        b.saturating_duration_since(a).as_secs_f64() * 1000.0
    }
}

/// Token usage as reported in an OpenAI-style `usage` object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn parse(v: &Value) -> Option<Self> {
        let u = v.get("usage").filter(|u| u.is_object())?;
        Some(Self {
            prompt_tokens: u.get("prompt_tokens").and_then(Value::as_u64),
            completion_tokens: u.get("completion_tokens").and_then(Value::as_u64),
            cached_tokens: u
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_u64),
            // OpenRouter (and some proxies) report the request cost here.
            cost_usd: u.get("cost").and_then(Value::as_f64),
        })
    }
}

/// llama.cpp server's native `timings` object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerTimings {
    pub prompt_n: Option<u64>,
    pub prompt_ms: Option<f64>,
    pub prompt_per_second: Option<f64>,
    pub predicted_n: Option<u64>,
    pub predicted_ms: Option<f64>,
    pub predicted_per_second: Option<f64>,
    pub cache_n: Option<u64>,
}

impl ServerTimings {
    pub fn parse(v: &Value) -> Option<Self> {
        let t = v.get("timings").filter(|t| t.is_object())?;
        let f = |k: &str| t.get(k).and_then(Value::as_f64);
        let n = |k: &str| t.get(k).and_then(Value::as_u64);
        let timings = Self {
            prompt_n: n("prompt_n"),
            prompt_ms: f("prompt_ms"),
            prompt_per_second: f("prompt_per_second"),
            predicted_n: n("predicted_n"),
            predicted_ms: f("predicted_ms"),
            predicted_per_second: f("predicted_per_second"),
            cache_n: n("cache_n"),
        };
        (timings.prompt_per_second.is_some() || timings.predicted_per_second.is_some())
            .then_some(timings)
    }
}

/// Everything gathered during one request, turned into [`RequestStats`] at the end.
#[derive(Default)]
pub struct Collector {
    pub timer: Timer,
    pub usage: Option<Usage>,
    pub timings: Option<ServerTimings>,
    pub served_model: Option<String>,
    pub header_cost_usd: Option<f64>,
    pub context_size: Option<u64>,
}

impl Collector {
    /// Pick up `usage`, `timings` and `model` from a response chunk or body.
    pub fn observe(&mut self, v: &Value) {
        if let Some(u) = Usage::parse(v) {
            self.usage = Some(u);
        }
        if let Some(t) = ServerTimings::parse(v) {
            self.timings = Some(t);
        }
        if let Some(m) = v
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
        {
            self.served_model = Some(m.to_owned());
        }
    }

    pub fn finish(self, requested_model: &str) -> RequestStats {
        self.snapshot(requested_model)
    }

    /// A non-destructive view of the stats so far, for a live readout while the response
    /// streams in. `total_ms` and speeds are recomputed against the current instant.
    pub fn snapshot(&self, requested_model: &str) -> RequestStats {
        let now = Instant::now();
        let t = &self.timer;
        let total_ms = Timer::ms(t.start, now);
        let ttft_ms = t.first_token.map(|f| Timer::ms(t.start, f));
        let usage = self.usage.clone().unwrap_or_default();

        let mut s = RequestStats {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            cached_tokens: usage.cached_tokens,
            ttft_ms,
            total_ms,
            cost_usd: self.header_cost_usd.or(usage.cost_usd),
            context_size: self.context_size,
            served_model: self
                .served_model
                .clone()
                .filter(|m| !m.eq_ignore_ascii_case(requested_model.trim())),
            ..Default::default()
        };

        if s.completion_tokens.is_none() && t.token_chunks > 0 {
            // Most servers send roughly one token per chunk.
            s.completion_tokens = Some(t.token_chunks);
            s.tokens_estimated = true;
        }

        if let Some(st) = self.timings.as_ref() {
            s.source = StatsSource::Server;
            s.prefill_tps = st.prompt_per_second;
            s.decode_tps = st.predicted_per_second;
            if st.predicted_n.is_some() {
                s.completion_tokens = st.predicted_n;
                s.tokens_estimated = false;
            }
            // llama.cpp's prompt_n excludes tokens reused from its cache.
            if s.prompt_tokens.is_none() {
                s.prompt_tokens = match (st.prompt_n, st.cache_n) {
                    (Some(p), Some(c)) => Some(p + c),
                    (p, _) => p,
                };
            }
            if s.cached_tokens.is_none() {
                s.cached_tokens = st.cache_n.filter(|&c| c > 0);
            }
            if let Some(ms) = st.prompt_ms {
                s.ttft_ms = Some(ms);
            }
            return s;
        }

        // Measured: prefill ≈ prompt tokens over time to first token (includes network and
        // queueing), decode = tokens after the first over the time they took to arrive.
        if let (Some(p), Some(ttft)) = (s.prompt_tokens, s.ttft_ms)
            && ttft > 0.0
        {
            let uncached = p.saturating_sub(s.cached_tokens.unwrap_or(0));
            s.prefill_tps = Some(uncached as f64 / (ttft / 1000.0));
        }
        if let (Some(n), Some(first), Some(last)) =
            (s.completion_tokens, t.first_token, t.last_token)
        {
            let span = Timer::ms(first, last) / 1000.0;
            if n > 1 && span > 0.0 {
                s.decode_tps = Some((n - 1) as f64 / span);
            }
        }
        s
    }
}

/// Totals for a whole conversation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub requests: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd: Option<f64>,
    pub total_ms: f64,
    pub avg_decode_tps: Option<f64>,
}

impl Totals {
    pub fn of<'a>(stats: impl IntoIterator<Item = &'a RequestStats>) -> Self {
        let mut t = Totals::default();
        let (mut decode_sum, mut decode_n) = (0.0, 0);
        for s in stats {
            t.requests += 1;
            t.prompt_tokens += s.prompt_tokens.unwrap_or(0);
            t.completion_tokens += s.completion_tokens.unwrap_or(0);
            t.total_ms += s.total_ms;
            if let Some(c) = s.cost_usd {
                *t.cost_usd.get_or_insert(0.0) += c;
            }
            if let Some(d) = s.decode_tps {
                decode_sum += d;
                decode_n += 1;
            }
        }
        if decode_n > 0 {
            t.avg_decode_tps = Some(decode_sum / decode_n as f64);
        }
        t
    }
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        0..=9_999 => {
            let s = n.to_string();
            if s.len() > 3 {
                format!("{},{}", &s[..s.len() - 3], &s[s.len() - 3..])
            } else {
                s
            }
        }
        10_000..=999_999 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{:.2}M", n as f64 / 1_000_000.0),
    }
}

pub fn fmt_rate(tps: f64) -> String {
    if tps >= 100.0 {
        format!("{tps:.0} t/s")
    } else {
        format!("{tps:.1} t/s")
    }
}

pub fn fmt_ms(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

pub fn fmt_cost(usd: f64) -> String {
    if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn collector_with(first_ms: u64, last_ms: u64, chunks: u64) -> Collector {
        let start = Instant::now() - Duration::from_millis(last_ms + 10);
        Collector {
            timer: Timer {
                start,
                first_token: Some(start + Duration::from_millis(first_ms)),
                last_token: Some(start + Duration::from_millis(last_ms)),
                token_chunks: chunks,
            },
            ..Default::default()
        }
    }

    #[test]
    fn measured_from_usage_and_timing() {
        let mut c = collector_with(500, 2500, 50);
        c.observe(
            &json!({"model": "gpt-x", "usage": {"prompt_tokens": 1000, "completion_tokens": 101,
            "prompt_tokens_details": {"cached_tokens": 0}}}),
        );
        let s = c.finish("gpt-x");
        assert_eq!(s.source, StatsSource::Measured);
        assert_eq!(s.completion_tokens, Some(101));
        assert!(!s.tokens_estimated);
        assert!((s.prefill_tps.unwrap() - 2000.0).abs() < 1.0); // 1000 tok / 0.5 s
        assert!((s.decode_tps.unwrap() - 50.0).abs() < 0.5); // 100 tok / 2 s
        assert_eq!(
            s.served_model, None,
            "same as requested model is not repeated"
        );
    }

    #[test]
    fn estimates_tokens_without_usage() {
        let s = collector_with(100, 1100, 21).finish("m");
        assert!(s.tokens_estimated);
        assert_eq!(s.completion_tokens, Some(21));
        assert!((s.decode_tps.unwrap() - 20.0).abs() < 0.5);
        assert_eq!(s.prefill_tps, None);
    }

    #[test]
    fn prefers_llama_cpp_timings() {
        let mut c = collector_with(100, 1100, 10);
        c.observe(
            &json!({"timings": {"prompt_n": 900, "cache_n": 100, "prompt_ms": 450.0,
            "prompt_per_second": 2000.0, "predicted_n": 64, "predicted_ms": 1280.0,
            "predicted_per_second": 50.0}}),
        );
        let s = c.finish("m");
        assert_eq!(s.source, StatsSource::Server);
        assert_eq!(s.prefill_tps, Some(2000.0));
        assert_eq!(s.decode_tps, Some(50.0));
        assert_eq!(s.prompt_tokens, Some(1000));
        assert_eq!(s.cached_tokens, Some(100));
        assert_eq!(s.completion_tokens, Some(64));
        assert_eq!(s.ttft_ms, Some(450.0));
    }

    #[test]
    fn served_model_and_cost() {
        let mut c = collector_with(10, 20, 2);
        c.observe(&json!({"model": "anthropic/claude-x", "usage": {"cost": 0.0123}}));
        let s = c.finish("my-alias");
        assert_eq!(s.served_model.as_deref(), Some("anthropic/claude-x"));
        assert_eq!(s.cost_usd, Some(0.0123));
        let mut c = collector_with(10, 20, 2);
        c.header_cost_usd = Some(0.5);
        c.observe(&json!({"usage": {"cost": 0.1}}));
        assert_eq!(c.finish("m").cost_usd, Some(0.5), "LiteLLM header wins");
    }

    #[test]
    fn totals_and_formatting() {
        let a = RequestStats {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            decode_tps: Some(10.0),
            cost_usd: Some(0.01),
            total_ms: 100.0,
            ..Default::default()
        };
        let b = RequestStats {
            prompt_tokens: Some(20),
            completion_tokens: Some(5),
            decode_tps: Some(30.0),
            total_ms: 200.0,
            ..Default::default()
        };
        let t = Totals::of([&a, &b]);
        assert_eq!(
            (t.requests, t.prompt_tokens, t.completion_tokens),
            (2, 30, 10)
        );
        assert_eq!(t.avg_decode_tps, Some(20.0));
        assert_eq!(t.cost_usd, Some(0.01));
        assert_eq!(fmt_tokens(1234), "1,234");
        assert_eq!(fmt_tokens(12_345), "12.3k");
        assert_eq!(fmt_rate(45.67), "45.7 t/s");
        assert_eq!(fmt_ms(312.4), "312 ms");
        assert_eq!(fmt_ms(8400.0), "8.40 s");
    }
}

//! Reasoning and sampling options, set per provider, per model and per chat. Every option is
//! unset by default, so the server's own default applies until the user changes it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::ProviderKind;

/// How hard a reasoning model thinks. `Off` turns thinking off where the server can.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Off,
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub const ALL: [ReasoningEffort; 5] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// The value of OpenAI's `reasoning_effort` (and the Responses API's `reasoning.effort`).
    pub fn wire(self) -> &'static str {
        match self {
            Self::Off => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// A thinking budget in tokens, for APIs that take a budget rather than an effort.
    pub fn budget(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Minimal => Some(1024),
            Self::Low => Some(2048),
            Self::Medium => Some(8192),
            Self::High => Some(24576),
        }
    }
}

/// Options sent with each request; `None` (or no stop sequences) leaves the server's default.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Thinking budget in tokens (Anthropic, OpenRouter and servers that take `thinking`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

impl ModelOptions {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// These options over `base`: each one set here wins, the rest come from `base`.
    pub fn over(&self, base: &ModelOptions) -> ModelOptions {
        ModelOptions {
            reasoning_effort: self.reasoning_effort.or(base.reasoning_effort),
            thinking_budget: self.thinking_budget.or(base.thinking_budget),
            temperature: self.temperature.or(base.temperature),
            top_p: self.top_p.or(base.top_p),
            max_tokens: self.max_tokens.or(base.max_tokens),
            stop: if self.stops().is_empty() {
                base.stop.clone()
            } else {
                self.stop.clone()
            },
        }
    }

    /// Stop sequences without blank ones.
    pub fn stops(&self) -> Vec<&str> {
        self.stop
            .iter()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// The thinking budget to use: the one set, else one derived from the effort.
    pub fn budget(&self) -> Option<u32> {
        match self.reasoning_effort {
            Some(ReasoningEffort::Off) => None,
            Some(e) => self.thinking_budget.or(e.budget()),
            None => self.thinking_budget,
        }
    }

    /// Add these options to an OpenAI Chat Completions body, in the form each server takes.
    pub fn apply_chat_completions(&self, body: &mut Value, kind: ProviderKind) {
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = self.top_p {
            body["top_p"] = json!(p);
        }
        if let Some(m) = self.max_tokens {
            body["max_tokens"] = json!(m);
        }
        let stops = self.stops();
        if !stops.is_empty() {
            body["stop"] = json!(stops);
        }
        let effort = self.reasoning_effort;
        match kind {
            // OpenRouter's unified `reasoning` object takes an effort or a token budget.
            ProviderKind::OpenRouter => {
                let reasoning = match (effort, self.thinking_budget) {
                    (Some(ReasoningEffort::Off), _) => Some(json!({"enabled": false})),
                    (_, Some(b)) => Some(json!({"max_tokens": b})),
                    (Some(e), None) => Some(json!({"effort": e.wire()})),
                    (None, None) => None,
                };
                if let Some(r) = reasoning {
                    body["reasoning"] = r;
                }
            }
            // Chat templates (Qwen, DeepSeek, gpt-oss, …) switch thinking with template kwargs.
            ProviderKind::LlamaCpp | ProviderKind::Vllm => {
                if let Some(e) = effort {
                    body["chat_template_kwargs"] = if e == ReasoningEffort::Off {
                        json!({"enable_thinking": false})
                    } else {
                        json!({"enable_thinking": true, "reasoning_effort": e.wire()})
                    };
                    if kind == ProviderKind::Vllm && e != ReasoningEffort::Off {
                        body["reasoning_effort"] = json!(e.wire());
                    }
                }
            }
            _ => {
                if let Some(e) = effort {
                    body["reasoning_effort"] = json!(e.wire());
                }
                // Anthropic-style budget, which LiteLLM and Anthropic's OpenAI layer accept.
                if let Some(b) = self.thinking_budget
                    && effort != Some(ReasoningEffort::Off)
                {
                    body["thinking"] = json!({"type": "enabled", "budget_tokens": b});
                }
            }
        }
    }

    /// A short summary for the menu button, e.g. "High · T 0.7".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(e) = self.reasoning_effort {
            parts.push(e.label().to_owned());
        } else if let Some(b) = self.thinking_budget {
            parts.push(format!("{b} tok"));
        }
        if let Some(t) = self.temperature {
            parts.push(format!("T {t:.2}"));
        }
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_options_win_over_model_and_provider() {
        let provider = ModelOptions {
            temperature: Some(0.2),
            max_tokens: Some(100),
            stop: vec!["END".into()],
            ..Default::default()
        };
        let chat = ModelOptions {
            temperature: Some(0.9),
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        };
        let o = chat.over(&provider);
        assert_eq!(o.temperature, Some(0.9));
        assert_eq!(o.max_tokens, Some(100));
        assert_eq!(o.stop, ["END"]);
        assert_eq!(o.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn unset_options_send_nothing() {
        let mut body = json!({});
        ModelOptions::default().apply_chat_completions(&mut body, ProviderKind::Generic);
        assert_eq!(body, json!({}));
    }

    #[test]
    fn reasoning_in_each_server_form() {
        let o = ModelOptions {
            reasoning_effort: Some(ReasoningEffort::Low),
            top_p: Some(0.5),
            stop: vec!["x".into(), String::new()],
            ..Default::default()
        };
        let mut body = json!({});
        o.apply_chat_completions(&mut body, ProviderKind::Generic);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["top_p"], 0.5);
        assert_eq!(body["stop"], json!(["x"]));

        let mut body = json!({});
        o.apply_chat_completions(&mut body, ProviderKind::OpenRouter);
        assert_eq!(body["reasoning"], json!({"effort": "low"}));
        assert!(body.get("reasoning_effort").is_none());

        let off = ModelOptions {
            reasoning_effort: Some(ReasoningEffort::Off),
            ..Default::default()
        };
        let mut body = json!({});
        off.apply_chat_completions(&mut body, ProviderKind::LlamaCpp);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);

        let budget = ModelOptions {
            thinking_budget: Some(4000),
            ..Default::default()
        };
        let mut body = json!({});
        budget.apply_chat_completions(&mut body, ProviderKind::LiteLlm);
        assert_eq!(body["thinking"]["budget_tokens"], 4000);
        let mut body = json!({});
        budget.apply_chat_completions(&mut body, ProviderKind::OpenRouter);
        assert_eq!(body["reasoning"]["max_tokens"], 4000);
    }

    #[test]
    fn budget_follows_effort_unless_set() {
        let mut o = ModelOptions {
            reasoning_effort: Some(ReasoningEffort::Medium),
            ..Default::default()
        };
        assert_eq!(o.budget(), Some(8192));
        o.thinking_budget = Some(3000);
        assert_eq!(o.budget(), Some(3000));
        o.reasoning_effort = Some(ReasoningEffort::Off);
        assert_eq!(o.budget(), None);
    }
}

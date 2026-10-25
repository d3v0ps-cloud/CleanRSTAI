//! Rendering of per-request and per-chat speed stats.

use eframe::egui;

use crate::stats::{
    PromptProgress, RequestStats, StatsSource, Totals, context_percent, fmt_bytes, fmt_context,
    fmt_cost, fmt_cost_estimated, fmt_count, fmt_ms, fmt_rate, fmt_tokens,
};

/// From this share of the context in use, the chat's context figure turns to the warning
/// colour: the next turns may not fit.
const CONTEXT_WARN_PERCENT: f64 = 80.0;

/// The compact parts of a stats readout: speeds, time-to-first-token, token counts, context
/// use, total time and cost, in that order. Shared by the per-response line and the live readout.
fn parts(s: &RequestStats) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(p) = s.prefill_tps {
        parts.push(format!("prefill {}", fmt_rate(p)));
    }
    if let Some(d) = s.decode_tps {
        parts.push(format!("decode {}", fmt_rate(d)));
    }
    if let Some(t) = s.ttft_ms {
        let label = match s.source {
            StatsSource::Server => "prompt",
            StatsSource::Measured => "TTFT",
        };
        parts.push(format!("{label} {}", fmt_ms(t)));
    }
    match (s.prompt_tokens, s.completion_tokens) {
        (Some(p), Some(c)) => parts.push(format!(
            "{} → {}{} tok",
            fmt_tokens(p),
            if s.tokens_estimated { "~" } else { "" },
            fmt_tokens(c)
        )),
        (None, Some(c)) => parts.push(format!(
            "{}{} tok",
            if s.tokens_estimated { "~" } else { "" },
            fmt_tokens(c)
        )),
        _ => {}
    }
    if let Some(c) = s.context_use() {
        parts.push(format!("context {:.0}%", context_percent(c)));
    }
    parts.push(fmt_ms(s.total_ms));
    if let Some(c) = s.cost_usd {
        parts.push(fmt_cost_estimated(c, s.cost_estimated));
    }
    parts
}

/// One compact line under a response; the hover shows the full breakdown.
pub fn request_line(ui: &mut egui::Ui, s: &RequestStats) {
    let text = parts(s).join(" · ");
    ui.add(egui::Label::new(egui::RichText::new(text).small().weak()).sense(egui::Sense::hover()))
        .on_hover_ui(|ui| details(ui, s));
}

/// Live readout of the request currently streaming in, shown above the input box. No hover:
/// the full breakdown appears in the finished response instead. While llama.cpp is still
/// reading the prompt, a progress bar takes its place.
pub fn live_line(ui: &mut egui::Ui, s: &RequestStats) {
    if let Some(p) = &s.prompt_progress {
        prompt_progress(ui, p);
        return;
    }
    ui.horizontal(|ui| {
        ui.spinner();
        ui.add_space(4.0);
        ui.label(egui::RichText::new(parts(s).join(" · ")).small().weak());
    });
}

/// "Reading prompt" with a bar: tokens read, how many came from the cache, time so far and
/// an estimate of the time left.
pub fn prompt_progress(ui: &mut egui::Ui, p: &PromptProgress) {
    let mut parts = vec![format!(
        "Reading prompt: {} of {} tokens",
        fmt_tokens(p.processed.min(p.total)),
        fmt_tokens(p.total)
    )];
    if p.cache > 0 {
        parts.push(format!("{} from cache", fmt_tokens(p.cache)));
    }
    if p.time_ms > 0.0 {
        parts.push(fmt_ms(p.time_ms));
    }
    if let Some(left) = p.remaining_ms().filter(|&ms| ms >= 1000.0) {
        parts.push(format!("about {} left", fmt_ms(left)));
    }
    ui.horizontal(|ui| {
        let accent = super::theme::palette(ui.visuals().dark_mode).signal;
        ui.add(
            egui::ProgressBar::new(p.fraction())
                .desired_width(120.0)
                .desired_height(6.0)
                .fill(accent),
        );
        ui.add_space(4.0);
        ui.label(egui::RichText::new(parts.join(" · ")).small().weak());
    });
}

fn details(ui: &mut egui::Ui, s: &RequestStats) {
    ui.strong(match s.source {
        StatsSource::Server => "Reported by the server",
        StatsSource::Measured => "Measured by CleanRSTAI",
    });
    if s.source == StatsSource::Measured {
        ui.weak("Includes network latency; prefill is prompt tokens ÷ time to first token.");
    }
    egui::Grid::new("stats-details")
        .num_columns(2)
        .spacing([16.0, 2.0])
        .show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(k);
                ui.monospace(v);
                ui.end_row();
            };
            if let Some(p) = s.prefill_tps {
                row("Prefill speed", fmt_rate(p));
            }
            if let Some(d) = s.decode_tps {
                row("Decode speed", fmt_rate(d));
            }
            if let Some(t) = s.ttft_ms {
                row(
                    if s.source == StatsSource::Server {
                        "Prompt processing"
                    } else {
                        "Time to first token"
                    },
                    fmt_ms(t),
                );
            }
            row("Total time", fmt_ms(s.total_ms));
            if let Some(p) = s.prompt_tokens {
                row("Prompt tokens", fmt_tokens(p));
            }
            if let Some(c) = s.cached_tokens {
                row("  from cache", fmt_tokens(c));
            }
            if let Some(c) = s.completion_tokens {
                let est = if s.tokens_estimated {
                    " (estimated)"
                } else {
                    ""
                };
                row("Completion tokens", format!("{}{est}", fmt_tokens(c)));
            }
            if let Some(c) = s.context_use() {
                row("Context used", fmt_context(c));
            }
            if let Some(sl) = s.slot_usage {
                row(
                    "Server slots",
                    format!("{}/{} in use", sl.busy.min(sl.total), sl.total.max(1)),
                );
            }
            if let Some(m) = &s.served_model {
                row("Served by", m.clone());
            }
            if let Some(c) = s.cost_usd {
                if s.cost_estimated {
                    row("Cost (estimated)", fmt_cost_estimated(c, true));
                } else {
                    row("Cost", fmt_cost(c));
                }
            }
        });
    if s.cost_estimated {
        ui.weak(
            "Cost estimated from the per-token prices the LiteLLM proxy lists: it doesn't \
             report the cost of streamed replies.",
        );
    }
}

/// Summary bar for the whole conversation. `context` is how full the context was after the
/// chat's latest request, when the server reports it.
pub fn totals_bar(ui: &mut egui::Ui, totals: &Totals, context: Option<(u64, u64)>) {
    if totals.requests == 0 {
        return;
    }
    let mut parts = vec![
        format!(
            "{} request{}",
            totals.requests,
            if totals.requests == 1 { "" } else { "s" }
        ),
        format!(
            "{} in / {} out tokens",
            fmt_tokens(totals.prompt_tokens),
            fmt_tokens(totals.completion_tokens)
        ),
        format!("{} total", fmt_ms(totals.total_ms)),
    ];
    if let Some(d) = totals.avg_decode_tps {
        parts.push(format!("avg decode {}", fmt_rate(d)));
    }
    if let Some(c) = totals.cost_usd {
        parts.push(fmt_cost_estimated(c, totals.cost_estimated));
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(
            egui::RichText::new(format!("This chat: {}", parts.join(" · ")))
                .small()
                .weak(),
        );
        if let Some(c) = context {
            let text = egui::RichText::new(format!(" · context {}", fmt_context(c))).small();
            let label = if context_percent(c) >= CONTEXT_WARN_PERCENT {
                ui.label(text.color(ui.visuals().warn_fg_color))
            } else {
                ui.label(text.weak())
            };
            label.on_hover_text(
                "Tokens in the model's context after the latest reply, out of the context size \
                 the server reports. When it fills up, the server may drop older parts of the \
                 chat or refuse the request; start a new chat to free it.",
            );
        }
    });
}

/// What a llama.cpp server reports about the model it runs, for the hover beside the model
/// picker. Rows the server doesn't report are left out.
pub fn server_details(ui: &mut egui::Ui, info: &crate::llm::LlamaServerInfo) {
    use crate::llm::LlamaStatus;
    ui.strong("llama.cpp server");
    egui::Grid::new("server-details")
        .num_columns(2)
        .spacing([16.0, 2.0])
        .show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(k);
                ui.monospace(v);
                ui.end_row();
            };
            row(
                "Status",
                match info.status {
                    LlamaStatus::Ready => "Ready",
                    LlamaStatus::Loading => "Loading the model",
                    LlamaStatus::Asleep => "Asleep (reloads on the next message)",
                }
                .to_owned(),
            );
            if let Some(path) = &info.model_path {
                let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
                row("Model file", file.to_owned());
            }
            if let Some(n) = info.n_params {
                row("Parameters", fmt_count(n));
            }
            if let Some(b) = info.size_bytes {
                row("Size", fmt_bytes(b));
            }
            if let Some(n) = info.n_ctx {
                let slots = match info.total_slots {
                    Some(s) if s > 1 => format!(" per slot · {s} slots"),
                    _ => String::new(),
                };
                row("Context", format!("{}{slots}", fmt_tokens(n)));
            }
            if let Some(n) = info.n_ctx_train {
                row("Trained context", fmt_tokens(n));
            }
            if let Some(v) = info.vision {
                row("Images", if v { "Yes" } else { "No" }.to_owned());
            }
            if let Some(m) = &info.metrics {
                if let (Some(busy), Some(queued)) = (m.requests_processing, m.requests_deferred) {
                    row(
                        "Requests",
                        format!("{busy:.0} processing · {queued:.0} queued"),
                    );
                }
                if let Some(a) = m.draft_acceptance() {
                    row("Draft acceptance", format!("{:.0}%", a * 100.0));
                }
            }
            if let Some(b) = &info.build {
                row("Build", b.clone());
            }
        });
}

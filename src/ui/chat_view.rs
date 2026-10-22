//! Rendering of the conversation: messages, tool-call cards, errors and the empty state.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use uuid::Uuid;

use super::attachments::{self as attach_ui, ImageCache};
use super::theme::{self, icon};
use super::widgets::{code_block, icon_button};
use crate::app::{Action, Run};
use crate::mcp::manager::McpManager;
use crate::types::{ChatMessage, Conversation, Role, ToolCall};

pub const MAX_CONTENT_WIDTH: f32 = 820.0;

/// Lay out `add` in a centred column of at most [`MAX_CONTENT_WIDTH`].
pub fn centered<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let avail = ui.available_width();
    let width = avail.min(MAX_CONTENT_WIDTH);
    let pad = ((avail - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width - 16.0);
            add(ui)
        })
        .inner
    })
    .inner
}

pub struct View<'a> {
    pub conv: &'a Conversation,
    pub run: Option<&'a Run>,
    pub error: Option<&'a String>,
    pub model: &'a str,
    pub show_stats: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    images: &mut ImageCache,
    v: View<'_>,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let conv = v.conv;
    let results: HashMap<&str, &ChatMessage> = conv
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
        .collect();
    let last_assistant = conv
        .messages
        .iter()
        .rposition(|m| m.role == Role::Assistant);

    if v.show_stats {
        let totals =
            crate::stats::Totals::of(conv.messages.iter().filter_map(|m| m.stats.as_ref()));
        if totals.requests > 0 {
            centered(ui, |ui| super::stats_view::totals_bar(ui, &totals));
        }
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.add_space(12.0);
            centered(ui, |ui| {
                for (i, msg) in conv.messages.iter().enumerate() {
                    match msg.role {
                        Role::Tool | Role::System => continue,
                        Role::User => {
                            user_message(ui, images, conv.id, i, msg, v.run.is_some(), &mut actions)
                        }
                        Role::Assistant => {
                            let streaming = v.run.is_some() && i + 1 == conv.messages.len();
                            // Consecutive assistant turns (tool rounds) share one header.
                            let first_of_turn = i == 0
                                || conv.messages[..i]
                                    .iter()
                                    .rev()
                                    .find(|m| m.role != Role::Tool)
                                    .is_none_or(|m| m.role != Role::Assistant);
                            assistant_message(
                                ui,
                                md_cache,
                                images,
                                AssistantCtx {
                                    conv: conv.id,
                                    idx: i,
                                    results: &results,
                                    run: v.run,
                                    streaming,
                                    show_stats: v.show_stats,
                                    header: first_of_turn.then_some(v.model),
                                    last: Some(i) == last_assistant && v.run.is_none(),
                                },
                                msg,
                                &mut actions,
                            );
                        }
                    }
                    ui.add_space(14.0);
                }
                if let Some(err) = v.error {
                    error_box(ui, err, v.run.is_none().then_some(conv.id), &mut actions);
                }
                ui.add_space(16.0);
            });
        });
    actions
}

fn user_message(
    ui: &mut egui::Ui,
    images: &mut ImageCache,
    conv: Uuid,
    idx: usize,
    msg: &ChatMessage,
    running: bool,
    actions: &mut Vec<Action>,
) {
    let dark = ui.visuals().dark_mode;
    let max_w = ui.available_width() * 0.85;
    // Size the bubble to its content: the longest text line, or room for a thumbnail.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text_w = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(msg.content.clone(), font, egui::Color32::WHITE)
            .size()
            .x
    });
    let mut width = text_w.ceil() + 2.0;
    if msg.attachments.iter().any(|a| a.is_image()) {
        width = width.max(320.0);
    } else if !msg.attachments.is_empty() {
        width = width.max(240.0);
    }
    let width = width.min(max_w);
    ui.horizontal(|ui| {
        // Right-align by spacing; the frame then gets exactly the bubble's width.
        let margin = 14.0;
        ui.add_space((ui.available_width() - width - 2.0 * margin).max(0.0));
        egui::Frame::new()
            .fill(theme::user_bubble(dark))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(margin as i8, 10))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(width);
                    if !msg.content.is_empty() {
                        ui.add(egui::Label::new(&msg.content).selectable(true).wrap());
                    }
                    if !msg.attachments.is_empty()
                        && let Some((uri, bytes, name)) =
                            attach_ui::show(ui, images, &format!("{conv}-{idx}"), &msg.attachments)
                    {
                        actions.push(Action::ViewImage(uri, bytes, name));
                    }
                });
            });
    });
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if !running && icon_button(ui, icon::EDIT, "Edit and resend").clicked() {
            actions.push(Action::EditFrom(conv, idx));
        }
        if icon_button(ui, icon::COPY, "Copy message").clicked() {
            actions.push(Action::Copy(msg.content.clone()));
        }
    });
}

struct AssistantCtx<'a> {
    conv: Uuid,
    idx: usize,
    results: &'a HashMap<&'a str, &'a ChatMessage>,
    run: Option<&'a Run>,
    streaming: bool,
    show_stats: bool,
    /// Show the header row with this model name.
    header: Option<&'a str>,
    /// The final reply: show Regenerate.
    last: bool,
}

fn assistant_message(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    images: &mut ImageCache,
    cx: AssistantCtx<'_>,
    msg: &ChatMessage,
    actions: &mut Vec<Action>,
) {
    let dark = ui.visuals().dark_mode;
    if let Some(model) = cx.header {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icon::SPARKLE)
                    .color(theme::accent(dark))
                    .size(16.0),
            );
            ui.label(RichText::new("Assistant").strong());
            if !model.is_empty() {
                ui.label(RichText::new(model).small().weak());
            }
        });
    }
    let waiting = cx.streaming && msg.content.is_empty() && msg.tool_calls.is_empty();
    if let Some(r) = msg.reasoning.as_deref().filter(|r| !r.trim().is_empty()) {
        egui::CollapsingHeader::new(RichText::new(format!("{}  Thinking", icon::BRAIN)).weak())
            .id_salt(("reasoning", cx.conv, cx.idx))
            .default_open(cx.streaming && msg.content.is_empty())
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(RichText::new(r).weak().italics())
                        .selectable(true)
                        .wrap(),
                );
            });
    } else if waiting {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Thinking…");
        });
    }
    if !msg.content.is_empty() {
        CommonMarkViewer::new().show(ui, md_cache, &msg.content);
    }
    for call in &msg.tool_calls {
        tool_card(
            ui,
            images,
            cx.conv,
            call,
            cx.results.get(call.id.as_str()).copied(),
            cx.run,
            actions,
        );
    }
    if !cx.streaming && (!msg.content.is_empty() || msg.stats.is_some()) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if !msg.content.is_empty() && icon_button(ui, icon::COPY, "Copy reply").clicked() {
                actions.push(Action::Copy(msg.content.clone()));
            }
            if cx.last && icon_button(ui, icon::REGENERATE, "Regenerate reply").clicked() {
                actions.push(Action::Regenerate(cx.conv));
            }
            if cx.show_stats
                && let Some(s) = &msg.stats
            {
                super::stats_view::request_line(ui, s);
            }
        });
    }
}

/// One-line summary of a tool call's arguments.
fn args_preview(arguments: &str) -> String {
    let compact = serde_json::from_str::<serde_json::Value>(arguments)
        .map(|v| match v {
            serde_json::Value::Object(map) if map.is_empty() => String::new(),
            serde_json::Value::Object(map) => map
                .iter()
                .map(|(k, v)| match v {
                    serde_json::Value::String(s) => format!("{k}: {s}"),
                    other => format!("{k}: {other}"),
                })
                .collect::<Vec<_>>()
                .join(", "),
            other => other.to_string(),
        })
        .unwrap_or_else(|_| arguments.to_owned());
    let one_line = compact.replace('\n', " ");
    if one_line.chars().count() > 70 {
        format!("{}…", one_line.chars().take(70).collect::<String>())
    } else {
        one_line
    }
}

fn tool_card(
    ui: &mut egui::Ui,
    images: &mut ImageCache,
    conv: Uuid,
    call: &ToolCall,
    result: Option<&ChatMessage>,
    run: Option<&Run>,
    actions: &mut Vec<Action>,
) {
    let dark = ui.visuals().dark_mode;
    let pending_approval = run.is_some_and(|r| r.approvals.iter().any(|(c, _)| c.id == call.id));
    let running = result.is_none()
        && run.is_some_and(|r| r.running_tool.as_deref() == Some(call.id.as_str()));
    let pretty_args = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| call.arguments.clone());

    ui.add_space(2.0);
    egui::Frame::new()
        .fill(theme::card(dark))
        .stroke(egui::Stroke::new(1.0, theme::border(dark)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let id = ui.make_persistent_id(("tool", &call.id));
            let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                false,
            );
            state
                .show_header(ui, |ui| {
                    if running {
                        ui.spinner();
                    } else {
                        let (glyph, color, label) = match result {
                            Some(r) if r.is_error => {
                                (icon::FAILED, ui.visuals().error_fg_color, "Failed")
                            }
                            Some(_) => (icon::OK, theme::success(dark), "Succeeded"),
                            None if pending_approval => {
                                (icon::WARNING, ui.visuals().warn_fg_color, "Needs approval")
                            }
                            None => (icon::TOOL, ui.visuals().weak_text_color(), "Not run"),
                        };
                        ui.label(RichText::new(glyph).color(color).size(15.0))
                            .on_hover_text(label);
                    }
                    ui.label(RichText::new(McpManager::display_name(&call.name)).strong());
                    let preview = args_preview(&call.arguments);
                    if !preview.is_empty() {
                        ui.add(egui::Label::new(RichText::new(preview).small().weak()).truncate());
                    }
                })
                .body(|ui| {
                    ui.label(RichText::new("Arguments").small().weak());
                    code_block(ui, &pretty_args);
                    if let Some(r) = result {
                        ui.label(
                            RichText::new(if r.is_error {
                                "Result (error)"
                            } else {
                                "Result"
                            })
                            .small()
                            .weak(),
                        );
                        const LIMIT: usize = 20_000;
                        if r.content.len() > LIMIT {
                            let mut end = LIMIT;
                            while !r.content.is_char_boundary(end) {
                                end -= 1;
                            }
                            code_block(
                                ui,
                                &format!(
                                    "{}\n… ({} more bytes)",
                                    &r.content[..end],
                                    r.content.len() - end
                                ),
                            );
                        } else {
                            code_block(ui, &r.content);
                        }
                    }
                });
            // Images and the approval prompt stay visible without expanding.
            if let Some(r) = result
                && !r.attachments.is_empty()
                && let Some((uri, bytes, name)) =
                    attach_ui::show(ui, images, &format!("tool-{}", call.id), &r.attachments)
            {
                actions.push(Action::ViewImage(uri, bytes, name));
            }
            if pending_approval {
                if !state_is_open(ui, id) {
                    code_block(ui, &pretty_args);
                }
                ui.horizontal(|ui| {
                    ui.label("Allow this tool call?");
                    if ui
                        .add(theme::primary_button(format!("{}  Run", icon::OK), dark))
                        .clicked()
                    {
                        actions.push(Action::Approve(conv, call.id.clone(), true));
                    }
                    if ui.button(format!("{}  Deny", icon::CLOSE)).clicked() {
                        actions.push(Action::Approve(conv, call.id.clone(), false));
                    }
                });
            }
        });
}

fn state_is_open(ui: &egui::Ui, id: egui::Id) -> bool {
    egui::collapsing_header::CollapsingState::load(ui.ctx(), id).is_some_and(|s| s.is_open())
}

fn error_box(ui: &mut egui::Ui, err: &str, retry: Option<Uuid>, actions: &mut Vec<Action>) {
    let color = ui.visuals().error_fg_color;
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icon::WARNING).color(color).size(16.0));
                ui.add(egui::Label::new(RichText::new(err).color(color)).wrap());
            });
            if let Some(id) = retry
                && ui.button(format!("{}  Retry", icon::REGENERATE)).clicked()
            {
                actions.push(Action::Regenerate(id));
            }
        });
}

/// Starter prompts shown in an empty chat.
const SUGGESTIONS: &[(&str, &str, &str)] = &[
    (icon::LIGHTNING, "Explain a concept", "Explain how "),
    (
        icon::FILE,
        "Summarise a document",
        "Summarise the attached document in five bullet points.",
    ),
    (
        icon::TOOL,
        "What tools do you have?",
        "What tools do you have access to, and what can you do with them?",
    ),
];

pub struct EmptyInfo<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub mcp_servers: usize,
    pub mcp_tools: usize,
    pub missing_key: bool,
}

pub fn empty_state(ui: &mut egui::Ui, info: EmptyInfo<'_>) -> Vec<Action> {
    let mut actions = Vec::new();
    let dark = ui.visuals().dark_mode;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.22).max(24.0));
        ui.label(
            RichText::new(icon::SPARKLE)
                .size(40.0)
                .color(theme::accent(dark)),
        );
        ui.add_space(6.0);
        ui.label(RichText::new("What can I help with?").size(24.0).strong());
        ui.add_space(4.0);
        let model = if info.model.is_empty() {
            "no model selected"
        } else {
            info.model
        };
        ui.weak(format!("{} · {model}", info.provider));
        if info.mcp_servers > 0 {
            ui.weak(format!(
                "{} {} tools from {} MCP server{}",
                icon::PLUGS,
                info.mcp_tools,
                info.mcp_servers,
                if info.mcp_servers == 1 { "" } else { "s" }
            ));
        }
        if info.missing_key {
            ui.add_space(8.0);
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "{}  No API key set: open Settings (⌘,) to configure your provider.",
                    icon::KEY
                ),
            );
        }
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            let total: f32 = 3.0 * 210.0 + 2.0 * ui.spacing().item_spacing.x;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            for (ic, title, text) in SUGGESTIONS {
                let resp = egui::Frame::new()
                    .fill(theme::card(dark))
                    .stroke(egui::Stroke::new(1.0, theme::border(dark)))
                    .corner_radius(12)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_width(186.0);
                        ui.label(RichText::new(*ic).size(18.0).color(theme::accent(dark)));
                        ui.label(RichText::new(*title).strong());
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    actions.push(Action::SetInput((*text).to_owned()));
                }
            }
        });
    });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_tool_arguments() {
        assert_eq!(
            args_preview(r#"{"path": "/tmp/a", "n": 3}"#),
            "n: 3, path: /tmp/a"
        );
        assert_eq!(args_preview("{}"), "");
        assert!(args_preview(&format!(r#"{{"q": "{}"}}"#, "x".repeat(200))).ends_with('…'));
        assert_eq!(args_preview("not json"), "not json");
    }
}

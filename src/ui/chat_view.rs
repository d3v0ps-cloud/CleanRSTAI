//! Rendering of the conversation: messages, tool-call cards, errors and the empty state.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use egui_commonmark::CommonMarkCache;
use uuid::Uuid;

use super::attachments::{self as attach_ui, ImageCache};
use super::markdown::{self, DrawingCache};
use super::theme::{self, icon};
use super::widgets::{code_block, icon_button};
use crate::app::{Action, Run};
use crate::approval::{Approval, PendingApproval};
use crate::config::ReasoningDisplay;
use crate::subagent::{SubagentReport, SubagentState};
use crate::tools::{self, Task, TaskStatus};
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
    pub reasoning: ReasoningDisplay,
    pub remote_images: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
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
    // Only the newest task list is shown in full; earlier updates collapse to a line.
    let latest_tasks = conv
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .rfind(|c| c.name == tools::TASKS)
        .map(|c| c.id.as_str());

    if v.show_stats {
        // Subagents' requests count towards the chat's totals too.
        let totals = crate::stats::Totals::of(conv.messages.iter().flat_map(|m| {
            m.stats
                .iter()
                .chain(m.subagents.iter().flat_map(|s| s.stats.iter()))
        }));
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
                                drawings,
                                images,
                                AssistantCtx {
                                    conv: conv.id,
                                    idx: i,
                                    results: &results,
                                    latest_tasks,
                                    run: v.run,
                                    streaming,
                                    show_stats: v.show_stats,
                                    reasoning: v.reasoning,
                                    remote_images: v.remote_images,
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
    latest_tasks: Option<&'a str>,
    run: Option<&'a Run>,
    streaming: bool,
    show_stats: bool,
    reasoning: ReasoningDisplay,
    remote_images: bool,
    /// Show the header row with this model name.
    header: Option<&'a str>,
    /// The final reply: show Regenerate.
    last: bool,
}

fn assistant_message(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
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
        let id = ui.make_persistent_id(("reasoning", cx.conv, cx.idx));
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            reasoning_open(cx.reasoning, cx.streaming, !msg.content.is_empty()),
        );
        if cx.reasoning == ReasoningDisplay::WhileStreaming && cx.streaming {
            // Follow the stream: open while thinking, fold once the answer starts.
            state.set_open(msg.content.is_empty());
        }
        state
            .show_header(ui, |ui| {
                let label = if cx.streaming && msg.content.is_empty() {
                    "Thinking…"
                } else {
                    "Thinking"
                };
                ui.label(RichText::new(format!("{}  {label}", icon::BRAIN)).weak());
            })
            .body(|ui| {
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
        let id = ui.make_persistent_id(("reply", cx.conv, cx.idx));
        markdown::show(
            ui,
            md_cache,
            drawings,
            id,
            &msg.content,
            cx.remote_images,
            actions,
        );
    }
    for call in &msg.tool_calls {
        let result = cx.results.get(call.id.as_str()).copied();
        if call.name == tools::TASKS
            && let Ok(list) = tools::parse_tasks(&call.arguments)
        {
            task_list(ui, call, &list, cx.latest_tasks == Some(call.id.as_str()));
            continue;
        }
        tool_card(ui, images, cx.conv, call, result, cx.run, actions);
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

/// Whether a reasoning section starts open, per the user's setting.
fn reasoning_open(mode: ReasoningDisplay, streaming: bool, has_answer: bool) -> bool {
    match mode {
        ReasoningDisplay::Collapsed => false,
        ReasoningDisplay::Expanded => true,
        ReasoningDisplay::WhileStreaming => streaming && !has_answer,
    }
}

/// "3 tasks" for a `delegate` call.
fn delegate_preview(arguments: &str) -> String {
    let n = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v["tasks"].as_array().map(Vec::len))
        .unwrap_or(0);
    match n {
        0 => String::new(),
        1 => "1 task".into(),
        n => format!("{n} tasks"),
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
    let pending = run.and_then(|r| r.approvals.iter().find(|p| p.call.id == call.id));
    let pending_approval = pending.is_some();
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
                    ui.label(RichText::new(tools::display_name(&call.name)).strong());
                    let preview = if call.name == tools::DELEGATE {
                        delegate_preview(&call.arguments)
                    } else {
                        args_preview(&call.arguments)
                    };
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
            if call.name == tools::DELEGATE {
                let live = run.and_then(|r| r.subagents.get(&call.id));
                let reports = match result {
                    Some(r) if !r.subagents.is_empty() => Some(&r.subagents),
                    _ => live,
                };
                if let Some(reports) = reports {
                    subagent_list(ui, &call.id, reports);
                }
            }
            if let Some(pending) = pending {
                if !state_is_open(ui, id) {
                    code_block(ui, &pretty_args);
                }
                approval_prompt(ui, conv, pending, actions);
            }
        });
}

/// "Allow this tool call?" with Run, the two "Always allow" choices, and Deny.
fn approval_prompt(
    ui: &mut egui::Ui,
    conv: Uuid,
    pending: &PendingApproval,
    actions: &mut Vec<Action>,
) {
    let dark = ui.visuals().dark_mode;
    let mut answer = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("Allow this tool call?");
        if ui
            .add(theme::primary_button(format!("{}  Run", icon::OK), dark))
            .clicked()
        {
            answer = Some(Approval::Once);
        }
        if ui
            .button("Always allow this tool")
            .on_hover_text("Run this tool without asking for the rest of this chat")
            .clicked()
        {
            answer = Some(Approval::AlwaysTool);
        }
        if pending.origin.is_some() {
            let server = match &pending.server_name {
                Some(name) => format!("the {name} MCP server"),
                None => "this MCP server".into(),
            };
            if ui
                .button("Always allow this server")
                .on_hover_text(format!(
                    "Run every tool from {server} without asking for the rest of this chat"
                ))
                .clicked()
            {
                answer = Some(Approval::AlwaysServer);
            }
        }
        if ui.button(format!("{}  Deny", icon::CLOSE)).clicked() {
            answer = Some(Approval::Deny);
        }
    });
    if let Some(answer) = answer {
        actions.push(Action::Approve(conv, pending.call.id.clone(), answer));
    }
}

/// A task list from the built-in task tool, as a checklist. Superseded lists show as one line.
fn task_list(ui: &mut egui::Ui, call: &ToolCall, list: &[Task], latest: bool) {
    let dark = ui.visuals().dark_mode;
    let done = list.iter().filter(|t| t.status == TaskStatus::Done).count();
    ui.add_space(2.0);
    egui::Frame::new()
        .fill(theme::card(dark))
        .stroke(egui::Stroke::new(1.0, theme::border(dark)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Keyed on `latest` so a list folds once a newer one replaces it.
            let id = ui.make_persistent_id(("tasks", &call.id, latest));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, latest)
                .show_header(ui, |ui| {
                    ui.label(
                        RichText::new(icon::LIST_CHECKS)
                            .color(theme::accent(dark))
                            .size(15.0),
                    );
                    ui.label(RichText::new("Tasks").strong());
                    ui.label(
                        RichText::new(format!("{done} of {} done", list.len()))
                            .small()
                            .weak(),
                    );
                })
                .body(|ui| {
                    for task in list {
                        ui.horizontal(|ui| {
                            let (glyph, color) = match task.status {
                                TaskStatus::Done => (icon::CHECK_SQUARE, theme::success(dark)),
                                TaskStatus::InProgress => (icon::ARROW_RIGHT, theme::accent(dark)),
                                TaskStatus::Pending => {
                                    (icon::SQUARE, ui.visuals().weak_text_color())
                                }
                            };
                            ui.label(RichText::new(glyph).color(color).size(15.0));
                            let text = RichText::new(task.title.trim());
                            let text = match task.status {
                                TaskStatus::Done => text.weak().strikethrough(),
                                TaskStatus::InProgress => text.strong(),
                                TaskStatus::Pending => text,
                            };
                            ui.add(egui::Label::new(text).wrap());
                        });
                    }
                });
        });
}

/// Progress and results of the subagents a `delegate` call started.
fn subagent_list(ui: &mut egui::Ui, call_id: &str, reports: &[SubagentReport]) {
    let dark = ui.visuals().dark_mode;
    ui.add_space(4.0);
    for (i, r) in reports.iter().enumerate() {
        let id = ui.make_persistent_id(("subagent", call_id, i));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| {
                match r.state {
                    SubagentState::Running => {
                        ui.spinner();
                    }
                    SubagentState::Queued => {
                        ui.label(
                            RichText::new(icon::HOURGLASS).color(ui.visuals().weak_text_color()),
                        )
                        .on_hover_text("Waiting for a free slot");
                    }
                    SubagentState::Done => {
                        ui.label(RichText::new(icon::OK).color(theme::success(dark)))
                            .on_hover_text("Done");
                    }
                    SubagentState::Failed => {
                        ui.label(RichText::new(icon::FAILED).color(ui.visuals().error_fg_color))
                            .on_hover_text("Failed");
                    }
                }
                ui.label(RichText::new(format!("Subagent {}", i + 1)).strong());
                let detail = if r.state == SubagentState::Running && !r.activity.is_empty() {
                    r.activity.clone()
                } else {
                    r.task.replace('\n', " ")
                };
                ui.add(egui::Label::new(RichText::new(detail).small().weak()).truncate());
            })
            .body(|ui| {
                ui.label(RichText::new("Task").small().weak());
                ui.add(egui::Label::new(&r.task).selectable(true).wrap());
                if !r.provider.is_empty() {
                    ui.label(
                        RichText::new(format!("{}  {} · {}", icon::SERVER, r.provider, r.model))
                            .small()
                            .weak(),
                    );
                }
                if !r.tools_used.is_empty() {
                    ui.label(
                        RichText::new(format!("{}  {}", icon::TOOL, r.tools_used.join(", ")))
                            .small()
                            .weak(),
                    );
                }
                if !r.result.is_empty() {
                    ui.label(
                        RichText::new(if r.state == SubagentState::Failed {
                            "Error"
                        } else {
                            "Result"
                        })
                        .small()
                        .weak(),
                    );
                    egui::Frame::new()
                        .fill(ui.visuals().extreme_bg_color)
                        .corner_radius(6)
                        .inner_margin(8)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(r.result.trim()).selectable(true).wrap());
                        });
                }
                if !r.stats.is_empty() {
                    let t = crate::stats::Totals::of(&r.stats);
                    ui.label(
                        RichText::new(format!(
                            "{} request{} · {} in · {} out · {}",
                            t.requests,
                            if t.requests == 1 { "" } else { "s" },
                            crate::stats::fmt_tokens(t.prompt_tokens),
                            crate::stats::fmt_tokens(t.completion_tokens),
                            crate::stats::fmt_ms(t.total_ms),
                        ))
                        .small()
                        .weak(),
                    );
                }
            });
    }
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
    fn reasoning_default_state() {
        use ReasoningDisplay::*;
        assert!(!reasoning_open(Collapsed, true, false));
        assert!(reasoning_open(Expanded, false, true));
        assert!(reasoning_open(WhileStreaming, true, false));
        assert!(!reasoning_open(WhileStreaming, true, true));
        assert!(!reasoning_open(WhileStreaming, false, false));
    }

    #[test]
    fn previews_tool_arguments() {
        assert_eq!(
            args_preview(r#"{"path": "/tmp/a", "n": 3}"#),
            "n: 3, path: /tmp/a"
        );
        assert_eq!(args_preview("{}"), "");
        assert!(args_preview(&format!(r#"{{"q": "{}"}}"#, "x".repeat(200))).ends_with('…'));
        assert_eq!(args_preview("not json"), "not json");
        assert_eq!(
            delegate_preview(r#"{"tasks": [{"task": "a"}, {"task": "b"}]}"#),
            "2 tasks"
        );
        assert_eq!(delegate_preview("{}"), "");
    }
}

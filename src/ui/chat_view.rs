//! Rendering of the conversation: messages, tool-call cards, errors and the empty state.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use egui_commonmark::CommonMarkCache;
use uuid::Uuid;

use super::attachments::{self as attach_ui, ImageCache};
use super::markdown::{self, DrawingCache};
use super::theme::{self, icon};
use super::widgets::{code_block, text_actions};
use crate::app::{Action, Run};
use crate::approval::{Approval, PendingApproval};
use crate::config::ReasoningDisplay;
use crate::subagent::{SubagentReport, SubagentState};
use crate::tools::{self, Task, TaskStatus};
use crate::types::{ChatMessage, Conversation, Role, ToolCall};

pub const MAX_CONTENT_WIDTH: f32 = 880.0;
/// Width of the transcript's left margin, which names who is speaking.
const MARGIN: f32 = 84.0;

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

/// A transcript row: `label` in the left margin, `add` in the text column beside it.
fn margin_row<R>(
    ui: &mut egui::Ui,
    label: Option<egui::WidgetText>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let top = ui.cursor().min;
    let width = ui.available_width();
    let column = egui::Rect::from_min_max(
        egui::pos2(top.x + MARGIN, top.y),
        egui::pos2(top.x + width, ui.max_rect().bottom().max(top.y + 1.0)),
    );
    let inner = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(column)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_width(column.width());
                add(ui)
            },
        )
        .inner;
    if let Some(label) = label {
        // A child Ui, so the label doesn't move the parent's cursor.
        let rect =
            egui::Rect::from_min_size(top + egui::vec2(0.0, 3.0), egui::vec2(MARGIN - 14.0, 18.0));
        ui.new_child(egui::UiBuilder::new().max_rect(rect))
            .add(egui::Label::new(label).truncate());
    }
    inner
}

/// A log entry inside a reply (tool call, task list, error): content indented past a
/// vertical hairline in `color`, no box around it.
fn entry<R>(ui: &mut egui::Ui, color: egui::Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 0,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    let r = inner.response.rect;
    ui.painter()
        .vline(r.left() + 1.0, r.y_range(), egui::Stroke::new(2.0, color));
    inner.inner
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
                let mut first = true;
                for (i, msg) in conv.messages.iter().enumerate() {
                    match msg.role {
                        Role::Tool | Role::System => continue,
                        Role::User => {
                            // A hairline opens each exchange, like a new entry in a log.
                            if !first {
                                ui.add_space(6.0);
                                theme::rule(ui);
                                ui.add_space(14.0);
                            }
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
                                    results: &results_after(&conv.messages, i),
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
                    first = false;
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
    let p = theme::palette(ui.visuals().dark_mode);
    margin_row(ui, Some(theme::caps("you").color(p.pencil).into()), |ui| {
        // What you typed is set in mono, what the model wrote in serif.
        if !msg.content.is_empty() {
            ui.add(
                egui::Label::new(RichText::new(&msg.content).monospace().color(p.ink))
                    .selectable(true)
                    .wrap(),
            );
        }
        if !msg.attachments.is_empty()
            && let Some((uri, bytes, name)) =
                attach_ui::show(ui, images, &format!("{conv}-{idx}"), &msg.attachments)
        {
            actions.push(Action::ViewImage(uri, bytes, name));
        }
        let labels: &[&str] = if running {
            &["copy"]
        } else {
            &["copy", "edit"]
        };
        match text_actions(ui, labels) {
            Some(0) => actions.push(Action::Copy(msg.content.clone())),
            Some(1) => actions.push(Action::EditFrom(conv, idx)),
            _ => {}
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

/// The tool results answering the calls of the assistant message at `idx`: the tool
/// messages right after it. (Matching by id across the whole chat would mix up turns from
/// servers that reuse ids like `call_0`.)
fn results_after(messages: &[ChatMessage], idx: usize) -> HashMap<&str, &ChatMessage> {
    messages[idx + 1..]
        .iter()
        .take_while(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
        .collect()
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
    let p = theme::palette(ui.visuals().dark_mode);
    // The margin names the model, without its vendor prefix ("org/model" → "model").
    let label = cx.header.map(|model| {
        let short = model.rsplit('/').next().unwrap_or(model);
        let short = if short.is_empty() { "model" } else { short };
        theme::caps(short).color(p.signal).into()
    });
    // Scoped by position: widget ids below use call ids, which servers may reuse.
    ui.push_id(("message", cx.idx), |ui| {
        margin_row(ui, label, |ui| {
            assistant_body(ui, md_cache, drawings, images, &cx, msg, actions)
        });
    });
}

fn assistant_body(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
    images: &mut ImageCache,
    cx: &AssistantCtx<'_>,
    msg: &ChatMessage,
    actions: &mut Vec<Action>,
) {
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
                    "thinking…"
                } else {
                    "thinking"
                };
                ui.label(RichText::new(label).small().weak());
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
            ui.label(RichText::new("waiting for the first token").small().weak());
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
            let mut labels = Vec::new();
            if !msg.content.is_empty() {
                labels.push("copy");
            }
            if cx.last {
                labels.push("regenerate");
            }
            match text_actions(ui, &labels).map(|i| labels[i]) {
                Some("copy") => actions.push(Action::Copy(msg.content.clone())),
                Some("regenerate") => actions.push(Action::Regenerate(cx.conv)),
                _ => {}
            }
            if cx.show_stats
                && let Some(s) = &msg.stats
            {
                ui.add_space(6.0);
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
    let pending = run
        .filter(|_| result.is_none())
        .and_then(|r| r.approvals.iter().find(|p| p.call.id == call.id));
    let pending_approval = pending.is_some();
    let running = result.is_none()
        && run.is_some_and(|r| r.running_tool.as_deref() == Some(call.id.as_str()));
    let pretty_args = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| call.arguments.clone());

    let p = theme::palette(dark);
    let (status, color) = match result {
        Some(r) if r.is_error => ("failed", p.error),
        Some(_) => ("ok", p.ok),
        None if pending_approval => ("approve?", p.signal),
        None if running => ("running", p.pencil),
        None => ("not run", p.pencil),
    };
    ui.add_space(2.0);
    entry(ui, if result.is_some() { p.rule } else { color }, |ui| {
        let id = ui.make_persistent_id(("tool", &call.id));
        let state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
        state
            .show_header(ui, |ui| {
                if running {
                    ui.spinner();
                }
                ui.label(RichText::new(tools::display_name(&call.name)).monospace());
                ui.label(RichText::new(status).small().color(color));
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
                ui.label(theme::caps("arguments").color(p.pencil));
                code_block(ui, &pretty_args);
                if let Some(r) = result {
                    ui.label(
                        theme::caps(if r.is_error { "error" } else { "result" })
                            .color(if r.is_error { p.error } else { p.pencil }),
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
        ui.label(RichText::new("Run this tool?").small());
        if ui.add(theme::primary_button("Run", dark)).clicked() {
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
        if ui.button("Deny").clicked() {
            answer = Some(Approval::Deny);
        }
    });
    if let Some(answer) = answer {
        actions.push(Action::Approve(conv, pending.call.id.clone(), answer));
    }
}

/// A task list from the built-in task tool, as a checklist. Superseded lists show as one line.
fn task_list(ui: &mut egui::Ui, call: &ToolCall, list: &[Task], latest: bool) {
    let p = theme::palette(ui.visuals().dark_mode);
    let done = list.iter().filter(|t| t.status == TaskStatus::Done).count();
    ui.add_space(2.0);
    entry(ui, if latest { p.signal } else { p.rule }, |ui| {
        // Keyed on `latest` so a list folds once a newer one replaces it.
        let id = ui.make_persistent_id(("tasks", &call.id, latest));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, latest)
            .show_header(ui, |ui| {
                ui.label(theme::caps("tasks").color(p.pencil));
                ui.label(
                    RichText::new(format!("{done}/{}", list.len()))
                        .small()
                        .color(p.pencil),
                );
            })
            .body(|ui| {
                for task in list {
                    ui.horizontal(|ui| {
                        // Plain-text checkboxes, like a list kept by hand.
                        let (mark, color) = match task.status {
                            TaskStatus::Done => ("[x]", p.ok),
                            TaskStatus::InProgress => ("[>]", p.signal),
                            TaskStatus::Pending => ("[ ]", p.pencil),
                        };
                        ui.label(RichText::new(mark).monospace().color(color));
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
                        .corner_radius(2)
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
    margin_row(ui, Some(theme::caps("error").color(color).into()), |ui| {
        entry(ui, color, |ui| {
            ui.add(egui::Label::new(RichText::new(err).monospace().color(color)).wrap());
            if let Some(id) = retry
                && ui.button("Retry").clicked()
            {
                actions.push(Action::Regenerate(id));
            }
        });
    });
}

pub struct EmptyInfo<'a> {
    pub provider: &'a str,
    pub base_url: &'a str,
    pub model: &'a str,
    pub mcp_servers: usize,
    pub mcp_tools: usize,
    pub missing_key: bool,
}

/// A new chat: what the next message will go to, as a readout. No greeting, no
/// suggestions; the composer below is the call to action.
pub fn empty_state(ui: &mut egui::Ui, info: EmptyInfo<'_>) {
    let p = theme::palette(ui.visuals().dark_mode);
    ui.add_space((ui.available_height() * 0.3).max(24.0));
    centered(ui, |ui| {
        margin_row(ui, Some(theme::caps("new").color(p.signal).into()), |ui| {
            let model = if info.model.is_empty() {
                "none selected"
            } else {
                info.model
            };
            let tools = match info.mcp_servers {
                0 => "built-in only".to_owned(),
                n => format!(
                    "{} from {n} MCP server{}",
                    info.mcp_tools,
                    if n == 1 { "" } else { "s" }
                ),
            };
            egui::Grid::new("empty-readout")
                .num_columns(2)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    for (key, value) in [
                        ("provider", info.provider),
                        ("endpoint", info.base_url),
                        ("model", model),
                        ("tools", tools.as_str()),
                    ] {
                        ui.label(RichText::new(key).monospace().color(p.pencil));
                        ui.label(RichText::new(value).monospace().color(p.ink));
                        ui.end_row();
                    }
                });
            if info.missing_key {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("No API key for this provider. Add one in Settings (⌘,).")
                        .monospace()
                        .color(p.warn),
                );
            }
        });
    });
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
    fn results_belong_to_their_own_turn() {
        // Two turns whose calls share the id `call_0`, as some local servers produce.
        let call = |n: &str| ToolCall {
            id: "call_0".into(),
            name: "x__echo".into(),
            arguments: format!(r#"{{"n": "{n}"}}"#),
        };
        let mut first = ChatMessage::new(Role::Assistant, "");
        first.tool_calls = vec![call("1")];
        let mut second = ChatMessage::new(Role::Assistant, "");
        second.tool_calls = vec![call("2")];
        let messages = vec![
            ChatMessage::new(Role::User, "a"),
            first.clone(),
            ChatMessage::tool_result(&first.tool_calls[0], "one", false),
            ChatMessage::new(Role::User, "b"),
            second,
        ];
        assert_eq!(results_after(&messages, 1)["call_0"].content, "one");
        // The second turn's call has not been answered yet.
        assert!(results_after(&messages, 4).is_empty());
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

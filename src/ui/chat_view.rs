//! Rendering of the conversation: messages, tool-call cards, errors and the empty state.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use egui_commonmark::CommonMarkCache;
use uuid::Uuid;

use super::attachments::{self as attach_ui, ImageCache};
use super::markdown::{self, DrawingCache};
use super::theme::{self, icon};
use super::widgets::{code_block, diff_block, diff_stat, quiet_icons};
use crate::app::{Action, Run};
use crate::approval::{Approval, PendingApproval};
use crate::artifacts::Artifact;
use crate::config::{ReasoningDisplay, SaveAsSettings};
use crate::save_as::Content;
use crate::subagent::{SubagentReport, SubagentState};
use crate::tools::{self, Task, TaskStatus};
use crate::types::{ChatMessage, Conversation, Role, ToolCall, results_after};

pub const MAX_CONTENT_WIDTH: f32 = 760.0;

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

/// A soft bordered card inside a reply: tool calls, task lists, errors.
fn card<R>(ui: &mut egui::Ui, stroke: egui::Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub struct View<'a> {
    pub conv: &'a Conversation,
    pub run: Option<&'a Run>,
    /// The reply waits for a free slot: how many queued replies are ahead of it.
    pub queued: Option<usize>,
    pub error: Option<&'a String>,
    pub show_stats: bool,
    pub reasoning: ReasoningDisplay,
    pub remote_images: bool,
    /// Every artifact in the chat, and the one open in the panel.
    pub artifacts: &'a [Artifact],
    pub open_artifact: Option<(usize, usize)>,
    /// The file types "Save as…" offers (Settings → General).
    pub save_as: SaveAsSettings,
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
        // The chat's own latest request; subagents have contexts of their own, and a summary
        // request's size says nothing about the chat's context.
        let context = conv
            .messages
            .iter()
            .rev()
            .filter(|m| m.compaction.is_none())
            .find_map(|m| m.stats.as_ref())
            .and_then(crate::stats::RequestStats::context_use);
        if totals.requests > 0 {
            centered(ui, |ui| super::stats_view::totals_bar(ui, &totals, context));
        }
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.add_space(12.0);
            // The last assistant message of each turn, and how many of its edits can be undone.
            let mut undo_at: HashMap<usize, usize> = HashMap::new();
            if v.run.is_none() && conv.workspace.is_some() {
                let mut i = 0;
                while i < conv.messages.len() {
                    let turn = crate::checkpoints::turn_of(&conv.messages, i);
                    let n = crate::checkpoints::undoable(&conv.messages, turn.clone());
                    if n > 0
                        && let Some(last) = conv.messages[turn.clone()]
                            .iter()
                            .rposition(|m| m.role == Role::Assistant)
                    {
                        undo_at.insert(turn.start + last, n);
                    }
                    i = turn.end.max(i + 1);
                }
            }
            centered(ui, |ui| {
                for (i, msg) in conv.messages.iter().enumerate() {
                    match msg.role {
                        Role::System => {
                            if let Some(info) = &msg.compaction {
                                let stats = msg.stats.as_ref().filter(|_| v.show_stats);
                                compaction_marker(ui, i, info, &msg.content, stats);
                                ui.add_space(18.0);
                            }
                            continue;
                        }
                        Role::Tool => continue,
                        Role::User => {
                            user_message(ui, images, conv.id, i, msg, v.run.is_some(), &mut actions)
                        }
                        Role::Assistant => {
                            let streaming = v.run.is_some() && i + 1 == conv.messages.len();
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
                                    artifacts: v.artifacts,
                                    open_artifact: v.open_artifact,
                                    last: Some(i) == last_assistant && v.run.is_none(),
                                    undo: undo_at.get(&i).copied(),
                                    save_as: v.save_as,
                                },
                                msg,
                                &mut actions,
                            );
                        }
                    }
                    ui.add_space(18.0);
                }
                if v.run.is_some_and(|r| r.compacting) {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Summarising older messages…").weak());
                    });
                    ui.add_space(12.0);
                }
                if let Some(ahead) = v.queued {
                    let p = theme::palette(ui.visuals().dark_mode);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::CLOCK).color(p.pencil));
                        ui.label(RichText::new("Queued").color(p.ink2));
                        let detail = match ahead {
                            0 => "starts when a slot frees up".to_owned(),
                            n => format!("{n} ahead of it"),
                        };
                        ui.label(RichText::new(format!("· {detail}")).weak());
                    });
                    ui.add_space(12.0);
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
    let margin = 14.0;
    let max_w = ui.available_width() * 0.8 - 2.0 * margin;
    // Size the bubble to its content: the longest line, a file's header (with room for the
    // expand arrow), or room for a thumbnail.
    let text_width = |ui: &egui::Ui, text: String, style: egui::TextStyle| {
        let font = style.resolve(ui.style());
        ui.ctx()
            .fonts_mut(|f| f.layout_no_wrap(text, font, egui::Color32::WHITE).size().x)
    };
    let mut width = text_width(ui, msg.content.clone(), egui::TextStyle::Body).ceil() + 2.0;
    for a in &msg.attachments {
        width = width.max(if a.is_image() {
            320.0
        } else {
            text_width(ui, attach_ui::file_label(a), egui::TextStyle::Body) + 28.0
        });
    }
    let width = width.min(max_w);
    // Your messages sit in a bubble on the right; replies run full width. Laid out from the
    // right edge, so the bubble can never reach past the column.
    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
        egui::Frame::new()
            .fill(p.raised)
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(margin as i8, 10))
            .show(ui, |ui| {
                // Narrow the (right-aligned) frame first, then lay the content out from its left.
                ui.set_width(width);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    if !msg.content.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(&msg.content).color(p.ink))
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
                });
            });
    });
    if msg.cancelled {
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
            ui.label(RichText::new("Cancelled").small().color(p.pencil));
        });
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let items: &[(&str, &str)] = if running {
            &[(icon::COPY, "Copy")]
        } else {
            &[
                (icon::EDIT, "Edit and resend"),
                (icon::COPY, "Copy"),
                (icon::FLOW, "Fork from here"),
            ]
        };
        match quiet_icons(ui, items).map(|i| items[i].1) {
            Some("Copy") => actions.push(Action::Copy(msg.content.clone())),
            Some("Edit and resend") => actions.push(Action::EditFrom(conv, idx)),
            Some("Fork from here") => actions.push(Action::ForkFrom(conv, idx)),
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
    artifacts: &'a [Artifact],
    open_artifact: Option<(usize, usize)>,
    /// The final reply: show Regenerate.
    last: bool,
    /// The last reply of a turn whose file edits can be undone: how many.
    undo: Option<usize>,
    save_as: SaveAsSettings,
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
    // Scoped by position: widget ids below use call ids, which servers may reuse.
    ui.push_id(("message", cx.idx), |ui| {
        assistant_body(ui, md_cache, drawings, images, &cx, msg, actions)
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
                    "Thinking…"
                } else {
                    "Thought process"
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
            ui.label(RichText::new("Thinking…").weak());
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
            Some(&markdown::Cards {
                conv: cx.conv,
                list: &cx
                    .artifacts
                    .iter()
                    .filter(|a| a.message == cx.idx)
                    .collect::<Vec<_>>(),
                open: cx.open_artifact,
            }),
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
            let mut items = Vec::new();
            if !msg.content.is_empty() {
                items.push((icon::COPY, "Copy"));
            }
            if cx.last {
                items.push((icon::REGENERATE, "Retry"));
            }
            if cx.run.is_none() {
                items.push((icon::FLOW, "Fork from here"));
            }
            match quiet_icons(ui, &items).map(|i| items[i].1) {
                Some("Copy") => actions.push(Action::Copy(msg.content.clone())),
                Some("Retry") => actions.push(Action::Regenerate(cx.conv)),
                Some("Fork from here") => actions.push(Action::ForkFrom(cx.conv, cx.idx)),
                _ => {}
            }
            if !msg.content.is_empty() {
                save_as_menu(
                    ui,
                    cx.conv,
                    None,
                    Content::Markdown(msg.content.clone()),
                    cx.save_as,
                    actions,
                );
            }
            if let Some(n) = cx.undo {
                let label = if n == 1 {
                    "Undo file edit".to_owned()
                } else {
                    format!("Undo {n} file edits")
                };
                if ui
                    .small_button(theme::labelled(icon::UNDO, &label))
                    .on_hover_text(
                        "Put back the files this turn changed with write_file or edit_file \
                         (newest first). Files changed again since are left alone; shell \
                         commands aren't undone.",
                    )
                    .clicked()
                {
                    actions.push(Action::UndoTurn(cx.conv, cx.idx));
                }
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

/// The "Save as…" menu: the formats enabled in Settings that `content` supports, each saving
/// through `Action::SaveAs`. `title` names the file (the chat's when `None`).
pub fn save_as_menu(
    ui: &mut egui::Ui,
    conv: Uuid,
    title: Option<String>,
    content: Content,
    settings: SaveAsSettings,
    actions: &mut Vec<Action>,
) {
    let formats = content.formats(&settings);
    if formats.is_empty() {
        return;
    }
    let pencil = theme::palette(ui.visuals().dark_mode).pencil;
    ui.menu_button(
        RichText::new(icon::DOWNLOAD).size(15.0).color(pencil),
        |ui| {
            for format in formats {
                if ui.button(format.label()).clicked() {
                    actions.push(Action::SaveAs {
                        conv,
                        title: title.clone(),
                        content: content.clone(),
                        format,
                    });
                    ui.close();
                }
            }
        },
    )
    .response
    .on_hover_text("Save as…");
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
        Some(r) if r.is_error => ("Failed", p.error),
        Some(_) => ("Done", p.pencil),
        None if pending_approval => ("Needs approval", p.signal),
        None if running => ("Running", p.pencil),
        None => ("Not run", p.pencil),
    };
    ui.add_space(2.0);
    card(
        ui,
        if pending_approval { p.outline } else { p.rule },
        |ui| {
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
                    }
                    ui.label(RichText::new(icon::TOOL).color(p.pencil));
                    ui.label(RichText::new(tools::display_name(&call.name)).color(p.ink));
                    ui.label(RichText::new(status).small().color(color));
                    if let Some(e) = result.and_then(|r| r.edit.as_ref()) {
                        if !e.diff.is_empty() {
                            let (add, del) = diff_stat(&e.diff);
                            ui.label(RichText::new(format!("+{add}")).small().color(p.ok));
                            ui.label(RichText::new(format!("−{del}")).small().color(p.error));
                        }
                        if e.undone {
                            ui.label(RichText::new("Undone").small().color(p.pencil));
                        }
                    }
                    if let Some(sb) = result.and_then(|r| r.sandbox.as_ref()) {
                        ui.label(RichText::new(format!("in {}", sb.image)).small().weak())
                            .on_hover_text(format!("Ran in the Docker container {}", sb.container));
                    }
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
                    if let Some(e) = result
                        .and_then(|r| r.edit.as_ref())
                        .filter(|e| !e.diff.is_empty())
                    {
                        ui.label(RichText::new("Change").small().color(p.pencil));
                        diff_block(ui, &e.diff);
                    }
                    ui.label(RichText::new("Arguments").small().color(p.pencil));
                    code_block(ui, &pretty_args);
                    if let Some(r) = result {
                        ui.label(
                            RichText::new(if r.is_error { "Error" } else { "Result" })
                                .small()
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
                match &pending.preview {
                    Some(diff) if diff.is_empty() => {
                        ui.label(RichText::new("No change to the file").small().weak());
                    }
                    Some(diff) => {
                        let (add, del) = diff_stat(diff);
                        ui.label(
                            RichText::new(format!("Change: +{add} −{del}"))
                                .small()
                                .color(p.pencil),
                        );
                        diff_block(ui, diff);
                    }
                    None if !state_is_open(ui, id) => code_block(ui, &pretty_args),
                    None => {}
                }
                approval_prompt(ui, conv, pending, actions);
            }
        },
    );
}

/// Where older messages were summarised: a divider with the summary to expand.
fn compaction_marker(
    ui: &mut egui::Ui,
    idx: usize,
    info: &crate::types::CompactionInfo,
    summary: &str,
    stats: Option<&crate::stats::RequestStats>,
) {
    let p = theme::palette(ui.visuals().dark_mode);
    card(ui, p.rule, |ui| {
        let id = ui.make_persistent_id(("compaction", idx));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
            .show_header(ui, |ui| {
                ui.label(RichText::new(icon::STACK).color(p.pencil));
                ui.label(RichText::new("Earlier messages summarised").color(p.ink));
                let tokens = if info.tokens_before > 0 {
                    format!(
                        " · ~{} tokens before",
                        crate::stats::fmt_tokens(info.tokens_before)
                    )
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!(
                        "{} message{}{}{}",
                        info.messages,
                        if info.messages == 1 { "" } else { "s" },
                        tokens,
                        if info.manual { " · by hand" } else { "" }
                    ))
                    .small()
                    .color(p.pencil),
                )
                .on_hover_text(
                    "Messages above this line are no longer sent to the model; it gets this \
                     summary instead. They stay here for you.",
                );
            })
            .body(|ui| {
                ui.add(
                    egui::Label::new(RichText::new(summary).color(p.ink2))
                        .selectable(true)
                        .wrap(),
                );
                if let Some(s) = stats {
                    ui.add_space(4.0);
                    super::stats_view::request_line(ui, s);
                }
            });
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
    let p = theme::palette(dark);
    let mut answer = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Allow this tool to run?").color(p.ink2));
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
    card(ui, p.rule, |ui| {
        // Keyed on `latest` so a list folds once a newer one replaces it.
        let id = ui.make_persistent_id(("tasks", &call.id, latest));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, latest)
            .show_header(ui, |ui| {
                ui.label(RichText::new(icon::LIST_CHECKS).color(p.pencil));
                ui.label(RichText::new("Tasks").color(p.ink));
                ui.label(
                    RichText::new(format!("{done} of {} done", list.len()))
                        .small()
                        .color(p.pencil),
                );
            })
            .body(|ui| {
                for task in list {
                    ui.horizontal(|ui| {
                        let (mark, color) = match task.status {
                            TaskStatus::Done => (icon::CHECK_SQUARE, p.ok),
                            TaskStatus::InProgress => (icon::ARROW_RIGHT, p.signal),
                            TaskStatus::Pending => (icon::SQUARE, p.pencil),
                        };
                        ui.label(RichText::new(mark).color(color));
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
                        .corner_radius(8)
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
    card(ui, color.gamma_multiply(0.6), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(icon::WARNING).color(color));
            ui.add(egui::Label::new(RichText::new(err).color(color)).wrap());
        });
        if let Some(id) = retry
            && ui
                .button(theme::labelled(icon::REGENERATE, "Retry"))
                .clicked()
        {
            actions.push(Action::Regenerate(id));
        }
    });
}

/// The new-chat greeting, "Good evening, Aaron", in the serif with a time-of-day icon.
pub fn greeting(ui: &mut egui::Ui, name: Option<&str>) {
    let p = theme::palette(ui.visuals().dark_mode);
    let hour = chrono::Timelike::hour(&chrono::Local::now());
    let (glyph, text) = match hour {
        5..=11 => (icon::SUN, "Good morning"),
        12..=17 => (icon::SUN, "Good afternoon"),
        _ => (icon::MOON, "Good evening"),
    };
    let text = match name {
        Some(n) => format!("{text}, {n}"),
        None => text.to_owned(),
    };
    let font = egui::FontId::new(34.0, egui::FontFamily::Proportional);
    // Centre the icon and the words together.
    let width = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), p.ink).size().x)
        + 44.0;
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        ui.label(RichText::new(glyph).size(30.0).color(p.signal));
        ui.add_space(4.0);
        ui.label(RichText::new(text).font(font).color(p.ink));
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

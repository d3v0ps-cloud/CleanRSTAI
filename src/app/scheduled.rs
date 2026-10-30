//! The Scheduled page: agents that run a prompt on a schedule (`schedule::ScheduledTask`), as
//! cards with their switch, timing, settings and recent runs, an editor for one at a time, and
//! templates to start from. The tasks are `general.schedules` (also editable in Settings →
//! Background); changes here are saved straight away.

use super::shell::folder_name;
use super::*;
use crate::schedule::Schedule;
use crate::ui::chrome;

/// Runs listed under each agent.
const RUNS_SHOWN: usize = 3;

/// Starting points for a new agent: name, prompt, schedule, and whether it needs a folder.
const TEMPLATES: [(&str, &str, Schedule, bool); 3] = [
    (
        "Morning briefing",
        "Search the web for today's most important news in technology and AI. Give me five \
         short bullet points, each with a link to its source.",
        Schedule::Daily {
            hour: 8,
            minute: 30,
        },
        false,
    ),
    (
        "Daily repository check",
        "Look at this repository's git status and its uncommitted diff. List what changed, \
         anything that looks unfinished or risky, and the next steps you would suggest.",
        Schedule::Daily { hour: 9, minute: 0 },
        true,
    ),
    (
        "Weekly dependency review",
        "Read the project's dependency manifests (such as Cargo.toml, package.json or \
         requirements.txt) and list the dependencies that look outdated, unmaintained or risky, \
         with what to check for each.",
        Schedule::Weekly {
            weekday: 0,
            hour: 10,
            minute: 0,
        },
        true,
    ),
];

/// How a run's chat is doing, for the mark beside it.
fn run_mark(
    app: &ChatApp,
    id: Uuid,
    p: &theme::Palette,
) -> (&'static str, egui::Color32, &'static str) {
    if app.runs.get(&id).is_some_and(|r| !r.approvals.is_empty()) {
        (icon::SHIELD, p.warn, "Waiting for approval")
    } else if app.runs.contains_key(&id) {
        (icon::HOURGLASS, p.signal, "Running")
    } else if app.queue.contains(id) {
        (icon::CLOCK, p.pencil, "Queued")
    } else if app.errors.contains_key(&id) {
        (icon::FAILED, p.error, "Failed")
    } else {
        (icon::OK, p.ok, "Finished")
    }
}

/// What the page's controls asked for, applied after drawing.
#[derive(Default)]
struct PageActs {
    master: Option<bool>,
    enable: Option<(Uuid, bool)>,
    run: Option<Uuid>,
    edit: Option<Uuid>,
    duplicate: Option<Uuid>,
    delete: Option<Uuid>,
    open_chat: Option<Uuid>,
    new_from: Option<ScheduledTask>,
    save: bool,
    cancel: bool,
}

impl ChatApp {
    pub(super) fn scheduled_page(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let mut acts = PageActs::default();
        let providers: Vec<(String, String)> = self
            .settings
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let chats: Vec<(Uuid, String)> = self
            .conversations
            .iter()
            .filter(|c| !c.messages.is_empty())
            .map(|c| (c.id, c.title.clone()))
            .collect();
        let plan_on = self.settings.general.plan.enabled;
        let enabled = self.settings.general.schedules.enabled;
        let tasks = self.settings.general.schedules.tasks.clone();
        let editing_new = self.schedule_edit.as_ref().is_some_and(|(_, new)| *new);

        egui::ScrollArea::vertical()
            .id_salt("scheduled-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(28.0);
                centered(ui, |ui| {
                    Self::page_heading(ui, "Scheduled", |ui| {
                        if ui
                            .add(theme::primary_button(
                                theme::labelled(icon::PLUS, "New agent"),
                                dark,
                            ))
                            .clicked()
                        {
                            acts.new_from = Some(ScheduledTask {
                                name: "New agent".into(),
                                ..Default::default()
                            });
                        }
                        let mut on = enabled;
                        if chrome::toggle(ui, &mut on)
                            .on_hover_text("Run scheduled agents while CleanRSTAI is open")
                            .changed()
                        {
                            acts.master = Some(on);
                        }
                        ui.label(RichText::new("Run agents").size(13.0).color(p.pencil));
                    });
                    ui.label(
                        RichText::new(
                            "Agents send a prompt on a schedule, each run in a chat of its own \
                             (or one you pick), through the reply queue. They run only while \
                             CleanRSTAI is open, and nobody is watching, so give them the \
                             permissions they need and no more.",
                        )
                        .color(p.pencil),
                    );
                    if !enabled {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "{}  Agents are paused: switch on Run agents to start them.",
                                icon::WARNING
                            ))
                            .color(p.warn),
                        );
                    }
                    ui.add_space(16.0);

                    if editing_new {
                        self.agent_editor(ui, &providers, &chats, plan_on, &mut acts);
                        ui.add_space(12.0);
                    }
                    for task in &tasks {
                        let editing = self
                            .schedule_edit
                            .as_ref()
                            .is_some_and(|(t, new)| !new && t.id == task.id);
                        if editing {
                            self.agent_editor(ui, &providers, &chats, plan_on, &mut acts);
                        } else {
                            self.agent_card(ui, task, enabled, &providers, &chats, &mut acts);
                        }
                        ui.add_space(10.0);
                    }
                    if tasks.is_empty() && !editing_new {
                        Self::empty_state(
                            ui,
                            "No scheduled agents yet",
                            "Start from a template below, or make one with New agent.",
                        );
                        ui.add_space(20.0);
                    }
                    if tasks.is_empty() {
                        self.agent_templates(ui, &mut acts);
                    }
                });
                ui.add_space(24.0);
            });
        self.apply_page_acts(acts, ui.ctx());
    }

    /// One agent: its switch, name and actions, then when it runs, how, and its recent runs.
    fn agent_card(
        &self,
        ui: &mut egui::Ui,
        task: &ScheduledTask,
        master: bool,
        providers: &[(String, String)],
        chats: &[(Uuid, String)],
        acts: &mut PageActs,
    ) {
        let p = theme::palette(ui.visuals().dark_mode);
        egui::Frame::new()
            .fill(p.raised)
            .stroke(egui::Stroke::new(1.0, p.hairline))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.push_id(task.id, |ui| {
                    ui.horizontal(|ui| {
                        let mut on = task.enabled;
                        if chrome::toggle(ui, &mut on)
                            .on_hover_text(if on {
                                "On: runs on its schedule"
                            } else {
                                "Off"
                            })
                            .changed()
                        {
                            acts.enable = Some((task.id, on));
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new(&task.name).size(16.0).color(if task.enabled {
                            p.ink
                        } else {
                            p.ink2
                        }));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.menu_button(RichText::new(icon::DOTS).size(16.0), |ui| {
                                if ui
                                    .button(theme::labelled(icon::COPY, "Duplicate"))
                                    .clicked()
                                {
                                    acts.duplicate = Some(task.id);
                                }
                                ui.separator();
                                if ui.button(theme::labelled(icon::TRASH, "Delete")).clicked() {
                                    acts.delete = Some(task.id);
                                }
                            });
                            if ui.button(theme::labelled(icon::EDIT, "Edit")).clicked() {
                                acts.edit = Some(task.id);
                            }
                            let ready = !task.prompt.trim().is_empty();
                            if ui
                                .add_enabled(
                                    ready,
                                    egui::Button::new(theme::labelled(
                                        icon::ARROW_RIGHT,
                                        "Run now",
                                    )),
                                )
                                .on_hover_text("Start a run now, whatever the schedule says")
                                .on_disabled_hover_text("Write a prompt first")
                                .clicked()
                            {
                                acts.run = Some(task.id);
                            }
                        });
                    });

                    // When: the schedule, the next run and the last.
                    let mut when = vec![schedule::describe(task.schedule)];
                    let local = |at: chrono::DateTime<chrono::Utc>| {
                        let at = at.with_timezone(&chrono::Local);
                        format!(
                            "{} {}",
                            chat_view::short_date(at, chrono::Local::now()),
                            at.format("%H:%M")
                        )
                    };
                    match (master && task.enabled, schedule::upcoming(task)) {
                        (true, Some(at)) => when.push(format!("Next {}", local(at))),
                        (true, None) => {}
                        (false, _) => when.push("Paused".to_owned()),
                    }
                    if let Some(at) = task.last_run {
                        when.push(format!("Last {}", local(at)));
                    }
                    ui.label(
                        RichText::new(format!("{}  {}", icon::CLOCK, when.join("  ·  ")))
                            .size(13.0)
                            .color(p.ink2),
                    );

                    // How: permissions, where it runs, folder and provider.
                    let mut how = vec![format!(
                        "{}  {}",
                        crate::ui::widgets::mode_icon(task.approval_mode),
                        task.approval_mode.label()
                    )];
                    how.push(match task.target {
                        Target::NewChat => "A new chat each run".to_owned(),
                        Target::Chat(id) => format!(
                            "Continues “{}”",
                            chats
                                .iter()
                                .find(|(c, _)| *c == id)
                                .map_or("a deleted chat", |(_, t)| t.as_str())
                        ),
                    });
                    if let Some(folder) = &task.workspace {
                        how.push(format!("{}  {}", icon::FOLDER, folder_name(folder)));
                    }
                    if let Some(name) = task
                        .provider
                        .as_ref()
                        .and_then(|id| providers.iter().find(|(p, _)| p == id))
                        .map(|(_, n)| n)
                    {
                        how.push(format!("{}  {name}", icon::CLOUD));
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new(how.join("   ·   "))
                                .size(13.0)
                                .color(p.pencil),
                        )
                        .truncate(),
                    );
                    let prompt = task
                        .prompt
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("No prompt yet");
                    ui.add(
                        egui::Label::new(
                            RichText::new(prompt).size(13.0).italics().color(p.pencil),
                        )
                        .truncate(),
                    );

                    // The chats its recent runs made, newest first.
                    let mut runs: Vec<&Conversation> = self
                        .conversations
                        .iter()
                        .filter(|c| c.scheduled_task == Some(task.id))
                        .collect();
                    runs.sort_by_key(|c| std::cmp::Reverse(c.created_at));
                    if !runs.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("Recent runs ({})", runs.len()))
                                .size(12.0)
                                .color(p.pencil),
                        );
                        ui.spacing_mut().item_spacing.y = chrome::ROW_GAP;
                        for conv in runs.iter().take(RUNS_SHOWN) {
                            let (glyph, tint, state) = run_mark(self, conv.id, &p);
                            let started = conv.created_at.with_timezone(&chrono::Local);
                            // A run's chat is titled with the agent's name and the time, so
                            // only a title the chat was given since is worth showing.
                            let renamed = !conv.title.starts_with(task.name.as_str());
                            let title = format!(
                                "{} {}  ·  {}",
                                chat_view::short_date(started, chrono::Local::now()),
                                started.format("%H:%M"),
                                if renamed { conv.title.as_str() } else { state }
                            );
                            if chrome::recent_row(ui, &title, false, Some((glyph, tint)), p.raised)
                                .on_hover_text(state)
                                .clicked()
                            {
                                acts.open_chat = Some(conv.id);
                            }
                        }
                    }
                });
            });
    }

    /// The agent being edited: name, prompt, then schedule, target, permissions, folder,
    /// provider and catch-up, with Save and Cancel.
    fn agent_editor(
        &mut self,
        ui: &mut egui::Ui,
        providers: &[(String, String)],
        chats: &[(Uuid, String)],
        plan_on: bool,
        acts: &mut PageActs,
    ) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let Some((task, new)) = &mut self.schedule_edit else {
            return;
        };
        egui::Frame::new()
            .fill(p.raised)
            .stroke(egui::Stroke::new(1.0, p.outline))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.push_id(("agent-editor", task.id), |ui| {
                    ui.label(
                        RichText::new(if *new { "New agent" } else { "Edit agent" })
                            .size(12.0)
                            .color(p.pencil),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut task.name)
                            .hint_text("Name")
                            .font(egui::FontId::proportional(16.0))
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut task.prompt)
                            .hint_text(
                                "What the agent should do each run. It has this chat's tools: \
                                 the defaults from Settings, and the folder's tools when it \
                                 has one.",
                            )
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(8.0);
                    crate::ui::schedule_settings::task_fields(ui, task, providers, chats, plan_on);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ready = !task.prompt.trim().is_empty();
                        if ui
                            .add_enabled(
                                ready,
                                theme::primary_button(
                                    if *new { "Create agent" } else { "Save" },
                                    dark,
                                ),
                            )
                            .on_disabled_hover_text("Write a prompt first")
                            .clicked()
                        {
                            acts.save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            acts.cancel = true;
                        }
                    });
                });
            });
    }

    /// Templates to start an agent from.
    fn agent_templates(&self, ui: &mut egui::Ui, acts: &mut PageActs) {
        let p = theme::palette(ui.visuals().dark_mode);
        ui.label(
            RichText::new("Start from a template")
                .size(13.0)
                .color(p.pencil),
        );
        ui.add_space(8.0);
        let gap = 12.0;
        let width = ((ui.available_width() - 2.0 * gap) / 3.0).max(150.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (name, prompt, schedule, needs_folder) in TEMPLATES {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(width, 112.0), egui::Sense::click());
                resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                let t = chrome::hover_amount(ui, &resp);
                chrome::card_frame(ui, rect, t);
                let inner = rect.shrink(12.0);
                let painter = ui.painter();
                painter.text(
                    inner.left_top(),
                    egui::Align2::LEFT_TOP,
                    icon::CALENDAR,
                    egui::FontId::proportional(16.0),
                    p.signal,
                );
                painter.text(
                    inner.left_top() + egui::vec2(0.0, 24.0),
                    egui::Align2::LEFT_TOP,
                    name,
                    egui::FontId::proportional(14.0),
                    p.ink,
                );
                let mut job = egui::text::LayoutJob::single_section(
                    format!(
                        "{}{}",
                        schedule::describe(schedule),
                        if needs_folder {
                            " · needs a folder"
                        } else {
                            ""
                        }
                    ),
                    egui::TextFormat::simple(egui::FontId::proportional(12.0), p.pencil),
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: inner.width(),
                    max_rows: 2,
                    ..Default::default()
                };
                let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
                ui.painter()
                    .galley(inner.left_top() + egui::vec2(0.0, 46.0), galley, p.pencil);
                if resp.on_hover_text(prompt).clicked() {
                    acts.new_from = Some(ScheduledTask {
                        name: name.to_owned(),
                        prompt: prompt.to_owned(),
                        schedule,
                        workspace: if needs_folder {
                            self.last_folder()
                        } else {
                            None
                        },
                        ..Default::default()
                    });
                }
            }
        });
    }

    fn apply_page_acts(&mut self, acts: PageActs, ctx: &egui::Context) {
        let mut changed = false;
        let schedules = &mut self.settings.general.schedules;
        if let Some(on) = acts.master {
            schedules.enabled = on;
            changed = true;
        }
        if let Some((id, on)) = acts.enable
            && let Some(t) = schedules.tasks.iter_mut().find(|t| t.id == id)
        {
            t.enabled = on;
            changed = true;
        }
        if let Some(id) = acts.duplicate
            && let Some(t) = schedules.tasks.iter().find(|t| t.id == id).cloned()
        {
            let copy = ScheduledTask {
                id: Uuid::new_v4(),
                name: format!("{} (copy)", t.name),
                enabled: false,
                last_run: None,
                created: Some(chrono::Utc::now()),
                ..t
            };
            let at = schedules
                .tasks
                .iter()
                .position(|x| x.id == id)
                .map_or(schedules.tasks.len(), |i| i + 1);
            schedules.tasks.insert(at, copy);
            changed = true;
        }
        if let Some(id) = acts.delete {
            schedules.tasks.retain(|t| t.id != id);
            if self.schedule_edit.as_ref().is_some_and(|(t, _)| t.id == id) {
                self.schedule_edit = None;
            }
            changed = true;
        }
        if let Some(id) = acts.edit
            && let Some(t) = self
                .settings
                .general
                .schedules
                .tasks
                .iter()
                .find(|t| t.id == id)
        {
            self.schedule_edit = Some((t.clone(), false));
        }
        if let Some(task) = acts.new_from {
            self.schedule_edit = Some((task, true));
        }
        if acts.cancel {
            self.schedule_edit = None;
        }
        if acts.save
            && let Some((mut task, new)) = self.schedule_edit.take()
        {
            let schedules = &mut self.settings.general.schedules;
            if new {
                task.created = Some(chrono::Utc::now());
                schedules.tasks.push(task);
            } else if let Some(live) = schedules.tasks.iter_mut().find(|t| t.id == task.id) {
                // Keep the run times the schedule has kept since editing began.
                task.last_run = live.last_run;
                task.created = live.created.or(task.created);
                *live = task;
            }
            schedules.normalize();
            changed = true;
        }
        if changed {
            self.save_settings();
        }
        if let Some(id) = acts.run
            && let Some(task) = self
                .settings
                .general
                .schedules
                .tasks
                .iter()
                .find(|t| t.id == id)
                .cloned()
        {
            self.run_now(task, ctx, None);
        }
        if let Some(id) = acts.open_chat {
            self.current = Some(id);
            self.page = Page::Chat;
        }
    }
}

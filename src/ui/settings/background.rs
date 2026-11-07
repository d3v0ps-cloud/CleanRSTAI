//! The Background page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn background_tab(
        &mut self,
        ui: &mut egui::Ui,
        chats: &[(Uuid, String)],
        actions: &mut Vec<SettingsAction>,
    ) {
        let providers: Vec<(String, String)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let plan_on = self.draft.general.plan.enabled;
        let schedules = &mut self.draft.general.schedules;
        let bg = &mut self.draft.general.background;
        let ex = &mut self.draft.general.exec;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(crate::tray::SUPPORTED, |ui| {
                    ui.checkbox(&mut bg.keep_running, crate::tray::setting_label())
                });
                if crate::tray::SUPPORTED {
                    ui.indent("bg-keep-running", |ui| {
                        ui.weak(
                            "Closing the window hides it, and replies and MCP servers keep \
                             running. Show the window or quit from the icon's menu. On macOS, \
                             Command-Q quits as usual.",
                        );
                    });
                } else {
                    ui.indent("bg-keep-running", |ui| {
                        ui.weak("Not available on Linux yet.");
                    });
                }
                ui.add_space(10.0);
                ui.checkbox(
                    &mut bg.notify,
                    egui::RichText::new("Notify when a reply finishes, fails or needs approval")
                        .strong(),
                );
                ui.indent("bg-notify", |ui| {
                    let on = bg.notify;
                    ui.add_enabled(
                        on,
                        egui::Checkbox::new(
                            &mut bg.notify_only_unfocused,
                            "Only while the CleanRSTAI window is in the background",
                        ),
                    );
                    ui.weak(
                        "Sent through the system's notifications. Each chat can also switch \
                         its notifications off from the tools menu.",
                    );
                });
                ui.add_space(10.0);
                egui::Grid::new("background-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Replies at once");
                        ui.add(
                            egui::DragValue::new(&mut bg.max_parallel)
                                .range(1..=crate::config::MAX_PARALLEL_RUNS),
                        );
                        ui.end_row();
                        ui.label("");
                        weak_note(
                            ui,
                            "Replies beyond this wait in a queue, marked Queued, and start in \
                             the order they were sent as others finish. Stop on a queued chat \
                             takes it out of the queue.",
                        );
                        ui.end_row();

                        ui.label("Time limit");
                        ui.add(
                            egui::DragValue::new(&mut bg.time_limit_mins)
                                .range(0..=crate::config::MAX_TIME_LIMIT_MINS)
                                .suffix(" min"),
                        );
                        ui.end_row();
                        ui.label("");
                        weak_note(
                            ui,
                            "0 means no limit. A reply that runs longer is stopped and says \
                             so. The time counts from when the reply starts, including any \
                             wait for approval.",
                        );
                        ui.end_row();
                    });
                ui.add_space(14.0);
                ui.label(egui::RichText::new("Command line").strong());
                ui.checkbox(
                    &mut ex.enabled,
                    "Allow `cleanrstai exec` to run requests from a terminal or script",
                );
                ui.indent("exec-settings", |ui| {
                    let on = ex.enabled;
                    egui::Grid::new("exec-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.add_enabled(on, egui::Label::new("Approvals"));
                            ui.add_enabled_ui(on, |ui| {
                                egui::ComboBox::from_id_salt("exec-approval-mode")
                                    .selected_text(ex.approval_mode.label())
                                    .show_ui(ui, |ui| {
                                        for m in ApprovalMode::offered(plan_on) {
                                            ui.selectable_value(
                                                &mut ex.approval_mode,
                                                m,
                                                m.label(),
                                            )
                                            .on_hover_text(m.description());
                                        }
                                    });
                            });
                            ui.end_row();
                            ui.label("");
                            weak_note(
                                ui,
                                "The mode of a command-line request; --mode overrides it. Calls \
                                 it doesn't allow are denied, not asked, and the model is told.",
                            );
                            ui.end_row();

                            ui.add_enabled(on, egui::Label::new("MCP servers"));
                            ui.add_enabled(
                                on,
                                egui::Checkbox::new(&mut ex.mcp, "Start them for each request"),
                            );
                            ui.end_row();

                            ui.add_enabled(on, egui::Label::new("Wait for them"));
                            ui.add_enabled(
                                on,
                                egui::DragValue::new(&mut ex.mcp_wait_secs)
                                    .range(0..=crate::exec::MAX_MCP_WAIT_SECS)
                                    .suffix(" s"),
                            );
                            ui.end_row();

                            ui.label("");
                            weak_note(
                                ui,
                                "How long a request waits for servers to connect before it \
                                 starts. --no-mcp skips them.",
                            );
                            ui.end_row();

                            ui.add_enabled(on, egui::Label::new("Chats"));
                            ui.add_enabled(
                                on,
                                egui::Checkbox::new(&mut ex.save_chats, "Save each run's chat"),
                            );
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    weak_note(
                        ui,
                        "Example: git diff | cleanrstai exec --mode read-only \"review this\" -\n\
                         Replies go to stdout and tool calls to stderr. Run \
                         `cleanrstai exec --help` for every option.",
                    );
                });
                crate::ui::schedule_settings::section(
                    ui, schedules, &providers, chats, plan_on, actions,
                );
            });
    }
}

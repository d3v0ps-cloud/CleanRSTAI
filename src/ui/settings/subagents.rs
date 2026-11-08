//! The Subagents page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn subagents_tab(&mut self, ui: &mut egui::Ui) {
        let providers: Vec<(String, String, Vec<String>)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.models.clone()))
            .collect();
        let servers: Vec<(String, String)> = self
            .draft
            .mcp_servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let sa = &mut self.draft.subagents;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.checkbox(
                    &mut sa.enabled,
                    egui::RichText::new("Let the model delegate tasks to subagents").strong(),
                );
                ui.indent("sa-enabled", |ui| {
                    ui.weak(
                        "Adds a \"delegate\" tool. Each subagent works on one task on its own, \
                         in parallel with the others, and its result goes back to the main chat. \
                         Delegating asks for approval unless tool calls are auto-approved \
                         (General). It can also be switched per chat from the tools menu.",
                    );
                });
                ui.add_space(10.0);
                ui.add_enabled_ui(sa.enabled, |ui| {
                    egui::Grid::new("subagent-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Provider");
                            let current = providers
                                .iter()
                                .find(|(id, ..)| *id == sa.provider_id)
                                .map_or("Same as the chat".to_owned(), |(_, n, _)| n.clone());
                            egui::ComboBox::from_id_salt("subagent-provider")
                                .selected_text(current)
                                .width(FIELD_WIDTH)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut sa.provider_id,
                                        String::new(),
                                        "Same as the chat",
                                    );
                                    for (id, name, _) in &providers {
                                        ui.selectable_value(&mut sa.provider_id, id.clone(), name);
                                    }
                                });
                            ui.end_row();

                            ui.label("Model");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut sa.model)
                                        .hint_text("the provider's selected model")
                                        .desired_width(FIELD_WIDTH - 40.0),
                                );
                                let models = providers
                                    .iter()
                                    .find(|(id, ..)| *id == sa.provider_id)
                                    .map(|(.., m)| m.clone())
                                    .unwrap_or_default();
                                if !models.is_empty() {
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        ui.set_max_height(320.0);
                                        egui::ScrollArea::vertical().show(ui, |ui| {
                                            for m in models {
                                                if ui.button(&m).clicked() {
                                                    sa.model = m;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                            ui.end_row();

                            ui.label("Per-task choice");
                            ui.checkbox(
                                &mut sa.allow_provider_choice,
                                "Let the model pick another provider profile or model for a task",
                            )
                            .on_hover_text(
                                "The delegate tool lists your provider profiles, so the model can, \
                                 for example, send simple tasks to a fast local model.",
                            );
                            ui.end_row();

                            ui.label("Tools");
                            ui.vertical(|ui| {
                                ui.checkbox(
                                    &mut sa.builtin_tools,
                                    "Time, calculator and web fetch (when on in the chat)",
                                );
                                ui.checkbox(&mut sa.mcp_tools, "The chat's MCP tools");
                                if sa.mcp_tools && !servers.is_empty() {
                                    ui.indent("sa-servers", |ui| {
                                        for (id, name) in &servers {
                                            let mut on = !sa.excluded_servers.contains(id);
                                            if ui.checkbox(&mut on, name).changed() {
                                                if on {
                                                    sa.excluded_servers.retain(|x| x != id);
                                                } else {
                                                    sa.excluded_servers.push(id.clone());
                                                }
                                            }
                                        }
                                    });
                                }
                                if sa.mcp_tools {
                                    ui.colored_label(
                                        ui.visuals().warn_fg_color,
                                        format!(
                                            "{}  Subagents call MCP tools without asking for approval.",
                                            icon::WARNING
                                        ),
                                    );
                                }
                                ui.checkbox(
                                    &mut sa.workspace,
                                    "The workspace tools, with commands in their own sandbox container",
                                )
                                .on_hover_text(
                                    "Only while the Docker sandbox is on. Each subagent gets its \
                                     own container with the chat's folder at /workspace, sharing \
                                     the chat's network rules. It is removed when the subagent \
                                     finishes.",
                                );
                                if sa.workspace {
                                    ui.colored_label(
                                        ui.visuals().warn_fg_color,
                                        format!(
                                            "{}  Subagents read, write and run commands without asking for approval; approving the delegate call allows it.",
                                            icon::WARNING
                                        ),
                                    );
                                }
                            });
                            ui.end_row();

                            ui.label("Run at once");
                            ui.add(egui::DragValue::new(&mut sa.max_parallel).range(1..=16))
                                .on_hover_text("How many subagents work at the same time");
                            ui.end_row();

                            ui.label("Tasks per call");
                            ui.add(egui::DragValue::new(&mut sa.max_tasks).range(1..=32))
                                .on_hover_text("Most tasks one delegate call may hand out");
                            ui.end_row();

                            ui.label("Max tool rounds");
                            ui.add(egui::DragValue::new(&mut sa.max_rounds).range(1..=100))
                                .on_hover_text("Tool-call rounds each subagent may make");
                            ui.end_row();

                            ui.label("Time limit");
                            ui.add(
                                egui::DragValue::new(&mut sa.timeout_secs)
                                    .range(10..=3600)
                                    .suffix(" s"),
                            )
                            .on_hover_text("Per subagent");
                            ui.end_row();
                        });
                    ui.add_space(12.0);
                    ui.strong("Subagent system prompt");
                    ui.add(
                        egui::TextEdit::multiline(&mut sa.system_prompt)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                });
            });
    }
}

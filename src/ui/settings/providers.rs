//! The Providers page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn providers_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let draft = &mut self.draft;
        self.selected_provider = self.selected_provider.min(draft.providers.len() - 1);

        egui::Panel::left("provider-list")
            .resizable(false)
            .exact_size(190.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(ui.available_height() - 36.0)
                    .show(ui, |ui| {
                        for (i, p) in draft.providers.iter().enumerate() {
                            let label = if i == draft.active_provider {
                                format!("● {}", p.name)
                            } else {
                                format!("   {}", p.name)
                            };
                            if ui
                                .selectable_label(i == self.selected_provider, label)
                                .clicked()
                            {
                                self.selected_provider = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui
                        .button(icon::PLUS)
                        .on_hover_text("Add provider")
                        .clicked()
                    {
                        draft.providers.push(ProviderConfig {
                            name: format!("Provider {}", draft.providers.len() + 1),
                            ..Default::default()
                        });
                        self.selected_provider = draft.providers.len() - 1;
                    }
                    if ui.button("⧉").on_hover_text("Duplicate").clicked() {
                        let mut copy = draft.providers[self.selected_provider].clone();
                        copy.id = uuid::Uuid::new_v4().to_string();
                        copy.name.push_str(" copy");
                        draft.providers.push(copy);
                        self.selected_provider = draft.providers.len() - 1;
                    }
                    if ui
                        .add_enabled(draft.providers.len() > 1, egui::Button::new("－"))
                        .on_hover_text("Remove provider")
                        .clicked()
                    {
                        draft.providers.remove(self.selected_provider);
                        if draft.active_provider >= self.selected_provider
                            && draft.active_provider > 0
                        {
                            draft.active_provider -= 1;
                        }
                        self.selected_provider = self.selected_provider.saturating_sub(1);
                    }
                });
            });

        let idx = self.selected_provider;
        let is_active = draft.active_provider == idx;
        let p = &mut draft.providers[idx];
        let mut make_active = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("provider-grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .striped(false)
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut p.name);
                        ui.end_row();

                        ui.label("Base URL");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut p.base_url)
                                    .hint_text("https://api.openai.com/v1")
                                    .desired_width(FIELD_WIDTH - 100.0),
                            );
                            egui::ComboBox::from_id_salt("preset")
                                .selected_text("Presets")
                                .width(90.0)
                                .show_ui(ui, |ui| {
                                    for (name, url) in PRESETS {
                                        if ui.selectable_label(false, *name).clicked() {
                                            p.base_url = (*url).to_owned();
                                        }
                                    }
                                });
                        });
                        ui.end_row();

                        ui.label("API key");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut p.api_key)
                                    .password(!self.show_key)
                                    .hint_text("sk-… (leave empty for local servers)")
                                    .desired_width(FIELD_WIDTH - 100.0),
                            );
                            ui.checkbox(&mut self.show_key, "Show");
                        });
                        ui.end_row();

                        ui.label("Model");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut p.model).desired_width(220.0));
                            if !p.models.is_empty() {
                                egui::ComboBox::from_id_salt("model-pick")
                                    .selected_text(format!("{} models", p.models.len()))
                                    .width(110.0)
                                    .height(400.0)
                                    .show_ui(ui, |ui| {
                                        for m in &p.models {
                                            ui.selectable_value(&mut p.model, m.clone(), m);
                                        }
                                    });
                            }
                            let label = if self.fetching_models {
                                "Fetching…"
                            } else {
                                "Fetch models"
                            };
                            if ui
                                .add_enabled(!self.fetching_models, egui::Button::new(label))
                                .clicked()
                            {
                                self.fetching_models = true;
                                self.models_error = None;
                                actions.push(SettingsAction::FetchModels(p.clone()));
                            }
                        });
                        ui.end_row();
                        if let Some(err) = &self.models_error {
                            ui.label("");
                            ui.colored_label(ui.visuals().error_fg_color, err);
                            ui.end_row();
                        }

                        ui.label("Context size").on_hover_text(
                            "Used when the server doesn't report the model's context size; for \
                             compaction and the context-use figure",
                        );
                        crate::ui::widgets::inherited_value(
                            ui,
                            &mut p.context_tokens,
                            None,
                            8192,
                            1024..=10_000_000,
                            256.0,
                        );
                        ui.end_row();

                        ui.label("API");
                        ui.vertical(|ui| {
                            egui::ComboBox::from_id_salt("provider-api")
                                .selected_text(p.api.label())
                                .width(260.0)
                                .show_ui(ui, |ui| {
                                    for api in crate::config::ProviderApi::ALL {
                                        ui.selectable_value(&mut p.api, api, api.label())
                                            .on_hover_text(api.description());
                                    }
                                });
                            ui.weak(p.api.description());
                        });
                        ui.end_row();

                        if p.api == crate::config::ProviderApi::Anthropic {
                            ui.label("Prompt caching");
                            ui.checkbox(
                                &mut p.prompt_caching,
                                "Cache the system prompt, tools and conversation so far",
                            )
                            .on_hover_text(
                                "Later requests read the cached part at a tenth of the price; \
                                 writing it costs a quarter more once.",
                            );
                            ui.end_row();
                        }

                        ui.label("Server type");
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("provider-kind")
                                    .selected_text(p.kind.label())
                                    .width(220.0)
                                    .show_ui(ui, |ui| {
                                        for kind in crate::config::ProviderKind::ALL {
                                            ui.selectable_value(&mut p.kind, kind, kind.label())
                                                .on_hover_text(kind.description());
                                        }
                                    });
                                let label = if self.detecting_kind { "Detecting…" } else { "Detect" };
                                if ui
                                    .add_enabled(!self.detecting_kind, egui::Button::new(label))
                                    .on_hover_text("Probe the server to find out what it is")
                                    .clicked()
                                {
                                    self.detecting_kind = true;
                                    self.detect_result = None;
                                    actions.push(SettingsAction::DetectKind(p.clone()));
                                }
                            });
                            ui.weak(p.kind.description());
                            if let Some(r) = &self.detect_result {
                                ui.weak(r);
                            }
                        });
                        ui.end_row();

                        ui.label("Token usage");
                        ui.checkbox(
                            &mut p.request_usage,
                            "Request usage in streamed responses (stream_options)",
                        )
                        .on_hover_text(
                            "Needed for exact token counts and speeds. Turn off if the server rejects the request.",
                        );
                        ui.end_row();

                        ui.label("Streaming");
                        ui.checkbox(
                            &mut p.stream,
                            "Stream responses (disable if the server doesn't support SSE)",
                        );
                        ui.end_row();

                        ui.label("Timeout");
                        ui.add(
                            egui::DragValue::new(&mut p.timeout_secs)
                                .range(10..=3600)
                                .suffix(" s"),
                        )
                        .on_hover_text(
                            "How long to wait for the server to start answering, and the longest pause while a reply streams in.",
                        );
                        ui.end_row();

                        ui.label("Retries");
                        ui.add(
                            egui::DragValue::new(&mut p.retries)
                                .range(0..=crate::config::MAX_RETRIES),
                        )
                        .on_hover_text(
                            "Times a request is tried again after a connection failure, a rate limit (429) or a busy server (5xx), before the reply starts.",
                        );
                        ui.end_row();

                        ui.label("Extra headers");
                        ui.vertical(|ui| {
                            kv_editor(
                                ui,
                                "provider-headers",
                                &mut p.extra_headers,
                                "Header",
                                "Value",
                                false,
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(12.0);
                ui.label(egui::RichText::new("Reasoning and sampling").strong());
                ui.weak(
                    "Unset options leave the server's default. A chat can change them under \
                     the message box.",
                );
                ui.add_space(4.0);
                crate::ui::widgets::model_options_grid(
                    ui,
                    "provider-options",
                    &mut p.options,
                    &Default::default(),
                    p.api,
                );
                let model = p.model.trim().to_owned();
                if !model.is_empty() {
                    ui.add_space(6.0);
                    let mut own = p.model_options.get(&model).cloned().unwrap_or_default();
                    let title = if own.is_empty() {
                        format!("Only for {model}")
                    } else {
                        format!("Only for {model} (changed)")
                    };
                    egui::CollapsingHeader::new(title)
                        .id_salt(("model-options", idx))
                        .show(ui, |ui| {
                            ui.weak("These win over the options above for this model.");
                            crate::ui::widgets::model_options_grid(
                                ui,
                                "model-options-grid",
                                &mut own,
                                &p.options,
                                p.api,
                            );
                        });
                    if own.is_empty() {
                        p.model_options.remove(&model);
                    } else {
                        p.model_options.insert(model, own);
                    }
                }

                ui.add_space(8.0);
                if is_active {
                    ui.weak("● This is the active provider.");
                } else if ui.button("Use this provider").clicked() {
                    make_active = true;
                }
            });
        if make_active {
            draft.active_provider = idx;
        }
    }
}

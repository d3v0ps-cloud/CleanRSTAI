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
        page(ui, |ui| {
            if is_active {
                setting_row(ui, "Active provider", "New chats use this provider", |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}  Active", icon::OK))
                            .color(crate::ui::theme::success(ui.visuals().dark_mode)),
                    );
                });
            } else {
                setting_row(ui, "Active provider", "New chats use another one", |ui| {
                    if ui.button("Use this provider").clicked() {
                        make_active = true;
                    }
                });
            }
            text_row(ui, "Name", "", &mut p.name, "");
            setting_row(ui, "Base URL", "", |ui| {
                egui::ComboBox::from_id_salt("preset")
                    .selected_text("Presets")
                    .width(80.0)
                    .show_ui(ui, |ui| {
                        for (name, url) in PRESETS {
                            if ui.selectable_label(false, *name).clicked() {
                                p.base_url = (*url).to_owned();
                            }
                        }
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut p.base_url)
                        .hint_text("https://api.openai.com/v1")
                        .desired_width(210.0),
                );
            });
            secret_row(
                ui,
                "API key",
                "Leave empty for local servers",
                &mut p.api_key,
                "sk-…",
                &mut self.show_key,
            );
            setting_row(
                ui,
                "Model",
                "Fetch models asks the server for its list",
                |ui| {
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
                    if !p.models.is_empty() {
                        egui::ComboBox::from_id_salt("model-pick")
                            .selected_text(format!("{} models", p.models.len()))
                            .width(90.0)
                            .height(400.0)
                            .show_ui(ui, |ui| {
                                for m in &p.models {
                                    ui.selectable_value(&mut p.model, m.clone(), m);
                                }
                            });
                    }
                    ui.add(egui::TextEdit::singleline(&mut p.model).desired_width(150.0));
                },
            );
            if let Some(err) = &self.models_error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            setting_row(
                ui,
                "Context size",
                "Used when the server doesn't report the model's context size; for compaction \
                 and the context-use figure",
                |ui| {
                    crate::ui::widgets::inherited_value(
                        ui,
                        &mut p.context_tokens,
                        None,
                        8192,
                        1024..=10_000_000,
                        256.0,
                    );
                },
            );

            section(ui, "Server");
            setting_row(ui, "API", p.api.description(), |ui| {
                egui::ComboBox::from_id_salt("provider-api")
                    .selected_text(p.api.label())
                    .show_ui(ui, |ui| {
                        for api in crate::config::ProviderApi::ALL {
                            ui.selectable_value(&mut p.api, api, api.label())
                                .on_hover_text(api.description());
                        }
                    });
            });
            if p.api == crate::config::ProviderApi::Anthropic {
                switch(
                    ui,
                    "Prompt caching",
                    "Cache the system prompt, tools and conversation so far. Later requests read \
                     the cached part at a tenth of the price; writing it costs a quarter more once",
                    &mut p.prompt_caching,
                );
            }
            let kind_about = match &self.detect_result {
                Some(r) => format!("{} {r}", p.kind.description()),
                None => p.kind.description().to_owned(),
            };
            setting_row(ui, "Server type", &kind_about, |ui| {
                let label = if self.detecting_kind {
                    "Detecting…"
                } else {
                    "Detect"
                };
                if ui
                    .add_enabled(!self.detecting_kind, egui::Button::new(label))
                    .on_hover_text("Probe the server to find out what it is")
                    .clicked()
                {
                    self.detecting_kind = true;
                    self.detect_result = None;
                    actions.push(SettingsAction::DetectKind(p.clone()));
                }
                egui::ComboBox::from_id_salt("provider-kind")
                    .selected_text(p.kind.label())
                    .show_ui(ui, |ui| {
                        for kind in crate::config::ProviderKind::ALL {
                            ui.selectable_value(&mut p.kind, kind, kind.label())
                                .on_hover_text(kind.description());
                        }
                    });
            });
            switch(
                ui,
                "Request token usage",
                "In streamed responses (stream_options). Needed for exact token counts and \
                 speeds. Turn off if the server rejects the request",
                &mut p.request_usage,
            );
            switch(
                ui,
                "Stream responses",
                "Turn off if the server doesn't support SSE",
                &mut p.stream,
            );
            setting_row(
                ui,
                "Timeout",
                "How long to wait for the server to start answering, and the longest pause \
                 while a reply streams in",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut p.timeout_secs)
                            .range(10..=3600)
                            .suffix(" s"),
                    );
                },
            );
            setting_row(
                ui,
                "Retries",
                "Times a request is tried again after a connection failure, a rate limit (429) \
                 or a busy server (5xx), before the reply starts",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut p.retries).range(0..=crate::config::MAX_RETRIES),
                    );
                },
            );

            section(ui, "Extra headers");
            ui.add_space(6.0);
            kv_editor(
                ui,
                "provider-headers",
                &mut p.extra_headers,
                "Header",
                "Value",
                false,
            );

            section(ui, "Reasoning and sampling");
            ui.add_space(4.0);
            note(
                ui,
                "Unset options leave the server's default. A chat can change them under the \
                 message box.",
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
        });
        if make_active {
            draft.active_provider = idx;
        }
    }
}

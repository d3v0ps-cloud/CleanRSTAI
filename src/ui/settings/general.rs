//! The General page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn general_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        remote: &RemoteStatus,
        (tunnel, access_error): (&TunnelStatus, Option<&str>),
        actions: &mut Vec<SettingsAction>,
    ) {
        let dirty = &self.draft != live;
        let (draft_remote, live_remote) = (&self.draft.general.remote, &live.general.remote);
        let remote_status = RemoteUiStatus {
            server: remote,
            tunnel,
            server_dirty: crate::remote::server_settings(draft_remote)
                != crate::remote::server_settings(live_remote),
            tunnel_dirty: draft_remote.tunnel != live_remote.tunnel,
            access_error,
        };
        // Applied after the scroll area, which holds a borrow of the draft.
        let mut import_choice: Option<Option<crate::export::ImportMode>> = None;
        let g = &mut self.draft.general;
        let plan_on = g.plan.enabled;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.strong("System prompt");
                ui.add(
                    egui::TextEdit::multiline(&mut g.system_prompt)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);

                egui::Grid::new("general-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Sending");
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut g.send_with_enter,
                                true,
                                "Enter sends, Shift+Enter for newline",
                            );
                            ui.radio_value(
                                &mut g.send_with_enter,
                                false,
                                "Ctrl/⌘+Enter sends, Enter for newline",
                            );
                        });
                        ui.end_row();

                        ui.label("Secrets");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut g.use_keychain,
                                "Store API keys and secrets in the system keychain",
                            )
                            .on_hover_text(
                                "API keys, header values and MCP environment variables are kept in the macOS Keychain, Windows Credential Manager or Secret Service instead of settings.json.",
                            );
                            if g.use_keychain
                                && let Err(e) = crate::secrets::OsKeychain::status()
                            {
                                ui.colored_label(
                                    ui.visuals().warn_fg_color,
                                    format!("Keychain unavailable ({e}); secrets will stay in settings.json."),
                                );
                            }
                        });
                        ui.end_row();

                        remote_rows(ui, &mut g.remote, &mut self.show_remote_password, &remote_status);

                        ui.label("Updates");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut g.check_for_updates,
                                "Check GitHub for a new release at startup",
                            );
                            ui.horizontal(|ui| {
                                ui.label("Update from");
                                ui.add(
                                    egui::TextEdit::singleline(&mut g.update_url)
                                        .hint_text("owner/repo or a releases API URL")
                                        .desired_width(380.0),
                                )
                                .on_hover_text(
                                    "A GitHub owner/repo, a github.com link, or a URL that \
                                     answers like GitHub's releases/latest API",
                                );
                                if g.update_url != crate::update::LATEST_RELEASE_URL
                                    && ui.button("Reset").clicked()
                                {
                                    g.update_url = crate::update::LATEST_RELEASE_URL.into();
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !self.checking_update,
                                        egui::Button::new("Check for updates now"),
                                    )
                                    .clicked()
                                {
                                    self.checking_update = true;
                                    self.update_status = None;
                                    actions.push(SettingsAction::CheckForUpdates(
                                        g.update_url.clone(),
                                    ));
                                }
                                if self.checking_update {
                                    ui.spinner();
                                }
                                ui.weak(concat!("Installed: v", env!("CARGO_PKG_VERSION")));
                            });
                            match &self.update_status {
                                Some(Ok(m)) => {
                                    ui.horizontal(|ui| {
                                        ui.label(m);
                                        if self.update_available
                                            && ui.button("Install update").clicked()
                                        {
                                            self.update_available = false;
                                            actions.push(SettingsAction::InstallUpdate);
                                        }
                                    });
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, e);
                                }
                                None => {}
                            }
                        });
                        ui.end_row();

                        ui.label("Web page preview");
                        let hp = &mut g.html_preview;
                        ui.vertical(|ui| {
                            ui.add_enabled_ui(
                                crate::ui::webview::SUPPORTED || crate::ui::webview::OWN_WINDOW,
                                |ui| {
                                ui.checkbox(&mut hp.enabled, "Show web pages live in the panel")
                                    .on_hover_text(
                                        "Off: the panel shows the code and \"Open in browser\" \
                                         opens the page",
                                    );
                                ui.add_enabled_ui(hp.enabled, |ui| {
                                    ui.checkbox(&mut hp.scripts, "Let pages run their scripts");
                                    ui.checkbox(
                                        &mut hp.network,
                                        "Let pages load from the network",
                                    )
                                    .on_hover_text(
                                        "Off: scripts, styles, images and requests from the web \
                                         are blocked, so a page can't send chat content anywhere. \
                                         Pages that use a CDN library need this on.",
                                    );
                                });
                            });
                            #[cfg(target_os = "linux")]
                            weak_note(ui, &crate::ui::linux_preview::status());
                            #[cfg(not(target_os = "linux"))]
                            if !crate::ui::webview::SUPPORTED {
                                weak_note(
                                    ui,
                                    "Needs the macOS or Windows app; on Linux web pages open in \
                                     your browser.",
                                );
                            }
                        });
                        ui.end_row();

                        ui.label("Save as files");
                        ui.vertical(|ui| {
                            ui.checkbox(&mut g.save_as.docx, "Word document (.docx)");
                            ui.checkbox(&mut g.save_as.pdf, "PDF (.pdf)");
                            ui.checkbox(&mut g.save_as.xlsx, "Excel workbook (.xlsx)")
                                .on_hover_text(
                                    "Offered for replies with a table and for CSV or TSV \
                                     artifacts",
                                );
                            weak_note(
                                ui,
                                "The Save as menu on a reply or artifact offers the checked \
                                 formats.",
                            );
                        });
                        ui.end_row();

                        ui.label("Tool calls");
                        ui.checkbox(
                            &mut g.auto_approve_tools,
                            "Run MCP tools and subagents without asking for approval",
                        );
                        ui.end_row();

                        ui.label("New chats start in");
                        egui::ComboBox::from_id_salt("default-approval-mode")
                            .selected_text(g.default_approval_mode.label())
                            .show_ui(ui, |ui| {
                                for m in ApprovalMode::offered(plan_on) {
                                    ui.selectable_value(&mut g.default_approval_mode, m, m.label())
                                        .on_hover_text(m.description());
                                }
                            });
                        ui.end_row();

                        ui.label("Max tool rounds");
                        ui.add(egui::DragValue::new(&mut g.max_tool_rounds).range(1..=200))
                            .on_hover_text(
                                "How many times the model may call tools before it must answer",
                            );
                        ui.end_row();

                        ui.label("Reply limits");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut g.reply_token_limit)
                                    .range(0..=100_000_000)
                                    .speed(1000.0)
                                    .suffix(" tokens"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut g.reply_cost_limit_usd)
                                    .range(0.0..=10_000.0)
                                    .speed(0.01)
                                    .max_decimals(2)
                                    .prefix("$"),
                            );
                            ui.weak("0: no limit");
                        })
                        .response
                        .on_hover_text(
                            "Stop a reply's tool calls once its requests used this many tokens, or cost this much, in all",
                        );
                        ui.end_row();

                        ui.label("Tool timeout");
                        ui.add(
                            egui::DragValue::new(&mut g.tool_timeout_secs)
                                .range(5..=3600)
                                .suffix(" s"),
                        );
                        ui.end_row();
                    });

                ui.add_space(16.0);
                ui.separator();
                ui.strong("Plan mode");
                ui.checkbox(&mut g.plan.enabled, "Offer Plan in the permission menus")
                    .on_hover_text(
                        "A Plan chat investigates with read-only tools and proposes a numbered \
                         plan. Off, Plan is hidden, and a chat set to it is read only.",
                    );
                ui.add_enabled_ui(g.plan.enabled, |ui| {
                    egui::Grid::new("plan-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Mode after approval");
                            egui::ComboBox::from_id_salt("plan-after-approval")
                                .selected_text(g.plan.after_mode().label())
                                .show_ui(ui, |ui| {
                                    for m in [
                                        ApprovalMode::Default,
                                        ApprovalMode::AutoWorkspace,
                                        ApprovalMode::AllowAll,
                                    ] {
                                        ui.selectable_value(&mut g.plan.after_approval, m, m.label())
                                            .on_hover_text(m.description());
                                    }
                                });
                            ui.end_row();
                            ui.label("Approval message");
                            ui.add(
                                egui::TextEdit::singleline(&mut g.plan.approve_message)
                                    .desired_width(FIELD_WIDTH),
                            )
                            .on_hover_text("Sent as the user's message when a plan is approved");
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    weak_note(
                        ui,
                        "Under a finished plan, Approve plan switches the chat to the mode \
                         above and sends the approval message. Plan and Read only can't be the \
                         mode after approval.",
                    );
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("Plan instructions");
                    if g.plan.prompt != crate::plan::PLAN_PROMPT
                        && ui.small_button("Reset").clicked()
                    {
                        g.plan.prompt = crate::plan::PLAN_PROMPT.into();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut g.plan.prompt)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);
                ui.separator();
                ui.strong("Branches");
                ui.checkbox(&mut g.branching.enabled, "Keep the version a reply replaces")
                    .on_hover_text(
                        "Regenerating a reply, or editing a message, keeps the version it \
                         replaces as a branch, with ‹ k/N › arrows to switch between them. Off, \
                         the replaced version is dropped. Versions already kept stay switchable. \
                         A chat's Branches switch overrides this.",
                    );
                ui.add_enabled_ui(g.branching.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Versions kept per message");
                        ui.add(
                            egui::DragValue::new(&mut g.branching.max_per_message).range(1..=50),
                        );
                    });
                    weak_note(
                        ui,
                        "Past this many, the oldest version of a message is dropped. Files \
                         changed by a version are only undone while it is the one shown.",
                    );
                });
                ui.add_space(16.0);
                ui.separator();
                ui.strong("Export & import");
                ui.horizontal(|ui| {
                    if ui
                        .button("Export settings…")
                        .on_hover_text("Providers, MCP servers and preferences, as saved")
                        .clicked()
                    {
                        actions.push(SettingsAction::ExportSettings {
                            include_secrets: self.export_include_secrets,
                        });
                    }
                    ui.checkbox(&mut self.export_include_secrets, "Include API keys and secrets");
                });
                if self.export_include_secrets {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "The file will contain API keys, header values and MCP environment variables in plain text.",
                    );
                } else {
                    ui.weak("API keys, header values and MCP environment variable values are left blank.");
                }
                if dirty {
                    ui.weak("Unsaved changes are not included; Apply first to export them.");
                }
                if ui
                    .button("Export conversations…")
                    .on_hover_text("All chats, including tool calls and results")
                    .clicked()
                {
                    actions.push(SettingsAction::ExportConversations);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Import settings…")
                        .on_hover_text("Load a settings export into this window for review")
                        .clicked()
                    {
                        actions.push(SettingsAction::ImportSettings);
                    }
                    if ui
                        .button("Import conversations…")
                        .on_hover_text("Add chats from a conversations export; existing chats are kept")
                        .clicked()
                    {
                        actions.push(SettingsAction::ImportConversations);
                    }
                });
                if let Some(incoming) = &self.pending_import {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.label(format!(
                            "Import {} provider(s) and {} MCP server(s)?",
                            incoming.providers.len(),
                            incoming.mcp_servers.len()
                        ));
                        ui.weak("Merge adds them, updating entries that already exist, and keeps your other settings. Replace uses the imported settings as they are.");
                        ui.horizontal(|ui| {
                            if ui.button("Merge").clicked() {
                                import_choice = Some(Some(crate::export::ImportMode::Merge));
                            }
                            if ui.button("Replace").clicked() {
                                import_choice = Some(Some(crate::export::ImportMode::Replace));
                            }
                            if ui.button("Cancel").clicked() {
                                import_choice = Some(None);
                            }
                        });
                    });
                }
                match &self.export_status {
                    Some(Ok(msg)) => {
                        ui.colored_label(egui::Color32::from_rgb(60, 170, 90), msg);
                    }
                    Some(Err(err)) => {
                        ui.colored_label(ui.visuals().error_fg_color, err);
                    }
                    None => {}
                }

                ui.add_space(16.0);
                ui.separator();
                ui.weak(format!("Settings file: {}", Settings::path().display()));
                ui.weak(format!(
                    "Conversations: {}",
                    crate::config::data_dir().join("conversations").display()
                ));
            });

        if let Some(choice) = import_choice
            && let Some(incoming) = self.pending_import.take()
        {
            if let Some(mode) = choice {
                self.draft = crate::export::apply_settings_import(&self.draft, *incoming, mode);
                let missing = crate::export::missing_secrets(&self.draft);
                let mut msg = "Imported. Review the settings, then Apply or Save.".to_owned();
                if !missing.is_empty() {
                    msg.push_str(&format!(" Still blank: {}.", missing.join(", ")));
                }
                self.export_status = Some(Ok(msg));
            } else {
                self.export_status = None;
            }
        }
    }
}

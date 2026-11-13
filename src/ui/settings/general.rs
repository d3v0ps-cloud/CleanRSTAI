//! The General, Permissions and Data & privacy pages of Settings.

use super::*;

impl SettingsUi {
    /// The system prompt, your name, the send key and updates.
    pub(super) fn general_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let g = &mut self.draft.general;
        page(ui, |ui| {
            long_text(
                ui,
                "System prompt",
                "Sent at the start of every chat.",
                &mut g.system_prompt,
                5,
                None,
            );
            text_row(
                ui,
                "Your name",
                "For the greeting and the sidebar; empty uses your account name",
                &mut g.display_name,
                "Account name",
            );
            choice(
                ui,
                "send-key",
                "Send messages with",
                "The other one starts a new line",
                &mut g.send_with_enter,
                &[
                    (true, "Enter (Shift+Enter for a new line)"),
                    (false, "Ctrl/⌘+Enter (Enter for a new line)"),
                ],
            );

            section(ui, "Updates");
            switch(
                ui,
                "Check for updates at startup",
                "Asks the release source below for a newer version when the app starts",
                &mut g.check_for_updates,
            );
            setting_row(
                ui,
                "Update from",
                "A GitHub owner/repo, a github.com link, or a URL that answers like GitHub's \
                 releases/latest API",
                |ui| {
                    if g.update_url != crate::update::LATEST_RELEASE_URL
                        && ui.button("Reset").clicked()
                    {
                        g.update_url = crate::update::LATEST_RELEASE_URL.into();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut g.update_url)
                            .hint_text("owner/repo or a releases API URL")
                            .desired_width(250.0),
                    );
                },
            );
            setting_row(
                ui,
                "Installed version",
                concat!("v", env!("CARGO_PKG_VERSION")),
                |ui| {
                    if ui
                        .add_enabled(!self.checking_update, egui::Button::new("Check now"))
                        .clicked()
                    {
                        self.checking_update = true;
                        self.update_status = None;
                        actions.push(SettingsAction::CheckForUpdates(g.update_url.clone()));
                    }
                    if self.checking_update {
                        ui.spinner();
                    }
                },
            );
            match &self.update_status {
                Some(Ok(m)) => {
                    ui.horizontal(|ui| {
                        ui.label(m);
                        if self.update_available && ui.button("Install update").clicked() {
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
            version_picker(
                ui,
                &self.releases,
                &mut self.listing_releases,
                &mut self.chosen_release,
                &g.update_url,
                actions,
            );
        });
    }

    /// What runs without asking, how far a reply may go, and plan mode.
    pub(super) fn permissions_tab(&mut self, ui: &mut egui::Ui) {
        let g = &mut self.draft.general;
        let plan_on = g.plan.enabled;
        page(ui, |ui| {
            setting_row(
                ui,
                "New chats start in",
                "The permission mode of a new chat; each chat can change its own under the \
                 message box",
                |ui| {
                    egui::ComboBox::from_id_salt("default-approval-mode")
                        .selected_text(g.default_approval_mode.label())
                        .show_ui(ui, |ui| {
                            for m in ApprovalMode::offered(plan_on) {
                                ui.selectable_value(&mut g.default_approval_mode, m, m.label())
                                    .on_hover_text(m.description());
                            }
                        });
                },
            );
            switch(
                ui,
                "Run MCP tools and subagents without asking",
                "Off: each call asks for approval first, in chats in the Default mode",
                &mut g.auto_approve_tools,
            );

            section(ui, "Limits");
            setting_row(
                ui,
                "Tool rounds per reply",
                "How many times the model may call tools before it must answer",
                |ui| {
                    ui.add(egui::DragValue::new(&mut g.max_tool_rounds).range(1..=200));
                },
            );
            setting_row(
                ui,
                "Token limit per reply",
                "Stops a reply's tool calls once its requests used this many tokens in all. \
                 0: no limit",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut g.reply_token_limit)
                            .range(0..=100_000_000)
                            .speed(1000.0)
                            .suffix(" tokens"),
                    );
                },
            );
            setting_row(
                ui,
                "Cost limit per reply",
                "The same, by what the requests cost. 0: no limit",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut g.reply_cost_limit_usd)
                            .range(0.0..=10_000.0)
                            .speed(0.01)
                            .max_decimals(2)
                            .prefix("$"),
                    );
                },
            );
            setting_row(
                ui,
                "Tool timeout",
                "A tool call that takes longer is stopped",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut g.tool_timeout_secs)
                            .range(5..=3600)
                            .suffix(" s"),
                    );
                },
            );

            section(ui, "Plan mode");
            switch(
                ui,
                "Offer Plan in the permission menus",
                "A Plan chat investigates with read-only tools and proposes a numbered plan. \
                 Off, Plan is hidden, and a chat set to it is read only",
                &mut g.plan.enabled,
            );
            ui.add_enabled_ui(g.plan.enabled, |ui| {
                setting_row(
                    ui,
                    "Mode after approval",
                    "Approve plan, under a finished plan, switches the chat to this mode. Plan \
                     and Read only can't be chosen",
                    |ui| {
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
                    },
                );
                text_row(
                    ui,
                    "Approval message",
                    "Sent as your message when a plan is approved",
                    &mut g.plan.approve_message,
                    "",
                );
            });
            long_text(
                ui,
                "Plan instructions",
                "Added to the system prompt of a chat in Plan mode.",
                &mut g.plan.prompt,
                4,
                Some(crate::plan::PLAN_PROMPT),
            );
        });
    }

    /// Where secrets are kept, what replies may load, export and import, and the file paths.
    pub(super) fn privacy_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        actions: &mut Vec<SettingsAction>,
    ) {
        let dirty = &self.draft != live;
        // Applied after the scroll area, which holds a borrow of the draft.
        let mut import_choice: Option<Option<crate::export::ImportMode>> = None;
        let g = &mut self.draft.general;
        page(ui, |ui| {
            switch(
                ui,
                "Keep secrets in the system keychain",
                "API keys, header values and MCP environment variables go to the macOS \
                 Keychain, Windows Credential Manager or Secret Service instead of settings.json",
                &mut g.use_keychain,
            );
            if g.use_keychain
                && let Err(e) = crate::secrets::OsKeychain::status()
            {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("Keychain unavailable ({e}); secrets will stay in settings.json."),
                );
            }
            switch(
                ui,
                "Load web images in replies",
                "Off: they show as links. Loading one tells its server the chat was opened.",
                &mut g.load_remote_images,
            );

            section(ui, "Export");
            setting_row(
                ui,
                "Settings",
                "Providers, MCP servers and preferences, as saved",
                |ui| {
                    if ui.button("Export settings…").clicked() {
                        actions.push(SettingsAction::ExportSettings {
                            include_secrets: self.export_include_secrets,
                        });
                    }
                },
            );
            switch(
                ui,
                "Include API keys and secrets",
                "Off: API keys, header values and MCP environment variable values are left blank",
                &mut self.export_include_secrets,
            );
            if self.export_include_secrets {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "The file will contain API keys, header values and MCP environment variables \
                     in plain text.",
                );
            }
            if dirty {
                note(
                    ui,
                    "Unsaved changes are not included; Apply first to export them.",
                );
            }
            setting_row(
                ui,
                "Conversations",
                "All chats, including tool calls and results",
                |ui| {
                    if ui.button("Export conversations…").clicked() {
                        actions.push(SettingsAction::ExportConversations);
                    }
                },
            );

            section(ui, "Import");
            setting_row(
                ui,
                "Settings",
                "Loads a settings export into this window for review",
                |ui| {
                    if ui.button("Import settings…").clicked() {
                        actions.push(SettingsAction::ImportSettings);
                    }
                },
            );
            setting_row(
                ui,
                "Conversations",
                "Adds chats from a conversations export; existing chats are kept",
                |ui| {
                    if ui.button("Import conversations…").clicked() {
                        actions.push(SettingsAction::ImportConversations);
                    }
                },
            );
            if let Some(incoming) = &self.pending_import {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(format!(
                        "Import {} provider(s) and {} MCP server(s)?",
                        incoming.providers.len(),
                        incoming.mcp_servers.len()
                    ));
                    ui.weak(
                        "Merge adds them, updating entries that already exist, and keeps your \
                         other settings. Replace uses the imported settings as they are.",
                    );
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
            section(ui, "Delete");
            let counts = self.chat_counts;
            setting_row(
                ui,
                "Delete all chats",
                "Removes the chats and their files from this computer; this can't be undone",
                |ui| {
                    let button = egui::Button::new("Delete all chats…");
                    if ui
                        .add_enabled(counts.all > 0 && self.delete_chats.is_none(), button)
                        .clicked()
                    {
                        self.delete_chats = Some(super::DeleteChats {
                            keep_pinned: true,
                            keep_projects: true,
                        });
                    }
                },
            );
            let mut close_delete = false;
            if let Some(choice) = &mut self.delete_chats {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.checkbox(
                        &mut choice.keep_pinned,
                        format!("Keep pinned chats ({})", counts.pinned),
                    );
                    ui.checkbox(
                        &mut choice.keep_projects,
                        format!("Keep chats in projects ({})", counts.in_project),
                    );
                    let n = counts.deleted(choice.keep_pinned, choice.keep_projects);
                    ui.label(format!(
                        "Delete {n} chat{}? Replies still running in them are stopped. This \
                         can't be undone; Export conversations first to keep a copy.",
                        if n == 1 { "" } else { "s" }
                    ));
                    ui.horizontal(|ui| {
                        let delete = egui::Button::new(
                            egui::RichText::new(format!("Delete {n}"))
                                .color(ui.visuals().error_fg_color),
                        );
                        if ui.add_enabled(n > 0, delete).clicked() {
                            actions.push(SettingsAction::DeleteAllChats {
                                keep_pinned: choice.keep_pinned,
                                keep_projects: choice.keep_projects,
                            });
                            close_delete = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close_delete = true;
                        }
                    });
                });
            }
            if close_delete {
                self.delete_chats = None;
            }

            match &self.export_status {
                Some(Ok(msg)) => {
                    ui.colored_label(crate::ui::theme::success(ui.visuals().dark_mode), msg);
                }
                Some(Err(err)) => {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
                None => {}
            }

            section(ui, "Where things are kept");
            ui.add_space(4.0);
            note(
                ui,
                &format!("Settings file: {}", Settings::path().display()),
            );
            note(
                ui,
                &format!(
                    "Conversations: {}",
                    crate::config::data_dir().join("conversations").display()
                ),
            );
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

/// "Install a version": every release (dev builds too) in a list, and Install for the
/// one picked, to move to a particular build or back to an older one.
fn version_picker(
    ui: &mut egui::Ui,
    releases: &Option<Result<Vec<crate::update::Release>, String>>,
    listing: &mut bool,
    chosen_release: &mut Option<String>,
    update_url: &str,
    actions: &mut Vec<SettingsAction>,
) {
    const CURRENT: &str = env!("CARGO_PKG_VERSION");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Install a version");
        let label = |r: &crate::update::Release| {
            let mut l = format!("v{}", r.version);
            if r.prerelease {
                l.push_str("  · dev");
            }
            if r.version == CURRENT {
                l.push_str("  · installed");
            }
            l
        };
        match releases {
            Some(Ok(list)) if !list.is_empty() => {
                let chosen = chosen_release
                    .as_ref()
                    .and_then(|v| list.iter().find(|r| &r.version == v))
                    .cloned();
                egui::ComboBox::from_id_salt("release-picker")
                    .width(220.0)
                    .selected_text(
                        chosen
                            .as_ref()
                            .map_or_else(|| "Choose a release…".to_owned(), label),
                    )
                    .show_ui(ui, |ui| {
                        for r in list {
                            let on = chosen_release.as_deref() == Some(r.version.as_str());
                            if ui.selectable_label(on, label(r)).clicked() {
                                *chosen_release = Some(r.version.clone());
                            }
                        }
                    });
                let installable = chosen
                    .as_ref()
                    .filter(|r| r.version != CURRENT)
                    .filter(|r| crate::update::can_install(r));
                let text = chosen.as_ref().map_or_else(
                    || "Install".to_owned(),
                    |r| format!("Install v{}", r.version),
                );
                let resp = ui.add_enabled(installable.is_some(), egui::Button::new(text));
                let resp = match &chosen {
                    Some(r) if r.version == CURRENT => {
                        resp.on_disabled_hover_text("This version is installed")
                    }
                    Some(_) if installable.is_none() => resp.on_disabled_hover_text(
                        "This release has no build this copy of the app can install itself; \
                         download it from the release page",
                    ),
                    _ => resp,
                };
                if resp.clicked()
                    && let Some(r) = installable
                {
                    actions.push(SettingsAction::InstallRelease(Box::new(r.clone())));
                }
                if let Some(r) = &chosen {
                    ui.hyperlink_to("Release notes", &r.url);
                }
            }
            _ => {
                if ui
                    .add_enabled(!*listing, egui::Button::new("List releases"))
                    .on_hover_text(
                        "Every release from the \"Update from\" source, dev builds included",
                    )
                    .clicked()
                {
                    *listing = true;
                    actions.push(SettingsAction::ListReleases(update_url.to_owned()));
                }
            }
        }
        if *listing {
            ui.spinner();
        }
    });
    match releases {
        Some(Err(e)) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Couldn't list releases: {e}"),
            );
        }
        Some(Ok(list)) if list.is_empty() => {
            ui.weak("No releases found.");
        }
        _ => {}
    }
    if let (Some(Ok(list)), Some(v)) = (releases, &*chosen_release)
        && list.iter().any(|r| &r.version == v)
        && crate::update::compare_versions(v, CURRENT) == std::cmp::Ordering::Less
    {
        ui.weak(
            "An older version may not know settings added since; they keep their defaults \
             there, and are lost if that version saves the settings. Before v0.9.3, images \
             in chats saved since don't show, and a chat it saves loses them.",
        );
    }
    let picked = chosen_release.as_ref().and_then(|v| {
        releases
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|r| &r.version == v)
    });
    if picked.is_some_and(|r| r.prerelease) {
        ui.weak(
            "Dev builds are previews and may have bugs. The automatic update check skips \
             them, so you stay on this one until you pick another or a newer stable release \
             comes out.",
        );
    }
}

//! The MCP servers page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn mcp_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        servers: &[ServerView],
        actions: &mut Vec<SettingsAction>,
    ) {
        let draft = &mut self.draft;
        let view_of = |id: &str| servers.iter().find(|v| v.id == id);

        egui::Panel::left("server-list")
            .resizable(false)
            .exact_size(200.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(ui.available_height() - 64.0)
                    .show(ui, |ui| {
                        if draft.mcp_servers.is_empty() {
                            ui.weak("No servers yet.");
                        }
                        for (i, s) in draft.mcp_servers.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let color = if s.enabled {
                                    status_color(ui, view_of(&s.id).map(|v| &v.status))
                                } else {
                                    ui.visuals().weak_text_color()
                                };
                                ui.colored_label(color, "●");
                                if ui
                                    .selectable_label(i == self.selected_server, &s.name)
                                    .clicked()
                                {
                                    self.selected_server = i;
                                    self.import_open = false;
                                }
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Add", icon::PLUS)).clicked() {
                        draft.mcp_servers.push(McpServerConfig::default());
                        self.selected_server = draft.mcp_servers.len() - 1;
                        self.import_open = false;
                    }
                    if ui
                        .add_enabled(
                            !draft.mcp_servers.is_empty(),
                            egui::Button::new("－ Remove"),
                        )
                        .clicked()
                    {
                        draft.mcp_servers.remove(self.selected_server);
                        self.selected_server = self.selected_server.saturating_sub(1);
                    }
                });
                if ui
                    .button("Import JSON…")
                    .on_hover_text("Paste a Claude Desktop / Cursor style mcpServers config")
                    .clicked()
                {
                    self.import_open = !self.import_open;
                }
            });

        if self.import_open {
            ui.heading("Import servers");
            ui.label("Paste JSON in the common `mcpServers` format:");
            ui.add(
                egui::TextEdit::multiline(&mut self.import_text)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text("{\n  \"mcpServers\": {\n    \"filesystem\": {\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/Users/me\"]\n    },\n    \"remote\": { \"url\": \"https://example.com/mcp\" }\n  }\n}"),
            );
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    match import_mcp_json(&self.import_text) {
                        Ok(list) => {
                            let n = list.len();
                            draft.mcp_servers.extend(list);
                            self.selected_server = draft.mcp_servers.len().saturating_sub(n);
                            self.import_open = false;
                            self.import_text.clear();
                            self.import_error = None;
                        }
                        Err(e) => self.import_error = Some(format!("{e:#}")),
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.import_open = false;
                }
            });
            if let Some(e) = &self.import_error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            return;
        }

        if draft.mcp_servers.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label("MCP servers give the model tools: files, databases, web, and more.");
                ui.weak("Add one manually, or import an existing mcpServers JSON config.");
            });
            return;
        }
        self.selected_server = self.selected_server.min(draft.mcp_servers.len() - 1);
        let s = &mut draft.mcp_servers[self.selected_server];
        let view = view_of(&s.id);
        let live_cfg = live.mcp_servers.iter().find(|c| c.id == s.id);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("mcp-grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut s.name)
                            .on_hover_text("Used to prefix tool names shown to the model");
                        ui.end_row();

                        ui.label("Enabled");
                        ui.checkbox(&mut s.enabled, "");
                        ui.end_row();

                        ui.label("Transport");
                        ui.horizontal(|ui| {
                            let is_stdio = matches!(s.transport, McpTransportConfig::Stdio { .. });
                            if ui.radio(is_stdio, "stdio (local process)").clicked() && !is_stdio {
                                s.transport = McpTransportConfig::default();
                            }
                            if ui.radio(!is_stdio, "Streamable HTTP").clicked() && is_stdio {
                                s.transport = McpTransportConfig::Http {
                                    url: String::new(),
                                    headers: Vec::new(),
                                    oauth_client_id: String::new(),
                                };
                            }
                        });
                        ui.end_row();

                        match &mut s.transport {
                            McpTransportConfig::Stdio {
                                command,
                                args,
                                env,
                                cwd,
                            } => {
                                ui.label("Command");
                                ui.add(
                                    egui::TextEdit::singleline(command)
                                        .hint_text("npx, uvx, docker, /path/to/server")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();
                                ui.label("Arguments");
                                ui.vertical(|ui| list_editor(ui, "args", args, "argument"));
                                ui.end_row();
                                ui.label("Environment");
                                ui.vertical(|ui| kv_editor(ui, "env", env, "NAME", "value", false));
                                ui.end_row();
                                ui.label("Working dir");
                                ui.add(
                                    egui::TextEdit::singleline(cwd)
                                        .hint_text("(optional)")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();
                            }
                            McpTransportConfig::Http {
                                url,
                                headers,
                                oauth_client_id,
                            } => {
                                ui.label("URL");
                                ui.add(
                                    egui::TextEdit::singleline(url)
                                        .hint_text("https://example.com/mcp")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();
                                ui.label("Headers");
                                ui.vertical(|ui| {
                                    kv_editor(
                                        ui,
                                        "headers",
                                        headers,
                                        "Authorization",
                                        "Bearer …",
                                        true,
                                    )
                                });
                                ui.end_row();
                                ui.label("OAuth client ID");
                                ui.add(
                                    egui::TextEdit::singleline(oauth_client_id)
                                        .hint_text("(optional) only if the server can't register CleanRSTAI automatically")
                                        .desired_width(FIELD_WIDTH),
                                )
                                .on_hover_text(
                                    "Servers that require sign-in usually register CleanRSTAI automatically. Set this only if the server's documentation gives you a client ID.",
                                );
                                ui.end_row();
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                let unsaved = live_cfg.is_none_or(|l| l.needs_reconnect(s));
                ui.horizontal(|ui| {
                    ui.strong("Status:");
                    match (live_cfg, view) {
                        _ if unsaved => {
                            ui.weak("Apply settings to connect with this configuration.");
                        }
                        (_, Some(v)) => {
                            let color = status_color(ui, Some(&v.status));
                            let text = match &v.status {
                                ServerStatus::Disabled => "Disabled".to_owned(),
                                ServerStatus::Connecting => "Connecting…".to_owned(),
                                ServerStatus::Connected => format!(
                                    "Connected · {} tools · {}",
                                    v.tools.len(),
                                    v.server_info
                                ),
                                ServerStatus::NeedsSignIn => "Sign-in required".to_owned(),
                                ServerStatus::SigningIn => {
                                    "Waiting for you to sign in in your browser…".to_owned()
                                }
                                ServerStatus::Error(_) => "Error".to_owned(),
                            };
                            ui.colored_label(color, text);
                            if matches!(
                                v.status,
                                ServerStatus::Connecting | ServerStatus::SigningIn
                            ) {
                                ui.spinner();
                            }
                        }
                        _ => {
                            ui.weak("Not connected");
                        }
                    }
                    if !unsaved && s.enabled {
                        let is_http = matches!(s.transport, McpTransportConfig::Http { .. });
                        let status = view.map(|v| &v.status);
                        if is_http
                            && matches!(
                                status,
                                Some(ServerStatus::NeedsSignIn | ServerStatus::Error(_))
                            )
                            && ui
                                .button(format!("{}  Sign in…", icon::SIGN_IN))
                                .on_hover_text("Opens your browser to sign in to this server")
                                .clicked()
                        {
                            actions.push(SettingsAction::SignIn(s.id.clone()));
                        }
                        if is_http
                            && view.is_some_and(|v| v.signed_in)
                            && ui.button("Sign out").clicked()
                        {
                            actions.push(SettingsAction::SignOut(s.id.clone()));
                        }
                        if ui.button("Reconnect").clicked() {
                            actions.push(SettingsAction::Reconnect(s.id.clone()));
                        }
                        if view.is_some_and(|v| v.status == ServerStatus::Connected)
                            && ui.button("Refresh").clicked()
                        {
                            actions.push(SettingsAction::RefreshTools(s.id.clone()));
                        }
                    }
                });

                if let Some(v) = view.filter(|_| !unsaved) {
                    if let ServerStatus::Error(e) = &v.status {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    if !v.tools.is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Tools");
                        ui.weak("Unchecked tools are not offered to the model.");
                        for t in &v.tools {
                            let mut on = !s.disabled_tools.contains(&t.name);
                            let resp = ui.checkbox(&mut on, &t.name);
                            let resp = if t.description.is_empty() {
                                resp
                            } else {
                                resp.on_hover_text(&t.description)
                            };
                            if resp.changed() {
                                if on {
                                    s.disabled_tools.retain(|n| n != &t.name);
                                } else {
                                    s.disabled_tools.push(t.name.clone());
                                }
                            }
                        }
                    }
                    if !v.resources.is_empty() {
                        egui::CollapsingHeader::new(format!("Resources ({})", v.resources.len()))
                            .id_salt(("resources", &s.id))
                            .show(ui, |ui| {
                                ui.weak(format!("Attach these to a message from the {} menu next to the input.", icon::BOOKS));
                                for r in &v.resources {
                                    ui.label(&r.name).on_hover_text(if r.description.is_empty() {
                                        r.uri.clone()
                                    } else {
                                        format!("{}\n{}", r.description, r.uri)
                                    });
                                }
                            });
                    }
                    if !v.prompts.is_empty() {
                        egui::CollapsingHeader::new(format!("Prompts ({})", v.prompts.len()))
                            .id_salt(("prompts", &s.id))
                            .show(ui, |ui| {
                                ui.weak(format!("Insert these from the {} menu next to the input.", icon::BOOKS));
                                for p in &v.prompts {
                                    let args: Vec<&str> =
                                        p.arguments.iter().map(|a| a.name.as_str()).collect();
                                    let label = if args.is_empty() {
                                        p.name.clone()
                                    } else {
                                        format!("{} ({})", p.name, args.join(", "))
                                    };
                                    ui.label(label).on_hover_text(&p.description);
                                }
                            });
                    }
                    if !v.stderr.is_empty() {
                        egui::CollapsingHeader::new("Server log (stderr)")
                            .id_salt(("stderr", &s.id))
                            .show(ui, |ui| {
                                let start = v.stderr.len().saturating_sub(60);
                                code_block(ui, &v.stderr[start..].join("\n"));
                            });
                    }
                }
            });
    }
}

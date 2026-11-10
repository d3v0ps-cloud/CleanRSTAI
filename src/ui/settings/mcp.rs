//! The MCP servers page of Settings.

use super::*;
use crate::config::RECOMMENDED_MCP;

/// Room kept under the server list for the Recommended rows.
const RECOMMENDED_HEIGHT: f32 = 110.0;

/// The Recommended servers, each with Add, or "Added" once one of `servers` has its URL.
/// `wide` shows each one's description too. Returns the index of a server it just added.
fn recommended(ui: &mut egui::Ui, servers: &mut Vec<McpServerConfig>, wide: bool) -> Option<usize> {
    let mut added = None;
    ui.label(egui::RichText::new("Recommended").strong());
    for r in RECOMMENDED_MCP {
        let have = r.is_added(servers);
        ui.horizontal(|ui| {
            let hover = format!("{}\n{}", r.description, r.url);
            if have {
                ui.add_enabled(false, egui::Button::new(format!("{}  Added", icon::OK)))
                    .on_disabled_hover_text("Already in your servers");
            } else if ui
                .button(format!("{}  Add", icon::PLUS))
                .on_hover_text(&hover)
                .clicked()
            {
                servers.push(r.server(servers));
                added = Some(servers.len() - 1);
            }
            ui.label(r.title).on_hover_text(&hover);
        });
        if wide {
            ui.weak(r.description);
            ui.add_space(4.0);
        }
    }
    added
}

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
                    .max_height(ui.available_height() - 64.0 - RECOMMENDED_HEIGHT)
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
                ui.horizontal_wrapped(|ui| {
                    if ui.button(format!("{}  Add", icon::PLUS)).clicked() {
                        draft.mcp_servers.push(McpServerConfig::default());
                        self.selected_server = draft.mcp_servers.len() - 1;
                        self.import_open = false;
                    }
                    if ui
                        .add_enabled(
                            !draft.mcp_servers.is_empty(),
                            egui::Button::new(format!("{}  Clone", icon::COPY)),
                        )
                        .on_hover_text(
                            "Add a copy of the selected server, with its settings and secrets",
                        )
                        .clicked()
                    {
                        let i = self.selected_server.min(draft.mcp_servers.len() - 1);
                        let copy = draft.mcp_servers[i]
                            .duplicate(draft.mcp_servers.iter().map(|s| s.name.as_str()));
                        draft.mcp_servers.insert(i + 1, copy);
                        self.selected_server = i + 1;
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
                ui.add_space(10.0);
                if let Some(i) = recommended(ui, &mut draft.mcp_servers, false) {
                    self.selected_server = i;
                    self.import_open = false;
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
                ui.weak("Add one manually, import an existing mcpServers JSON config, or start with a recommended one:");
            });
            ui.add_space(12.0);
            if let Some(i) = recommended(ui, &mut draft.mcp_servers, true) {
                self.selected_server = i;
            }
            return;
        }
        self.selected_server = self.selected_server.min(draft.mcp_servers.len() - 1);
        let s = &mut draft.mcp_servers[self.selected_server];
        let view = view_of(&s.id);
        let live_cfg = live.mcp_servers.iter().find(|c| c.id == s.id);

        page(ui, |ui| {
            text_row(
                ui,
                "Name",
                "Used to prefix tool names shown to the model",
                &mut s.name,
                "",
            );
            switch(ui, "Enabled", "", &mut s.enabled);
            let mut stdio = matches!(s.transport, McpTransportConfig::Stdio { .. });
            let was = stdio;
            choice(
                ui,
                "mcp-transport",
                "Transport",
                "",
                &mut stdio,
                &[(true, "stdio (local process)"), (false, "Streamable HTTP")],
            );
            if stdio != was {
                s.transport = if stdio {
                    McpTransportConfig::default()
                } else {
                    McpTransportConfig::new_http()
                };
            }
            match &mut s.transport {
                McpTransportConfig::Stdio {
                    command,
                    args,
                    env,
                    cwd,
                } => {
                    text_row(
                        ui,
                        "Command",
                        "",
                        command,
                        "npx, uvx, docker, /path/to/server",
                    );
                    text_row(ui, "Working folder", "Optional", cwd, "");
                    section(ui, "Arguments");
                    ui.add_space(6.0);
                    list_editor(ui, "args", args, "argument");
                    section(ui, "Environment");
                    ui.add_space(6.0);
                    kv_editor(ui, "env", env, "NAME", "value", false);
                }
                McpTransportConfig::Http {
                    url,
                    headers,
                    oauth_client_id,
                } => {
                    text_row(ui, "URL", "", url, "https://example.com/mcp");
                    text_row(
                        ui,
                        "OAuth client ID",
                        "Optional. Servers that require sign-in usually register CleanRSTAI \
                         automatically; set this only if the server's documentation gives you a \
                         client ID",
                        oauth_client_id,
                        "",
                    );
                    section(ui, "Headers");
                    ui.add_space(6.0);
                    kv_editor(ui, "headers", headers, "Authorization", "Bearer …", true);
                }
            }

            section(ui, "Status");
            ui.add_space(4.0);
            let unsaved = live_cfg.is_none_or(|l| l.needs_reconnect(s));
            ui.horizontal_wrapped(|ui| {
                match (live_cfg, view) {
                    _ if unsaved => {
                        ui.weak("Apply settings to connect with this configuration.");
                    }
                    (_, Some(v)) => {
                        let color = status_color(ui, Some(&v.status));
                        let text = match &v.status {
                            ServerStatus::Disabled => "Disabled".to_owned(),
                            ServerStatus::Connecting => "Connecting…".to_owned(),
                            ServerStatus::Connected => {
                                format!("Connected · {} tools · {}", v.tools.len(), v.server_info)
                            }
                            ServerStatus::NeedsSignIn => "Sign-in required".to_owned(),
                            ServerStatus::SigningIn => {
                                "Waiting for you to sign in in your browser…".to_owned()
                            }
                            ServerStatus::Error(_) => "Error".to_owned(),
                        };
                        ui.colored_label(color, text);
                        if matches!(v.status, ServerStatus::Connecting | ServerStatus::SigningIn) {
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
                    section(ui, "Tools");
                    ui.add_space(4.0);
                    note(ui, "Tools that are off are not offered to the model.");
                    for t in &v.tools {
                        let mut on = !s.disabled_tools.contains(&t.name);
                        let before = on;
                        switch(ui, &t.name, &t.description, &mut on);
                        if on != before {
                            if on {
                                s.disabled_tools.retain(|n| n != &t.name);
                            } else {
                                s.disabled_tools.push(t.name.clone());
                            }
                        }
                    }
                    ui.add_space(8.0);
                }
                if !v.resources.is_empty() {
                    egui::CollapsingHeader::new(format!("Resources ({})", v.resources.len()))
                        .id_salt(("resources", &s.id))
                        .show(ui, |ui| {
                            ui.weak(format!(
                                "Attach these to a message from the {} menu next to the input.",
                                icon::BOOKS
                            ));
                            for r in &v.resources {
                                ui.label(&r.name)
                                    .on_hover_text(if r.description.is_empty() {
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
                            ui.weak(format!(
                                "Insert these from the {} menu next to the input.",
                                icon::BOOKS
                            ));
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

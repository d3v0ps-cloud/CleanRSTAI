//! Settings window: providers, MCP servers and general preferences. Edits are made on a
//! draft and only take effect on Apply / Save.

use eframe::egui;

use super::theme::icon;
use super::widgets::{code_block, kv_editor, list_editor, optional_value};
use crate::config::{
    McpServerConfig, McpTransportConfig, ProviderConfig, Settings, Theme, import_mcp_json,
};
use crate::mcp::manager::{ServerStatus, ServerView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Providers,
    Mcp,
    Tools,
    Subagents,
    General,
}

pub enum SettingsAction {
    Apply(Settings),
    Close,
    FetchModels(ProviderConfig),
    DetectKind(ProviderConfig),
    Reconnect(String),
    RefreshTools(String),
    SignIn(String),
    SignOut(String),
    ExportSettings { include_secrets: bool },
    ExportConversations,
    ImportSettings,
    ImportConversations,
    CheckForUpdates,
}

pub struct SettingsUi {
    pub draft: Settings,
    tab: Tab,
    selected_provider: usize,
    selected_server: usize,
    show_key: bool,
    import_open: bool,
    import_text: String,
    import_error: Option<String>,
    search: String,
    queued: Vec<SettingsAction>,
    pub fetching_models: bool,
    pub models_error: Option<String>,
    pub detecting_kind: bool,
    pub detect_result: Option<String>,
    export_include_secrets: bool,
    /// Outcome of the last export or import: Ok(message) or Err(error).
    pub export_status: Option<Result<String, String>>,
    /// A settings file waiting for the user to choose Merge or Replace.
    pub pending_import: Option<Box<Settings>>,
    pub checking_update: bool,
    /// Outcome of the last manual update check: Ok(message) or Err(error).
    pub update_status: Option<Result<String, String>>,
}

pub const FIELD_WIDTH: f32 = 420.0;

const PRESETS: &[(&str, &str)] = &[
    ("OpenAI", "https://api.openai.com/v1"),
    ("OpenRouter", "https://openrouter.ai/api/v1"),
    ("Groq", "https://api.groq.com/openai/v1"),
    ("Mistral", "https://api.mistral.ai/v1"),
    ("DeepSeek", "https://api.deepseek.com/v1"),
    ("Together", "https://api.together.xyz/v1"),
    (
        "Gemini (OpenAI compat)",
        "https://generativelanguage.googleapis.com/v1beta/openai",
    ),
    ("Ollama (local)", "http://localhost:11434/v1"),
    ("LM Studio (local)", "http://localhost:1234/v1"),
    ("llama.cpp / vLLM (local)", "http://localhost:8000/v1"),
];

pub fn status_color(ui: &egui::Ui, status: Option<&ServerStatus>) -> egui::Color32 {
    match status {
        Some(ServerStatus::Connected) => egui::Color32::from_rgb(60, 180, 90),
        Some(ServerStatus::Connecting) => egui::Color32::from_rgb(220, 170, 40),
        Some(ServerStatus::Error(_)) => ui.visuals().error_fg_color,
        _ => ui.visuals().weak_text_color(),
    }
}

impl SettingsUi {
    pub fn new(settings: &Settings, tab: Tab) -> Self {
        Self {
            selected_provider: settings.active_provider,
            draft: settings.clone(),
            tab,
            selected_server: 0,
            show_key: false,
            import_open: false,
            import_text: String::new(),
            import_error: None,
            search: String::new(),
            queued: Vec::new(),
            fetching_models: false,
            models_error: None,
            detecting_kind: false,
            detect_result: None,
            export_include_secrets: false,
            export_status: None,
            pending_import: None,
            checking_update: false,
            update_status: None,
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Run an action on the next frame as if its button had been pressed (used by the
    /// command palette for export/import).
    pub fn queue(&mut self, action: SettingsAction) {
        self.queued.push(action);
    }

    pub fn take_queued(&mut self) -> Vec<SettingsAction> {
        std::mem::take(&mut self.queued)
    }

    /// Render the settings page into `ui` (it takes over the main area while open).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        servers: &[ServerView],
    ) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        let dark = ui.visuals().dark_mode;

        egui::Panel::left("settings-nav")
            .resizable(false)
            .exact_size(220.0)
            .show(ui, |ui| {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.heading("Settings");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if super::widgets::icon_button(ui, icon::CLOSE, "Close settings (Esc)")
                            .clicked()
                        {
                            actions.push(SettingsAction::Close);
                        }
                    });
                });
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text(format!("{}  Search settings", icon::SEARCH))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                if self.search.trim().is_empty() {
                    for (tab, ic, label) in [
                        (Tab::Providers, icon::SERVER, "Providers"),
                        (Tab::Mcp, icon::PLUGS, "MCP Servers"),
                        (Tab::Tools, icon::TOOLBOX, "Built-in Tools"),
                        (Tab::Subagents, icon::ROBOT, "Subagents"),
                        (Tab::General, icon::GEAR, "General"),
                    ] {
                        if ui
                            .add(
                                egui::Button::selectable(
                                    self.tab == tab,
                                    format!("{ic}   {label}"),
                                )
                                .right_text("")
                                .min_size(egui::vec2(ui.available_width(), 32.0)),
                            )
                            .clicked()
                        {
                            self.tab = tab;
                        }
                    }
                } else {
                    let hits = search_settings(&self.search);
                    if hits.is_empty() {
                        ui.weak("No matching settings");
                    }
                    for (tab, section, label) in hits {
                        let resp = ui.add(
                            egui::Button::selectable(self.tab == tab, label)
                                .right_text(egui::RichText::new(section).small().weak())
                                .min_size(egui::vec2(ui.available_width(), 28.0)),
                        );
                        if resp.clicked() {
                            self.tab = tab;
                            self.search.clear();
                        }
                    }
                }
            });

        egui::Panel::bottom("settings-buttons")
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let dirty = &self.draft != live;
                    if ui
                        .add_enabled(dirty, super::theme::primary_button("Save & Close", dark))
                        .clicked()
                    {
                        actions.push(SettingsAction::Apply(self.draft.clone()));
                        actions.push(SettingsAction::Close);
                    }
                    if ui.add_enabled(dirty, egui::Button::new("Apply")).clicked() {
                        actions.push(SettingsAction::Apply(self.draft.clone()));
                    }
                    if ui
                        .button(if dirty { "Discard changes" } else { "Close" })
                        .clicked()
                    {
                        actions.push(SettingsAction::Close);
                    }
                    if dirty {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            format!("{} Unsaved changes", icon::WARNING),
                        );
                    }
                });
                ui.add_space(4.0);
            });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.heading(match self.tab {
                Tab::Providers => "Providers",
                Tab::Mcp => "MCP Servers",
                Tab::Tools => "Built-in Tools",
                Tab::Subagents => "Subagents",
                Tab::General => "General",
            });
            ui.weak(match self.tab {
                Tab::Providers => "OpenAI-compatible endpoints the app can talk to.",
                Tab::Mcp => "Model Context Protocol servers that give the model tools, resources and prompts.",
                Tab::Tools => "Tools that ship with the app, so they work without an MCP server.",
                Tab::Subagents => "Let the model delegate independent tasks to agents that work in parallel and report back.",
                Tab::General => "Appearance, behaviour, secrets and data.",
            });
            ui.add_space(8.0);
            match self.tab {
                Tab::Providers => self.providers_tab(ui, &mut actions),
                Tab::Mcp => self.mcp_tab(ui, live, servers, &mut actions),
                Tab::Tools => self.tools_tab(ui),
                Tab::Subagents => self.subagents_tab(ui),
                Tab::General => self.general_tab(ui, live, &mut actions),
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && &self.draft == live {
            actions.push(SettingsAction::Close);
        }
        actions
    }

    fn providers_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
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

                        ui.label("Temperature");
                        optional_value(ui, &mut p.temperature, 0.7, 0.0..=2.0, 0.01);
                        ui.end_row();

                        ui.label("Max tokens");
                        optional_value(ui, &mut p.max_tokens, 4096, 1..=1_000_000, 16.0);
                        ui.end_row();

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

    fn mcp_tab(
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

    fn tools_tab(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.draft.general.builtin_tools;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    "Built-in tools run on this computer and don't ask for approval. These are \
                     the defaults for new chats; switch them for one chat from the tools menu \
                     next to the message box.",
                );
                ui.add_space(10.0);
                for (on, ic, name, desc) in [
                    (
                        &mut b.time,
                        icon::CLOCK,
                        "Time & date",
                        "The current date, time, weekday and time zone, and the time at other UTC offsets.",
                    ),
                    (
                        &mut b.calculator,
                        icon::CALCULATOR,
                        "Calculator",
                        "Exact arithmetic and functions (powers, roots, logarithms, trigonometry, rounding, min/max/avg) instead of mental maths.",
                    ),
                    (
                        &mut b.tasks,
                        icon::LIST_CHECKS,
                        "Task list",
                        "For multi-step requests the model breaks the work into actionable tasks, shows them as a checklist in the chat, and ticks them off as it goes.",
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.checkbox(on, egui::RichText::new(format!("{ic}  {name}")).strong());
                    });
                    ui.indent(name, |ui| {
                        ui.weak(desc);
                    });
                    ui.add_space(8.0);
                }
                ui.weak(format!(
                    "{}  The subagent tool has its own page: Subagents.",
                    icon::ROBOT
                ));
            });
    }

    fn subagents_tab(&mut self, ui: &mut egui::Ui) {
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
                                    "Time and calculator (when on in the chat)",
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

    fn general_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        actions: &mut Vec<SettingsAction>,
    ) {
        let dirty = &self.draft != live;
        // Applied after the scroll area, which holds a borrow of the draft.
        let mut import_choice: Option<Option<crate::export::ImportMode>> = None;
        let g = &mut self.draft.general;
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
                        ui.label("Theme");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut g.theme, Theme::System, "System");
                            ui.radio_value(&mut g.theme, Theme::Light, "Light");
                            ui.radio_value(&mut g.theme, Theme::Dark, "Dark");
                        });
                        ui.end_row();

                        ui.label("UI scale");
                        ui.add(
                            egui::Slider::new(&mut g.zoom, 0.5..=2.5)
                                .step_by(0.05)
                                .suffix("×"),
                        );
                        ui.end_row();

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

                        ui.label("Updates");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut g.check_for_updates,
                                "Check GitHub for a new release at startup",
                            );
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
                                    actions.push(SettingsAction::CheckForUpdates);
                                }
                                if self.checking_update {
                                    ui.spinner();
                                }
                                ui.weak(concat!("Installed: v", env!("CARGO_PKG_VERSION")));
                            });
                            match &self.update_status {
                                Some(Ok(m)) => {
                                    ui.label(m);
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, e);
                                }
                                None => {}
                            }
                        });
                        ui.end_row();

                        ui.label("Reasoning");
                        ui.vertical(|ui| {
                            for mode in crate::config::ReasoningDisplay::ALL {
                                ui.radio_value(&mut g.reasoning_display, mode, mode.label());
                            }
                        })
                        .response
                        .on_hover_text("How the model's \"Thinking\" section starts out");
                        ui.end_row();

                        ui.label("Stats");
                        ui.checkbox(
                            &mut g.show_stats,
                            "Show prefill/decode speed and token counts under responses",
                        );
                        ui.end_row();

                        ui.label("Images");
                        ui.checkbox(
                            &mut g.load_remote_images,
                            "Load web images in replies",
                        )
                        .on_hover_text(
                            "When off, images from the web show as links. Loading them tells \
                             the image's server that this chat was opened, and a reply could \
                             put chat contents in the image address.",
                        );
                        ui.end_row();

                        ui.label("Tool calls");
                        ui.checkbox(
                            &mut g.auto_approve_tools,
                            "Run MCP tools and subagents without asking for approval",
                        );
                        ui.end_row();

                        ui.label("Max tool rounds");
                        ui.add(egui::DragValue::new(&mut g.max_tool_rounds).range(1..=200))
                            .on_hover_text(
                                "How many times the model may call tools before it must answer",
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

/// Settings the search box can find: (tab, section, label).
const SEARCH_INDEX: &[(Tab, &str, &str)] = &[
    (Tab::Providers, "Providers", "Base URL / endpoint"),
    (Tab::Providers, "Providers", "API key"),
    (Tab::Providers, "Providers", "Model"),
    (Tab::Providers, "Providers", "Fetch models"),
    (
        Tab::Providers,
        "Providers",
        "Server type (llama.cpp, LiteLLM, OpenAI)",
    ),
    (Tab::Providers, "Providers", "Temperature"),
    (Tab::Providers, "Providers", "Max tokens"),
    (Tab::Providers, "Providers", "Streaming"),
    (Tab::Providers, "Providers", "Token usage (stream_options)"),
    (Tab::Providers, "Providers", "Timeout"),
    (Tab::Providers, "Providers", "Extra headers"),
    (Tab::Mcp, "MCP", "Add server"),
    (Tab::Mcp, "MCP", "Import mcpServers JSON"),
    (Tab::Mcp, "MCP", "stdio command and arguments"),
    (Tab::Mcp, "MCP", "Environment variables"),
    (Tab::Mcp, "MCP", "Streamable HTTP URL and headers"),
    (Tab::Mcp, "MCP", "OAuth sign-in / client ID"),
    (Tab::Mcp, "MCP", "Enable or disable tools"),
    (Tab::Mcp, "MCP", "Resources and prompts"),
    (Tab::Tools, "Tools", "Time & date tool"),
    (Tab::Tools, "Tools", "Calculator tool"),
    (Tab::Tools, "Tools", "Task list / planning tool"),
    (
        Tab::Subagents,
        "Subagents",
        "Enable subagents (delegate tool)",
    ),
    (Tab::Subagents, "Subagents", "Subagent provider and model"),
    (Tab::Subagents, "Subagents", "Subagent tools and MCP access"),
    (
        Tab::Subagents,
        "Subagents",
        "Parallel subagents, rounds and time limit",
    ),
    (Tab::Subagents, "Subagents", "Subagent system prompt"),
    (Tab::General, "General", "System prompt"),
    (Tab::General, "General", "Theme (light / dark)"),
    (Tab::General, "General", "UI scale / zoom"),
    (Tab::General, "General", "Send with Enter"),
    (Tab::General, "General", "Keychain / secrets"),
    (Tab::General, "General", "Show speed stats"),
    (Tab::General, "General", "Load web images in replies"),
    (
        Tab::General,
        "General",
        "Reasoning / thinking display (collapsed by default)",
    ),
    (Tab::General, "General", "Auto-approve tool calls"),
    (Tab::General, "General", "Max tool rounds"),
    (Tab::General, "General", "Tool timeout"),
    (Tab::General, "General", "Export settings"),
    (Tab::General, "General", "Export conversations"),
    (Tab::General, "General", "Import settings"),
    (Tab::General, "General", "Import conversations"),
    (Tab::General, "General", "Check for updates"),
];

fn search_settings(query: &str) -> Vec<(Tab, &'static str, &'static str)> {
    let mut hits: Vec<(i32, Tab, &str, &str)> = SEARCH_INDEX
        .iter()
        .filter_map(|&(tab, section, label)| {
            super::palette::score(query, label).map(|s| (s, tab, section, label))
        })
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().map(|(_, t, s, l)| (t, s, l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_settings() {
        let hits = search_settings("api key");
        assert_eq!(hits[0], (Tab::Providers, "Providers", "API key"));
        assert_eq!(search_settings("theme")[0].0, Tab::General);
        assert_eq!(search_settings("oauth")[0].0, Tab::Mcp);
        assert_eq!(search_settings("calculator")[0].0, Tab::Tools);
        assert_eq!(search_settings("subagent model")[0].0, Tab::Subagents);
        assert!(search_settings("zzzz").is_empty());
    }
}

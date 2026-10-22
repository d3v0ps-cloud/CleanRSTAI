use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::{self, AgentRequest, AppEvent, ChatEvent, EventSender};
use crate::attach::Attachment;
use crate::config::{Settings, Theme};
use crate::export;
use crate::llm;
use crate::mcp::manager::McpManager;
use crate::types::{ChatMessage, Conversation, Role, ToolCall};
use crate::ui::attachments::{self as attach_ui, ImageCache};
use crate::ui::settings::{SettingsAction, SettingsUi, Tab};
use crate::ui::widgets::code_block;

const MAX_CONTENT_WIDTH: f32 = 880.0;
const INPUT_ID: &str = "chat-input";

struct Run {
    task: JoinHandle<()>,
    approvals: Vec<(ToolCall, oneshot::Sender<bool>)>,
    running_tool: Option<String>,
}

enum Action {
    Approve(Uuid, String, bool),
    Copy(String),
    Regenerate(Uuid),
    EditFrom(Uuid, usize),
    ViewImage(String, egui::load::Bytes, String),
}

pub struct ChatApp {
    rt: tokio::runtime::Handle,
    http: reqwest::Client,
    settings: Settings,
    mcp: Arc<McpManager>,
    tx: EventSender,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    conversations: Vec<Conversation>,
    current: Option<Uuid>,
    input: String,
    runs: HashMap<Uuid, Run>,
    errors: HashMap<Uuid, String>,
    md_cache: CommonMarkCache,
    settings_ui: Option<SettingsUi>,
    renaming: Option<(Uuid, String)>,
    search: String,
    notice: Option<String>,
    keychain_warned: bool,
    pending_attachments: Vec<Attachment>,
    images: ImageCache,
    viewing: Option<(String, egui::load::Bytes, String)>,
    prompt_form: Option<PromptForm>,
    /// For background tasks to request a repaint when their results arrive.
    repaint_ctx: egui::Context,
}

/// Arguments being filled in for an MCP prompt.
struct PromptForm {
    server_id: String,
    prompt: crate::mcp::McpPrompt,
    values: std::collections::BTreeMap<String, String>,
}

fn apply_appearance(ctx: &egui::Context, settings: &Settings) {
    ctx.set_theme(match settings.general.theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    });
    ctx.set_zoom_factor(settings.general.zoom);
}

impl ChatApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Handle) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let loaded = Settings::load();
        let settings = loaded.settings;
        apply_appearance(&cc.egui_ctx, &settings);

        let http = llm::build_http_client();
        let (tx, rx) = mpsc::unbounded_channel();
        let mcp = McpManager::new(rt.clone(), http.clone());
        {
            let ctx = cc.egui_ctx.clone();
            mcp.set_on_change(move || ctx.request_repaint());
        }
        mcp.sync(&settings.mcp_servers);

        let conversations = Conversation::load_all();
        let current = conversations.first().map(|c| c.id);
        let mut app = Self {
            rt,
            http,
            settings,
            mcp,
            tx,
            rx,
            conversations,
            current,
            input: String::new(),
            runs: HashMap::new(),
            errors: HashMap::new(),
            md_cache: CommonMarkCache::default(),
            settings_ui: None,
            renaming: None,
            search: String::new(),
            notice: loaded.notice,
            keychain_warned: false,
            pending_attachments: Vec::new(),
            images: ImageCache::new(),
            viewing: None,
            prompt_form: None,
            repaint_ctx: cc.egui_ctx.clone(),
        };
        if loaded.needs_migration {
            // Move plain-text secrets from settings.json into the keychain.
            app.save_settings();
        }
        app
    }

    fn conv_mut(&mut self, id: Uuid) -> Option<&mut Conversation> {
        self.conversations.iter_mut().find(|c| c.id == id)
    }

    fn save_settings(&mut self) {
        match self.settings.save() {
            Ok(None) => self.keychain_warned = false,
            // Say it once rather than on every save.
            Ok(Some(warning)) if !self.keychain_warned => {
                self.keychain_warned = true;
                self.notice = Some(warning);
            }
            Ok(Some(_)) => {}
            Err(e) => self.notice = Some(format!("Could not save settings: {e:#}")),
        }
    }

    fn new_chat(&mut self) {
        // Reuse an existing empty chat instead of piling them up.
        if let Some(c) = self.conversations.iter().find(|c| c.messages.is_empty()) {
            self.current = Some(c.id);
        } else {
            let c = Conversation::new();
            self.current = Some(c.id);
            self.conversations.insert(0, c);
        }
        self.input.clear();
    }

    fn delete_chat(&mut self, id: Uuid) {
        self.stop(id);
        Conversation::delete(id);
        self.conversations.retain(|c| c.id != id);
        self.errors.remove(&id);
        if self.current == Some(id) {
            self.current = self.conversations.first().map(|c| c.id);
        }
    }

    fn send(&mut self, ctx: &egui::Context) {
        let text = self.input.trim().to_owned();
        if text.is_empty() && self.pending_attachments.is_empty() {
            return;
        }
        if self.current.is_none_or(|id| self.conv_mut(id).is_none()) {
            self.new_chat();
        }
        let id = self.current.unwrap();
        if self.runs.contains_key(&id) {
            return;
        }
        let attachments = std::mem::take(&mut self.pending_attachments);
        let conv = self.conv_mut(id).unwrap();
        let mut msg = ChatMessage::new(Role::User, text);
        msg.attachments = attachments;
        conv.messages.push(msg);
        conv.auto_title();
        conv.updated_at = chrono::Utc::now();
        conv.save();
        self.input.clear();
        self.start_run(id, ctx);
    }

    fn start_run(&mut self, id: Uuid, ctx: &egui::Context) {
        let Some(conv) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        self.errors.remove(&id);
        let req = AgentRequest {
            conv_id: id,
            provider: self.settings.provider().clone(),
            general: self.settings.general.clone(),
            history: conv.messages.clone(),
        };
        let (http, mcp, tx, ctx) = (
            self.http.clone(),
            self.mcp.clone(),
            self.tx.clone(),
            ctx.clone(),
        );
        let task = self.rt.spawn(async move {
            agent::run(req, http, mcp, tx, move || ctx.request_repaint()).await;
        });
        self.runs.insert(
            id,
            Run {
                task,
                approvals: Vec::new(),
                running_tool: None,
            },
        );
    }

    fn stop(&mut self, id: Uuid) {
        if let Some(run) = self.runs.remove(&id) {
            run.task.abort();
            if let Some(conv) = self.conv_mut(id) {
                if conv.messages.last().is_some_and(|m| {
                    m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty()
                }) {
                    conv.messages.pop();
                }
                conv.updated_at = chrono::Utc::now();
                conv.save();
            }
        }
    }

    fn regenerate(&mut self, id: Uuid, ctx: &egui::Context) {
        if self.runs.contains_key(&id) {
            return;
        }
        let Some(conv) = self.conv_mut(id) else {
            return;
        };
        while conv.messages.last().is_some_and(|m| m.role != Role::User) {
            conv.messages.pop();
        }
        if conv.messages.is_empty() {
            return;
        }
        self.start_run(id, ctx);
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::ModelsFetched {
                    provider_id,
                    result,
                } => {
                    if let Some(ui) = &mut self.settings_ui {
                        ui.fetching_models = false;
                        match result {
                            Ok(models) => {
                                if let Some(p) =
                                    ui.draft.providers.iter_mut().find(|p| p.id == provider_id)
                                {
                                    if p.model.is_empty() {
                                        p.model = models.first().cloned().unwrap_or_default();
                                    }
                                    p.models = models;
                                }
                            }
                            Err(e) => ui.models_error = Some(e),
                        }
                    }
                }
                AppEvent::McpResourceRead { name, result } => match result {
                    Ok(list) => self.pending_attachments.extend(list),
                    Err(e) => self.notice = Some(format!("Couldn't read {name}: {e}")),
                },
                AppEvent::McpPromptReady { name, result } => match result {
                    Ok(messages) => self.insert_prompt(messages),
                    Err(e) => self.notice = Some(format!("Prompt {name} failed: {e}")),
                },
                AppEvent::KindDetected {
                    provider_id,
                    kind,
                    explicit,
                } => {
                    if let Some(ui) = &mut self.settings_ui {
                        if explicit {
                            ui.detecting_kind = false;
                        }
                        if let Some(p) = ui.draft.providers.iter_mut().find(|p| p.id == provider_id)
                        {
                            // A background probe only upgrades from Generic; it never overrides a choice.
                            if explicit || p.kind == crate::config::ProviderKind::Generic {
                                p.kind = kind;
                            }
                            if explicit {
                                ui.detect_result = Some(format!("Detected: {}", kind.label()));
                            }
                        }
                    }
                }
                AppEvent::Chat { conv_id, event } => self.handle_chat_event(conv_id, event),
            }
        }
    }

    fn handle_chat_event(&mut self, id: Uuid, event: ChatEvent) {
        // Late events from a stopped run are ignored (except approvals, which get dropped → denied).
        if !self.runs.contains_key(&id) {
            return;
        }
        match event {
            ChatEvent::ToolApproval { call, respond } => {
                self.runs
                    .get_mut(&id)
                    .unwrap()
                    .approvals
                    .push((call, respond));
                return;
            }
            ChatEvent::ToolRunning(call_id) => {
                self.runs.get_mut(&id).unwrap().running_tool = Some(call_id);
                return;
            }
            ChatEvent::Error(e) => {
                self.errors.insert(id, e);
                return;
            }
            _ => {}
        }
        let done = matches!(event, ChatEvent::Done);
        let is_tool_result = matches!(event, ChatEvent::ToolResult(_));
        let Some(conv) = self.conv_mut(id) else {
            return;
        };
        match event {
            ChatEvent::AssistantStart => conv.messages.push(ChatMessage::new(Role::Assistant, "")),
            ChatEvent::ContentDelta(s) => {
                if let Some(m) = conv
                    .messages
                    .last_mut()
                    .filter(|m| m.role == Role::Assistant)
                {
                    m.content.push_str(&s);
                }
            }
            ChatEvent::ReasoningDelta(s) => {
                if let Some(m) = conv
                    .messages
                    .last_mut()
                    .filter(|m| m.role == Role::Assistant)
                {
                    m.reasoning.get_or_insert_default().push_str(&s);
                }
            }
            ChatEvent::AssistantFinal(msg) => {
                if let Some(m) = conv
                    .messages
                    .last_mut()
                    .filter(|m| m.role == Role::Assistant)
                {
                    *m = *msg;
                }
            }
            ChatEvent::ToolResult(msg) => conv.messages.push(*msg),
            ChatEvent::Done => {
                if conv.messages.last().is_some_and(|m| {
                    m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty()
                }) {
                    conv.messages.pop();
                }
                conv.updated_at = chrono::Utc::now();
                conv.save();
            }
            _ => {}
        }
        if done {
            self.runs.remove(&id);
            self.conversations
                .sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        } else if is_tool_result && let Some(run) = self.runs.get_mut(&id) {
            run.running_tool = None;
        }
    }

    fn apply_settings(&mut self, new: Settings, ctx: &egui::Context) {
        self.settings = new;
        self.settings.normalize();
        apply_appearance(ctx, &self.settings);
        self.mcp.sync(&self.settings.mcp_servers);
        self.save_settings();
    }

    /// Menu of MCP resources (attach) and prompts (insert) from connected servers.
    fn mcp_menu(&mut self, ui: &mut egui::Ui, width: f32) {
        let views = self.mcp.snapshot();
        let usable: Vec<_> = views
            .iter()
            .filter(|v| !v.resources.is_empty() || !v.prompts.is_empty())
            .collect();
        if usable.is_empty() {
            return;
        }
        let names: HashMap<&str, &str> = self
            .settings
            .mcp_servers
            .iter()
            .map(|s| (s.id.as_str(), s.name.as_str()))
            .collect();
        let mut read = None;
        let mut prompt = None;
        ui.scope(|ui| {
            ui.set_width(width);
            ui.menu_button("📚 MCP", |ui| {
                ui.set_max_height(420.0);
                for v in &usable {
                    let name = names.get(v.id.as_str()).copied().unwrap_or("server");
                    ui.menu_button(name, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(380.0)
                            .show(ui, |ui| {
                                if !v.resources.is_empty() {
                                    ui.label(egui::RichText::new("Attach resource").small().weak());
                                    for r in &v.resources {
                                        let resp = ui.button(&r.name);
                                        let resp = if r.description.is_empty() {
                                            resp.on_hover_text(&r.uri)
                                        } else {
                                            resp.on_hover_text(format!(
                                                "{}\n{}",
                                                r.description, r.uri
                                            ))
                                        };
                                        if resp.clicked() {
                                            read = Some((v.id.clone(), r.clone()));
                                            ui.close();
                                        }
                                    }
                                }
                                if !v.prompts.is_empty() {
                                    ui.separator();
                                    ui.label(egui::RichText::new("Insert prompt").small().weak());
                                    for p in &v.prompts {
                                        let resp = ui.button(&p.name);
                                        let resp = if p.description.is_empty() {
                                            resp
                                        } else {
                                            resp.on_hover_text(&p.description)
                                        };
                                        if resp.clicked() {
                                            prompt = Some((v.id.clone(), p.clone()));
                                            ui.close();
                                        }
                                    }
                                }
                            });
                    });
                }
            })
            .response
            .on_hover_text("Attach a resource or insert a prompt from an MCP server");
        });
        if let Some((server, res)) = read {
            self.read_resource(server, res);
        }
        if let Some((server_id, prompt)) = prompt {
            if prompt.arguments.is_empty() {
                self.run_prompt(server_id, prompt.name, Default::default());
            } else {
                let values = prompt
                    .arguments
                    .iter()
                    .map(|a| (a.name.clone(), String::new()))
                    .collect();
                self.prompt_form = Some(PromptForm {
                    server_id,
                    prompt,
                    values,
                });
            }
        }
    }

    fn read_resource(&self, server_id: String, res: crate::mcp::McpResource) {
        let Some(client) = self.mcp.client(&server_id) else {
            return;
        };
        let (tx, ctx) = (self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn(async move {
            let result = client
                .read_resource(&res.uri)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::McpResourceRead {
                name: res.name,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn run_prompt(
        &self,
        server_id: String,
        name: String,
        args: std::collections::BTreeMap<String, String>,
    ) {
        let Some(client) = self.mcp.client(&server_id) else {
            return;
        };
        let (tx, ctx) = (self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn(async move {
            let result = client
                .get_prompt(&name, &args)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::McpPromptReady { name, result });
            ctx.request_repaint();
        });
    }

    /// Put a prompt's messages into the chat: earlier messages are added to the
    /// conversation; a final user message goes into the input box for review.
    fn insert_prompt(&mut self, mut messages: Vec<crate::mcp::PromptMessage>) {
        let last_user = messages
            .last()
            .is_some_and(|m| m.role == Role::User)
            .then(|| messages.pop())
            .flatten();
        if !messages.is_empty() {
            if self.current.is_none_or(|id| self.conv_mut(id).is_none()) {
                self.new_chat();
            }
            let id = self.current.unwrap();
            if self.runs.contains_key(&id) {
                self.notice = Some("Wait for the current response to finish.".into());
                return;
            }
            let conv = self.conv_mut(id).unwrap();
            for m in messages {
                let mut msg = ChatMessage::new(m.role, m.text);
                msg.attachments = m.attachments;
                conv.messages.push(msg);
            }
            conv.auto_title();
            conv.save();
        }
        if let Some(m) = last_user {
            if !self.input.trim().is_empty() {
                self.input.push_str("\n\n");
            }
            self.input.push_str(&m.text);
            self.pending_attachments.extend(m.attachments);
        }
    }

    fn prompt_form_window(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.prompt_form else {
            return;
        };
        let mut open = true;
        let mut submit = false;
        egui::Window::new(format!("Prompt: {}", form.prompt.name))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                if !form.prompt.description.is_empty() {
                    ui.label(&form.prompt.description);
                    ui.add_space(6.0);
                }
                egui::Grid::new("prompt-args")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        for a in &form.prompt.arguments {
                            let label = if a.required {
                                format!("{} *", a.name)
                            } else {
                                a.name.clone()
                            };
                            ui.label(label).on_hover_text(&a.description);
                            let value = form.values.entry(a.name.clone()).or_default();
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .hint_text(&a.description)
                                    .desired_width(300.0),
                            );
                            ui.end_row();
                        }
                    });
                let missing = form.prompt.arguments.iter().any(|a| {
                    a.required && form.values.get(&a.name).is_none_or(|v| v.trim().is_empty())
                });
                ui.add_space(6.0);
                if ui
                    .add_enabled(!missing, egui::Button::new("Insert"))
                    .clicked()
                {
                    submit = true;
                }
            });
        if submit {
            let form = self.prompt_form.take().unwrap();
            let args = form
                .values
                .into_iter()
                .filter(|(_, v)| !v.trim().is_empty())
                .collect();
            self.run_prompt(form.server_id, form.prompt.name, args);
        } else if !open {
            self.prompt_form = None;
        }
    }

    fn export_markdown(&mut self, id: Uuid) {
        let Some(c) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        let Some(path) =
            export::pick_save_path_as(&export::safe_file_name(&c.title, "md"), "Markdown", "md")
        else {
            return;
        };
        let md = export::conversation_markdown(c);
        self.notice = Some(match crate::config::write_private(&path, md.as_bytes()) {
            Ok(()) => format!("Exported \"{}\" to {}", c.title, path.display()),
            Err(e) => format!("Export failed: {e:#}"),
        });
    }

    fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>) {
        let mut errors = Vec::new();
        for p in paths {
            match Attachment::from_path(&p) {
                Ok(a) => self.pending_attachments.push(a),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        if !errors.is_empty() {
            self.notice = Some(format!("Couldn't attach: {}", errors.join("; ")));
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        let mut errors = Vec::new();
        for f in dropped {
            let name = f
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let result = f
                .bytes()
                .map_err(|e| anyhow::anyhow!("{name}: {e}"))
                .and_then(|bytes| Attachment::from_bytes(name, bytes));
            match result {
                Ok(a) => self.pending_attachments.push(a),
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        if !errors.is_empty() {
            self.notice = Some(format!("Couldn't attach: {}", errors.join("; ")));
        }
    }

    /// Add imported chats that aren't already present; returns a status message.
    fn import_conversations(&mut self, incoming: Vec<Conversation>) -> String {
        let (fresh, skipped) = export::new_conversations(&self.conversations, incoming);
        for c in &fresh {
            c.save();
        }
        let added = fresh.len();
        self.conversations.extend(fresh);
        self.conversations
            .sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        match skipped {
            0 => format!("Imported {added} conversation(s)."),
            n => format!("Imported {added} conversation(s); skipped {n} already present."),
        }
    }

    fn spawn_detect(&self, p: crate::config::ProviderConfig, explicit: bool, ctx: &egui::Context) {
        let (http, tx, ctx) = (self.http.clone(), self.tx.clone(), ctx.clone());
        self.rt.spawn(async move {
            let kind = llm::detect_kind(&http, &p).await;
            let _ = tx.send(AppEvent::KindDetected {
                provider_id: p.id,
                kind,
                explicit,
            });
            ctx.request_repaint();
        });
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (new_chat, open_settings) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::N),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma),
            )
        });
        if new_chat {
            self.new_chat();
        }
        if open_settings && self.settings_ui.is_none() {
            self.settings_ui = Some(SettingsUi::new(&self.settings, Tab::Providers));
        }
    }

    // ---------------------------------------------------------------- top bar

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("CleanRSTAI");
            ui.separator();

            let mut changed = false;
            let providers: Vec<(usize, String)> = self
                .settings
                .providers
                .iter()
                .enumerate()
                .map(|(i, p)| (i, p.name.clone()))
                .collect();
            egui::ComboBox::from_id_salt("provider")
                .selected_text(self.settings.provider().name.clone())
                .show_ui(ui, |ui| {
                    for (i, name) in providers {
                        changed |= ui
                            .selectable_value(&mut self.settings.active_provider, i, name)
                            .changed();
                    }
                });

            let p = self.settings.provider_mut();
            if p.models.is_empty() {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut p.model)
                            .hint_text("model")
                            .desired_width(200.0),
                    )
                    .lost_focus();
            } else {
                let models = p.models.clone();
                egui::ComboBox::from_id_salt("model")
                    .selected_text(if p.model.is_empty() {
                        "Select model".to_owned()
                    } else {
                        p.model.clone()
                    })
                    .height(420.0)
                    .show_ui(ui, |ui| {
                        for m in models {
                            changed |= ui.selectable_value(&mut p.model, m.clone(), m).changed();
                        }
                    });
            }
            if changed {
                self.save_settings();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙ Settings").clicked() && self.settings_ui.is_none() {
                    self.settings_ui = Some(SettingsUi::new(&self.settings, Tab::Providers));
                }
                let (servers, tools) = self.mcp.connected_tool_count();
                let configured = self
                    .settings
                    .mcp_servers
                    .iter()
                    .filter(|s| s.enabled)
                    .count();
                let label = if configured == 0 {
                    "No MCP servers".to_owned()
                } else {
                    format!("MCP {servers}/{configured} · {tools} tools")
                };
                if ui.link(label).on_hover_text("Manage MCP servers").clicked()
                    && self.settings_ui.is_none()
                {
                    self.settings_ui = Some(SettingsUi::new(&self.settings, Tab::Mcp));
                }
            });
        });
    }

    // ---------------------------------------------------------------- sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        if ui
            .add_sized(
                [ui.available_width(), 30.0],
                egui::Button::new("＋ New chat"),
            )
            .on_hover_text("⌘/Ctrl + N")
            .clicked()
        {
            self.new_chat();
        }
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("🔍 Search chats")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);

        let mut select = None;
        let mut delete = None;
        let mut export_md = None;
        let mut rename_commit = None;
        let needle = self.search.to_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for c in &self.conversations {
                    if !needle.is_empty()
                        && !c.title.to_lowercase().contains(&needle)
                        && !c
                            .messages
                            .iter()
                            .any(|m| m.content.to_lowercase().contains(&needle))
                    {
                        continue;
                    }
                    if let Some((rid, text)) = &mut self.renaming
                        && *rid == c.id
                    {
                        let resp =
                            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                        resp.request_focus();
                        if resp.lost_focus() {
                            rename_commit = Some((c.id, text.clone()));
                        }
                        continue;
                    }
                    let running = self.runs.contains_key(&c.id);
                    let title = if running {
                        format!("⏳ {}", c.title)
                    } else {
                        c.title.clone()
                    };
                    let resp = ui
                        .add_sized(
                            [ui.available_width(), 24.0],
                            egui::Button::selectable(self.current == Some(c.id), title).truncate(),
                        )
                        .on_hover_text(
                            c.updated_at
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M")
                                .to_string(),
                        );
                    if resp.clicked() {
                        select = Some(c.id);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("✏ Rename").clicked() {
                            self.renaming = Some((c.id, c.title.clone()));
                            ui.close();
                        }
                        if ui.button("⬇ Export as Markdown…").clicked() {
                            export_md = Some(c.id);
                            ui.close();
                        }
                        if ui.button("🗑 Delete").clicked() {
                            delete = Some(c.id);
                            ui.close();
                        }
                    });
                }
            });

        if let Some(id) = export_md {
            self.export_markdown(id);
        }
        if let Some(id) = select {
            self.current = Some(id);
        }
        if let Some((id, title)) = rename_commit {
            self.renaming = None;
            if let Some(c) = self.conv_mut(id) {
                let title = title.trim();
                if !title.is_empty() {
                    c.title = title.to_owned();
                    c.save();
                }
            }
        }
        if let Some(id) = delete {
            self.delete_chat(id);
        }
    }

    // ---------------------------------------------------------------- input

    fn input_area(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::unique(INPUT_ID);
        let running = self.current.is_some_and(|c| self.runs.contains_key(&c));
        let has_focus = ui.memory(|m| m.has_focus(id));
        let mut send = false;
        if has_focus && !running {
            send = ui.input_mut(|i| {
                let cmd_enter = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
                let enter = self.settings.general.send_with_enter
                    && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                cmd_enter || enter
            });
        }

        ui.add_space(6.0);
        centered(ui, |ui| {
            if let Some(n) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, n);
                    if ui.small_button("✖").clicked() {
                        self.notice = None;
                    }
                });
            }
            if let Some(n) =
                attach_ui::pending_chips(ui, &mut self.images, &self.pending_attachments)
            {
                self.pending_attachments.remove(n);
            }
            ui.horizontal(|ui| {
                let button_w = 80.0;
                let hint = if self.settings.general.send_with_enter {
                    "Message… (Enter to send, Shift+Enter for newline)"
                } else {
                    "Message… (⌘/Ctrl+Enter to send)"
                };
                egui::ScrollArea::vertical()
                    .id_salt("input-scroll")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.input)
                                .id(id)
                                .hint_text(hint)
                                .desired_rows(3)
                                .lock_focus(true)
                                .desired_width(ui.available_width() - button_w - 8.0),
                        );
                    });
                ui.vertical(|ui| {
                    if running {
                        if ui
                            .add_sized([button_w, 32.0], egui::Button::new("■ Stop"))
                            .clicked()
                        {
                            let cid = self.current.unwrap();
                            self.stop(cid);
                        }
                    } else if ui
                        .add_enabled(
                            !self.input.trim().is_empty() || !self.pending_attachments.is_empty(),
                            egui::Button::new("Send ➤").min_size([button_w, 32.0].into()),
                        )
                        .clicked()
                    {
                        send = true;
                    }
                    if ui
                        .add_sized([button_w, 24.0], egui::Button::new("📎 Attach"))
                        .on_hover_text("Attach images or text files (or drop them on the window)")
                        .clicked()
                        && let Some(paths) = rfd::FileDialog::new()
                            .set_title("Attach files")
                            .pick_files()
                    {
                        self.attach_paths(paths);
                    }
                    self.mcp_menu(ui, button_w);
                });
            });
        });
        ui.add_space(6.0);
        if send {
            let ctx = ui.ctx().clone();
            self.send(&ctx);
            ui.memory_mut(|m| m.request_focus(id));
        }
    }

    // ---------------------------------------------------------------- messages

    fn chat_view(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        let Some(conv) = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
        else {
            self.empty_state(ui);
            return actions;
        };
        if conv.messages.is_empty() && !self.runs.contains_key(&conv.id) {
            self.empty_state(ui);
            return actions;
        }

        let run = self.runs.get(&conv.id);
        let results: HashMap<&str, &ChatMessage> = conv
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
            .collect();
        let last_assistant = conv
            .messages
            .iter()
            .rposition(|m| m.role == Role::Assistant);
        let md_cache = &mut self.md_cache;
        let images = &mut self.images;
        let show_stats = self.settings.general.show_stats;
        if show_stats {
            let totals =
                crate::stats::Totals::of(conv.messages.iter().filter_map(|m| m.stats.as_ref()));
            if totals.requests > 0 {
                centered(ui, |ui| crate::ui::stats_view::totals_bar(ui, &totals));
            }
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add_space(8.0);
                centered(ui, |ui| {
                    for (i, msg) in conv.messages.iter().enumerate() {
                        match msg.role {
                            Role::Tool | Role::System => continue,
                            Role::User => user_message(
                                ui,
                                images,
                                conv.id,
                                i,
                                msg,
                                run.is_some(),
                                &mut actions,
                            ),
                            Role::Assistant => {
                                let streaming = run.is_some() && i + 1 == conv.messages.len();
                                assistant_message(
                                    ui,
                                    md_cache,
                                    images,
                                    conv.id,
                                    i,
                                    msg,
                                    &results,
                                    run,
                                    streaming,
                                    show_stats,
                                    &mut actions,
                                );
                                if Some(i) == last_assistant && run.is_none() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("📋 Copy").clicked() {
                                            actions.push(Action::Copy(msg.content.clone()));
                                        }
                                        if ui.small_button("↻ Regenerate").clicked() {
                                            actions.push(Action::Regenerate(conv.id));
                                        }
                                    });
                                }
                            }
                        }
                        ui.add_space(10.0);
                    }
                    if let Some(err) = self.errors.get(&conv.id) {
                        egui::Frame::new()
                            .fill(ui.visuals().error_fg_color.gamma_multiply(0.12))
                            .stroke(egui::Stroke::new(1.0, ui.visuals().error_fg_color))
                            .corner_radius(6)
                            .inner_margin(10)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.colored_label(ui.visuals().error_fg_color, err);
                                if run.is_none() && ui.button("↻ Retry").clicked() {
                                    actions.push(Action::Regenerate(conv.id));
                                }
                            });
                    }
                    ui.add_space(12.0);
                });
            });
        actions
    }

    fn empty_state(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading("What can I help with?");
            ui.add_space(8.0);
            let p = self.settings.provider();
            ui.weak(format!(
                "{} · {}",
                p.name,
                if p.model.is_empty() {
                    "no model selected"
                } else {
                    &p.model
                }
            ));
            let (servers, tools) = self.mcp.connected_tool_count();
            if servers > 0 {
                ui.weak(format!(
                    "{tools} tools available from {servers} MCP server(s)"
                ));
            }
            if p.api_key.is_empty()
                && !p.base_url.contains("localhost")
                && !p.base_url.contains("127.0.0.1")
            {
                ui.add_space(8.0);
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "No API key set — open Settings to configure your provider.",
                );
            }
        });
    }

    fn apply_actions(&mut self, actions: Vec<Action>, ctx: &egui::Context) {
        for a in actions {
            match a {
                Action::Copy(text) => ctx.copy_text(text),
                Action::Regenerate(id) => self.regenerate(id, ctx),
                Action::Approve(conv, call_id, ok) => {
                    if let Some(run) = self.runs.get_mut(&conv)
                        && let Some(pos) = run.approvals.iter().position(|(c, _)| c.id == call_id)
                    {
                        let (_, tx) = run.approvals.remove(pos);
                        let _ = tx.send(ok);
                    }
                }
                Action::ViewImage(uri, bytes, name) => self.viewing = Some((uri, bytes, name)),
                Action::EditFrom(conv, idx) => {
                    if self.runs.contains_key(&conv) {
                        continue;
                    }
                    if let Some(c) = self.conv_mut(conv)
                        && idx < c.messages.len()
                    {
                        let text = c.messages[idx].content.clone();
                        let attachments = c.messages[idx].attachments.clone();
                        c.messages.truncate(idx);
                        c.save();
                        self.input = text;
                        self.pending_attachments = attachments;
                        self.errors.remove(&conv);
                        ctx.memory_mut(|m| m.request_focus(egui::Id::unique(INPUT_ID)));
                    }
                }
            }
        }
    }
}

/// Ask for an export file and parse it. `None` if the user cancelled the dialog.
fn import_file() -> Option<Result<export::Imported, String>> {
    let path = export::pick_open_path()?;
    Some(
        std::fs::read(&path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))
            .and_then(|bytes| {
                export::parse_import(&bytes).map_err(|e| format!("Import failed: {e:#}"))
            }),
    )
}

/// Ask where to save `doc` and write it. `None` if the user cancelled the dialog.
fn run_export(kind: &str, doc: &serde_json::Value) -> Option<Result<String, String>> {
    let path = export::pick_save_path(&export::default_file_name(kind))?;
    Some(match export::write_document(&path, doc) {
        Ok(()) => Ok(format!("Exported {kind} to {}", path.display())),
        Err(e) => Err(format!("Export failed: {e:#}")),
    })
}

/// Lay out `add` in a centered column of at most `MAX_CONTENT_WIDTH`.
fn centered(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let avail = ui.available_width();
    let width = avail.min(MAX_CONTENT_WIDTH);
    let pad = ((avail - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width - 16.0);
            add(ui);
        });
    });
}

fn bubble(ui: &mut egui::Ui, fill: egui::Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

#[allow(clippy::too_many_arguments)]
fn user_message(
    ui: &mut egui::Ui,
    images: &mut ImageCache,
    conv: Uuid,
    idx: usize,
    msg: &ChatMessage,
    running: bool,
    actions: &mut Vec<Action>,
) {
    let fill = ui
        .visuals()
        .faint_bg_color
        .lerp_to_gamma(ui.visuals().selection.bg_fill, 0.25);
    bubble(ui, fill, |ui| {
        ui.horizontal(|ui| {
            ui.strong("You");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !running
                    && ui
                        .small_button("✏")
                        .on_hover_text("Edit and resend")
                        .clicked()
                {
                    actions.push(Action::EditFrom(conv, idx));
                }
                if ui.small_button("📋").on_hover_text("Copy").clicked() {
                    actions.push(Action::Copy(msg.content.clone()));
                }
            });
        });
        if !msg.content.is_empty() {
            ui.add(egui::Label::new(&msg.content).selectable(true).wrap());
        }
        if !msg.attachments.is_empty()
            && let Some((uri, bytes, name)) =
                attach_ui::show(ui, images, &format!("{conv}-{idx}"), &msg.attachments)
        {
            actions.push(Action::ViewImage(uri, bytes, name));
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn assistant_message(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    images: &mut ImageCache,
    conv: Uuid,
    idx: usize,
    msg: &ChatMessage,
    results: &HashMap<&str, &ChatMessage>,
    run: Option<&Run>,
    streaming: bool,
    show_stats: bool,
    actions: &mut Vec<Action>,
) {
    bubble(ui, ui.visuals().extreme_bg_color, |ui| {
        ui.horizontal(|ui| {
            ui.strong("Assistant");
            if streaming && msg.content.is_empty() && msg.tool_calls.is_empty() {
                ui.spinner();
            }
        });
        if let Some(r) = msg.reasoning.as_deref().filter(|r| !r.trim().is_empty()) {
            egui::CollapsingHeader::new(egui::RichText::new("Thinking").weak())
                .id_salt(("reasoning", conv, idx))
                .default_open(streaming && msg.content.is_empty())
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(r).weak().italics())
                            .selectable(true)
                            .wrap(),
                    );
                });
        }
        if !msg.content.is_empty() {
            CommonMarkViewer::new().show(ui, md_cache, &msg.content);
        }
        for call in &msg.tool_calls {
            tool_call_view(
                ui,
                images,
                conv,
                call,
                results.get(call.id.as_str()).copied(),
                run,
                actions,
            );
        }
        if show_stats && let Some(s) = &msg.stats {
            crate::ui::stats_view::request_line(ui, s);
        }
    });
}

fn tool_call_view(
    ui: &mut egui::Ui,
    images: &mut ImageCache,
    conv: Uuid,
    call: &ToolCall,
    result: Option<&ChatMessage>,
    run: Option<&Run>,
    actions: &mut Vec<Action>,
) {
    let pending_approval = run.is_some_and(|r| r.approvals.iter().any(|(c, _)| c.id == call.id));
    let running = run.is_some_and(|r| r.running_tool.as_deref() == Some(call.id.as_str()))
        && result.is_none();
    ui.add_space(4.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (icon, color) = match result {
                Some(r) if r.is_error => ("✖", ui.visuals().error_fg_color),
                Some(_) => ("✔", egui::Color32::from_rgb(60, 170, 90)),
                None => ("🔧", ui.visuals().text_color()),
            };
            ui.colored_label(color, icon);
            ui.strong(McpManager::display_name(&call.name));
            if running {
                ui.spinner();
                ui.weak("running…");
            }
        });

        let pretty_args = serde_json::from_str::<serde_json::Value>(&call.arguments)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| call.arguments.clone());
        if pending_approval {
            // Always show what the user is being asked to approve.
            code_block(ui, &pretty_args);
        } else {
            egui::CollapsingHeader::new("Arguments")
                .id_salt(("args", &call.id))
                .show(ui, |ui| code_block(ui, &pretty_args));
        }

        if let Some(r) = result {
            let title = if r.is_error {
                "Result (error)"
            } else {
                "Result"
            };
            egui::CollapsingHeader::new(title)
                .id_salt(("result", &call.id))
                .show(ui, |ui| {
                    const LIMIT: usize = 20_000;
                    if r.content.len() > LIMIT {
                        let mut end = LIMIT;
                        while !r.content.is_char_boundary(end) {
                            end -= 1;
                        }
                        code_block(
                            ui,
                            &format!(
                                "{}\n… ({} more bytes)",
                                &r.content[..end],
                                r.content.len() - end
                            ),
                        );
                    } else {
                        code_block(ui, &r.content);
                    }
                });
            if !r.attachments.is_empty()
                && let Some((uri, bytes, name)) =
                    attach_ui::show(ui, images, &format!("tool-{}", call.id), &r.attachments)
            {
                actions.push(Action::ViewImage(uri, bytes, name));
            }
        } else if pending_approval {
            ui.horizontal(|ui| {
                ui.label("Allow this tool call?");
                if ui.button("✔ Run").clicked() {
                    actions.push(Action::Approve(conv, call.id.clone(), true));
                }
                if ui.button("✖ Deny").clicked() {
                    actions.push(Action::Approve(conv, call.id.clone(), false));
                }
            });
        } else if run.is_none() {
            ui.weak("Not run");
        }
    });
}

impl eframe::App for ChatApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.handle_shortcuts(ctx);
        self.handle_dropped_files(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("top-bar").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(240.0)
            .size_range(160.0..=420.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("input").show(ui, |ui| self.input_area(ui));
        let actions = egui::CentralPanel::default_margins()
            .show(ui, |ui| self.chat_view(ui))
            .inner;
        self.apply_actions(actions, &ctx);

        self.prompt_form_window(&ctx);
        if let Some(image) = &self.viewing
            && !attach_ui::viewer(&ctx, image)
        {
            self.viewing = None;
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            // Show where dropped files will go.
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop-overlay"),
            ));
            painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to attach",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }

        if let Some(mut sui) = self.settings_ui.take() {
            let views = self.mcp.snapshot();
            let mut keep = true;
            for action in sui.show(&ctx, &self.settings, &views) {
                match action {
                    SettingsAction::Apply(s) => {
                        self.apply_settings(s, &ctx);
                        // Keep fields the user is still editing, but pick up normalisation.
                        sui.draft.active_provider = self.settings.active_provider;
                    }
                    SettingsAction::Close => keep = false,
                    SettingsAction::ExportSettings { include_secrets } => {
                        let doc = export::settings_document(&self.settings, include_secrets);
                        sui.export_status = run_export("settings", &doc);
                    }
                    SettingsAction::ImportSettings => {
                        sui.export_status = import_file().map(|r| {
                            r.and_then(|imported| match imported {
                                export::Imported::Settings(s) => {
                                    sui.pending_import = Some(s);
                                    Ok("Choose how to import the settings below.".to_owned())
                                }
                                export::Imported::Conversations(_) => Err(
                                    "That file is a conversations export; use Import conversations.".to_owned(),
                                ),
                            })
                        });
                    }
                    SettingsAction::ImportConversations => {
                        sui.export_status = import_file().map(|r| {
                            r.and_then(|imported| match imported {
                                export::Imported::Conversations(list) => {
                                    Ok(self.import_conversations(list))
                                }
                                export::Imported::Settings(_) => {
                                    Err("That file is a settings export; use Import settings."
                                        .to_owned())
                                }
                            })
                        });
                    }
                    SettingsAction::ExportConversations => {
                        let doc = export::conversations_document(&self.conversations);
                        sui.export_status = run_export("conversations", &doc);
                    }
                    SettingsAction::Reconnect(id) => self.mcp.connect(&id),
                    SettingsAction::RefreshTools(id) => self.mcp.refresh_tools(&id),
                    SettingsAction::DetectKind(p) => self.spawn_detect(p, true, &ctx),
                    SettingsAction::FetchModels(p) => {
                        self.spawn_detect(p.clone(), false, &ctx);
                        let (http, tx, ctx) = (self.http.clone(), self.tx.clone(), ctx.clone());
                        self.rt.spawn(async move {
                            let result = llm::list_models(&http, &p)
                                .await
                                .map_err(|e| format!("{e:#}"));
                            let _ = tx.send(AppEvent::ModelsFetched {
                                provider_id: p.id.clone(),
                                result,
                            });
                            ctx.request_repaint();
                        });
                    }
                }
            }
            if keep {
                self.settings_ui = Some(sui);
            }
        }
    }
}

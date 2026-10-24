use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::{self, AgentRequest, AppEvent, ChatEvent, EventSender};
use crate::approval::{AlwaysAllow, Approval, PendingApproval};
use crate::attach::Attachment;
use crate::config::{Settings, Theme};
use crate::export;
use crate::llm;
use crate::mcp::manager::McpManager;
use crate::subagent::Delegation;
use crate::tools::{ToolOverrides, ToolSelection};
use crate::types::{ChatMessage, Conversation, Role};
use crate::ui::attachments::{self as attach_ui, ImageCache};
use crate::ui::chat_view::{self, centered};
use crate::ui::palette::{Command, Entry, Palette};
use crate::ui::settings::{SettingsAction, SettingsUi, Tab};
use crate::ui::theme::{self, icon};
use crate::ui::widgets::icon_button;
use egui::RichText;

const INPUT_ID: &str = "chat-input";

pub(crate) struct Run {
    task: JoinHandle<()>,
    pub(crate) approvals: Vec<PendingApproval>,
    pub(crate) running_tool: Option<String>,
    /// Stats of the request currently streaming in, for the readout above the input box.
    pub(crate) live_stats: Option<crate::stats::RequestStats>,
    /// Live subagent progress, by `delegate` call id.
    pub(crate) subagents: HashMap<String, Vec<crate::subagent::SubagentReport>>,
}

pub(crate) enum Action {
    Approve(Uuid, String, Approval),
    Copy(String),
    Regenerate(Uuid),
    EditFrom(Uuid, usize),
    ViewImage(String, egui::load::Bytes, String),
    /// Save a drawing from a reply as an SVG file.
    SaveSvg(Vec<u8>),
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
    /// First name for the greeting, from the OS account.
    user_name: Option<String>,
    /// Tools and servers the user chose to always allow, per chat, until the app quits.
    always_allow: HashMap<Uuid, AlwaysAllow>,
    md_cache: CommonMarkCache,
    drawings: crate::ui::markdown::DrawingCache,
    settings_ui: Option<SettingsUi>,
    renaming: Option<(Uuid, String)>,
    search: String,
    notice: Option<String>,
    keychain_warned: bool,
    pending_attachments: Vec<Attachment>,
    /// Files still being read for `pending_attachments`.
    reading_files: usize,
    images: ImageCache,
    /// MCP server state for the composer and top bar, re-read when the manager changes.
    mcp_views: (u64, Arc<Vec<crate::mcp::manager::ServerView>>),
    viewing: Option<(String, egui::load::Bytes, String)>,
    prompt_form: Option<PromptForm>,
    /// For background tasks to request a repaint when their results arrive.
    repaint_ctx: egui::Context,
    palette: Palette,
    sidebar_open: bool,
    /// Tool switches made before the first message, applied to the chat it creates.
    draft_tool_overrides: ToolOverrides,
    /// A newer release found on GitHub.
    update: Option<crate::update::Release>,
    /// Progress of installing `update`, once the user starts it.
    update_install: Option<UpdateInstall>,
}

enum UpdateInstall {
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    /// Installed; restarting runs the new version from this path.
    Ready(std::path::PathBuf),
    /// Installing failed (the reason went to `notice`); the release page is the way now.
    Failed,
}

/// Arguments being filled in for an MCP prompt.
struct PromptForm {
    server_id: String,
    prompt: crate::mcp::McpPrompt,
    values: std::collections::BTreeMap<String, String>,
}

/// A file to attach: picked in the file dialog, or dropped on the window.
enum FileSource {
    Path(std::path::PathBuf),
    Dropped(egui::DroppedFileHandle),
}

impl FileSource {
    fn read(self) -> anyhow::Result<Attachment> {
        match self {
            Self::Path(p) => Attachment::from_path(&p),
            Self::Dropped(f) => {
                let name = f
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into());
                let bytes = f.bytes().map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
                Attachment::from_bytes(name, bytes)
            }
        }
    }
}

/// "aaron.bolton" or "aaron" → "Aaron", from the OS account; `None` for root or when unset.
fn user_first_name() -> Option<String> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()?;
    let first = user.split(['.', '_', '-', ' ']).next()?.trim();
    if first.is_empty() || first == "root" {
        return None;
    }
    let mut chars = first.chars();
    let head = chars.next()?.to_uppercase();
    Some(head.chain(chars).collect())
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
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
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
            user_name: user_first_name(),
            always_allow: HashMap::new(),
            md_cache: CommonMarkCache::default(),
            drawings: Default::default(),
            settings_ui: None,
            renaming: None,
            search: String::new(),
            notice: loaded.notice,
            keychain_warned: false,
            pending_attachments: Vec::new(),
            reading_files: 0,
            images: ImageCache::default(),
            mcp_views: (u64::MAX, Arc::default()),
            viewing: None,
            prompt_form: None,
            repaint_ctx: cc.egui_ctx.clone(),
            palette: Palette::default(),
            sidebar_open: true,
            draft_tool_overrides: ToolOverrides::default(),
            update: None,
            update_install: None,
        };
        if app.settings.general.check_for_updates {
            app.spawn_update_check(false, app.settings.general.update_url.clone());
        }
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
        let draft = std::mem::take(&mut self.draft_tool_overrides);
        if !draft.is_empty()
            && let Some(id) = self.current
            && let Some(c) = self.conv_mut(id)
        {
            c.tool_overrides = draft;
        }
        self.input.clear();
    }

    /// The current chat's tool switches (or the draft ones before a chat exists).
    fn tool_overrides(&self) -> &ToolOverrides {
        self.current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .map_or(&self.draft_tool_overrides, |c| &c.tool_overrides)
    }

    fn set_tool_override(&mut self, key: String, on: bool, default: bool) {
        match self.current.and_then(|id| self.conv_mut(id)) {
            Some(conv) => {
                conv.tool_overrides.set(key, on, default);
                if !conv.messages.is_empty() {
                    conv.save();
                }
            }
            None => self.draft_tool_overrides.set(key, on, default),
        }
    }

    fn reset_tool_overrides(&mut self) {
        match self.current.and_then(|id| self.conv_mut(id)) {
            Some(conv) => {
                conv.tool_overrides = ToolOverrides::default();
                if !conv.messages.is_empty() {
                    conv.save();
                }
            }
            None => self.draft_tool_overrides = ToolOverrides::default(),
        }
    }

    /// Dropdown next to the input: switch built-in tools, MCP servers and single MCP tools
    /// on or off for this chat. Settings holds the defaults.
    fn tools_menu(&mut self, ui: &mut egui::Ui) {
        use crate::tools::BUILTINS;
        use egui::containers::menu::{MenuButton, MenuConfig};

        let overrides = self.tool_overrides().clone();
        let sel = ToolSelection::new(&self.settings, &overrides);
        let views = self.mcp_views();
        let servers: Vec<(String, String)> = self
            .settings
            .mcp_servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let has_workspace = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .is_some_and(|c| c.workspace.is_some());
        let workspace_on = if has_workspace {
            crate::tools::workspace::TOOLS
                .iter()
                .filter(|(k, _)| sel.builtin_on(k))
                .count()
        } else {
            0
        };
        let builtins_on = BUILTINS
            .iter()
            .filter(|(_, k, _)| sel.builtin_on(k))
            .count()
            + workspace_on;
        let mcp_on: usize = views
            .iter()
            .filter(|v| v.status == crate::mcp::manager::ServerStatus::Connected)
            .map(|v| {
                v.tools
                    .iter()
                    .filter(|t| sel.tool_on(&v.id, &t.name))
                    .count()
            })
            .sum();

        let mut changes: Vec<(String, bool, bool)> = Vec::new();
        let mut reset = false;
        let dark = ui.visuals().dark_mode;
        let label =
            RichText::new(format!("{}  {}", icon::SLIDERS, builtins_on + mcp_on)).size(14.0);
        let label = if overrides.is_empty() {
            label
        } else {
            label.color(theme::accent(dark))
        };
        let (resp, _) =
            MenuButton::from_button(egui::Button::new(label).frame_when_inactive(false))
                .config(
                    MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| {
                    ui.set_min_width(280.0);
                    ui.label(RichText::new("Tools for this chat").strong());
                    ui.label(
                        RichText::new("Applies from the next message. Defaults are in Settings.")
                            .small()
                            .weak(),
                    );
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new("Built-in").small().weak());
                            for (_, key, name) in BUILTINS {
                                let mut on = sel.builtin_on(key);
                                let ic = match key {
                                    "time" => icon::CLOCK,
                                    "calculator" => icon::CALCULATOR,
                                    "tasks" => icon::LIST_CHECKS,
                                    "fetch" => icon::GLOBE,
                                    _ => icon::ROBOT,
                                };
                                if ui.checkbox(&mut on, format!("{ic}  {name}")).changed() {
                                    changes.push((
                                        ToolOverrides::builtin_key(key),
                                        on,
                                        sel.builtin_default(key),
                                    ));
                                }
                            }
                            if has_workspace {
                                ui.add_space(4.0);
                                ui.separator();
                                ui.label(RichText::new("Workspace").small().weak());
                                for (key, name) in crate::tools::workspace::TOOLS {
                                    let mut on = sel.builtin_on(key);
                                    let ic = if key == crate::tools::workspace::SHELL {
                                        icon::TERMINAL
                                    } else {
                                        icon::FOLDER
                                    };
                                    if ui.checkbox(&mut on, format!("{ic}  {name}")).changed() {
                                        changes.push((
                                            ToolOverrides::builtin_key(key),
                                            on,
                                            sel.builtin_default(key),
                                        ));
                                    }
                                }
                            }
                            for (id, name) in &servers {
                                ui.add_space(4.0);
                                ui.separator();
                                let view = views.iter().find(|v| &v.id == id);
                                let connected = view.is_some_and(|v| {
                                    v.status == crate::mcp::manager::ServerStatus::Connected
                                });
                                let mut on = sel.server_on(id);
                                let resp = ui.checkbox(&mut on, RichText::new(name).strong());
                                if resp.changed() {
                                    changes.push((ToolOverrides::server_key(id), on, true));
                                }
                                match view {
                                    Some(v) if connected && !v.tools.is_empty() => {
                                        ui.indent(("tools", id), |ui| {
                                            ui.add_enabled_ui(on, |ui| {
                                                for t in &v.tools {
                                                    let default = sel.tool_default(id, &t.name);
                                                    let mut t_on = overrides
                                                        .get(&ToolOverrides::tool_key(id, &t.name))
                                                        .unwrap_or(default);
                                                    let r = ui.checkbox(&mut t_on, &t.name);
                                                    let r = if t.description.is_empty() {
                                                        r
                                                    } else {
                                                        r.on_hover_text(&t.description)
                                                    };
                                                    if r.changed() {
                                                        changes.push((
                                                            ToolOverrides::tool_key(id, &t.name),
                                                            t_on,
                                                            default,
                                                        ));
                                                    }
                                                }
                                            });
                                        });
                                    }
                                    Some(_) if connected => {
                                        ui.weak("No tools");
                                    }
                                    _ => {
                                        ui.weak("Not connected");
                                    }
                                }
                            }
                        });
                    if !overrides.is_empty() {
                        ui.separator();
                        if ui
                            .button(format!("{}  Reset to defaults", icon::REGENERATE))
                            .clicked()
                        {
                            reset = true;
                        }
                    }
                });
        resp.on_hover_text(format!(
            "Tools for this chat: {} on{}",
            builtins_on + mcp_on,
            if overrides.is_empty() {
                ""
            } else {
                " (changed from the defaults)"
            }
        ));
        for (key, on, default) in changes {
            self.set_tool_override(key, on, default);
        }
        if reset {
            self.reset_tool_overrides();
        }
    }

    fn delete_chat(&mut self, id: Uuid) {
        self.stop(id);
        Conversation::delete(id);
        self.conversations.retain(|c| c.id != id);
        self.errors.remove(&id);
        self.always_allow.remove(&id);
        if self.current == Some(id) {
            self.current = self.conversations.first().map(|c| c.id);
        }
    }

    fn send(&mut self, ctx: &egui::Context) {
        let text = self.input.trim().to_owned();
        if text.is_empty() && self.pending_attachments.is_empty() {
            return;
        }
        if self.reading_files > 0 {
            self.notice = Some("Wait for the attached files to finish reading.".into());
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
        let selection = ToolSelection::new(&self.settings, &conv.tool_overrides);
        let req = AgentRequest {
            conv_id: id,
            provider: self.settings.provider().clone(),
            general: self.settings.general.clone(),
            history: conv
                .messages
                .iter()
                .filter(|m| !m.cancelled)
                .cloned()
                .collect(),
            delegation: Delegation::new(&self.settings, &selection),
            selection,
            workspace: conv.workspace.clone(),
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
                live_stats: None,
                subagents: HashMap::new(),
            },
        );
    }

    /// Cancel a generation. The prompt it was answering stays in the chat, marked cancelled, so
    /// it can be edited and resent; it and any partial reply are left out of what the next
    /// prompt sends to the model.
    fn stop(&mut self, id: Uuid) {
        let Some(run) = self.runs.remove(&id) else {
            return;
        };
        run.task.abort();
        let Some(conv) = self.conv_mut(id) else {
            return;
        };
        if let Some(at) = conv.messages.iter().rposition(|m| m.role == Role::User) {
            conv.messages.truncate(at + 1);
            conv.messages[at].cancelled = true;
        }
        conv.updated_at = chrono::Utc::now();
        conv.save();
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
        let Some(last) = conv.messages.last_mut() else {
            return;
        };
        last.cancelled = false;
        self.start_run(id, ctx);
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::UpdateChecked { result, explicit } => {
                    use crate::update::UpdateCheck;
                    if let Some(ui) = self.settings_ui.as_mut().filter(|_| explicit) {
                        ui.checking_update = false;
                        ui.update_status = Some(match &result {
                            Ok(UpdateCheck::UpToDate) => Ok(format!(
                                "You're on the latest version (v{}).",
                                env!("CARGO_PKG_VERSION")
                            )),
                            Ok(UpdateCheck::Available(r)) => {
                                Ok(format!("Version {} is available.", r.version))
                            }
                            Err(e) => Err(format!("Update check failed: {e}")),
                        });
                        ui.update_available = matches!(
                            &result,
                            Ok(UpdateCheck::Available(r)) if crate::update::can_install(r)
                        );
                    }
                    match result {
                        Ok(UpdateCheck::Available(r)) => self.update = Some(r),
                        Ok(UpdateCheck::UpToDate) => self.update = None,
                        Err(e) => log::info!("update check failed: {e}"),
                    }
                }
                AppEvent::UpdateProgress { done, total } => {
                    if let Some(UpdateInstall::Downloading { .. }) = self.update_install {
                        self.update_install = Some(UpdateInstall::Downloading { done, total });
                    }
                }
                AppEvent::UpdateInstalled(result) => {
                    self.update_install = Some(match result {
                        Ok(target) => UpdateInstall::Ready(target),
                        Err(e) => {
                            self.notice = Some(format!("Couldn't install the update: {e}"));
                            UpdateInstall::Failed
                        }
                    });
                }
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
                AppEvent::FileAttached { result } => {
                    self.reading_files = self.reading_files.saturating_sub(1);
                    match result {
                        Ok(a) => self.pending_attachments.push(a),
                        // Several files can fail in one drop; list them together.
                        Err(e) => match &mut self.notice {
                            Some(n) if n.starts_with("Couldn't attach: ") => {
                                n.push_str("; ");
                                n.push_str(&e);
                            }
                            n => *n = Some(format!("Couldn't attach: {e}")),
                        },
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
            ChatEvent::ToolApproval {
                call,
                origin,
                respond,
            } => {
                let server_name = origin.as_ref().and_then(|(server, _)| {
                    self.settings
                        .mcp_servers
                        .iter()
                        .find(|s| &s.id == server)
                        .map(|s| s.name.clone())
                });
                let pending = PendingApproval::new(call, origin, server_name, respond);
                if self
                    .always_allow
                    .get(&id)
                    .is_some_and(|a| a.allows(&pending))
                {
                    pending.answer(true);
                } else {
                    self.runs.get_mut(&id).unwrap().approvals.push(pending);
                }
                return;
            }
            ChatEvent::ToolRunning(call_id) => {
                self.runs.get_mut(&id).unwrap().running_tool = Some(call_id);
                return;
            }
            ChatEvent::StatsLive(s) => {
                self.runs.get_mut(&id).unwrap().live_stats = Some(s);
                return;
            }
            ChatEvent::SubagentProgress { call_id, reports } => {
                self.runs
                    .get_mut(&id)
                    .unwrap()
                    .subagents
                    .insert(call_id, reports);
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
        let is_final = matches!(event, ChatEvent::AssistantFinal(_));
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
        } else {
            // The final stats are attached to the message now, so drop the live readout.
            if is_final && let Some(run) = self.runs.get_mut(&id) {
                run.live_stats = None;
            }
            if is_tool_result && let Some(run) = self.runs.get_mut(&id) {
                run.running_tool = None;
            }
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
    fn mcp_menu(&mut self, ui: &mut egui::Ui) {
        let views = self.mcp_views();
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
            let w = &mut ui.visuals_mut().widgets.inactive;
            w.bg_stroke = egui::Stroke::NONE;
            w.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.menu_button(RichText::new(icon::BOOKS).size(16.0), |ui| {
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
    /// The MCP servers' state, snapshotted again only after the manager reports a change
    /// (the composer menus and top bar need it every frame).
    fn mcp_views(&mut self) -> Arc<Vec<crate::mcp::manager::ServerView>> {
        let version = self.mcp.version();
        if self.mcp_views.0 != version {
            self.mcp_views = (version, Arc::new(self.mcp.snapshot()));
        }
        self.mcp_views.1.clone()
    }

    fn insert_prompt(&mut self, mut messages: Vec<crate::mcp::PromptMessage>) {
        // Messages before the last go straight into the chat, which can't change mid-run.
        let adds_history =
            messages.len() > 1 || messages.last().is_some_and(|m| m.role != Role::User);
        if adds_history && self.current.is_some_and(|id| self.runs.contains_key(&id)) {
            self.notice = Some("Wait for the current response to finish.".into());
            return;
        }
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
        self.read_files(paths.into_iter().map(FileSource::Path).collect());
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        self.read_files(dropped.into_iter().map(FileSource::Dropped).collect());
    }

    /// Read files into attachments off the UI thread, since extracting a large document's
    /// text takes a moment. Each arrives as an `AppEvent::FileAttached`.
    fn read_files(&mut self, files: Vec<FileSource>) {
        if files.is_empty() {
            return;
        }
        self.reading_files += files.len();
        let (tx, ctx) = (self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn_blocking(move || {
            for file in files {
                let result = file.read().map_err(|e| format!("{e:#}"));
                let _ = tx.send(AppEvent::FileAttached { result });
                ctx.request_repaint();
            }
        });
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

    fn spawn_update_check(&self, explicit: bool, setting: String) {
        let (http, tx, ctx) = (self.http.clone(), self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn(async move {
            let result = crate::update::check(
                &http,
                &crate::update::release_api_url(&setting),
                env!("CARGO_PKG_VERSION"),
            )
            .await
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::UpdateChecked { result, explicit });
            ctx.request_repaint();
        });
    }

    /// Download and install `self.update` in the background; progress shows in the top bar.
    fn start_update_install(&mut self) {
        if self.update_install.is_some() {
            return;
        }
        let Some(release) = self.update.clone() else {
            return;
        };
        let (Some((name, package)), Ok(target)) = (
            crate::update::platform_package(),
            crate::update::install_target(),
        ) else {
            return;
        };
        self.update_install = Some(UpdateInstall::Downloading {
            done: 0,
            total: None,
        });
        let (http, tx, ctx) = (self.http.clone(), self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn(async move {
            // Report whole percent steps (or each MiB when the size is unknown), not every chunk.
            let last = std::sync::atomic::AtomicU64::new(u64::MAX);
            let result = crate::update::install(
                &http,
                &release,
                (&name, package),
                &target,
                |done, total| {
                    let step = total.map_or(done >> 20, |t| done * 100 / t.max(1));
                    if last.swap(step, std::sync::atomic::Ordering::Relaxed) != step {
                        let _ = tx.send(AppEvent::UpdateProgress { done, total });
                        ctx.request_repaint();
                    }
                },
            )
            .await
            .map(|()| target)
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(AppEvent::UpdateInstalled(result));
            ctx.request_repaint();
        });
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
        let (new_chat, settings, palette, sidebar) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::N),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::K),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::B),
            )
        });
        if new_chat {
            self.settings_ui = None;
            self.new_chat();
        }
        if settings {
            self.open_settings(Tab::Providers);
        }
        if palette {
            self.palette.toggle();
        }
        if sidebar {
            self.sidebar_open = !self.sidebar_open;
        }
    }

    /// Everything the command palette can do right now.
    fn palette_entries(&self) -> Vec<Entry> {
        let mut e = vec![
            Entry::new(icon::PLUS, "New chat", "Chat", Command::NewChat),
            Entry::new(icon::PAPERCLIP, "Attach files…", "Chat", Command::Attach),
            Entry::new(
                icon::DOWNLOAD,
                "Export this chat as Markdown…",
                "Chat",
                Command::ExportChatMarkdown,
            ),
            Entry::new(
                icon::SIDEBAR,
                "Toggle sidebar",
                "View",
                Command::ToggleSidebar,
            ),
            Entry::new(
                icon::MONITOR,
                "System theme",
                "Theme",
                Command::SetTheme(Theme::System),
            ),
            Entry::new(
                icon::SUN,
                "Light theme",
                "Theme",
                Command::SetTheme(Theme::Light),
            ),
            Entry::new(
                icon::MOON,
                "Dark theme",
                "Theme",
                Command::SetTheme(Theme::Dark),
            ),
            Entry::new(
                icon::SERVER,
                "Providers",
                "Settings",
                Command::OpenSettings(Tab::Providers),
            ),
            Entry::new(
                icon::PLUGS,
                "MCP servers",
                "Settings",
                Command::OpenSettings(Tab::Mcp),
            ),
            Entry::new(
                icon::TOOLBOX,
                "Built-in tools",
                "Settings",
                Command::OpenSettings(Tab::Tools),
            ),
            Entry::new(
                icon::ROBOT,
                "Subagents",
                "Settings",
                Command::OpenSettings(Tab::Subagents),
            ),
            Entry::new(
                icon::GEAR,
                "General",
                "Settings",
                Command::OpenSettings(Tab::General),
            ),
            Entry::new(
                icon::EXPORT,
                "Export settings…",
                "Data",
                Command::ExportSettings,
            ),
            Entry::new(
                icon::EXPORT,
                "Export conversations…",
                "Data",
                Command::ExportConversations,
            ),
            Entry::new(
                icon::DOWNLOAD,
                "Import settings…",
                "Data",
                Command::ImportSettings,
            ),
            Entry::new(
                icon::DOWNLOAD,
                "Import conversations…",
                "Data",
                Command::ImportConversations,
            ),
            Entry::new(
                icon::DOWNLOAD,
                "Check for updates",
                "App",
                Command::CheckForUpdates,
            ),
        ];
        for (i, p) in self.settings.providers.iter().enumerate() {
            e.push(Entry::new(
                icon::SERVER,
                format!("Use {}", p.name),
                "Provider",
                Command::UseProvider(i),
            ));
        }
        for m in &self.settings.provider().models {
            e.push(Entry::new(
                icon::SPARKLE,
                m.clone(),
                "Model",
                Command::UseModel(m.clone()),
            ));
        }
        for c in &self.conversations {
            e.push(Entry::new(
                icon::CHAT,
                c.title.clone(),
                "Chat",
                Command::OpenChat(c.id),
            ));
        }
        e
    }

    fn run_command(&mut self, cmd: Command, ctx: &egui::Context) {
        match cmd {
            Command::NewChat => {
                self.settings_ui = None;
                self.new_chat();
            }
            Command::OpenChat(id) => {
                self.settings_ui = None;
                self.current = Some(id);
            }
            Command::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Command::OpenSettings(tab) => self.open_settings(tab),
            Command::UseProvider(i) => {
                self.settings.active_provider = i;
                self.save_settings();
            }
            Command::UseModel(m) => {
                self.settings.provider_mut().model = m;
                self.save_settings();
            }
            Command::SetTheme(t) => {
                self.settings.general.theme = t;
                apply_appearance(ctx, &self.settings);
                self.save_settings();
            }
            Command::Attach => {
                if let Some(paths) = rfd::FileDialog::new()
                    .set_title("Attach files")
                    .pick_files()
                {
                    self.attach_paths(paths);
                }
            }
            Command::ExportChatMarkdown => {
                if let Some(id) = self.current {
                    self.export_markdown(id);
                }
            }
            Command::ExportSettings
            | Command::ExportConversations
            | Command::ImportSettings
            | Command::ImportConversations => {
                // These live on the General settings page, which shows their result and
                // (for settings imports) the merge/replace choice.
                self.open_settings(Tab::General);
                if let Some(sui) = &mut self.settings_ui {
                    sui.queue(match cmd {
                        Command::ExportSettings => SettingsAction::ExportSettings {
                            include_secrets: false,
                        },
                        Command::ExportConversations => SettingsAction::ExportConversations,
                        Command::ImportSettings => SettingsAction::ImportSettings,
                        _ => SettingsAction::ImportConversations,
                    });
                }
            }
            Command::CheckForUpdates => {
                self.open_settings(Tab::General);
                if let Some(sui) = &mut self.settings_ui {
                    sui.checking_update = true;
                    sui.update_status = None;
                }
                self.spawn_update_check(true, self.settings.general.update_url.clone());
            }
        }
    }

    // ---------------------------------------------------------------- top bar

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let mut start_install = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            // With the sidebar open, its own header has the toggle.
            if !self.sidebar_open && icon_button(ui, icon::SIDEBAR, "Show sidebar (⌘B)").clicked()
            {
                self.sidebar_open = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, icon::GEAR, "Settings (⌘,)").clicked() {
                    self.open_settings(Tab::Providers);
                }
                if icon_button(ui, icon::COMMAND, "Command palette (⌘K)").clicked() {
                    self.palette.toggle();
                }
                if let Some(r) = &self.update {
                    let link = |label: String| {
                        egui::Button::new(RichText::new(label).small().color(p.signal))
                            .frame_when_inactive(false)
                    };
                    match &self.update_install {
                        Some(UpdateInstall::Downloading { done, total }) => {
                            let label = match total {
                                Some(t) => format!(
                                    "Downloading {}… {}%",
                                    r.version,
                                    done * 100 / (*t).max(1)
                                ),
                                None => format!("Downloading {}…", r.version),
                            };
                            ui.label(RichText::new(label).small().weak());
                            ui.spinner();
                        }
                        Some(UpdateInstall::Ready(target)) => {
                            if ui
                                .add(link("Restart to finish updating".into()))
                                .on_hover_text(format!("Version {} is installed", r.version))
                                .clicked()
                            {
                                match crate::update::relaunch(target) {
                                    Ok(()) => {
                                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close)
                                    }
                                    Err(e) => self.notice = Some(format!("{e:#}")),
                                }
                            }
                        }
                        Some(UpdateInstall::Failed) | None => {
                            let install =
                                self.update_install.is_none() && crate::update::can_install(r);
                            let (label, hover) = if install {
                                (
                                    format!("Update to {}", r.version),
                                    "Download and install it, then restart",
                                )
                            } else {
                                (
                                    format!("Download {}", r.version),
                                    "Open the release page to download it",
                                )
                            };
                            if ui.add(link(label)).on_hover_text(hover).clicked() {
                                if install {
                                    start_install = true;
                                } else if let Err(e) = open::that_detached(&r.url) {
                                    self.notice = Some(format!("Couldn't open the browser: {e}"));
                                }
                            }
                        }
                    }
                }
                let (servers, tools) = self.mcp.connected_tool_count();
                let configured = self
                    .settings
                    .mcp_servers
                    .iter()
                    .filter(|s| s.enabled)
                    .count();
                if configured > 0 {
                    let needs_sign_in = self
                        .mcp_views()
                        .iter()
                        .any(|v| v.status == crate::mcp::manager::ServerStatus::NeedsSignIn);
                    let (dot, hover) = if needs_sign_in {
                        (p.warn, "An MCP server needs you to sign in")
                    } else if servers == configured {
                        (p.ok, "MCP servers connected")
                    } else {
                        (p.pencil, "Some MCP servers aren't connected")
                    };
                    let text = (
                        RichText::new("●").size(9.0).color(dot),
                        RichText::new(format!("{tools} tools"))
                            .small()
                            .color(p.pencil),
                    );
                    if ui
                        .add(egui::Button::new(text).frame_when_inactive(false))
                        .on_hover_text(hover)
                        .clicked()
                    {
                        self.open_settings(Tab::Mcp);
                    }
                }
            });
        });
        if start_install {
            self.start_update_install();
        }
    }

    /// The chat's working folder, under the composer: "Set working directory", or the
    /// folder's name with a button to clear it.
    fn workspace_picker(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        let current = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .and_then(|c| c.workspace.clone());
        let label = match &current {
            Some(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                RichText::new(theme::labelled(icon::FOLDER, &name)).color(p.ink2)
            }
            None => RichText::new(theme::labelled(icon::FOLDER, "Set working directory"))
                .color(p.pencil),
        };
        let hover = match &current {
            Some(path) => format!(
                "{}\nThe model can read, search, edit and run commands in this folder. Click to change.",
                path.display()
            ),
            None => "Choose a local folder for this chat; the model gets tools to read, \
                     search, edit and run commands in it"
                .to_owned(),
        };
        let resp = ui
            .add(egui::Button::new(label.small()).frame_when_inactive(false))
            .on_hover_text(hover);
        if resp.clicked() {
            let mut dialog = rfd::FileDialog::new().set_title("Choose a working folder");
            if let Some(path) = &current {
                dialog = dialog.set_directory(path);
            }
            if let Some(folder) = dialog.pick_folder() {
                self.set_workspace(Some(folder));
            }
        }
        if current.is_some()
            && ui
                .add(
                    egui::Button::new(RichText::new(icon::CLOSE).size(11.0).color(p.pencil))
                        .frame_when_inactive(false),
                )
                .on_hover_text("Stop using this folder")
                .clicked()
        {
            self.set_workspace(None);
        }
    }

    /// Set or clear the current chat's working folder (starting a chat if there is none).
    fn set_workspace(&mut self, folder: Option<std::path::PathBuf>) {
        if self.current.is_none_or(|id| self.conv_mut(id).is_none()) {
            self.new_chat();
        }
        let Some(conv) = self.current.and_then(|id| self.conv_mut(id)) else {
            return;
        };
        conv.workspace = folder;
        if !conv.messages.is_empty() {
            conv.save();
        }
    }

    /// Model and provider, under the composer: "model  Provider ⌄".
    fn model_picker(&mut self, ui: &mut egui::Ui) {
        use egui::containers::menu::{MenuButton, MenuConfig};
        let p = theme::palette(ui.visuals().dark_mode);
        let provider = self.settings.provider();
        let model = if provider.model.is_empty() {
            "Choose a model".to_owned()
        } else {
            provider.model.clone()
        };
        let label = (
            RichText::new(model).color(p.ink2),
            RichText::new(&provider.name).color(p.pencil),
            RichText::new(icon::CARET_DOWN).size(11.0).color(p.pencil),
        );
        let mut changed = false;
        MenuButton::from_button(egui::Button::new(label).frame_when_inactive(false))
            .config(MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                ui.set_min_width(260.0);
                ui.label(RichText::new("Provider").small().color(p.pencil));
                let names: Vec<String> = self
                    .settings
                    .providers
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                for (i, name) in names.into_iter().enumerate() {
                    changed |= ui
                        .selectable_value(&mut self.settings.active_provider, i, name)
                        .changed();
                }
                ui.separator();
                ui.label(RichText::new("Model").small().color(p.pencil));
                let provider = self.settings.provider_mut();
                if provider.models.is_empty() {
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut provider.model)
                                .hint_text("model name")
                                .desired_width(240.0),
                        )
                        .on_hover_text("Or fetch the list in Settings → Providers")
                        .lost_focus();
                } else {
                    let models = provider.models.clone();
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            for m in models {
                                changed |= ui
                                    .selectable_value(&mut provider.model, m.clone(), m)
                                    .changed();
                            }
                        });
                }
            });
        if changed {
            self.save_settings();
        }
    }

    fn open_settings(&mut self, tab: Tab) {
        if self.settings_ui.is_none() {
            self.settings_ui = Some(SettingsUi::new(&self.settings, tab));
        } else if let Some(s) = &mut self.settings_ui {
            s.set_tab(tab);
        }
    }

    // ---------------------------------------------------------------- sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if icon_button(ui, icon::SIDEBAR, "Hide sidebar (⌘B)").clicked() {
                self.sidebar_open = false;
            }
        });
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search", icon::SEARCH))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        // Sidebar rows: no outline, a soft fill when hovered or current.
        let row = |text: RichText, current: bool| {
            egui::Button::new(text)
                .frame_when_inactive(current)
                .fill(if current {
                    p.selected
                } else {
                    egui::Color32::TRANSPARENT
                })
                .stroke(egui::Stroke::NONE)
        };
        if ui
            .add(
                row(
                    RichText::new(theme::labelled(icon::PLUS, "New chat")).color(p.ink),
                    true,
                )
                .min_size(egui::vec2(ui.available_width(), 30.0)),
            )
            .on_hover_text("⌘N")
            .clicked()
        {
            self.new_chat();
        }
        if ui
            .add(
                row(
                    RichText::new(theme::labelled(icon::GEAR, "Settings")).color(p.ink2),
                    false,
                )
                .min_size(egui::vec2(ui.available_width(), 30.0)),
            )
            .clicked()
        {
            self.open_settings(Tab::Providers);
        }
        ui.add_space(4.0);

        let mut select = None;
        let mut delete = None;
        let mut export_md = None;
        let mut toggle_pin = None;
        let mut rename_commit = None;
        let needle = self.search.to_lowercase();
        let mut rows: Vec<(bool, &Conversation)> = self
            .conversations
            .iter()
            // An empty new chat isn't listed until it has a message.
            .filter(|c| !c.messages.is_empty() || self.runs.contains_key(&c.id))
            .filter(|c| {
                needle.is_empty()
                    || c.title.to_lowercase().contains(&needle)
                    || c.messages
                        .iter()
                        .any(|m| m.content.to_lowercase().contains(&needle))
            })
            .map(|c| (c.pinned, c))
            .collect();
        // Pinned chats first, then everything else, newest first.
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated_at.cmp(&a.1.updated_at)));

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if rows.is_empty() && !needle.is_empty() {
                    ui.label(RichText::new("No matching chats").small().color(p.pencil));
                }
                let mut last_group = None;
                for (group, c) in rows {
                    if last_group != Some(group) {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(if group { "Pinned" } else { "Recents" })
                                .small()
                                .color(p.pencil),
                        );
                        last_group = Some(group);
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
                    let selected = self.current == Some(c.id);
                    let title =
                        RichText::new(&c.title).color(if selected { p.ink } else { p.ink2 });
                    let mark = match (running, c.pinned) {
                        (true, _) => RichText::new(icon::HOURGLASS).size(12.0).color(p.signal),
                        (false, true) => RichText::new(icon::PIN).size(12.0).color(p.pencil),
                        _ => RichText::new(""),
                    };
                    let resp = ui.add(
                        row(title, selected)
                            .truncate()
                            .right_text(mark)
                            .min_size(egui::vec2(ui.available_width(), 30.0)),
                    );
                    let resp = resp.on_hover_text(format!(
                        "{}\n{}",
                        c.title,
                        c.updated_at
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                    ));
                    if resp.clicked() {
                        select = Some(c.id);
                    }
                    resp.context_menu(|ui| {
                        let pin_label = if c.pinned {
                            theme::labelled(icon::UNPIN, "Unpin")
                        } else {
                            theme::labelled(icon::PIN, "Pin")
                        };
                        if ui.button(pin_label).clicked() {
                            toggle_pin = Some(c.id);
                            ui.close();
                        }
                        if ui.button(theme::labelled(icon::EDIT, "Rename")).clicked() {
                            self.renaming = Some((c.id, c.title.clone()));
                            ui.close();
                        }
                        if ui
                            .button(theme::labelled(icon::DOWNLOAD, "Export as Markdown…"))
                            .clicked()
                        {
                            export_md = Some(c.id);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button(theme::labelled(icon::TRASH, "Delete")).clicked() {
                            delete = Some(c.id);
                            ui.close();
                        }
                    });
                }
            });

        if let Some(id) = toggle_pin
            && let Some(c) = self.conv_mut(id)
        {
            c.pinned = !c.pinned;
            c.save();
        }
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

    // ---------------------------------------------------------------- composer

    fn composer(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let id = egui::Id::unique(INPUT_ID);
        let running = self.current.is_some_and(|c| self.runs.contains_key(&c));
        let fresh = self
            .current
            .and_then(|c| self.conversations.iter().find(|x| x.id == c))
            .is_none_or(|c| c.messages.is_empty());
        // A new chat is ready to type into.
        if fresh && ui.memory(|m| m.focused().is_none()) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let has_focus = ui.memory(|m| m.has_focus(id));
        let mut send = false;
        if has_focus && !running {
            send = ui.input_mut(|i| {
                let cmd_enter = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
                let enter = self.settings.general.send_with_enter && consume_plain_enter(i);
                cmd_enter || enter
            });
        }

        ui.add_space(8.0);
        centered(ui, |ui| {
            if let Some(n) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(n).small().color(p.warn));
                    if icon_button(ui, icon::CLOSE, "Dismiss").clicked() {
                        self.notice = None;
                    }
                });
            }
            // Live stats for the request currently streaming in; the finished response
            // carries the same figures, so this only shows mid-completion.
            if self.settings.general.show_stats
                && let Some(s) = self
                    .current
                    .and_then(|c| self.runs.get(&c))
                    .and_then(|r| r.live_stats.as_ref())
            {
                super::ui::stats_view::live_line(ui, s);
                ui.add_space(6.0);
            }
            egui::Frame::new()
                .fill(p.raised)
                .stroke(egui::Stroke::new(
                    1.0,
                    if has_focus { p.pencil } else { p.outline },
                ))
                .corner_radius(16)
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 10,
                    top: 12,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some(n) =
                        attach_ui::pending_chips(ui, &mut self.images, &self.pending_attachments)
                    {
                        self.pending_attachments.remove(n);
                    }
                    if self.reading_files > 0 {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let n = self.reading_files;
                            ui.label(
                                RichText::new(format!(
                                    "Reading {n} file{}…",
                                    if n == 1 { "" } else { "s" }
                                ))
                                .small()
                                .weak(),
                            );
                        });
                    }
                    let hint = if fresh {
                        "How can I help you today?"
                    } else {
                        "Reply…"
                    };
                    egui::ScrollArea::vertical()
                        .id_salt("input-scroll")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.input)
                                    .id(id)
                                    .hint_text(RichText::new(hint).color(p.pencil))
                                    .frame(egui::Frame::NONE)
                                    .desired_rows(2)
                                    .lock_focus(true)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        // Icon buttons without outlines, like the rest of the box.
                        let w = &mut ui.visuals_mut().widgets.inactive;
                        w.bg_stroke = egui::Stroke::NONE;
                        w.weak_bg_fill = egui::Color32::TRANSPARENT;
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui
                            .add(egui::Button::new(RichText::new(icon::PLUS).size(17.0)))
                            .on_hover_text("Attach images or text files (or drop them here)")
                            .clicked()
                            && let Some(paths) = rfd::FileDialog::new()
                                .set_title("Attach files")
                                .pick_files()
                        {
                            self.attach_paths(paths);
                        }
                        self.tools_menu(ui);
                        self.mcp_menu(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if running {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(icon::STOP).size(15.0).color(p.paper),
                                        )
                                        .fill(p.ink)
                                        .min_size(egui::vec2(32.0, 32.0)),
                                    )
                                    .on_hover_text("Stop")
                                    .clicked()
                                {
                                    let cid = self.current.unwrap();
                                    self.stop(cid);
                                }
                            } else {
                                let can_send = (!self.input.trim().is_empty()
                                    || !self.pending_attachments.is_empty())
                                    && self.reading_files == 0;
                                let fill = if can_send { p.signal } else { p.selected };
                                if ui
                                    .add_enabled(
                                        can_send,
                                        egui::Button::new(
                                            RichText::new(icon::ARROW_UP).size(16.0).color(
                                                if can_send {
                                                    egui::Color32::WHITE
                                                } else {
                                                    p.pencil
                                                },
                                            ),
                                        )
                                        .fill(fill)
                                        .min_size(egui::vec2(32.0, 32.0)),
                                    )
                                    .on_hover_text(if self.settings.general.send_with_enter {
                                        "Send (Enter) · Shift+Enter for a new line"
                                    } else {
                                        "Send (⌘/Ctrl+Enter)"
                                    })
                                    .clicked()
                                {
                                    send = true;
                                }
                            }
                        });
                    });
                });
            // Under the box: the working folder on the left, the model on the right.
            ui.horizontal(|ui| {
                self.workspace_picker(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.model_picker(ui);
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

    // ---------------------------------------------------------------- chat

    /// True when the open chat has nothing in it yet: show the greeting with the composer.
    fn is_fresh_chat(&self) -> bool {
        self.current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .is_none_or(|c| c.messages.is_empty() && !self.runs.contains_key(&c.id))
    }

    fn chat_area(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        if self.is_fresh_chat() {
            ui.add_space((ui.available_height() * 0.3 - 40.0).max(24.0));
            chat_view::greeting(ui, self.user_name.as_deref());
            ui.add_space(18.0);
            self.composer(ui);
            let p = self.settings.provider();
            let local = ["localhost", "127.0.0.1", "0.0.0.0"]
                .iter()
                .any(|h| p.base_url.contains(h));
            if p.api_key.is_empty() && !local {
                let warn = theme::palette(ui.visuals().dark_mode).warn;
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("No API key for this provider: add one in Settings (⌘,).")
                            .small()
                            .color(warn),
                    );
                });
            }
            return Vec::new();
        }
        let conv = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .expect("not a fresh chat");
        chat_view::show(
            ui,
            &mut self.md_cache,
            &mut self.drawings,
            &mut self.images,
            chat_view::View {
                conv,
                run: self.runs.get(&conv.id),
                error: self.errors.get(&conv.id),
                show_stats: self.settings.general.show_stats,
                reasoning: self.settings.general.reasoning_display,
                remote_images: self.settings.general.load_remote_images,
            },
        )
    }

    fn apply_actions(&mut self, actions: Vec<Action>, ctx: &egui::Context) {
        for a in actions {
            match a {
                Action::Copy(text) => ctx.copy_text(text),
                Action::Regenerate(id) => self.regenerate(id, ctx),
                Action::Approve(conv, call_id, answer) => {
                    let Some(run) = self.runs.get_mut(&conv) else {
                        continue;
                    };
                    let Some(pos) = run.approvals.iter().position(|p| p.call.id == call_id) else {
                        continue;
                    };
                    let pending = run.approvals.remove(pos);
                    let allow = self.always_allow.entry(conv).or_default();
                    allow.remember(&pending, answer);
                    pending.answer(answer.runs());
                    // Other calls already waiting may now be covered too.
                    let (now_allowed, waiting) = std::mem::take(&mut run.approvals)
                        .into_iter()
                        .partition(|p| allow.allows(p));
                    run.approvals = waiting;
                    for p in now_allowed {
                        p.answer(true);
                    }
                }
                Action::ViewImage(uri, bytes, name) => self.viewing = Some((uri, bytes, name)),
                Action::SaveSvg(svg) => {
                    if let Some(path) = export::pick_save_path_as("drawing.svg", "SVG image", "svg")
                    {
                        self.notice = Some(match std::fs::write(&path, svg) {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => format!("Couldn't save {}: {e}", path.display()),
                        });
                    }
                }
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

impl eframe::App for ChatApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.handle_shortcuts(ctx);
        self.handle_dropped_files(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        let dark = ui.visuals().dark_mode;
        let settings_open = self.settings_ui.is_some();
        // The sidebar runs the full height; the top bar only spans the main area.
        if self.sidebar_open && !settings_open {
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(270.0)
                .size_range(200.0..=420.0)
                .frame(
                    egui::Frame::new()
                        .fill(theme::palette(dark).shelf)
                        .inner_margin(egui::Margin::symmetric(10, 0)),
                )
                .show(ui, |ui| self.sidebar(ui));
        }
        egui::Panel::top("top-bar")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ui, |ui| self.top_bar(ui));

        if !settings_open {
            // A fresh chat shows the composer under the greeting instead.
            if !self.is_fresh_chat() {
                egui::Panel::bottom("composer")
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::new()
                            .fill(ui.visuals().panel_fill)
                            .inner_margin(egui::Margin::symmetric(12, 0)),
                    )
                    .show(ui, |ui| self.composer(ui));
            }
            let actions = egui::CentralPanel::no_frame()
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill))
                .show(ui, |ui| self.chat_area(ui))
                .inner;
            self.apply_actions(actions, &ctx);
        }

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
            let mut actions = sui.take_queued();
            actions.extend(
                egui::CentralPanel::no_frame()
                    .frame(egui::Frame::new().fill(ui.visuals().panel_fill))
                    .show(ui, |ui| sui.show(ui, &self.settings, &views))
                    .inner,
            );
            let mut install_update = false;
            for action in actions {
                match action {
                    SettingsAction::Apply(s) => {
                        self.apply_settings(s, &ctx);
                        // Keep fields the user is still editing, but pick up normalisation.
                        sui.draft.active_provider = self.settings.active_provider;
                    }
                    SettingsAction::Close => keep = false,
                    SettingsAction::CheckForUpdates(url) => self.spawn_update_check(true, url),
                    SettingsAction::InstallUpdate => install_update = true,
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
                    SettingsAction::SignIn(id) => self.mcp.sign_in(&id),
                    SettingsAction::SignOut(id) => self.mcp.sign_out(&id),
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
            if install_update {
                self.start_update_install();
            }
        }

        let entries = if self.palette.open {
            self.palette_entries()
        } else {
            Vec::new()
        };
        if let Some(cmd) = self.palette.show(&ctx, &entries) {
            self.run_command(cmd, &ctx);
        }
    }
}

/// Consume an Enter press that has no Shift held. `InputState::consume_key` ignores an extra
/// Shift, so Shift+Enter has to be left alone here for the text field to insert a newline.
fn consume_plain_enter(i: &mut egui::InputState) -> bool {
    let shifted = i.events.iter().any(|e| {
        matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if modifiers.shift)
    });
    !shifted && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
}

#[cfg(test)]
mod enter_key_tests {
    use eframe::egui;

    fn run(shift: bool) -> (bool, String) {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("probe");
        let mut text = String::from("hi");
        let mut sent = false;
        let mods = if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };
        for frame in 0..3 {
            let mut raw = egui::RawInput::default();
            if frame == 1 {
                raw.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: Some(egui::Key::Enter),
                    pressed: true,
                    repeat: false,
                    modifiers: mods,
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                ui.memory_mut(|m| m.request_focus(id));
                let s = ui.input_mut(super::consume_plain_enter);
                sent |= s;
                ui.add(egui::TextEdit::multiline(&mut text).id(id).lock_focus(true));
            });
            out.textures_delta.clear();
        }
        (sent, text)
    }

    #[test]
    fn shift_enter_inserts_newline_and_enter_sends() {
        assert_eq!(run(true), (false, "hi\n".to_owned()));
        assert_eq!(run(false), (true, "hi".to_owned()));
    }
}

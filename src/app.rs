use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_commonmark::CommonMarkCache;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::{self, AgentRequest, AppEvent, ChatEvent, EventSender};
use crate::approval::{AlwaysAllow, Approval, PendingApproval};
use crate::attach::{Attachment, Reading};
use crate::background::{self, Outcome, Queued, RunQueue};
use crate::branches::{self, BranchSettings, Hold};
use crate::config::{RemoteTls, Settings, Theme};
use crate::export;
use crate::llm;
use crate::mcp::manager::McpManager;
use crate::projects::{self, Project};
use crate::remote::{
    ApprovalView, AttachmentView, ChatSummary, ChatView, MessageView, ProviderView, RemoteCommand,
    RemoteEvent, RemoteStatus, StateView, ToolCallView,
};
use crate::schedule::{self, ScheduleSettings, ScheduledTask, Target};
use crate::tools::workspace::Workspace;
use crate::tools::{self, ToolOverrides, ToolSelection};
use crate::tray;
use crate::types::{ChatMessage, Conversation, Role, fork_messages};
use crate::ui::artifact_panel::{self, ArtifactView};
use crate::ui::attachments::{self as attach_ui, ImageCache};
use crate::ui::chat_view::{self, centered};
use crate::ui::commit_dialog::{CommitAction, CommitDialog};
use crate::ui::palette::{Command, Entry, Palette};
use crate::ui::project_editor::{self, EditorOutcome, ProjectEditor};
use crate::ui::settings::{SettingsAction, SettingsUi, Tab};
use crate::ui::theme::{self, icon};
use crate::ui::widgets::icon_button;
use egui::RichText;

mod scheduled;
mod shell;

use shell::{ArtifactIndex, History, Page};

const INPUT_ID: &str = "chat-input";

pub(crate) struct Run {
    task: JoinHandle<()>,
    pub(crate) approvals: Vec<PendingApproval>,
    pub(crate) running_tool: Option<String>,
    /// Stats of the request currently streaming in, for the readout above the input box.
    pub(crate) live_stats: Option<crate::stats::RequestStats>,
    /// Live subagent progress, by `delegate` call id.
    pub(crate) subagents: HashMap<String, Vec<crate::subagent::SubagentReport>>,
    /// Older messages are being summarised.
    pub(crate) compacting: bool,
    /// A "Compact now" run rather than a reply.
    pub(crate) compact_only: bool,
    /// When the run started, for the time limit (Settings → Background).
    pub(crate) started: Instant,
}

pub(crate) enum Action {
    Approve(Uuid, String, Approval),
    Copy(String),
    Regenerate(Uuid),
    EditFrom(Uuid, usize),
    /// Copy the chat up to this message into a new chat: chat, message index.
    ForkFrom(Uuid, usize),
    ViewImage(String, egui::load::Bytes, String),
    /// Save a drawing from a reply as an SVG file.
    SaveSvg(Vec<u8>),
    /// Show a reply's artifact in the side panel: chat, message index, artifact index.
    OpenArtifact(Uuid, usize, usize),
    CloseArtifact,
    SaveArtifact(crate::artifacts::Artifact),
    OpenArtifactInBrowser(crate::artifacts::Artifact),
    /// Show a web page in its own live preview window (Linux, see `ui::linux_preview`).
    OpenLivePreview(crate::artifacts::Artifact),
    /// Undo the file edits of the turn holding this message.
    UndoTurn(Uuid, usize),
    /// Show another version of the message's tail: chat, message index, version index (see
    /// `branches`).
    SwitchBranch(Uuid, usize, usize),
    /// Save a reply or artifact as a Word, PDF or Excel file: the chat, the file's title (the
    /// chat's when `None`), what to save and the format.
    SaveAs {
        conv: Uuid,
        title: Option<String>,
        content: crate::save_as::Content,
        format: crate::save_as::Format,
    },
    /// Approve the plan a Plan chat's latest reply proposed (see `plan`).
    ApprovePlan(Uuid),
    /// Keep planning: focus the message box.
    KeepPlanning,
    /// Read a reply aloud, or stop it when it is the one being read: chat, message index.
    Speak(Uuid, usize),
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
    /// Replies waiting for a free slot (Settings → Background), first come first served.
    queue: RunQueue,
    /// Chats whose reply ended while another chat was open, until they are opened.
    unread: HashSet<Uuid>,
    /// Whether the window has focus, as of the last frame (for notifications).
    window_focused: bool,
    /// The menu bar / system tray icon, while "Keep running" is on (see `tray.rs`).
    tray: Option<tray::Tray>,
    /// The tray icon was tried since the setting was last turned on, so a failed one isn't
    /// retried every frame.
    tray_tried: bool,
    /// The window is hidden to the tray.
    hidden_to_tray: bool,
    /// Quit was chosen, or the app is restarting for an update: the next close ends the app.
    quitting: bool,
    /// The "still running" notification was sent in this app run.
    tray_notified: bool,
    errors: HashMap<Uuid, String>,
    /// Replaced tails waiting for the reply that replaces them (see `branches`), by chat.
    holds: HashMap<Uuid, Hold>,
    /// A message being edited: its chat and position. The turn it starts is replaced when the
    /// edit is sent.
    editing: Option<(Uuid, usize)>,
    /// First name for the greeting, from the OS account.
    user_name: Option<String>,
    /// Tools and servers the user chose to always allow, per chat, until the app quits.
    always_allow: HashMap<Uuid, AlwaysAllow>,
    md_cache: CommonMarkCache,
    drawings: crate::ui::markdown::DrawingCache,
    /// The artifact panel, when open.
    artifact: Option<ArtifactView>,
    /// The open chat's artifacts, re-found when its messages change.
    artifacts: ArtifactList,
    /// The project instruction files each chat's latest request read.
    instructions: HashMap<Uuid, Vec<String>>,
    /// The web view for live previews of web page artifacts.
    html_preview: crate::ui::webview::HtmlPreview,
    settings_ui: Option<SettingsUi>,
    renaming: Option<(Uuid, String)>,
    search: String,
    notice: Option<String>,
    /// Voice input and reading replies aloud, in progress (see `voice`).
    voice: crate::voice::VoiceState,
    /// Repaint handle for the background threads that report back.
    egui_ctx: egui::Context,
    keychain_warned: bool,
    pending_attachments: Vec<Attachment>,
    /// Files still being read for `pending_attachments`.
    reading_files: usize,
    /// Of those, scanned PDFs being read with OCR.
    reading_ocr: usize,
    images: ImageCache,
    /// MCP server state for the composer and top bar, re-read when the manager changes.
    mcp_views: (u64, Arc<Vec<crate::mcp::manager::ServerView>>),
    viewing: Option<(String, egui::load::Bytes, String)>,
    prompt_form: Option<PromptForm>,
    /// The "Changed files" window is open.
    changed_files_open: bool,
    /// The Commit dialog, while it is open.
    commit_dialog: Option<crate::ui::commit_dialog::CommitDialog>,
    /// For background tasks to request a repaint when their results arrive.
    repaint_ctx: egui::Context,
    palette: Palette,
    sidebar_open: bool,
    /// Tool switches made before the first message, applied to the chat it creates.
    draft_tool_overrides: ToolOverrides,
    /// Reasoning and sampling options picked before the chat exists.
    draft_model_options: crate::model_options::ModelOptions,
    /// Tool permissions picked before the chat exists (`None`: Settings' default mode).
    draft_permissions: Option<crate::approval::ChatPermissions>,
    /// A newer release found on GitHub.
    update: Option<crate::update::Release>,
    /// Progress of installing `update`, once the user starts it.
    update_install: Option<UpdateInstall>,
    /// Whether the window icon currently shown is the dark variant (`None` before the first
    /// frame), so it is only swapped when the theme changes.
    window_icon_dark: Option<bool>,
    /// What the active provider's llama.cpp server reports about its model.
    server_info: Option<ServerInfoState>,
    /// The message box has had focus once (it takes it as the app opens).
    input_focused_once: bool,
    /// Focus is being moved with the keyboard, so the focused widget gets a visible ring
    /// (until the next pointer press).
    keyboard_nav: bool,
    /// Slash commands suggested under the message box while a `/name` is typed.
    slash_cache: crate::commands::SuggestionCache,
    /// Skills of the current chat's folder, for the tools menu (read once per folder).
    skill_cache: crate::skills::SkillCache,
    /// The Docker sandbox for the workspace shell tool, while it's on.
    sandbox: Option<crate::sandbox::Sandbox>,
    /// When idle sandbox containers were last removed.
    sandbox_reaped: Instant,
    /// The browser remote-access server, while it runs.
    remote: Option<crate::remote::RemoteHandle>,
    /// Whether the remote-access server runs, and where.
    pub(crate) remote_status: RemoteStatus,
    /// The Cloudflare Tunnel's cloudflared, while it runs.
    tunnel: Option<crate::remote::TunnelHandle>,
    /// Why the tunnel did not start, when it didn't.
    tunnel_error: Option<String>,
    /// The tunnel's public hostnames, shared with the server (which also answers them).
    tunnel_hosts: crate::remote::TunnelHosts,
    /// Cloudflare Access checks, shared with the server; the settings page shows its last error.
    access: Arc<crate::remote::AccessGate>,
    /// Fingerprints of each chat as last sent to the browsers (see `chat_fingerprint`).
    remote_seen: HashMap<Uuid, u64>,
    /// Fingerprint of the chat list and models as last sent (see `list_fingerprint`).
    remote_list_seen: u64,
    /// When each chat was last notified to the browsers, to space out a streaming chat's
    /// notices (see `chat_notices`).
    remote_sent: HashMap<Uuid, Instant>,
    /// When scheduled tasks were last checked (see `run_schedules`).
    schedule_checked: Option<Instant>,
    /// The memories (see `memory`), shared with the replies that use them.
    memory: Arc<crate::memory::MemoryStore>,
    /// The projects, by name (see `projects`).
    projects: Vec<Project>,
    /// Projects whose chats are folded away in the sidebar (remembered until the app quits).
    collapsed_projects: HashSet<Uuid>,
    /// The project editor, while a project is being created or edited.
    project_editor: Option<ProjectEditor>,
    /// A project waiting for the user to confirm its deletion.
    project_to_delete: Option<Uuid>,
    /// What the main canvas shows: the chat (or home), Projects or Artifacts.
    page: Page,
    /// Places visited, for the title row's back and forward.
    history: History,
    /// Artifacts across recent chats, for the home cards and the Artifacts page.
    artifact_index: ArtifactIndex,
    /// The sidebar's search field is shown.
    searching: bool,
    /// The search field should take focus when it next appears.
    search_focus: bool,
    /// The sidebar's Recents list is unfolded.
    recents_open: bool,
    /// Voice mode: transcripts are sent, replies read aloud, and it listens again.
    voice_mode: bool,
    /// The microphones found for the composer's device menu (looked up when first needed).
    mic_devices: Option<Vec<String>>,
    /// When Settings opened, for its fade and zoom in.
    settings_opened: Option<Instant>,
    /// The scheduled agent being edited on the Scheduled page, and whether it is new.
    schedule_edit: Option<(ScheduledTask, bool)>,
}

/// A llama.cpp server's report on one provider's model, refreshed in the background while
/// it is the active one.
/// The open chat's artifacts, found again when its messages change: the key is the chat, its
/// message count and the length of their text, which a streaming reply or an edit changes.
#[derive(Default)]
struct ArtifactList {
    key: Option<(Uuid, usize, usize)>,
    list: Vec<crate::artifacts::Artifact>,
}

impl ArtifactList {
    fn refresh(&mut self, conv: Option<&Conversation>) {
        let key = conv.map(|c| {
            let text = c.messages.iter().map(|m| m.content.len()).sum();
            (c.id, c.messages.len(), text)
        });
        if key != self.key {
            self.key = key;
            self.list = conv
                .map(|c| crate::artifacts::in_conversation(&c.messages))
                .unwrap_or_default();
        }
    }
}

struct ServerInfoState {
    /// Provider id and model.
    key: (String, String),
    fetched: Option<std::time::Instant>,
    pending: bool,
    info: Option<crate::llm::LlamaServerInfo>,
    /// A router's models and their load state.
    router: Option<Vec<crate::llm::RouterModel>>,
    /// For a LiteLLM provider: the model as the proxy lists it, and the key's spend.
    litellm: Option<crate::llm::LiteLlmModel>,
    litellm_key: Option<crate::llm::LiteLlmKey>,
    /// Ollama, LM Studio, vLLM and OpenRouter: what the server reports, as rows.
    details: Option<crate::servers::ServerDetails>,
}

impl ServerInfoState {
    /// The router's state of the selected model, when the server is a router.
    fn current(&self) -> Option<&crate::llm::RouterModel> {
        self.router.as_ref()?.iter().find(|m| m.id == self.key.1)
    }
}

/// How a router model's state reads in the model picker: label, colour, and whether it can
/// be loaded (true), unloaded (false), or neither for now (None).
fn router_state(
    m: &crate::llm::RouterModel,
    p: &theme::Palette,
) -> (&'static str, egui::Color32, Option<bool>) {
    if m.failed {
        return ("failed to load", p.error, Some(true));
    }
    match m.status.as_str() {
        "loaded" => ("loaded", p.ok, Some(false)),
        "loading" => ("loading…", p.warn, None),
        "downloading" => ("downloading…", p.warn, None),
        "sleeping" => ("asleep", p.pencil, Some(false)),
        "incomplete" => ("incomplete download", p.error, None),
        _ => ("not loaded", p.pencil, Some(true)),
    }
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
    fn read(self) -> anyhow::Result<Reading> {
        match self {
            Self::Path(p) => Attachment::read_path(&p),
            Self::Dropped(f) => {
                let name = f
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into());
                let bytes = f.bytes().map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
                Attachment::read_bytes(name, bytes)
            }
        }
    }
}

/// Copies a task's run times and save time from the live settings to the Settings draft (the
/// draft keeps the user's other edits to it).
fn copy_run_state(live: &ScheduleSettings, draft: &mut ScheduleSettings, id: Uuid) {
    let Some(from) = live.tasks.iter().find(|t| t.id == id) else {
        return;
    };
    if let Some(to) = draft.tasks.iter_mut().find(|t| t.id == id) {
        to.last_run = from.last_run;
        to.created = from.created;
    }
}

/// "aaron.bolton" or "aaron" → "Aaron", from the OS account; `None` for root or when unset.
/// The sidebar's pending actions, applied after its list is drawn.
#[derive(Default)]
struct SidebarActs {
    select: Option<Uuid>,
    delete: Option<Uuid>,
    export_md: Option<Uuid>,
    export_html: Option<Uuid>,
    toggle_pin: Option<Uuid>,
    rename_commit: Option<(Uuid, String)>,
    /// Move a chat into a project (`Some`), or out of its project (`None`).
    move_to: Option<(Uuid, Option<Uuid>)>,
    tools_to_project: Option<Uuid>,
    new_in_project: Option<Uuid>,
    toggle_project: Option<Uuid>,
    edit_project: Option<Uuid>,
    export_project: Option<Uuid>,
    delete_project: Option<Uuid>,
    new_project: bool,
    import_project: bool,
}

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
    theme::set_motion(ctx, settings.general.reduce_motion);
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
        // The app opens on the home screen: a new chat, ready to type into.
        let current = None;
        let sandbox = settings
            .sandbox
            .enabled
            .then(|| crate::sandbox::Sandbox::new(settings.sandbox.clone()));
        if settings.sandbox.enabled {
            // Containers left by an earlier run of the app. Docker isn't touched when it's off.
            let cfg = settings.sandbox.clone();
            rt.spawn_blocking(move || crate::sandbox::reap_orphans(&cfg));
        }
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
            queue: RunQueue::default(),
            unread: HashSet::new(),
            window_focused: true,
            tray: None,
            tray_tried: false,
            hidden_to_tray: false,
            quitting: false,
            tray_notified: false,
            errors: HashMap::new(),
            holds: HashMap::new(),
            editing: None,
            user_name: user_first_name(),
            always_allow: HashMap::new(),
            md_cache: CommonMarkCache::default(),
            drawings: Default::default(),
            artifact: None,
            artifacts: ArtifactList::default(),
            instructions: HashMap::new(),
            html_preview: Default::default(),
            settings_ui: None,
            renaming: None,
            search: String::new(),
            notice: loaded.notice,
            voice: Default::default(),
            egui_ctx: cc.egui_ctx.clone(),
            keychain_warned: false,
            pending_attachments: Vec::new(),
            reading_files: 0,
            reading_ocr: 0,
            images: ImageCache::default(),
            mcp_views: (u64::MAX, Arc::default()),
            viewing: None,
            prompt_form: None,
            changed_files_open: false,
            commit_dialog: None,
            repaint_ctx: cc.egui_ctx.clone(),
            palette: Palette::default(),
            sidebar_open: true,
            page: Page::Chat,
            history: History::default(),
            artifact_index: ArtifactIndex::default(),
            searching: false,
            search_focus: true,
            recents_open: true,
            voice_mode: false,
            mic_devices: None,
            settings_opened: None,
            schedule_edit: None,
            draft_tool_overrides: ToolOverrides::default(),
            draft_model_options: Default::default(),
            draft_permissions: None,
            update: None,
            update_install: None,
            window_icon_dark: None,
            server_info: None,
            input_focused_once: false,
            keyboard_nav: false,
            slash_cache: Default::default(),
            skill_cache: Default::default(),
            sandbox,
            sandbox_reaped: Instant::now(),
            remote: None,
            remote_status: RemoteStatus::Off,
            tunnel: None,
            tunnel_error: None,
            tunnel_hosts: Default::default(),
            access: Default::default(),
            remote_seen: HashMap::new(),
            remote_list_seen: 0,
            remote_sent: HashMap::new(),
            schedule_checked: None,
            memory: crate::memory::MemoryStore::open(),
            projects: Project::load_all(),
            collapsed_projects: HashSet::new(),
            project_editor: None,
            project_to_delete: None,
        };
        if app.settings.general.check_for_updates {
            app.spawn_update_check(false, app.settings.general.update_url.clone());
        }
        if loaded.needs_migration {
            // Move plain-text secrets from settings.json into the keychain.
            app.save_settings();
        }
        app.sync_remote();
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
        let id = self.create_chat();
        self.fit_new_chat_to_mode(id);
        self.current = Some(id);
        self.page = Page::Chat;
        self.input.clear();
    }

    /// A new chat with memory switched off (incognito), as `new_chat` otherwise makes it.
    fn new_incognito_chat(&mut self) {
        let id = self.create_chat();
        let default = self.settings.general.memory.enabled;
        if let Some(conv) = self.conv_mut(id) {
            conv.tool_overrides
                .set(ToolOverrides::feature_key(tools::MEMORY), false, default);
        }
        self.fit_new_chat_to_mode(id);
        self.current = Some(id);
        self.page = Page::Chat;
        self.input.clear();
    }

    /// Whether the chat being written in has memory switched off (incognito).
    fn incognito_now(&self) -> bool {
        !ToolSelection::new(&self.settings, self.tool_overrides()).feature_on(tools::MEMORY)
    }

    /// Applies a change to the memories made in Settings; a failure is shown as a notice.
    fn apply_memory_change(&mut self, change: crate::ui::settings::MemoryChange) {
        use crate::ui::settings::MemoryChange;
        let result = match change {
            MemoryChange::Add { text, scope } => match self.memory.add(&text, scope, None) {
                Ok(crate::memory::Added::Known(_)) => Err("That is already remembered.".into()),
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            },
            MemoryChange::Edit { id, text } => self.memory.edit(id, &text),
            MemoryChange::Delete(id) => self.memory.remove(id, |_| true).map(|_| ()),
            MemoryChange::DeleteAll => self.memory.remove_all(|_| true).map(|_| ()),
        };
        if let Err(e) = result {
            self.notice = Some(e);
        }
    }

    /// A chat to write in, without selecting it: the first empty one, or a new one. The draft
    /// tool switches go to it.
    fn create_chat(&mut self) -> Uuid {
        // Reuse an existing empty chat instead of piling them up (one outside a project: a
        // project's empty chat has its own settings).
        let id = match self
            .conversations
            .iter()
            .find(|c| c.messages.is_empty() && c.project.is_none())
        {
            Some(c) => c.id,
            None => {
                let c =
                    Conversation::with_permission_mode(self.settings.general.default_approval_mode);
                let id = c.id;
                self.conversations.insert(0, c);
                id
            }
        };
        let draft = std::mem::take(&mut self.draft_tool_overrides);
        if !draft.is_empty()
            && let Some(c) = self.conv_mut(id)
        {
            c.tool_overrides = draft;
        }
        if let Some(permissions) = self.draft_permissions.take()
            && let Some(c) = self.conv_mut(id)
        {
            c.permissions = permissions;
        }
        let options = std::mem::take(&mut self.draft_model_options);
        if !options.is_empty()
            && let Some(c) = self.conv_mut(id)
        {
            c.model_options = options;
        }
        id
    }

    /// The current chat's tool permissions (or the draft ones; without a draft, the mode
    /// Settings gives new chats).
    fn permissions(&self) -> crate::approval::ChatPermissions {
        match self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
        {
            Some(c) => c.permissions.clone(),
            None => {
                self.draft_permissions
                    .clone()
                    .unwrap_or_else(|| crate::approval::ChatPermissions {
                        mode: self.settings.general.default_approval_mode,
                        ..Default::default()
                    })
            }
        }
    }

    fn set_permissions(&mut self, permissions: crate::approval::ChatPermissions) {
        match self.current.and_then(|id| self.conv_mut(id)) {
            Some(conv) => {
                conv.permissions = permissions;
                if !conv.messages.is_empty() {
                    conv.save();
                }
            }
            None => self.draft_permissions = Some(permissions),
        }
    }

    /// The current chat's reasoning and sampling options (or the draft ones).
    fn model_options(&self) -> &crate::model_options::ModelOptions {
        self.current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .map_or(&self.draft_model_options, |c| &c.model_options)
    }

    fn set_model_options(&mut self, options: crate::model_options::ModelOptions) {
        match self.current.and_then(|id| self.conv_mut(id)) {
            Some(conv) => {
                conv.model_options = options;
                if !conv.messages.is_empty() {
                    conv.save();
                }
            }
            None => self.draft_model_options = options,
        }
    }

    /// Dropdown next to the input: reasoning effort and sampling for this chat, over the
    /// provider's and model's options from Settings.
    fn model_options_menu(&mut self, ui: &mut egui::Ui) {
        use egui::containers::menu::{MenuButton, MenuConfig};

        let provider = self.settings.provider();
        let inherited = provider.resolved_options();
        let api = provider.api;
        let mut options = self.model_options().clone();
        let before = options.clone();
        let effective = options.over(&inherited);
        let dark = ui.visuals().dark_mode;
        let summary = effective.summary();
        let text = if summary.is_empty() {
            "·  Default".to_owned()
        } else {
            format!("·  {summary}")
        };
        // Dimmer than the model name; the accent when this chat has its own options.
        let label = RichText::new(text).size(13.0);
        let label = if options.is_empty() {
            label.color(theme::palette(dark).pencil)
        } else {
            label.color(theme::accent(dark))
        };
        let mut reset = false;
        let (resp, _) =
            MenuButton::from_button(egui::Button::new(label).frame_when_inactive(false))
                .config(
                    MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| {
                    ui.set_min_width(320.0);
                    ui.label(RichText::new("Model options for this chat").strong());
                    ui.label(
                        RichText::new(
                            "Applies from the next message. Defaults are in Settings → Providers.",
                        )
                        .small()
                        .weak(),
                    );
                    ui.separator();
                    crate::ui::widgets::model_options_grid(
                        ui,
                        "chat-model-options",
                        &mut options,
                        &inherited,
                        api,
                    );
                    if !options.is_empty() {
                        ui.separator();
                        if ui
                            .button(format!("{}  Reset to defaults", icon::REGENERATE))
                            .clicked()
                        {
                            reset = true;
                        }
                    }
                });
        resp.on_hover_text(if summary.is_empty() {
            "Reasoning and sampling for this chat: server defaults".to_owned()
        } else {
            format!("Reasoning and sampling for this chat: {summary}")
        });
        if reset {
            options = Default::default();
        }
        if options != before {
            self.set_model_options(options);
        }
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

        let overrides = self.tool_overrides().clone();
        let mut permissions = self.permissions();
        let permissions_before = permissions.clone();
        let plan_on = self.settings.general.plan.enabled;
        let read_only = permissions.mode.effective(plan_on).is_read_only();
        let sandbox_on = self.settings.sandbox.enabled;
        let workspace_dir = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .and_then(|c| c.workspace.clone());
        let has_skills = !self
            .skill_cache
            .get(&self.settings.general.skills, workspace_dir.as_deref())
            .is_empty();
        let in_project = self.in_listed_project_chat();
        let sel = ToolSelection::new(&self.settings, &overrides)
            .with_mode(permissions.mode)
            .with_skills(has_skills);
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
        let mut compact = false;
        let mut review = false;
        let mut show_changed = false;
        let has_edits = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .is_some_and(|c| c.messages.iter().any(|m| m.edit.is_some()));
        let loaded = self
            .current
            .and_then(|id| self.instructions.get(&id))
            .cloned()
            .unwrap_or_default();
        let dark = ui.visuals().dark_mode;
        let label = RichText::new(format!(
            "{}  Tools · {} on",
            icon::SLIDERS,
            builtins_on + mcp_on
        ));
        let label = if overrides.is_empty() && permissions.is_default() {
            label
        } else {
            label.color(theme::accent(dark))
        };
        // A submenu of the composer's "+" menu.
        let resp = ui
            .menu_button(label, |ui| {
                ui.set_min_width(280.0);
                ui.label(RichText::new("Tools for this chat").strong());
                ui.label(
                    RichText::new("Applies from the next message. Defaults are in Settings.")
                        .small()
                        .weak(),
                );
                if !sel.feature_on(crate::tools::MEMORY) {
                    ui.label(
                        RichText::new(format!(
                            "{}  Incognito: no memories for this chat",
                            icon::EYE
                        ))
                        .small()
                        .color(theme::accent(ui.visuals().dark_mode)),
                    );
                }
                ui.separator();
                crate::ui::widgets::permissions_picker(ui, &mut permissions, sandbox_on, plan_on);
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Built-in").small().weak());
                        for (_, key, name) in BUILTINS {
                            // Skills only appear for a chat whose folder has some.
                            if key == "skills" && !has_skills {
                                continue;
                            }
                            let mut on = sel.builtin_on(key);
                            let ic = match key {
                                "time" => icon::CLOCK,
                                "calculator" => icon::CALCULATOR,
                                "tasks" => icon::LIST_CHECKS,
                                "fetch" => icon::GLOBE,
                                "web_search" => icon::SEARCH,
                                "skills" => icon::BOOKS,
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
                                let locked =
                                    read_only && crate::tools::workspace::changes_things(key);
                                let resp = ui.add_enabled(
                                    !locked,
                                    egui::Checkbox::new(&mut on, format!("{ic}  {name}")),
                                );
                                let resp = if locked {
                                    resp.on_disabled_hover_text("Off in Read only mode")
                                } else {
                                    resp
                                };
                                if resp.changed() {
                                    changes.push((
                                        ToolOverrides::builtin_key(key),
                                        on,
                                        sel.builtin_default(key),
                                    ));
                                }
                            }
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        ui.label(RichText::new("Chat features").small().weak());
                        for (key, name, desc) in crate::tools::FEATURES {
                            // The project switch only means something in a project's chat.
                            if key == crate::tools::PROJECT && !in_project {
                                continue;
                            }
                            let mut on = sel.feature_on(key);
                            let ic = match key {
                                crate::tools::COMPACTION => icon::STACK,
                                crate::tools::INSTRUCTIONS => icon::FILE,
                                crate::tools::HOOKS => icon::TERMINAL,
                                crate::tools::PROJECT => icon::FOLDER,
                                crate::tools::MEMORY => icon::EDIT,
                                _ => icon::UNDO,
                            };
                            let mut hover = desc.to_owned();
                            if key == crate::tools::INSTRUCTIONS && !loaded.is_empty() {
                                hover.push_str(&format!(". Last read: {}", loaded.join(", ")));
                            }
                            if ui
                                .checkbox(&mut on, format!("{ic}  {name}"))
                                .on_hover_text(hover)
                                .changed()
                            {
                                changes.push((
                                    ToolOverrides::feature_key(key),
                                    on,
                                    sel.feature_default(key),
                                ));
                            }
                            if key == crate::tools::INSTRUCTIONS && on && !loaded.is_empty() {
                                ui.indent("instructions-loaded", |ui| {
                                    ui.label(
                                        RichText::new(format!("Read: {}", loaded.join(", ")))
                                            .small()
                                            .weak(),
                                    );
                                });
                            }
                        }
                        if ui
                            .add_enabled(
                                self.current.is_some_and(|id| !self.busy(id)),
                                egui::Button::new(format!("{}  Compact now", icon::STACK)).small(),
                            )
                            .on_hover_text(
                                "Summarise this chat's older messages now; the latest ones \
                                     are kept word for word",
                            )
                            .clicked()
                        {
                            compact = true;
                        }
                        if self.settings.general.review.enabled {
                            let idle = self.current.is_some_and(|id| !self.busy(id));
                            if ui
                                .add_enabled(
                                    idle && has_workspace,
                                    egui::Button::new(format!("{}  Review changes", icon::CODE))
                                        .small(),
                                )
                                .on_hover_text(
                                    "Send the working folder's git diff to the model with the \
                                         review instructions in Settings → Context & files",
                                )
                                .clicked()
                            {
                                review = true;
                            }
                        }
                        if ui
                            .add_enabled(
                                has_edits,
                                egui::Button::new(format!("{}  Changed files", icon::FILE)).small(),
                            )
                            .on_hover_text("Every file this chat's edits changed, with their diffs")
                            .clicked()
                        {
                            show_changed = true;
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
            })
            .response;
        resp.on_hover_text(format!(
            "Tools for this chat: {} on{} · {}",
            builtins_on + mcp_on,
            if overrides.is_empty() {
                ""
            } else {
                " (changed from the defaults)"
            },
            permissions.mode.label()
        ));
        if permissions != permissions_before {
            self.set_permissions(permissions);
        }
        for (key, on, default) in changes {
            self.set_tool_override(key, on, default);
        }
        if reset {
            self.reset_tool_overrides();
        }
        if compact
            && let Some(id) = self.current
            && let Err(e) = self.compact_now(id, &ui.ctx().clone())
        {
            self.notice = Some(e);
        }
        if review
            && let Some(id) = self.current
            && let Err(e) = self.start_review(id, &ui.ctx().clone())
        {
            self.notice = Some(e);
        }
        if show_changed {
            self.changed_files_open = true;
        }
    }

    fn delete_chat(&mut self, id: Uuid) {
        self.stop(id);
        Conversation::delete(id);
        if let Some(sandbox) = &self.sandbox {
            sandbox.remove_chat(id);
        }
        self.conversations.retain(|c| c.id != id);
        self.errors.remove(&id);
        self.unread.remove(&id);
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
        if self.busy(id) {
            return;
        }
        let text = self.expand_command(id, text);
        // `/name` for a skill (not a command) asks the model to load it.
        let text = if text.starts_with('/') {
            let workspace = self.current_workspace();
            let skills = self
                .skill_cache
                .get(&self.settings.general.skills, workspace.as_deref());
            crate::commands::skill_request(&text, skills.iter().map(|s| s.name.as_str()))
                .unwrap_or(text)
        } else {
            text
        };
        let attachments = std::mem::take(&mut self.pending_attachments);
        match self.send_message(id, text, attachments, ctx) {
            Ok(()) => self.input.clear(),
            Err(e) => self.notice = Some(e),
        }
    }

    /// The text to send for a message: a `/name` command's prompt when one matches, otherwise
    /// the message as typed.
    fn expand_command(&self, id: Uuid, text: String) -> String {
        let settings = &self.settings.general.slash_commands;
        if !text.starts_with('/') {
            return text;
        }
        let workspace = self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.workspace.as_deref());
        let commands = crate::commands::available(settings, workspace);
        crate::commands::expand(&text, &commands).unwrap_or(text)
    }

    /// The open chat's working folder.
    fn current_workspace(&self) -> Option<std::path::PathBuf> {
        self.current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .and_then(|c| c.workspace.clone())
    }

    /// Adds a user message to a chat and starts its reply. Fails when the chat is missing or a
    /// reply is still running in it.
    fn send_message(
        &mut self,
        id: Uuid,
        text: String,
        attachments: Vec<Attachment>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.send_message_with(id, text, attachments, ctx, false)
    }

    /// `send_message`; with `review` the reply comes from the review provider (Settings →
    /// Context & files) instead of the chat's.
    fn send_message_with(
        &mut self,
        id: Uuid,
        text: String,
        attachments: Vec<Attachment>,
        ctx: &egui::Context,
        review: bool,
    ) -> Result<(), String> {
        if self.busy(id) {
            return Err("A reply is still running in this chat".into());
        }
        let edit_at = self.editing.take_if(|(c, _)| *c == id).map(|(_, at)| at);
        let settings = self.branch_settings(id);
        let conv = self.conv_mut(id).ok_or("Chat not found")?;
        // An edited prompt replaces the turn from it on: that turn stays as a version.
        let hold = edit_at
            .filter(|&at| conv.messages.get(at).is_some_and(|m| m.role == Role::User))
            .map(|at| branches::divide(&mut conv.messages, at, &settings));
        let mut msg = ChatMessage::new(Role::User, text);
        msg.attachments = attachments;
        conv.messages.push(msg);
        if let Some(hold) = hold {
            branches::attach(&mut conv.messages, &hold);
        }
        conv.auto_title();
        conv.updated_at = chrono::Utc::now();
        conv.save();
        self.start_run_with(id, ctx, false, review);
        Ok(())
    }

    /// Approves the plan a Plan chat proposed: the chat switches to its after-approval mode and
    /// the approval message starts a reply that carries the plan out.
    fn approve_plan(&mut self, id: Uuid, ctx: &egui::Context) {
        if self.busy(id) {
            return;
        }
        let plan = self.settings.general.plan.clone();
        let Some(conv) = self.conv_mut(id) else {
            return;
        };
        let text = crate::plan::approve(conv, &plan);
        let _ = self.send_message(id, text, Vec::new(), ctx);
    }

    fn start_run(&mut self, id: Uuid, ctx: &egui::Context) {
        self.start_run_with(id, ctx, false, false);
    }

    /// Reads the working folder's diff in the background; `ReviewReady` then sends it.
    fn start_review(&mut self, id: Uuid, ctx: &egui::Context) -> Result<(), String> {
        if !self.settings.general.review.enabled {
            return Err("Code review is off (Settings → Context & files)".into());
        }
        if self.busy(id) {
            return Err("A reply is still running in this chat".into());
        }
        let conv = self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .ok_or("Chat not found")?;
        let dir = conv
            .workspace
            .clone()
            .ok_or("This chat has no working folder")?;
        let settings = self.settings.general.review.clone();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        self.rt.spawn(async move {
            let result = crate::review::collect(&dir, &settings).await;
            let _ = tx.send(AppEvent::ReviewReady {
                conv_id: id,
                result,
            });
            ctx.request_repaint();
        });
        Ok(())
    }

    /// Sends a diff read by `start_review` as a message in its chat.
    fn finish_review(
        &mut self,
        id: Uuid,
        result: Result<crate::review::ReviewDiff, String>,
    ) -> Result<(), String> {
        let diff = result?;
        let review = &self.settings.general.review;
        let text = crate::review::message_text(&review.prompt, review.strictness, &diff);
        let ctx = self.repaint_ctx.clone();
        self.send_message_with(id, text, vec![crate::review::attachment(&diff)], &ctx, true)
    }

    /// Whether a chat has a reply running, or waiting for a free slot.
    fn busy(&self, id: Uuid) -> bool {
        self.runs.contains_key(&id) || self.queue.contains(id)
    }

    /// Starts a reply in a chat, or with `compact_only` only summarises its older messages.
    /// With `review` the reply uses the review provider. When `max_parallel` replies are already
    /// running, or others are waiting, the chat joins the queue; `start_queued` starts it later.
    fn start_run_with(&mut self, id: Uuid, ctx: &egui::Context, compact_only: bool, review: bool) {
        self.enqueue_run(
            Queued {
                conv: id,
                compact_only,
                review,
                provider: None,
            },
            ctx,
        );
    }

    /// Starts a queued reply now, or puts it in the queue when the parallel limit is reached.
    fn enqueue_run(&mut self, item: Queued, ctx: &egui::Context) {
        if self.busy(item.conv) {
            return;
        }
        let limit = self.settings.general.background.max_parallel as usize;
        if !self.queue.is_empty() || self.runs.len() >= limit.max(1) {
            // The last reply's error would read as this one's while it waits.
            self.errors.remove(&item.conv);
            self.queue.push(item);
            return;
        }
        self.launch_run(item, ctx);
    }

    /// Starts queued replies while there are free slots.
    fn start_queued(&mut self, ctx: &egui::Context) {
        let limit = self.settings.general.background.max_parallel as usize;
        while let Some(item) = self.queue.pop_ready(self.runs.len(), limit) {
            if !self.runs.contains_key(&item.conv) {
                self.launch_run(item, ctx);
            }
        }
    }

    /// Starts a reply now, reading the chat as it is at this moment.
    fn launch_run(&mut self, item: Queued, ctx: &egui::Context) {
        let Queued {
            conv: id,
            compact_only,
            review,
            provider: provider_id,
        } = item;
        let Some(conv) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        self.errors.remove(&id);
        let mut req = AgentRequest::for_chat(
            &self.settings,
            conv,
            self.sandbox.as_ref(),
            review,
            compact_only,
            provider_id.as_deref(),
        );
        req.memory = crate::memory::for_request(
            &self.memory,
            &req.selection,
            &self.settings.general.memory,
            conv,
        );
        // A chat in a project reads the project's files on the runtime, not on this thread.
        let project = (self.settings.general.projects.enabled
            && req.selection.feature_on(tools::PROJECT))
        .then(|| {
            conv.project
                .and_then(|pid| self.projects.iter().find(|p| p.id == pid))
        })
        .flatten()
        .cloned();
        let project_settings = self.settings.general.projects.clone();
        let (http, mcp, tx, ctx) = (
            self.http.clone(),
            self.mcp.clone(),
            self.tx.clone(),
            ctx.clone(),
        );
        let task = self.rt.spawn(async move {
            let mut req = req;
            if let Some(project) = project {
                req.project = tokio::task::spawn_blocking(move || {
                    projects::context_for(&project, &project_settings)
                })
                .await
                .ok()
                .flatten();
            }
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
                compacting: false,
                compact_only,
                started: Instant::now(),
            },
        );
    }

    /// Summarise a chat's older messages now.
    fn compact_now(&mut self, id: Uuid, ctx: &egui::Context) -> Result<(), String> {
        if self.busy(id) {
            return Err("A reply is still running in this chat".into());
        }
        let conv = self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .ok_or("Chat not found")?;
        let from = crate::types::last_compaction(&conv.messages).map_or(0, |i| i + 1);
        let n = conv.messages[from..]
            .iter()
            .filter(|m| !m.cancelled && m.compaction.is_none())
            .count();
        if n <= self.settings.general.compaction.keep_recent as usize {
            return Err(format!(
                "Nothing to compact yet: the chat has {n} message(s) since the last summary, \
                 and the latest {} are always kept (Settings → Context).",
                self.settings.general.compaction.keep_recent
            ));
        }
        self.start_run_with(id, ctx, true, false);
        Ok(())
    }

    /// Copy chat `id` up to message `idx` into a new chat, which becomes the current one.
    fn fork_chat(&mut self, id: Uuid, idx: usize) -> Result<Uuid, String> {
        if self.busy(id) {
            return Err("Wait for the reply to finish before forking this chat.".into());
        }
        let orig = self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .ok_or("This chat no longer exists.")?;
        if idx >= orig.messages.len() {
            return Err("That message no longer exists.".into());
        }
        let mut forked = Conversation::new();
        forked.title = format!("{} (fork)", orig.title);
        forked.messages = fork_messages(&orig.messages, idx);
        forked.workspace = orig.workspace.clone();
        forked.tool_overrides = orig.tool_overrides.clone();
        forked.model_options = orig.model_options.clone();
        forked.permissions = orig.permissions.clone();
        forked.save();
        let new_id = forked.id;
        self.conversations.insert(0, forked);
        self.current = Some(new_id);
        Ok(new_id)
    }

    /// Undo the file edits of the turn holding message `idx`.
    fn undo_turn(&mut self, id: Uuid, idx: usize) -> Result<String, String> {
        if self.busy(id) {
            return Err("Wait for the reply to finish first".into());
        }
        let conv = self.conv_mut(id).ok_or("Chat not found")?;
        let root = conv
            .workspace
            .clone()
            .ok_or("This chat has no working folder")?;
        if idx >= conv.messages.len() {
            return Err("Message not found".into());
        }
        let turn = crate::checkpoints::turn_of(&conv.messages, idx);
        let report = crate::checkpoints::undo(&mut conv.messages, turn, &root);
        conv.save();
        Ok(report.summary())
    }

    /// Cancel a generation. The prompt it was answering stays in the chat, marked cancelled, so
    /// it can be edited and resent; it and any partial reply are left out of what the next
    /// prompt sends to the model.
    fn stop(&mut self, id: Uuid) {
        let queued = self.queue.remove(id);
        let run = self.runs.remove(&id);
        let held = self.holds.remove(&id);
        let compact_only = match (&run, &queued) {
            (Some(run), _) => run.compact_only,
            (None, Some(item)) => item.compact_only,
            (None, None) => false,
        };
        let started = run.is_some() || queued.is_some();
        if let Some(run) = run {
            run.task.abort();
        }
        // A stopped regeneration puts back the reply it was replacing, rather than cutting the
        // chat back to its prompt.
        if let Some(hold) = held {
            if let Some(conv) = self.conv_mut(id) {
                branches::restore(&mut conv.messages, hold);
                conv.updated_at = chrono::Utc::now();
                conv.save();
            }
            return;
        }
        // A compaction has no prompt of its own to take back.
        if !started || compact_only {
            return;
        }
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

    fn regenerate(&mut self, id: Uuid, ctx: &egui::Context) -> Result<(), String> {
        if self.busy(id) {
            return Err("A reply is still running in this chat".into());
        }
        let settings = self.branch_settings(id);
        let conv = self.conv_mut(id).ok_or("Chat not found")?;
        // The replaced reply waits in a hold until the new one arrives (see `handle_chat_event`).
        if let Some(hold) = branches::prepare_regenerate(&mut conv.messages, &settings)? {
            self.holds.insert(id, hold);
        }
        if self.editing.is_some_and(|(c, _)| c == id) {
            self.editing = None;
        }
        self.start_run(id, ctx);
        Ok(())
    }

    /// Branch settings for a chat: Settings, unless the chat's `feature:branches` switch
    /// overrides them.
    fn branch_settings(&self, id: Uuid) -> BranchSettings {
        let mut settings = self.settings.general.branching.clone();
        if let Some(conv) = self.conversations.iter().find(|c| c.id == id) {
            settings.enabled = conv
                .tool_overrides
                .get(&ToolOverrides::feature_key(tools::BRANCHES))
                .unwrap_or(settings.enabled);
        }
        settings
    }

    /// Answers a tool approval waiting in a chat, and any other waiting call the answer now
    /// covers. False when no such approval is waiting.
    fn approve(&mut self, conv: Uuid, call_id: &str, answer: Approval) -> bool {
        let Some(run) = self.runs.get_mut(&conv) else {
            return false;
        };
        let Some(pos) = run.approvals.iter().position(|p| p.call.id == call_id) else {
            return false;
        };
        let pending = run.approvals.remove(pos);
        let allow = self.always_allow.entry(conv).or_default();
        allow.remember(&pending, answer);
        pending.answer(answer.runs());
        let (now_allowed, waiting) = std::mem::take(&mut run.approvals)
            .into_iter()
            .partition(|p| allow.allows(p));
        run.approvals = waiting;
        for p in now_allowed {
            p.answer(true);
        }
        true
    }

    /// Sets a chat's title. Surrounding spaces are dropped; an empty title is refused.
    fn rename_chat(&mut self, id: Uuid, title: &str) -> Result<(), String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("Title can't be empty".into());
        }
        let conv = self.conv_mut(id).ok_or("Chat not found")?;
        conv.title = title.to_owned();
        conv.save();
        Ok(())
    }

    fn set_pinned(&mut self, id: Uuid, pinned: bool) -> Result<(), String> {
        let conv = self.conv_mut(id).ok_or("Chat not found")?;
        conv.pinned = pinned;
        conv.save();
        Ok(())
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::VoiceCaptured(result) => {
                    self.voice.recording = None;
                    match result {
                        Ok(wav) => {
                            self.voice.transcribing = true;
                            let ctx = self.egui_ctx.clone();
                            crate::voice::spawn_transcription(
                                &self.rt,
                                &self.http,
                                &self.settings,
                                wav,
                                &self.tx,
                                move || ctx.request_repaint(),
                            );
                        }
                        Err(e) => self.notice = Some(e),
                    }
                }
                AppEvent::VoiceTranscribed(result) => {
                    self.voice.transcribing = false;
                    match result {
                        Ok(text) => {
                            let id = egui::Id::unique(INPUT_ID);
                            let cursor = egui::TextEdit::load_state(&self.egui_ctx, id)
                                .and_then(|s| s.cursor.char_range())
                                .map(|r| r.primary.index.0);
                            crate::voice::insert_text(&mut self.input, cursor, &text);
                            self.egui_ctx.memory_mut(|m| m.request_focus(id));
                            // Voice mode sends what was said at once.
                            if self.voice_mode && !text.trim().is_empty() {
                                let ctx = self.egui_ctx.clone();
                                self.send(&ctx);
                            } else if self.voice_mode {
                                self.voice_listen_again();
                            }
                        }
                        Err(e) => {
                            self.voice_mode = false;
                            self.notice = Some(format!("Voice input failed: {e}"));
                        }
                    }
                }
                AppEvent::SpeechEnded { key, result } => {
                    if self.voice.speaking_key() == Some(key) {
                        self.voice.speaking = None;
                    }
                    match result {
                        Err(e) => {
                            self.voice_mode = false;
                            self.notice = Some(format!("Reading aloud failed: {e}"));
                        }
                        Ok(()) => self.voice_listen_again(),
                    }
                }
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
                AppEvent::SandboxChecked(result) => {
                    if let Some(ui) = &mut self.settings_ui {
                        ui.sandbox_busy = false;
                        ui.sandbox_status = Some(result);
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
                AppEvent::ReviewReady { conv_id, result } => {
                    if let Err(e) = self.finish_review(conv_id, result) {
                        self.notice = Some(e);
                    }
                }
                AppEvent::CommitListed { conv_id, result } => {
                    if let Some(d) = self.commit_dialog.as_mut().filter(|d| d.conv == conv_id) {
                        d.set_listing(result);
                    }
                }
                AppEvent::Committed { conv_id, result } => {
                    if self
                        .commit_dialog
                        .as_ref()
                        .is_some_and(|d| d.conv == conv_id)
                    {
                        match result {
                            Ok(text) => {
                                self.commit_dialog = None;
                                self.notice = text.lines().next().map(str::to_owned);
                            }
                            Err(e) => {
                                if let Some(d) = self.commit_dialog.as_mut() {
                                    d.busy = false;
                                    d.error = Some(e);
                                }
                            }
                        }
                    }
                }
                AppEvent::OcrStarted => self.reading_ocr += 1,
                AppEvent::FileAttached { result, ocr } => {
                    self.reading_files = self.reading_files.saturating_sub(1);
                    if ocr {
                        self.reading_ocr = self.reading_ocr.saturating_sub(1);
                    }
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
                AppEvent::ServerInfo(report) => {
                    let crate::agent::ServerReport {
                        key,
                        info,
                        router,
                        litellm,
                        litellm_key,
                        details,
                    } = *report;
                    if let Some(state) = self.server_info.as_mut().filter(|s| s.key == key) {
                        state.info = info;
                        state.router = router;
                        state.litellm = litellm;
                        state.litellm_key = litellm_key;
                        state.details = details;
                        state.pending = false;
                        state.fetched = Some(std::time::Instant::now());
                    }
                }
                AppEvent::ModelLoadChanged(result) => {
                    if let Err(e) = result {
                        self.notice = Some(format!("Couldn't change the model's load state: {e}"));
                    }
                    // Show the new state straight away.
                    if let Some(state) = &mut self.server_info {
                        state.fetched = None;
                    }
                }
                AppEvent::Remote(cmd) => self.handle_remote(cmd),
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
                AppEvent::SavedAs(result) => {
                    self.notice = Some(match result {
                        Ok(msg) | Err(msg) => msg,
                    });
                }
            }
        }
    }

    /// "Save as…" for a reply or artifact: the file dialog here (as the other saves do, since
    /// macOS wants it on the main thread), then the file is built and written off the UI
    /// thread and its outcome comes back as `AppEvent::SavedAs`.
    fn save_as(
        &mut self,
        conv: Uuid,
        title: Option<String>,
        content: crate::save_as::Content,
        format: crate::save_as::Format,
    ) {
        let title = title.unwrap_or_else(|| {
            self.conversations
                .iter()
                .find(|c| c.id == conv)
                .map(|c| c.title.clone())
                .unwrap_or_default()
        });
        let stem = crate::save_as::file_stem(&title);
        let name = export::safe_file_name(stem, format.extension());
        let Some(path) = export::pick_save_path_as(&name, format.label(), format.extension())
        else {
            return;
        };
        let (tx, ctx) = (self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn_blocking(move || {
            let result = content
                .render(format)
                .map_err(|e| format!("Couldn't save {}: {e:#}", path.display()))
                .and_then(|bytes| {
                    crate::config::write_private(&path, &bytes)
                        .map_err(|e| format!("Couldn't save {}: {e:#}", path.display()))
                })
                .map(|()| format!("Saved {}", path.display()));
            let _ = tx.send(AppEvent::SavedAs(result));
            ctx.request_repaint();
        });
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
                preview,
                respond,
            } => {
                let server_name = origin.as_ref().and_then(|(server, _)| {
                    self.settings
                        .mcp_servers
                        .iter()
                        .find(|s| &s.id == server)
                        .map(|s| s.name.clone())
                });
                let tool = call.name.clone();
                let pending =
                    PendingApproval::new(call, origin, server_name, respond).with_preview(preview);
                if self
                    .always_allow
                    .get(&id)
                    .is_some_and(|a| a.allows(&pending))
                {
                    pending.answer(true);
                } else {
                    self.runs.get_mut(&id).unwrap().approvals.push(pending);
                    self.notify_chat(id, Outcome::NeedsApproval(tool));
                }
                return;
            }
            ChatEvent::ToolRunning(call_id) => {
                self.runs.get_mut(&id).unwrap().running_tool = Some(call_id);
                return;
            }
            ChatEvent::StatsLive(s) => {
                self.runs.get_mut(&id).unwrap().live_stats = Some(*s);
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
            ChatEvent::Compacting => {
                self.runs.get_mut(&id).unwrap().compacting = true;
                return;
            }
            ChatEvent::Instructions(files) => {
                self.instructions.insert(id, files);
                return;
            }
            ChatEvent::HookNotice(text) => {
                self.notice = Some(text);
                return;
            }
            _ => {}
        }
        let done = matches!(event, ChatEvent::Done);
        let is_tool_result = matches!(event, ChatEvent::ToolResult(_));
        if matches!(event, ChatEvent::Compacted { .. })
            && let Some(run) = self.runs.get_mut(&id)
        {
            run.compacting = false;
        }
        let is_final = matches!(event, ChatEvent::AssistantFinal(_));
        let keep_turns = self.settings.general.file_edits.keep_turns;
        // Disjoint fields, so the held tails can change while the chat is borrowed.
        let holds = &mut self.holds;
        let Some(conv) = self.conversations.iter_mut().find(|c| c.id == id) else {
            return;
        };
        match event {
            ChatEvent::AssistantStart => {
                conv.messages.push(ChatMessage::new(Role::Assistant, ""));
                // The first reply of a regeneration takes the versions it is replacing.
                if let Some(hold) = holds.get(&id) {
                    branches::reply_started(&mut conv.messages, hold);
                }
            }
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
                    // The message's versions stay with it.
                    let (alternatives, alt_index) =
                        (std::mem::take(&mut m.alternatives), m.alt_index);
                    *m = *msg;
                    m.alternatives = alternatives;
                    m.alt_index = alt_index;
                }
            }
            ChatEvent::ToolResult(msg) => conv.messages.push(*msg),
            ChatEvent::Compacted {
                kept,
                summary,
                info,
                stats,
            } => {
                let at = compaction_marker_at(&conv.messages, kept);
                let mut marker = ChatMessage::new(Role::System, summary);
                marker.compaction = Some(info);
                // The summary request counts in the chat's totals, but not as its context.
                marker.stats = Some(*stats);
                conv.messages.insert(at, marker);
                // A held tail moves up with the messages after the marker.
                if let Some(hold) = holds.get_mut(&id) {
                    branches::marker_inserted(hold, at);
                }
                conv.save();
            }
            ChatEvent::Done => {
                if let Some(hold) = holds.remove(&id) {
                    branches::settle(&mut conv.messages, hold);
                }
                if conv.messages.last().is_some_and(|m| {
                    m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty()
                }) {
                    conv.messages.pop();
                }
                crate::checkpoints::prune(&mut conv.messages, keep_turns);
                conv.updated_at = chrono::Utc::now();
                conv.save();
            }
            _ => {}
        }
        if done {
            let compact_only = self.runs.get(&id).is_some_and(|r| r.compact_only);
            let failed = self.errors.contains_key(&id);
            self.open_new_artifact(id);
            self.runs.remove(&id);
            if !compact_only {
                let outcome = if failed {
                    Outcome::Failed
                } else {
                    Outcome::Finished
                };
                self.reply_ended(id, outcome);
            }
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

    /// A reply ended: marks its chat unread when another chat is open, and notifies.
    fn reply_ended(&mut self, id: Uuid, outcome: Outcome) {
        if self.current != Some(id) {
            self.unread.insert(id);
        }
        // Voice mode waits while the reply waits for an approval.
        let (finished, failed) = (
            matches!(outcome, Outcome::Finished),
            matches!(outcome, Outcome::Failed),
        );
        self.notify_chat(id, outcome);
        if finished {
            self.voice_reply_done(id);
        } else if failed {
            self.voice_mode = false;
        }
    }

    /// Sends a desktop notification about a chat, when Settings → Background and the chat's
    /// `notify` switch ask for one at this moment.
    fn notify_chat(&self, id: Uuid, outcome: Outcome) {
        let Some(conv) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        let mut bg = self.settings.general.background.clone();
        bg.notify =
            ToolSelection::new(&self.settings, &conv.tool_overrides).feature_on(tools::NOTIFY);
        if background::should_notify(&bg, self.current == Some(id), self.window_focused) {
            background::notify(&self.rt, background::message(&conv.title, &outcome));
        }
    }

    /// Frees the slot of a reply whose task ended without its `Done` event (a panic, say), so
    /// a stuck run can't hold back the queue. The events a finished task sent are drained first.
    fn reap_finished_runs(&mut self) {
        if !self.runs.values().any(|r| r.task.is_finished()) {
            return;
        }
        self.handle_events();
        let stuck: Vec<Uuid> = self
            .runs
            .iter()
            .filter(|(_, r)| r.task.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in stuck {
            self.runs.remove(&id);
            if let Some(hold) = self.holds.remove(&id)
                && let Some(conv) = self.conv_mut(id)
            {
                branches::settle(&mut conv.messages, hold);
            }
            self.errors
                .entry(id)
                .or_insert_with(|| "The reply stopped unexpectedly.".into());
            self.reply_ended(id, Outcome::Failed);
        }
    }

    /// Stops replies that have run past the time limit (Settings → Background), recording why,
    /// and asks for a repaint when the next one is due, so this also runs while idle.
    fn check_time_limits(&mut self, ctx: &egui::Context) {
        let Some(limit) = background::time_limit(&self.settings.general.background) else {
            return;
        };
        let now = Instant::now();
        let expired: Vec<Uuid> = self
            .runs
            .iter()
            .filter(|(_, r)| now.duration_since(r.started) >= limit)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.stop(id);
            let mins = self.settings.general.background.time_limit_mins;
            self.errors.insert(id, background::time_limit_message(mins));
            self.reply_ended(id, Outcome::Failed);
        }
        let next = self
            .runs
            .values()
            .map(|r| background::time_left(r.started, limit, now))
            .min();
        if let Some(next) = next {
            ctx.request_repaint_after(next);
        }
    }

    /// Starts the scheduled tasks that are due, checking at most every `schedule::CHECK_EVERY`
    /// (and asking for a repaint then, so idle or hidden windows still check). Run times are
    /// saved with the settings, so they survive a restart.
    fn run_schedules(&mut self, ctx: &egui::Context) {
        let schedules = &self.settings.general.schedules;
        if !schedules.enabled || !schedules.tasks.iter().any(|t| t.enabled) {
            return;
        }
        ctx.request_repaint_after(schedule::CHECK_EVERY);
        if self
            .schedule_checked
            .is_some_and(|at| at.elapsed() < schedule::CHECK_EVERY)
        {
            return;
        }
        self.schedule_checked = Some(Instant::now());
        let tick = schedule::check(
            &mut self.settings.general.schedules.tasks,
            chrono::Utc::now(),
        );
        if tick.fire.is_empty() && tick.touched.is_empty() {
            return;
        }
        let mut sui = self.settings_ui.take();
        for id in &tick.fire {
            let Some(task) = self
                .settings
                .general
                .schedules
                .tasks
                .iter()
                .find(|t| t.id == *id)
                .cloned()
            else {
                continue;
            };
            let draft = sui.as_mut().map(|s| &mut s.draft.general.schedules);
            self.run_task(&task, ctx, draft);
        }
        if let Some(s) = sui.as_mut() {
            for id in &tick.touched {
                copy_run_state(
                    &self.settings.general.schedules,
                    &mut s.draft.general.schedules,
                    *id,
                );
            }
        }
        self.settings_ui = sui;
        self.save_settings();
    }

    /// Runs a task now, as the settings or the command palette asks: its last run becomes now.
    /// `draft` is the open Settings draft's schedules, which get the same run times.
    fn run_now(
        &mut self,
        task: ScheduledTask,
        ctx: &egui::Context,
        mut draft: Option<&mut ScheduleSettings>,
    ) {
        let now = chrono::Utc::now();
        if let Some(live) = self
            .settings
            .general
            .schedules
            .tasks
            .iter_mut()
            .find(|t| t.id == task.id)
        {
            live.last_run = Some(now);
            live.created.get_or_insert(now);
        }
        // The task as given: from Settings, its draft, so unsaved edits run.
        self.run_task(&task, ctx, draft.as_deref_mut());
        if let Some(draft) = draft {
            copy_run_state(&self.settings.general.schedules, draft, task.id);
        }
        self.save_settings();
    }

    /// Starts one run of a task: a new chat (or the task's own), with its prompt, through the
    /// reply queue. A task whose chat was deleted is switched off, and a chat still replying
    /// skips this run; both say so in the notice. `draft` is the open Settings draft's schedules,
    /// to switch off the same task there.
    fn run_task(
        &mut self,
        task: &ScheduledTask,
        ctx: &egui::Context,
        draft: Option<&mut ScheduleSettings>,
    ) {
        let id = match task.target {
            Target::NewChat => {
                let mut conv = Conversation::with_permission_mode(task.approval_mode);
                conv.title = schedule::chat_title(&task.name, chrono::Local::now());
                conv.workspace = task.workspace.clone();
                conv.scheduled_task = Some(task.id);
                let id = conv.id;
                self.conversations.insert(0, conv);
                id
            }
            Target::Chat(id) => {
                if self.conversations.iter().all(|c| c.id != id) {
                    if let Some(live) = self
                        .settings
                        .general
                        .schedules
                        .tasks
                        .iter_mut()
                        .find(|t| t.id == task.id)
                    {
                        live.enabled = false;
                    }
                    if let Some(d) =
                        draft.and_then(|d| d.tasks.iter_mut().find(|t| t.id == task.id))
                    {
                        d.enabled = false;
                    }
                    self.save_settings();
                    self.notice = Some(schedule::chat_gone_message(&task.name));
                    return;
                }
                if self.busy(id) {
                    self.notice = Some(schedule::chat_busy_message(&task.name));
                    return;
                }
                if let Some(conv) = self.conv_mut(id) {
                    conv.permissions.mode = task.approval_mode;
                }
                id
            }
        };
        let Some(conv) = self.conv_mut(id) else {
            return;
        };
        conv.messages
            .push(ChatMessage::new(Role::User, task.prompt.clone()));
        conv.auto_title();
        conv.updated_at = chrono::Utc::now();
        conv.save();
        self.enqueue_run(
            Queued {
                conv: id,
                compact_only: false,
                review: false,
                provider: task.provider.clone(),
            },
            ctx,
        );
    }

    /// When a reply in the open chat made an artifact, show the newest in the panel.
    fn open_new_artifact(&mut self, id: Uuid) {
        if !self.settings.general.open_artifacts
            || self.current != Some(id)
            || self.artifact.as_ref().is_some_and(|v| v.conv == id)
        {
            return;
        }
        let Some(conv) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        let asked = conv
            .messages
            .iter()
            .rposition(|m| m.role == Role::User)
            .unwrap_or(0);
        let list = crate::artifacts::in_conversation(&conv.messages);
        if let Some(newest) = list.last().filter(|a| a.message > asked) {
            self.artifact = Some(ArtifactView::new(id, newest, true));
        }
    }

    fn apply_settings(&mut self, new: Settings, ctx: &egui::Context) {
        let sandbox_before = self.settings.sandbox.clone();
        if new.general.html_preview != self.settings.general.html_preview {
            self.html_preview.reset();
        }
        self.settings = new;
        self.settings.normalize();
        self.slash_cache.clear();
        self.skill_cache.clear();
        if self.settings.sandbox != sandbox_before {
            // Dropping the old sandbox removes its containers.
            self.sandbox = self
                .settings
                .sandbox
                .enabled
                .then(|| crate::sandbox::Sandbox::new(self.settings.sandbox.clone()));
        }
        apply_appearance(ctx, &self.settings);
        self.mcp.sync(&self.settings.mcp_servers);
        self.save_settings();
        self.sync_remote();
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
            ui.menu_button(theme::labelled(icon::BOOKS, "Resources & prompts"), |ui| {
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
        if adds_history && self.current.is_some_and(|id| self.busy(id)) {
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

    /// Opens the Commit dialog on chat `id` and asks for its working folder's changes.
    fn open_commit_dialog(&mut self, id: Uuid, ctx: &egui::Context) -> Result<(), String> {
        let root = self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .ok_or("Chat not found")?
            .workspace
            .clone()
            .ok_or("This chat has no working folder")?;
        self.commit_dialog = Some(CommitDialog::new(id, root.clone()));
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        self.rt.spawn(async move {
            let result = match Workspace::open(&root) {
                Ok(ws) => ws.commit_changes_list().await,
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(AppEvent::CommitListed {
                conv_id: id,
                result,
            });
            ctx.request_repaint();
        });
        Ok(())
    }

    /// Commits `paths` (as the Commit dialog checked them) in the dialog's working folder.
    fn start_commit(&mut self, id: Uuid, message: String, paths: Vec<String>, ctx: &egui::Context) {
        let Some(dialog) = self.commit_dialog.as_mut() else {
            return;
        };
        dialog.busy = true;
        dialog.error = None;
        let root = dialog.root.clone();
        // The user's click in the dialog is the approval; signing follows Settings.
        let sign = self.settings.general.builtin_tools.allow_signing;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        self.rt.spawn(async move {
            let result = match Workspace::open(&root) {
                Ok(ws) => {
                    ws.with_commit_signing(sign)
                        .commit_changes(&message, Some(&paths))
                        .await
                }
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(AppEvent::Committed {
                conv_id: id,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Draws the Commit dialog and runs what the user asks for.
    fn commit_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.commit_dialog.take() else {
            return;
        };
        let idle = !self.busy(dialog.conv);
        match crate::ui::commit_dialog::show(ctx, &mut dialog, idle) {
            CommitAction::Keep => self.commit_dialog = Some(dialog),
            CommitAction::Close => {}
            CommitAction::Commit { message, paths } => {
                let id = dialog.conv;
                self.commit_dialog = Some(dialog);
                self.start_commit(id, message, paths, ctx);
            }
        }
    }

    /// The current chat's file edits, one collapsible section per file, with their diffs.
    fn changed_files_window(&mut self, ctx: &egui::Context) {
        if !self.changed_files_open {
            return;
        }
        let files = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .map(|c| crate::checkpoints::changed_files(&c.messages))
            .unwrap_or_default();
        let has_workspace = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .is_some_and(|c| c.workspace.is_some());
        let mut commit = false;
        egui::Window::new("Changed files")
            .open(&mut self.changed_files_open)
            .resizable(true)
            .default_width(600.0)
            .show(ctx, |ui| {
                let button = egui::Button::new("Commit…").small();
                if ui
                    .add_enabled(has_workspace, button)
                    .on_hover_text("Commit the working folder's changes to its git repository")
                    .clicked()
                {
                    commit = true;
                }
                ui.add_space(4.0);
                if files.is_empty() {
                    ui.label(RichText::new("This chat hasn't changed any files.").weak());
                    return;
                }
                let stat = |edits: &[(usize, crate::types::FileEdit)]| {
                    edits.iter().fold((0, 0), |(a, r), (_, e)| {
                        let (x, y) = crate::ui::widgets::diff_stat(&e.diff);
                        (a + x, r + y)
                    })
                };
                let all: Vec<_> = files.iter().flat_map(|f| f.edits.clone()).collect();
                let (added, removed) = stat(&all);
                ui.label(format!("{} files, +{added} \u{2212}{removed}", files.len()));
                ui.add_space(6.0);
                for file in &files {
                    let (a, r) = stat(&file.edits);
                    let created = file.edits.iter().any(|(_, e)| e.created);
                    let undone = file.edits.iter().all(|(_, e)| e.undone);
                    let mut title = format!("{}  +{a} \u{2212}{r}", file.path);
                    if created {
                        title.push_str("  \u{00B7} created");
                    }
                    if undone {
                        title.push_str("  \u{00B7} undone");
                    }
                    egui::CollapsingHeader::new(RichText::new(title).monospace())
                        .id_salt(("changed-file", &file.path))
                        .default_open(files.len() <= 3)
                        .show(ui, |ui| {
                            for (n, (_, edit)) in file.edits.iter().enumerate() {
                                let (ea, er) = crate::ui::widgets::diff_stat(&edit.diff);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("Edit {}", n + 1)).strong());
                                    ui.label(format!("+{ea} \u{2212}{er}"));
                                    if edit.undone {
                                        ui.label(RichText::new("Undone").weak());
                                    }
                                });
                                if edit.diff.is_empty() {
                                    ui.label(RichText::new("No diff recorded").weak());
                                } else {
                                    egui::ScrollArea::horizontal()
                                        .id_salt(("changed-diff", &file.path, n))
                                        .show(ui, |ui| {
                                            crate::ui::widgets::diff_block(ui, &edit.diff);
                                        });
                                }
                                ui.add_space(6.0);
                            }
                        });
                }
            });
        if commit
            && let Some(id) = self.current
            && let Err(e) = self.open_commit_dialog(id, ctx)
        {
            self.notice = Some(e);
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

    /// Save the current chat as a self-contained HTML file for sharing.
    fn export_html(&mut self, id: Uuid) {
        let Some(c) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        let Some(path) =
            export::pick_save_path_as(&export::safe_file_name(&c.title, "html"), "HTML", "html")
        else {
            return;
        };
        let html = export::conversation_html(c);
        self.notice = Some(match crate::config::write_private(&path, html.as_bytes()) {
            Ok(()) => format!("Shared \"{}\" to {}", c.title, path.display()),
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
    /// text takes a moment. Each arrives as an `AppEvent::FileAttached`. A scanned PDF goes
    /// through OCR when it is on (`ocr::attach`), which announces itself with `OcrStarted`.
    fn read_files(&mut self, files: Vec<FileSource>) {
        if files.is_empty() {
            return;
        }
        self.reading_files += files.len();
        let (tx, ctx) = (self.tx.clone(), self.repaint_ctx.clone());
        let ocr = crate::ocr::Job::from_settings(&self.settings, &self.http);
        self.rt.spawn(async move {
            for file in files {
                let read = tokio::task::spawn_blocking(move || file.read()).await;
                let (ocr_used, result) = match read {
                    Ok(Ok(Reading::Done(a))) => (false, Ok(a)),
                    Ok(Ok(Reading::Scanned(s))) => match &ocr {
                        Some(job) => {
                            let _ = tx.send(AppEvent::OcrStarted);
                            let name = s.name.clone();
                            let result = crate::ocr::attach(job, s.name, s.bytes)
                                .await
                                .map_err(|e| format!("{name} couldn't be read with OCR: {e:#}"));
                            (true, result)
                        }
                        None => (false, Ok(Reading::Scanned(s).without_ocr())),
                    },
                    Ok(Err(e)) => (false, Err(format!("{e:#}"))),
                    Err(e) => (false, Err(e.to_string())),
                };
                let _ = tx.send(AppEvent::FileAttached {
                    result,
                    ocr: ocr_used,
                });
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

    /// Runs a Docker test, pull or build on the runtime and reports it as `SandboxChecked`.
    fn spawn_sandbox<F>(&self, work: F)
    where
        F: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.repaint_ctx.clone());
        self.rt.spawn(async move {
            let _ = tx.send(AppEvent::SandboxChecked(work.await));
            ctx.request_repaint();
        });
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
        let (mic, read_aloud) = ctx.input_mut(|i| {
            let shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
            (
                i.consume_key(shift, egui::Key::M),
                i.consume_key(shift, egui::Key::S),
            )
        });
        if mic {
            self.toggle_recording();
        }
        if read_aloud {
            self.read_latest_or_stop();
        }
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
            self.open_settings(Tab::General);
        }
        if palette {
            self.palette.toggle();
        }
        if sidebar {
            self.sidebar_open = !self.sidebar_open;
        }
        let (back, forward) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::OpenBracket),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::CloseBracket),
            )
        });
        if back {
            self.go_back();
        }
        if forward {
            self.go_forward();
        }
    }

    /// Voice input: starts a recording, or ends it and transcribes the clip.
    fn toggle_recording(&mut self) {
        if self.voice.recording.is_some() {
            self.voice.stop_recording();
            return;
        }
        if !self.settings.general.voice.input.enabled {
            self.notice = Some("Turn on voice input in Settings → Voice".into());
            return;
        }
        let max_secs = self.settings.general.voice.input.max_secs;
        let device = self.settings.general.voice.input.device.clone();
        let ctx = self.egui_ctx.clone();
        if let Err(e) = self
            .voice
            .start_recording(max_secs, device, &self.tx, move || ctx.request_repaint())
        {
            self.notice = Some(e);
        }
    }

    /// Reads a reply aloud, or stops it when that reply is the one being read.
    fn toggle_speaking(&mut self, conv: Uuid, idx: usize) {
        if self.voice.speaking_key() == Some((conv, idx)) {
            self.voice.stop_speaking();
            return;
        }
        if !self.settings.general.voice.output.enabled {
            self.notice = Some("Turn on reading replies aloud in Settings → Voice".into());
            return;
        }
        let text = self
            .conversations
            .iter()
            .find(|c| c.id == conv)
            .and_then(|c| c.messages.get(idx))
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let speech = crate::voice::speak::prose(
            &text,
            self.settings.general.voice.output.max_chars as usize,
        );
        if speech.trim().is_empty() {
            self.notice = Some("There is nothing to read in that reply".into());
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.egui_ctx.clone());
        self.voice.start_speaking(
            &self.rt,
            &self.http,
            &self.settings,
            (conv, idx),
            speech,
            move |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            },
        );
    }

    /// Ctrl/⌘+Shift+S: stops the reading in progress, or reads the chat's latest reply.
    fn read_latest_or_stop(&mut self) {
        if self.voice.speaking.is_some() {
            self.voice.stop_speaking();
            return;
        }
        let Some(conv) = self.current else { return };
        let latest = self
            .conversations
            .iter()
            .find(|c| c.id == conv)
            .and_then(|c| crate::voice::latest_reply(&c.messages));
        match latest {
            Some(idx) => self.toggle_speaking(conv, idx),
            None => self.notice = Some("There is no reply to read yet".into()),
        }
    }

    /// The microphone under the message box: starts a recording, shows its elapsed time in red
    /// while it runs, and a spinner while the clip is transcribed.
    fn mic_button(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        if let Some(started) = self.voice.recording.as_ref().map(|r| r.started) {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            let secs = started.elapsed().as_secs();
            let label = RichText::new(format!(
                "{}  {}:{:02}",
                icon::MICROPHONE,
                secs / 60,
                secs % 60
            ))
            .color(p.error);
            if ui
                .add(egui::Button::new(label))
                .on_hover_text("Stop and transcribe (⌘/Ctrl+Shift+M)")
                .clicked()
            {
                self.toggle_recording();
            }
        } else if self.voice.transcribing {
            ui.spinner().on_hover_text("Transcribing…");
        } else if ui
            .add(egui::Button::new(
                RichText::new(icon::MICROPHONE).size(17.0),
            ))
            .on_hover_text("Dictate into the message box (⌘/Ctrl+Shift+M)")
            .clicked()
        {
            self.toggle_recording();
        }
    }

    /// Everything the command palette can do right now.
    fn palette_entries(&self) -> Vec<Entry> {
        let mut e = vec![
            Entry::new(icon::PLUS, "New chat", "Chat", Command::NewChat),
            Entry::new(
                icon::EYE,
                "New incognito chat (no memory)",
                "Chat",
                Command::NewIncognitoChat,
            ),
            Entry::new(icon::PAPERCLIP, "Attach files…", "Chat", Command::Attach),
            Entry::new(icon::FOLDERS, "Projects", "Go to", Command::ShowProjects),
            Entry::new(icon::SHAPES, "Artifacts", "Go to", Command::ShowArtifacts),
            Entry::new(
                icon::CALENDAR,
                "Scheduled agents",
                "Go to",
                Command::ShowScheduled,
            ),
            Entry::new(icon::CARET_LEFT, "Back  ⌘[", "Go to", Command::GoBack),
            Entry::new(
                icon::CARET_RIGHT,
                "Forward  ⌘]",
                "Go to",
                Command::GoForward,
            ),
            Entry::new(
                icon::CHAT_CIRCLE,
                "Chat mode",
                "Go to",
                Command::SetMode(crate::config::AppMode::Chat),
            ),
            Entry::new(
                icon::CODE,
                "Code mode (chats with a working folder)",
                "Go to",
                Command::SetMode(crate::config::AppMode::Code),
            ),
            Entry::new(
                icon::WAVEFORM,
                "Voice mode on or off",
                "Chat",
                Command::ToggleVoiceMode,
            ),
            Entry::new(
                icon::DOWNLOAD,
                "Export this chat as Markdown…",
                "Chat",
                Command::ExportChatMarkdown,
            ),
            Entry::new(
                icon::ARROW_SQUARE_OUT,
                "Share this chat as an HTML file…",
                "Chat",
                Command::ExportChatHtml,
            ),
            Entry::new(
                icon::STACK,
                "Compact this chat (summarise older messages)",
                "Chat",
                Command::CompactChat,
            ),
            Entry::new(
                icon::UNDO,
                "Undo the last turn's file edits",
                "Chat",
                Command::UndoLastEdits,
            ),
            Entry::new(icon::FLOW, "Fork this chat", "Chat", Command::ForkChat),
            Entry::new(
                icon::FILE,
                "Show files changed in this chat",
                "Chat",
                Command::ShowChangedFiles,
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
                icon::GEAR,
                "General",
                "Settings",
                Command::OpenSettings(Tab::General),
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
                icon::STACK,
                "Context & files",
                "Settings",
                Command::OpenSettings(Tab::Context),
            ),
            Entry::new(
                icon::ROBOT,
                "Subagents",
                "Settings",
                Command::OpenSettings(Tab::Subagents),
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
        if self.settings.general.projects.enabled {
            e.push(Entry::new(
                icon::FOLDER,
                "New project",
                "Project",
                Command::NewProject,
            ));
            e.push(Entry::new(
                icon::DOWNLOAD,
                "Import project…",
                "Project",
                Command::ImportProject,
            ));
        }
        for task in &self.settings.general.schedules.tasks {
            e.push(Entry::new(
                icon::CLOCK,
                format!("Run scheduled task: {}", task.name),
                "Schedule",
                Command::RunScheduledTask(task.id),
            ));
        }
        for mode in crate::approval::ApprovalMode::offered(self.settings.general.plan.enabled) {
            e.push(Entry::new(
                icon::SHIELD,
                format!("Permissions: {}", mode.label()),
                "Chat",
                Command::SetApprovalMode(mode),
            ));
        }
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
        if self.settings.general.review.enabled {
            e.push(Entry::new(
                icon::CODE,
                "Review changes in the working folder",
                "Chat",
                Command::ReviewChanges,
            ));
        }
        if self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .is_some_and(|c| c.workspace.is_some())
        {
            e.push(Entry::new(
                icon::CHECK_SQUARE,
                "Commit changes…",
                "Chat",
                Command::CommitChanges,
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
            Command::NewIncognitoChat => {
                self.settings_ui = None;
                self.new_incognito_chat();
            }
            Command::OpenChat(id) => {
                self.settings_ui = None;
                self.current = Some(id);
            }
            Command::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Command::ShowProjects => self.page = Page::Projects,
            Command::ShowArtifacts => self.page = Page::Artifacts,
            Command::ShowScheduled => self.page = Page::Scheduled,
            Command::GoBack => self.go_back(),
            Command::GoForward => self.go_forward(),
            Command::SetMode(mode) => self.set_mode(mode),
            Command::ToggleVoiceMode => self.toggle_voice_mode(),
            Command::RunScheduledTask(id) => {
                if let Some(task) = self
                    .settings
                    .general
                    .schedules
                    .tasks
                    .iter()
                    .find(|t| t.id == id)
                    .cloned()
                {
                    let mut sui = self.settings_ui.take();
                    self.run_now(
                        task,
                        ctx,
                        sui.as_mut().map(|s| &mut s.draft.general.schedules),
                    );
                    self.settings_ui = sui;
                }
            }
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
            Command::ExportChatHtml => {
                if let Some(id) = self.current {
                    self.export_html(id);
                }
            }
            Command::CompactChat => {
                if let Some(id) = self.current
                    && let Err(e) = self.compact_now(id, ctx)
                {
                    self.notice = Some(e);
                }
            }
            Command::ForkChat => {
                if let Some(id) = self.current {
                    let last = self
                        .conversations
                        .iter()
                        .find(|c| c.id == id)
                        .and_then(|c| c.messages.len().checked_sub(1));
                    self.notice = Some(match last {
                        Some(i) => match self.fork_chat(id, i) {
                            Ok(_) => "Forked into a new chat".into(),
                            Err(e) => e,
                        },
                        None => "This chat has no messages.".into(),
                    });
                }
            }
            Command::ShowChangedFiles => self.changed_files_open = true,
            Command::NewProject => self.open_project_editor(None),
            Command::ImportProject => self.import_project(),
            Command::ReviewChanges => {
                if let Some(id) = self.current
                    && let Err(e) = self.start_review(id, ctx)
                {
                    self.notice = Some(e);
                }
            }
            Command::CommitChanges => {
                if let Some(id) = self.current
                    && let Err(e) = self.open_commit_dialog(id, ctx)
                {
                    self.notice = Some(e);
                }
            }
            Command::UndoLastEdits => {
                if let Some(id) = self.current {
                    let last = self
                        .conversations
                        .iter()
                        .find(|c| c.id == id)
                        .and_then(|c| c.messages.len().checked_sub(1));
                    self.notice = Some(match last.map(|i| self.undo_turn(id, i)) {
                        Some(Ok(msg)) | Some(Err(msg)) => msg,
                        None => "This chat has no messages.".into(),
                    });
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
            Command::SetApprovalMode(mode) => {
                let mut permissions = self.permissions();
                permissions.mode = mode;
                self.set_permissions(permissions);
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

    /// The canvas title row's status items, right to left: the artifact panel's toggle, an
    /// available update, and MCP servers.
    fn status_items(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let mut start_install = false;
        ui.spacing_mut().item_spacing.x = 8.0;
        {
            {
                // Only for a chat that has artifacts, so the corner stays quiet otherwise.
                if self.settings_ui.is_none()
                    && let Some(conv) = self.current
                    && (!self.artifacts.list.is_empty()
                        || self.artifact.as_ref().is_some_and(|v| v.conv == conv))
                {
                    let open = self.artifact.as_ref().is_some_and(|v| v.conv == conv);
                    let newest = self.artifacts.list.last();
                    let tip = if open {
                        "Hide the artifact panel"
                    } else if newest.is_some() {
                        "Show the artifact panel"
                    } else {
                        "No artifacts in this chat yet"
                    };
                    if artifact_panel::toggle_button(ui, open, newest.is_some())
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.artifact = match newest {
                            Some(a) if !open => Some(ArtifactView::new(conv, a, true)),
                            _ => None,
                        };
                    }
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
                            let (label, hover) = if crate::update::is_installer(target) {
                                (
                                    format!("Install {}", r.version),
                                    "Close CleanRSTAI and run the downloaded installer; \
                                     open CleanRSTAI again when it finishes"
                                        .to_owned(),
                                )
                            } else {
                                (
                                    "Restart to finish updating".to_owned(),
                                    format!("Version {} is installed", r.version),
                                )
                            };
                            if ui.add(link(label)).on_hover_text(hover).clicked() {
                                match crate::update::relaunch(target) {
                                    Ok(()) => {
                                        // The close must end the app, not hide it to the tray.
                                        self.quitting = true;
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
            }
        }
        if start_install {
            self.start_update_install();
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
        if let Some(id) = self.current
            && self.chat_has_own_model(id)
        {
            self.chat_model_picker(ui, id);
            return;
        }
        let p = theme::palette(ui.visuals().dark_mode);
        let provider = self.settings.provider();
        let model = if provider.model.is_empty() {
            "Choose a model".to_owned()
        } else {
            provider.model.clone()
        };
        let label = RichText::new(shell::short_model(&model))
            .size(13.0)
            .color(p.ink2);
        // A llama.cpp router's models and their load state, for this provider.
        let router: Option<Vec<crate::llm::RouterModel>> = self
            .server_info
            .as_ref()
            .filter(|s| s.key.0 == provider.id)
            .and_then(|s| s.router.clone());
        let mut load_changes: Vec<(String, bool)> = Vec::new();
        let mut changed = false;
        let chat_id = self.current;
        let mut pin_to_chat = false;
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
                if let Some(router) = &router {
                    // A router: its models, each with its load state and Load / Unload.
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            for m in router {
                                ui.horizontal(|ui| {
                                    changed |= ui
                                        .selectable_value(&mut provider.model, m.id.clone(), &m.id)
                                        .changed();
                                    let (state, color, can) = router_state(m, &p);
                                    ui.label(RichText::new(state).small().color(color));
                                    if let Some(load) = can {
                                        let (label, hover) = if load {
                                            ("Load", "Load this model on the server now")
                                        } else {
                                            (
                                                "Unload",
                                                "Free the memory this model uses on the server",
                                            )
                                        };
                                        let button = egui::Button::new(
                                            RichText::new(label).small().color(p.signal),
                                        )
                                        .frame_when_inactive(false);
                                        if ui.add(button).on_hover_text(hover).clicked() {
                                            load_changes.push((m.id.clone(), load));
                                        }
                                    }
                                });
                            }
                        });
                } else if provider.models.is_empty() {
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
                if chat_id.is_some() {
                    ui.separator();
                    if ui
                        .button("Use this model for this chat only")
                        .on_hover_text(
                            "The chat keeps this provider and model, whatever Settings change to",
                        )
                        .clicked()
                    {
                        pin_to_chat = true;
                    }
                }
            });
        if changed {
            self.save_settings();
        }
        if let (true, Some(id)) = (pin_to_chat, chat_id) {
            let (pid, model) = (
                self.settings.provider().id.clone(),
                self.settings.provider().model.trim().to_owned(),
            );
            if let Some(conv) = self.conv_mut(id) {
                conv.provider = Some(pid);
                conv.model = Some(model).filter(|m| !m.is_empty());
                if !conv.messages.is_empty() {
                    conv.save();
                }
            }
        }
        for (model, load) in load_changes {
            let provider = self.settings.provider().clone();
            let (http, tx, ctx) = (self.http.clone(), self.tx.clone(), ui.ctx().clone());
            self.rt.spawn(async move {
                let result = crate::llm::llama_cpp_set_loaded(&http, &provider, &model, load)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(AppEvent::ModelLoadChanged(result));
                ctx.request_repaint();
            });
        }
    }

    /// Whether a chat has a provider or model of its own (from its project, or picked for it).
    fn chat_has_own_model(&self, id: Uuid) -> bool {
        self.conversations
            .iter()
            .any(|c| c.id == id && (c.provider.is_some() || c.model.is_some()))
    }

    /// The model picker of a chat with its own provider or model, in place of the global one.
    /// Picking a provider resets the model to that provider's; the last item goes back to
    /// Settings' provider and model.
    fn chat_model_picker(&mut self, ui: &mut egui::Ui, id: Uuid) {
        use egui::containers::menu::{MenuButton, MenuConfig};
        let p = theme::palette(ui.visuals().dark_mode);
        let providers: Vec<(String, String)> = self
            .settings
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let active = self.settings.provider();
        let Some(conv) = self.conversations.iter_mut().find(|c| c.id == id) else {
            return;
        };
        let provider = conv
            .provider
            .as_deref()
            .and_then(|pid| self.settings.providers.iter().find(|p| p.id == pid))
            .unwrap_or(active);
        let shown_model = conv.model.clone().unwrap_or_else(|| provider.model.clone());
        let models = provider.models.clone();
        let default_model = provider.model.clone();
        let label = RichText::new(shell::short_model(&shown_model))
            .size(13.0)
            .color(p.ink2);
        let mut changed = false;
        MenuButton::from_button(egui::Button::new(label).frame_when_inactive(false))
            .config(MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                ui.set_min_width(260.0);
                ui.label(
                    RichText::new("Provider for this chat")
                        .small()
                        .color(p.pencil),
                );
                for (pid, name) in &providers {
                    let current = conv.provider.as_deref() == Some(pid.as_str());
                    if ui.selectable_label(current, name).clicked() {
                        conv.provider = Some(pid.clone());
                        conv.model = None;
                        changed = true;
                    }
                }
                ui.separator();
                ui.label(RichText::new("Model").small().color(p.pencil));
                if models.is_empty() {
                    let mut text = conv.model.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut text)
                                .hint_text(default_model.as_str())
                                .desired_width(240.0),
                        )
                        .changed()
                    {
                        conv.model = Some(text).filter(|m| !m.trim().is_empty());
                        changed = true;
                    }
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            for m in &models {
                                if ui.selectable_label(*m == shown_model, m).clicked() {
                                    conv.model = Some(m.clone());
                                    changed = true;
                                }
                            }
                        });
                }
                ui.separator();
                if ui.button("Use Settings' provider and model").clicked() {
                    conv.provider = None;
                    conv.model = None;
                    changed = true;
                }
            });
        if changed && !conv.messages.is_empty() {
            conv.save();
        }
    }

    /// Beside the model picker, for a llama.cpp provider: "Loading model…" or "Asleep" when
    /// the server says so, and an icon whose hover lists what it reports about the model.
    /// Refreshed every 15 s while shown (every 3 s while the model loads). For a LiteLLM
    /// provider, the icon's hover shows the model as the proxy lists it and the key's spend,
    /// refreshed every minute.
    fn server_status(&mut self, ui: &mut egui::Ui) {
        use crate::config::ProviderKind;
        use crate::llm::LlamaStatus;
        let provider = self.settings.provider().clone();
        let kind = provider.kind;
        if kind == ProviderKind::Generic || provider.model.trim().is_empty() {
            return;
        }
        let key = (provider.id.clone(), provider.model.trim().to_owned());
        if self.server_info.as_ref().is_none_or(|s| s.key != key) {
            // Another model on the same server: its router list still holds meanwhile.
            let router = self
                .server_info
                .take()
                .filter(|s| s.key.0 == key.0)
                .and_then(|s| s.router);
            self.server_info = Some(ServerInfoState {
                key: key.clone(),
                fetched: None,
                pending: false,
                info: None,
                router,
                litellm: None,
                litellm_key: None,
                details: None,
            });
        }
        let Some(state) = self.server_info.as_mut() else {
            return;
        };
        let loading = state
            .info
            .as_ref()
            .is_some_and(|i| i.status == LlamaStatus::Loading)
            || state
                .router
                .iter()
                .flatten()
                .any(|m| matches!(m.status.as_str(), "loading" | "downloading"));
        let every = std::time::Duration::from_secs(match (kind, loading) {
            (ProviderKind::LiteLlm | ProviderKind::OpenRouter, _) => 60,
            (_, true) => 3,
            _ => 15,
        });
        if !state.pending && state.fetched.is_none_or(|t| t.elapsed() >= every) {
            state.pending = true;
            let (http, tx, ctx) = (self.http.clone(), self.tx.clone(), ui.ctx().clone());
            self.rt.spawn(async move {
                let report = match kind {
                    ProviderKind::LiteLlm => {
                        let (litellm, litellm_key) = tokio::join!(
                            crate::llm::litellm_model_info(&http, &provider),
                            crate::llm::litellm_key_info(&http, &provider),
                        );
                        crate::agent::ServerReport {
                            key,
                            litellm,
                            litellm_key,
                            ..Default::default()
                        }
                    }
                    ProviderKind::LlamaCpp => {
                        let (info, router) = tokio::join!(
                            crate::llm::llama_cpp_server_info(&http, &provider),
                            crate::llm::llama_cpp_router_models(&http, &provider),
                        );
                        crate::agent::ServerReport {
                            key,
                            info,
                            router,
                            ..Default::default()
                        }
                    }
                    _ => crate::agent::ServerReport {
                        key,
                        details: crate::servers::details(&http, &provider).await,
                        ..Default::default()
                    },
                };
                let _ = tx.send(AppEvent::ServerInfo(Box::new(report)));
                ctx.request_repaint();
            });
        }
        ui.ctx().request_repaint_after(every);

        let p = theme::palette(ui.visuals().dark_mode);
        if !matches!(kind, ProviderKind::LlamaCpp | ProviderKind::LiteLlm) {
            if let Some(d) = &state.details {
                if let Some(status) = &d.status {
                    ui.label(RichText::new(status).small().color(p.pencil))
                        .on_hover_text("The server loads it when you send a message.");
                }
                ui.add(
                    egui::Label::new(RichText::new(icon::INFO).size(13.0).color(p.pencil))
                        .sense(egui::Sense::hover()),
                )
                .on_hover_ui(|ui| super::ui::stats_view::server_rows(ui, d));
            }
            return;
        }
        if kind == ProviderKind::LiteLlm {
            if state.litellm.is_some() || state.litellm_key.is_some() {
                let (model, key) = (state.litellm.as_ref(), state.litellm_key.as_ref());
                ui.add(
                    egui::Label::new(RichText::new(icon::INFO).size(13.0).color(p.pencil))
                        .sense(egui::Sense::hover()),
                )
                .on_hover_ui(|ui| super::ui::stats_view::litellm_details(ui, model, key));
            }
            return;
        }
        // A router that hasn't loaded the selected model: say so (it loads on the next
        // message, or from the model picker).
        if let Some(m) = state.current().filter(|m| m.status != "loaded") {
            let (label, color, _) = router_state(m, &p);
            let mut label = label.to_owned();
            if let Some(first) = label.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            ui.label(RichText::new(label).small().color(color))
                .on_hover_text(
                    "The server runs several models and hasn't got this one loaded. It loads it \
                 when you send a message, or load it now from the model picker.",
                );
            return;
        }
        let Some(info) = &state.info else {
            return;
        };
        let details = |ui: &mut egui::Ui| super::ui::stats_view::server_details(ui, info);
        ui.add(
            egui::Label::new(RichText::new(icon::INFO).size(13.0).color(p.pencil))
                .sense(egui::Sense::hover()),
        )
        .on_hover_ui(details);
        match info.status {
            LlamaStatus::Loading => {
                ui.label(RichText::new("Loading model…").small().color(p.warn))
                    .on_hover_ui(details);
            }
            LlamaStatus::Asleep => {
                ui.label(RichText::new("Asleep").small().color(p.pencil))
                    .on_hover_text(
                        "The server unloaded the model after idling; your next message reloads it.",
                    );
            }
            LlamaStatus::Ready => {}
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

    /// Applies what the sidebar's and the Projects page's rows and menus asked for.
    fn apply_sidebar_acts(&mut self, acts: SidebarActs) {
        if let Some(id) = acts.toggle_pin
            && let Some(pinned) = self.conv_mut(id).map(|c| !c.pinned)
        {
            let _ = self.set_pinned(id, pinned);
        }
        if let Some(id) = acts.export_md {
            self.export_markdown(id);
        }
        if let Some(id) = acts.export_html {
            self.export_html(id);
        }
        if let Some(id) = acts.select {
            self.current = Some(id);
            self.page = Page::Chat;
        }
        if let Some((id, title)) = acts.rename_commit {
            self.renaming = None;
            // An empty title keeps the old one.
            let _ = self.rename_chat(id, &title);
        }
        if let Some(id) = acts.delete {
            self.delete_chat(id);
        }
        if let Some((id, project)) = acts.move_to {
            self.move_chat_to(id, project);
        }
        if let Some(id) = acts.tools_to_project {
            self.save_tools_to_project(id);
        }
        if let Some(pid) = acts.new_in_project {
            self.new_project_chat(pid);
        }
        if let Some(pid) = acts.toggle_project
            && !self.collapsed_projects.remove(&pid)
        {
            self.collapsed_projects.insert(pid);
        }
        if let Some(pid) = acts.edit_project {
            self.open_project_editor(Some(pid));
        }
        if let Some(pid) = acts.export_project {
            self.export_project(pid);
        }
        if let Some(pid) = acts.delete_project {
            self.project_to_delete = Some(pid);
        }
        if acts.new_project {
            self.open_project_editor(None);
        }
        if acts.import_project {
            self.import_project();
        }
    }

    /// Whether chat `idx` belongs to a project that is listed.
    fn in_listed_project(&self, idx: usize) -> bool {
        self.conversations[idx]
            .project
            .is_some_and(|pid| self.projects.iter().any(|p| p.id == pid))
    }

    /// Whether the open chat belongs to a listed project.
    fn in_listed_project_chat(&self) -> bool {
        self.current
            .and_then(|id| self.conversations.iter().position(|c| c.id == id))
            .is_some_and(|i| self.in_listed_project(i))
    }

    /// One chat's row in the sidebar or on the Projects page: its title, state mark, and
    /// menu. `surface` is the colour behind it, for the fade of a long title.
    fn chat_row(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        acts: &mut SidebarActs,
        surface: egui::Color32,
    ) {
        let p = theme::palette(ui.visuals().dark_mode);
        let c = &self.conversations[idx];
        if let Some((rid, text)) = &mut self.renaming
            && *rid == c.id
        {
            let resp = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
            resp.request_focus();
            if resp.lost_focus() {
                acts.rename_commit = Some((c.id, text.clone()));
            }
            return;
        }
        let selected = self.current == Some(c.id) && self.page == Page::Chat;
        // One state mark per row, the most urgent first.
        let (mark, state) = if self
            .runs
            .get(&c.id)
            .is_some_and(|r| !r.approvals.is_empty())
        {
            (Some((icon::SHIELD, p.warn)), "Waiting for approval")
        } else if self.runs.contains_key(&c.id) {
            (Some((icon::HOURGLASS, p.signal)), "Replying…")
        } else if self.queue.contains(c.id) {
            (Some((icon::CLOCK, p.pencil)), "Queued")
        } else if self.unread.contains(&c.id) && !selected {
            (Some((icon::OK, p.signal)), "Reply ended, not read yet")
        } else if c.pinned {
            (Some((icon::PIN, p.pencil)), "Pinned")
        } else if self.artifact_index.chats.contains(&c.id) {
            (Some((icon::FILE, p.pencil)), "Has artifacts")
        } else {
            (None, "")
        };
        let resp = crate::ui::chrome::recent_row(ui, &c.title, selected, mark, surface);
        let resp = resp.on_hover_text(format!(
            "{}{}\n{}",
            c.title,
            if state.is_empty() {
                String::new()
            } else {
                format!("\n{state}")
            },
            c.updated_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
        ));
        if resp.clicked() {
            acts.select = Some(c.id);
        }
        let projects_on = self.settings.general.projects.enabled;
        resp.context_menu(|ui| {
            let pin_label = if c.pinned {
                theme::labelled(icon::UNPIN, "Unpin")
            } else {
                theme::labelled(icon::PIN, "Pin")
            };
            if ui.button(pin_label).clicked() {
                acts.toggle_pin = Some(c.id);
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
                acts.export_md = Some(c.id);
                ui.close();
            }
            if ui
                .button(theme::labelled(icon::ARROW_SQUARE_OUT, "Share as HTML…"))
                .clicked()
            {
                acts.export_html = Some(c.id);
                ui.close();
            }
            if projects_on {
                ui.separator();
                if !self.projects.is_empty() {
                    ui.menu_button(theme::labelled(icon::FOLDER, "Move to project"), |ui| {
                        for project in &self.projects {
                            let current = c.project == Some(project.id);
                            if ui
                                .add_enabled(!current, egui::Button::new(project.name.as_str()))
                                .clicked()
                            {
                                acts.move_to = Some((c.id, Some(project.id)));
                                ui.close();
                            }
                        }
                    });
                }
                if c.project.is_some() {
                    if ui.button("Remove from project").clicked() {
                        acts.move_to = Some((c.id, None));
                        ui.close();
                    }
                    if ui
                        .button("Use this chat's tool switches for its project")
                        .clicked()
                    {
                        acts.tools_to_project = Some(c.id);
                        ui.close();
                    }
                }
            }
            ui.separator();
            if ui.button(theme::labelled(icon::TRASH, "Delete")).clicked() {
                acts.delete = Some(c.id);
                ui.close();
            }
        });
    }

    // ---------------------------------------------------------------- projects

    fn open_project_editor(&mut self, id: Option<Uuid>) {
        let draft = match id.and_then(|id| self.projects.iter().find(|p| p.id == id)) {
            Some(project) => project.clone(),
            None => Project::new("", &self.settings.general.projects),
        };
        self.project_editor = Some(ProjectEditor {
            is_new: id.is_none(),
            draft,
        });
    }

    /// The project editor and the delete confirmation, while they are open.
    fn project_windows(&mut self, ctx: &egui::Context) {
        if let Some(editor) = &mut self.project_editor {
            let providers: Vec<(String, String)> = self
                .settings
                .providers
                .iter()
                .map(|p| (p.id.clone(), p.name.clone()))
                .collect();
            match project_editor::show(ctx, editor, &providers) {
                EditorOutcome::Open => {}
                EditorOutcome::Cancel => self.project_editor = None,
                EditorOutcome::Save(project) => {
                    self.project_editor = None;
                    self.save_project(*project);
                }
            }
        }
        if let Some(id) = self.project_to_delete {
            let name = self
                .projects
                .iter()
                .find(|p| p.id == id)
                .map_or(String::new(), |p| p.name.clone());
            match project_editor::confirm_delete(ctx, &name) {
                Some(true) => {
                    self.project_to_delete = None;
                    self.delete_project(id);
                }
                Some(false) => self.project_to_delete = None,
                None => {}
            }
        }
    }

    /// Save a project to disk and in the list.
    fn save_project(&mut self, mut project: Project) {
        project.updated = chrono::Utc::now();
        project.save();
        match self.projects.iter_mut().find(|p| p.id == project.id) {
            Some(existing) => *existing = project,
            None => self.projects.push(project),
        }
        self.projects.sort_by_key(|p| p.name.to_lowercase());
    }

    /// Delete a project's file. Its chats stay, outside the project.
    fn delete_project(&mut self, id: Uuid) {
        Project::delete(id);
        self.projects.retain(|p| p.id != id);
        self.collapsed_projects.remove(&id);
        let linked: Vec<Uuid> = self
            .conversations
            .iter()
            .filter(|c| c.project == Some(id))
            .map(|c| c.id)
            .collect();
        projects::unlink_chats(&mut self.conversations, id);
        for c in self
            .conversations
            .iter()
            .filter(|c| linked.contains(&c.id) && !c.messages.is_empty())
        {
            c.save();
        }
        self.notice = Some("Project deleted; its chats are kept.".into());
    }

    /// A new chat in a project, with the project's settings. An empty one of the project is
    /// reused, as an empty chat is anywhere.
    fn new_project_chat(&mut self, pid: Uuid) {
        let Some(project) = self.projects.iter().find(|p| p.id == pid).cloned() else {
            return;
        };
        let default_mode = self.settings.general.default_approval_mode;
        self.collapsed_projects.remove(&pid);
        let reuse = self
            .conversations
            .iter()
            .find(|c| c.project == Some(pid) && c.messages.is_empty())
            .map(|c| c.id);
        let id = match reuse {
            Some(id) => id,
            None => {
                let conv = projects::new_chat(&project, default_mode);
                let id = conv.id;
                self.conversations.insert(0, conv);
                id
            }
        };
        if let Some(conv) = self.conv_mut(id) {
            projects::apply_to(&project, conv, default_mode);
        }
        self.current = Some(id);
        self.input.clear();
    }

    /// Move a chat into a project (`Some`), or out of its project (`None`).
    fn move_chat_to(&mut self, id: Uuid, project: Option<Uuid>) {
        let target = match project {
            None => None,
            Some(pid) => match self.projects.iter().find(|p| p.id == pid).cloned() {
                Some(p) => Some(p),
                None => return,
            },
        };
        if let Some(conv) = self.conv_mut(id) {
            projects::move_chat(conv, target.as_ref());
            if !conv.messages.is_empty() {
                conv.save();
            }
        }
    }

    /// Make the project's tool switches those of the chat (from the chat's menu).
    fn save_tools_to_project(&mut self, chat: Uuid) {
        let Some(conv) = self.conversations.iter().find(|c| c.id == chat) else {
            return;
        };
        let (overrides, Some(pid)) = (conv.tool_overrides.clone(), conv.project) else {
            return;
        };
        let Some(mut project) = self.projects.iter().find(|p| p.id == pid).cloned() else {
            return;
        };
        project.tool_overrides = overrides;
        self.save_project(project);
        self.notice = Some("The project's tool switches are now this chat's.".into());
    }

    /// Save a project and its chats to one file (see `projects::bundle`).
    fn export_project(&mut self, id: Uuid) {
        let Some(project) = self.projects.iter().find(|p| p.id == id) else {
            return;
        };
        let Some(path) = projects::pick_export_path(&project.name) else {
            return;
        };
        let doc = projects::bundle(project, &self.conversations);
        self.notice = Some(match projects::write_bundle(&path, &doc) {
            Ok(()) => format!("Exported \"{}\" to {}", project.name, path.display()),
            Err(e) => format!("Export failed: {e:#}"),
        });
    }

    /// Add a project and its chats from an export file, with new ids.
    fn import_project(&mut self) {
        let Some(path) = projects::pick_import_path() else {
            return;
        };
        let parsed = std::fs::read(&path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))
            .and_then(|bytes| {
                projects::parse_bundle(&bytes).map_err(|e| format!("Import failed: {e:#}"))
            });
        self.notice = Some(match parsed {
            Ok(doc) => {
                let (project, chats) = projects::unbundle(doc);
                let (name, count) = (project.name.clone(), chats.len());
                for c in chats.iter().filter(|c| !c.messages.is_empty()) {
                    c.save();
                }
                self.conversations.extend(chats);
                self.conversations
                    .sort_by_key(|c| std::cmp::Reverse(c.updated_at));
                self.save_project(project);
                format!("Imported project \"{name}\" with {count} chat(s).")
            }
            Err(e) => e,
        });
    }

    // ---------------------------------------------------------------- composer

    fn composer(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let id = egui::Id::unique(INPUT_ID);
        let running = self.current.is_some_and(|c| self.busy(c));
        let fresh = self
            .current
            .and_then(|c| self.conversations.iter().find(|x| x.id == c))
            .is_none_or(|c| c.messages.is_empty());
        // A new chat, and the app as it opens, are ready to type into.
        if (fresh || !self.input_focused_once) && ui.memory(|m| m.focused().is_none()) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        self.input_focused_once = true;
        let has_focus = ui.memory(|m| m.has_focus(id));
        let mut send = false;
        if has_focus && !running {
            send = ui.input_mut(|i| {
                let cmd_enter = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
                let enter = self.settings.general.send_with_enter && consume_plain_enter(i);
                cmd_enter || enter
            });
        }

        // Slash commands and skills whose names start with the `/name` being typed (only while
        // the first word is typed after the slash).
        let typing_command =
            self.input.starts_with('/') && !self.input.contains(char::is_whitespace);
        let suggestions: Vec<(bool, String, String)> = if typing_command {
            let workspace = self.current_workspace();
            let mut out: Vec<(bool, String, String)> = Vec::new();
            if self.settings.general.slash_commands.enabled {
                let all = self
                    .slash_cache
                    .get(&self.settings.general.slash_commands, workspace.as_deref());
                out.extend(
                    crate::commands::suggestions(&self.input, all)
                        .into_iter()
                        .map(|c| (false, c.name, c.description)),
                );
            }
            let typed = &self.input[1..];
            let skills = self
                .skill_cache
                .get(&self.settings.general.skills, workspace.as_deref());
            for skill in skills {
                if skill.name.starts_with(typed) && !out.iter().any(|(_, n, _)| *n == skill.name) {
                    out.push((true, skill.name.clone(), skill.description.clone()));
                }
            }
            out.truncate(12);
            out
        } else {
            Vec::new()
        };

        let width = if fresh {
            chat_view::HOME_WIDTH
        } else {
            chat_view::MAX_CONTENT_WIDTH
        };
        if !fresh {
            ui.add_space(8.0);
        }
        chat_view::centered_at(ui, width, |ui| {
            if let Some(n) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(n).small().color(p.warn));
                    if icon_button(ui, icon::CLOSE, "Dismiss").clicked() {
                        self.notice = None;
                    }
                });
            }
            if !suggestions.is_empty() {
                let mut picked = None;
                egui::Frame::new()
                    .fill(p.raised)
                    .stroke(egui::Stroke::new(1.0, p.hairline))
                    .corner_radius(12)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let mut last = None;
                        for (skill, name, description) in &suggestions {
                            if last != Some(*skill) {
                                ui.label(
                                    RichText::new(if *skill { "Skills" } else { "Commands" })
                                        .size(12.0)
                                        .color(p.pencil),
                                );
                                last = Some(*skill);
                            }
                            let glyph = if *skill { icon::BOOKS } else { icon::COMMAND };
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                &format!("{glyph}  /{name}"),
                                0.0,
                                egui::TextFormat::simple(egui::FontId::proportional(14.0), p.ink),
                            );
                            if !description.is_empty() {
                                job.append(
                                    description,
                                    12.0,
                                    egui::TextFormat::simple(
                                        egui::FontId::proportional(13.0),
                                        p.pencil,
                                    ),
                                );
                            }
                            job.wrap = egui::text::TextWrapping::truncate_at_width(
                                ui.available_width() - 16.0,
                            );
                            if ui
                                .add(
                                    egui::Button::selectable(false, job)
                                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                                )
                                .clicked()
                            {
                                picked = Some(name.clone());
                            }
                        }
                    });
                if let Some(name) = picked {
                    // Replace the typed name and put the cursor after the space.
                    self.input = format!("/{name} ");
                    let end = egui::text::CCursor::new(self.input.chars().count());
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(end)));
                        state.store(ui.ctx(), id);
                    }
                    ui.memory_mut(|m| m.request_focus(id));
                }
                ui.add_space(6.0);
            }
            // Live stats for the request currently streaming in; the finished response
            // carries the same figures, so this only shows mid-completion. Prompt progress
            // is a status rather than a stat, so it shows with stats off too.
            if let Some(s) = self
                .current
                .and_then(|c| self.runs.get(&c))
                .and_then(|r| r.live_stats.as_ref())
                .filter(|s| self.settings.general.show_stats || s.prompt_progress.is_some())
            {
                super::ui::stats_view::live_line(ui, s);
                ui.add_space(6.0);
            }
            egui::Frame::new()
                .fill(p.raised)
                .stroke(egui::Stroke::new(
                    1.0,
                    if has_focus { p.outline } else { p.hairline },
                ))
                .corner_radius(14)
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 8,
                    top: 12,
                    bottom: 6,
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
                            let text = if self.reading_ocr > 0 {
                                "Reading scanned PDF… (OCR)".to_owned()
                            } else {
                                format!("Reading {n} file{}…", if n == 1 { "" } else { "s" })
                            };
                            ui.label(RichText::new(text).small().weak());
                        });
                    }
                    let hint = if self.voice.recording.is_some() {
                        "Listening…"
                    } else if self.voice.transcribing {
                        "Transcribing…"
                    } else if !fresh {
                        "Reply…"
                    } else if self.settings.general.mode == crate::config::AppMode::Code {
                        "Describe a task, or type / for skills"
                    } else {
                        "Type / for skills"
                    };
                    egui::ScrollArea::vertical()
                        .id_salt("input-scroll")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.input)
                                    .id(id)
                                    .font(egui::FontId::proportional(15.5))
                                    .hint_text(RichText::new(hint).size(15.5).color(p.pencil))
                                    .frame(egui::Frame::NONE)
                                    .desired_rows(2)
                                    // Tab moves on to the buttons below, so keyboard users
                                    // aren't trapped in the box (Shift+Enter makes newlines).
                                    .lock_focus(false)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    ui.horizontal(|ui| {
                        // Icon buttons without outlines, like the rest of the box.
                        let w = &mut ui.visuals_mut().widgets.inactive;
                        w.bg_stroke = egui::Stroke::NONE;
                        w.weak_bg_fill = egui::Color32::TRANSPARENT;
                        ui.spacing_mut().item_spacing.x = 2.0;
                        self.plus_menu(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let can_send = (!self.input.trim().is_empty()
                                || !self.pending_attachments.is_empty())
                                && self.reading_files == 0;
                            if running {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(icon::STOP).size(15.0).color(p.paper),
                                        )
                                        .fill(p.ink)
                                        .corner_radius(8)
                                        .min_size(egui::vec2(32.0, 32.0)),
                                    )
                                    .on_hover_text("Stop")
                                    .clicked()
                                {
                                    let cid = self.current.unwrap();
                                    self.stop(cid);
                                }
                            } else if can_send {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(icon::ARROW_UP)
                                                .size(16.0)
                                                .color(egui::Color32::WHITE),
                                        )
                                        .fill(p.signal)
                                        .corner_radius(8)
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
                            } else {
                                let tip = if self.voice_mode {
                                    "End voice mode"
                                } else {
                                    "Voice mode: talk, hear the reply read aloud, and talk again"
                                };
                                if crate::ui::chrome::ghost_icon(
                                    ui,
                                    icon::WAVEFORM,
                                    tip,
                                    self.voice_mode,
                                )
                                .clicked()
                                {
                                    self.toggle_voice_mode();
                                }
                            }
                            self.mic_device_menu(ui);
                            self.mic_button(ui);
                        });
                    });
                });
            ui.add_space(4.0);
            self.control_strip(ui);
        });
        ui.add_space(6.0);
        if send {
            let ctx = ui.ctx().clone();
            self.send(&ctx);
            ui.memory_mut(|m| m.request_focus(id));
        }
    }

    /// The composer's "+": attach files, and submenus for this chat's tools and the MCP
    /// servers' resources and prompts.
    fn plus_menu(&mut self, ui: &mut egui::Ui) {
        use egui::containers::menu::{MenuButton, MenuConfig};
        let p = theme::palette(ui.visuals().dark_mode);
        let mut attach = false;
        let mut slash = false;
        let (resp, _) = MenuButton::new(RichText::new(icon::PLUS).size(18.0).color(p.ink2))
            .config(MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                ui.set_min_width(230.0);
                if ui
                    .button(theme::labelled(icon::PAPERCLIP, "Attach files…"))
                    .clicked()
                {
                    attach = true;
                    ui.close();
                }
                self.tools_menu(ui);
                self.mcp_menu(ui);
                if ui
                    .button(theme::labelled(icon::COMMAND, "Commands and skills  /"))
                    .clicked()
                {
                    slash = true;
                    ui.close();
                }
            });
        resp.on_hover_text("Attach files, tools, resources and prompts (or drop files here)");
        if attach
            && let Some(paths) = rfd::FileDialog::new()
                .set_title("Attach files")
                .pick_files()
        {
            self.attach_paths(paths);
        }
        if slash && self.input.is_empty() {
            self.input = "/".to_owned();
            let id = egui::Id::unique(INPUT_ID);
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(1),
                )));
            state.store(ui.ctx(), id);
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
            self.home(ui);
            return Vec::new();
        }
        let conv = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id))
            .expect("not a fresh chat");
        let plan_ready = !self.busy(conv.id)
            && !self.errors.contains_key(&conv.id)
            && crate::plan::awaiting_approval(conv, &self.settings.general.plan);
        chat_view::show(
            ui,
            &mut self.md_cache,
            &mut self.drawings,
            &mut self.images,
            chat_view::View {
                conv,
                run: self.runs.get(&conv.id),
                queued: self.queue.position(conv.id),
                error: self.errors.get(&conv.id),
                show_stats: self.settings.general.show_stats,
                reasoning: self.settings.general.reasoning_display,
                remote_images: self.settings.general.load_remote_images,
                artifacts: &self.artifacts.list,
                open_artifact: self
                    .artifact
                    .as_ref()
                    .filter(|v| v.conv == conv.id)
                    .map(|v| (v.message, v.index)),
                save_as: self.settings.general.save_as,
                plan_ready,
                voice_out: self.settings.general.voice.output.enabled,
                speaking: self
                    .voice
                    .speaking_key()
                    .filter(|(c, _)| *c == conv.id)
                    .map(|(_, i)| i),
            },
        )
    }

    /// "Open live preview" (Linux): the page in a window of its own. Without WebKitGTK, or a
    /// display, the browser opens it instead, with a notice the first time.
    fn open_live_preview(&mut self, a: &crate::artifacts::Artifact) {
        #[cfg(target_os = "linux")]
        {
            use crate::ui::linux_preview;
            match linux_preview::show(&a.title, &a.source, &self.settings.general.html_preview) {
                Ok(()) => return,
                Err(why) if linux_preview::take_fallback_notice() => {
                    self.notice = Some(if linux_preview::probe().is_err() {
                        "Live preview needs WebKitGTK (libwebkit2gtk-4.1); opened in the browser \
                         instead"
                            .to_owned()
                    } else {
                        format!("Live preview couldn't open ({why}); opened in the browser instead")
                    });
                }
                Err(_) => {}
            }
        }
        if let Err(e) = crate::artifacts::open_in_browser(a) {
            self.notice = Some(format!("Couldn't open it in the browser: {e}"));
        }
    }

    fn apply_actions(&mut self, actions: Vec<Action>, ctx: &egui::Context) {
        for a in actions {
            match a {
                Action::Copy(text) => ctx.copy_text(text),
                Action::Regenerate(id) => {
                    let _ = self.regenerate(id, ctx);
                }
                Action::Approve(conv, call_id, answer) => {
                    self.approve(conv, &call_id, answer);
                }
                Action::ApprovePlan(conv) => self.approve_plan(conv, ctx),
                Action::Speak(conv, idx) => self.toggle_speaking(conv, idx),
                Action::KeepPlanning => {
                    ctx.memory_mut(|m| m.request_focus(egui::Id::unique(INPUT_ID)));
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
                Action::OpenArtifact(conv, message, index) => {
                    let list = &self.artifacts.list;
                    if let Some(pos) = list
                        .iter()
                        .position(|a| (a.message, a.index) == (message, index))
                    {
                        self.artifact =
                            Some(ArtifactView::new(conv, &list[pos], pos + 1 == list.len()));
                    }
                }
                Action::CloseArtifact => self.artifact = None,
                Action::SaveArtifact(a) => {
                    let name = a.file_name();
                    let ext = name.rsplit_once('.').map_or("txt", |(_, e)| e).to_owned();
                    if let Some(path) = export::pick_save_path_as(&name, "File", &ext) {
                        self.notice = Some(match std::fs::write(&path, &a.source) {
                            Ok(()) => format!("Saved {}", path.display()),
                            Err(e) => format!("Couldn't save {}: {e}", path.display()),
                        });
                    }
                }
                Action::UndoTurn(conv, idx) => {
                    self.notice = Some(match self.undo_turn(conv, idx) {
                        Ok(msg) | Err(msg) => msg,
                    });
                }
                Action::SaveAs {
                    conv,
                    title,
                    content,
                    format,
                } => self.save_as(conv, title, content, format),
                Action::OpenArtifactInBrowser(a) => {
                    if let Err(e) = crate::artifacts::open_in_browser(&a) {
                        self.notice = Some(format!("Couldn't open it in the browser: {e}"));
                    }
                }
                Action::OpenLivePreview(a) => self.open_live_preview(&a),
                Action::ForkFrom(conv, idx) => {
                    self.notice = Some(match self.fork_chat(conv, idx) {
                        Ok(_) => "Forked into a new chat".into(),
                        Err(e) => e,
                    });
                }
                Action::SwitchBranch(conv, at, to) => {
                    if self.busy(conv) {
                        continue;
                    }
                    if let Some(c) = self.conv_mut(conv)
                        && branches::switch(&mut c.messages, at, to)
                    {
                        c.updated_at = chrono::Utc::now();
                        c.save();
                    }
                    if self.editing.is_some_and(|(c, _)| c == conv) {
                        self.editing = None;
                    }
                }
                Action::EditFrom(conv, idx) => {
                    if self.busy(conv) {
                        continue;
                    }
                    if let Some(c) = self.conv_mut(conv)
                        && idx < c.messages.len()
                    {
                        // The turn is replaced when the edit is sent, so until then the chat
                        // still shows it.
                        let text = c.messages[idx].content.clone();
                        let attachments = c.messages[idx].attachments.clone();
                        self.editing = Some((conv, idx));
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

impl ChatApp {
    /// Starts, restarts or stops the remote-access server and its Cloudflare Tunnel to match
    /// the settings. A running server or tunnel with the same settings is left alone; a
    /// change to the tunnel alone doesn't restart the server.
    fn sync_remote(&mut self) {
        // Access settings don't restart the server (they are not in `server_settings`), so
        // the server's checks are updated here.
        self.access.configure(&self.settings.general.remote.tunnel);
        self.sync_server();
        self.sync_tunnel();
    }

    fn sync_server(&mut self) {
        let cfg = &self.settings.general.remote;
        let server = crate::remote::server_settings(cfg);
        if self
            .remote
            .as_ref()
            .is_some_and(|h| crate::remote::server_settings(h.config()) == server)
        {
            return;
        }
        // Stop the old server first, so its port is free for the new one.
        self.remote = None;
        self.remote_seen.clear();
        self.remote_list_seen = 0;
        if !cfg.enabled {
            self.remote_status = RemoteStatus::Off;
            return;
        }
        let ctx = self.repaint_ctx.clone();
        let repaint = Arc::new(move || ctx.request_repaint());
        let hosts = self.tunnel_hosts.clone();
        let access = self.access.clone();
        match crate::remote::start(&self.rt, cfg, self.tx.clone(), repaint, hosts, access) {
            Ok(handle) => {
                self.remote_status = RemoteStatus::Running {
                    urls: handle.urls().to_vec(),
                    plain_http: cfg.tls == RemoteTls::Off,
                };
                self.remote = Some(handle);
            }
            Err(e) => {
                log::warn!("remote access did not start: {e}");
                self.remote_status = RemoteStatus::Failed(e);
            }
        }
    }

    fn sync_tunnel(&mut self) {
        let cfg = &self.settings.general.remote;
        let wanted = cfg.enabled && cfg.tunnel.enabled;
        let password_ok =
            cfg.password.trim().chars().count() >= crate::remote::MIN_TUNNEL_PASSWORD_CHARS;
        let service = crate::remote::tunnel_service_url(cfg);
        if wanted
            && password_ok
            && self.remote.is_some()
            && self
                .tunnel
                .as_ref()
                .is_some_and(|t| *t.config() == cfg.tunnel && t.service() == service)
        {
            return;
        }
        self.tunnel = None;
        self.tunnel_error = None;
        if !wanted {
            return;
        }
        if self.remote.is_none() {
            self.tunnel_error = Some("The tunnel starts once the server is running".into());
            return;
        }
        let ctx = self.repaint_ctx.clone();
        let repaint = Arc::new(move || ctx.request_repaint());
        match crate::remote::start_tunnel(&self.rt, cfg, self.tunnel_hosts.clone(), repaint) {
            Ok(handle) => self.tunnel = Some(handle),
            Err(e) => {
                log::warn!("the Cloudflare Tunnel did not start: {e}");
                self.tunnel_error = Some(e);
            }
        }
    }

    /// The tunnel's state for the settings page.
    fn tunnel_status(&self) -> crate::remote::TunnelStatus {
        match (&self.tunnel, &self.tunnel_error) {
            (Some(t), _) => t.status(),
            (None, Some(e)) => crate::remote::TunnelStatus::Failed {
                error: e.clone(),
                retrying: false,
            },
            (None, None) => crate::remote::TunnelStatus::Off,
        }
    }

    /// Answers a request from a browser, through the same code the desktop UI uses.
    fn handle_remote(&mut self, cmd: RemoteCommand) {
        let ctx = self.repaint_ctx.clone();
        match cmd {
            RemoteCommand::State { reply } => {
                let _ = reply.send(self.remote_state());
            }
            RemoteCommand::Chat { id, reply } => {
                let _ = reply.send(self.remote_chat(id));
            }
            RemoteCommand::NewChat { reply } => {
                let _ = reply.send(self.create_chat());
            }
            RemoteCommand::Send { chat, text, reply } => {
                let _ = reply.send(self.remote_send(chat, text, &ctx));
            }
            RemoteCommand::Stop { chat, reply } => {
                self.stop(chat);
                let _ = reply.send(());
            }
            RemoteCommand::Regenerate { chat, reply } => {
                let _ = reply.send(self.regenerate(chat, &ctx));
            }
            RemoteCommand::Approve {
                chat,
                call_id,
                answer,
                reply,
            } => {
                let result = if self.approve(chat, &call_id, answer) {
                    Ok(())
                } else {
                    Err("No such approval is waiting".into())
                };
                let _ = reply.send(result);
            }
            RemoteCommand::Rename { chat, title, reply } => {
                let _ = reply.send(self.rename_chat(chat, &title));
            }
            RemoteCommand::Pin {
                chat,
                pinned,
                reply,
            } => {
                let _ = reply.send(self.set_pinned(chat, pinned));
            }
            RemoteCommand::Delete { chat, reply } => {
                self.delete_chat(chat);
                let _ = reply.send(());
            }
            RemoteCommand::SetModel {
                provider,
                model,
                reply,
            } => {
                let _ = reply.send(self.set_remote_model(provider, model));
            }
        }
    }

    /// Sends a browser's message. Without a chat, it goes to a new one (or the empty one).
    fn remote_send(
        &mut self,
        chat: Option<Uuid>,
        text: String,
        ctx: &egui::Context,
    ) -> Result<Uuid, String> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err("The message is empty".into());
        }
        let id = chat.unwrap_or_else(|| self.create_chat());
        self.send_message(id, text, Vec::new(), ctx)?;
        Ok(id)
    }

    /// Picks a provider and its model, as the model picker does.
    fn set_remote_model(&mut self, index: usize, model: String) -> Result<(), String> {
        let model = remote_model_name(&model)?;
        let provider = self
            .settings
            .providers
            .get_mut(index)
            .ok_or("No such provider")?;
        provider.model = model.to_owned();
        self.settings.active_provider = index;
        self.save_settings();
        Ok(())
    }

    fn remote_state(&self) -> StateView {
        let mut chats: Vec<&Conversation> = self
            .conversations
            .iter()
            .filter(|c| listed(c, &self.runs))
            .collect();
        // Pinned first, then newest first, as in the sidebar.
        chats.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        StateView {
            chats: chats
                .into_iter()
                .map(|c| chat_summary(c, &self.runs, &self.projects))
                .collect(),
            providers: self
                .settings
                .providers
                .iter()
                .map(|p| ProviderView {
                    name: p.name.clone(),
                    model: p.model.clone(),
                    models: p.models.clone(),
                })
                .collect(),
            active_provider: self
                .settings
                .active_provider
                .min(self.settings.providers.len().saturating_sub(1)),
            send_with_enter: self.settings.general.send_with_enter,
            theme: theme_name(&self.settings.general.theme).to_owned(),
            user_name: self.user_name.clone(),
        }
    }

    fn remote_chat(&self, id: Uuid) -> Option<ChatView> {
        let conv = self.conversations.iter().find(|c| c.id == id)?;
        let run = self.runs.get(&id);
        Some(ChatView {
            id,
            title: conv.title.clone(),
            pinned: conv.pinned,
            messages: conv.messages.iter().map(message_view).collect(),
            running: run.is_some(),
            running_tool: run.and_then(|r| r.running_tool.clone()),
            approvals: run
                .map(|r| r.approvals.iter().map(approval_view).collect())
                .unwrap_or_default(),
            error: self.errors.get(&id).cloned(),
        })
    }

    /// Tells the connected browsers about the chats and the list that changed since the last
    /// frame. Does nothing while the server is off. A chat that changes faster than
    /// `REMOTE_CHAT_INTERVAL` is told about later, and a repaint is requested for then.
    fn publish_remote(&mut self) {
        let Some(remote) = &self.remote else {
            return;
        };
        let current: Vec<(Uuid, u64)> = self
            .conversations
            .iter()
            .map(|conv| {
                let fp = chat_fingerprint(conv, self.runs.get(&conv.id), self.errors.get(&conv.id));
                (conv.id, fp)
            })
            .collect();
        let (chats, wait) = chat_notices(
            &current,
            &mut self.remote_seen,
            &mut self.remote_sent,
            Instant::now(),
        );
        let mut events: Vec<RemoteEvent> = chats
            .into_iter()
            .map(|id| RemoteEvent::Chat { id })
            .collect();
        let list = list_fingerprint(
            &self.conversations,
            &self.runs,
            &self.settings,
            &self.projects,
        );
        if list != self.remote_list_seen {
            self.remote_list_seen = list;
            events.push(RemoteEvent::List);
        }
        for event in events {
            remote.notify(event);
        }
        if let Some(wait) = wait {
            self.repaint_ctx.request_repaint_after(wait);
        }
    }
}

/// The least time between two `RemoteEvent::Chat` notices for the same chat.
const REMOTE_CHAT_INTERVAL: Duration = Duration::from_millis(200);

/// Decides which chats to tell the browsers about this frame. `current` lists each chat with
/// its fingerprint now, `seen` the fingerprints last sent, `sent` when each chat was last
/// notified. Returns the chats to notify, and how long until a changed chat that was held back
/// may go out (`None` when nothing is held back). A held-back chat keeps its old fingerprint, so
/// it still differs on a later frame and is notified then. Deleted chats are notified at once.
fn chat_notices(
    current: &[(Uuid, u64)],
    seen: &mut HashMap<Uuid, u64>,
    sent: &mut HashMap<Uuid, Instant>,
    now: Instant,
) -> (Vec<Uuid>, Option<Duration>) {
    let mut notify = Vec::new();
    let mut wait: Option<Duration> = None;
    let mut next_seen = HashMap::with_capacity(current.len());
    for &(id, fp) in current {
        let previous = seen.get(&id).copied();
        if previous == Some(fp) {
            next_seen.insert(id, fp);
            continue;
        }
        match throttle(sent, id, now) {
            None => {
                notify.push(id);
                next_seen.insert(id, fp);
            }
            Some(left) => {
                wait = Some(wait.map_or(left, |w| w.min(left)));
                if let Some(old) = previous {
                    next_seen.insert(id, old);
                }
            }
        }
    }
    let live: HashSet<Uuid> = current.iter().map(|(id, _)| *id).collect();
    // Chats deleted since the last frame: a final notice, never held back.
    notify.extend(seen.keys().filter(|id| !live.contains(id)));
    sent.retain(|id, _| live.contains(id));
    *seen = next_seen;
    (notify, wait)
}

/// Whether chat `id` may be notified at `now`: `None` and the notice is recorded, or the time
/// left until it may be.
fn throttle(sent: &mut HashMap<Uuid, Instant>, id: Uuid, now: Instant) -> Option<Duration> {
    if let Some(&at) = sent.get(&id) {
        let elapsed = now.saturating_duration_since(at);
        if elapsed < REMOTE_CHAT_INTERVAL {
            return Some(REMOTE_CHAT_INTERVAL - elapsed);
        }
    }
    sent.insert(id, now);
    None
}

/// Where a compaction marker goes so that `kept` messages (not counting cancelled prompts or
/// earlier markers, which the agent never saw) follow it.
fn compaction_marker_at(messages: &[ChatMessage], kept: usize) -> usize {
    let mut seen = 0;
    for (i, m) in messages.iter().enumerate().rev() {
        if seen == kept {
            return i + 1;
        }
        if !m.cancelled && m.compaction.is_none() {
            seen += 1;
        }
    }
    if seen == kept { 0 } else { messages.len() }
}

/// The model name a browser sent, trimmed. Names are limited to 200 characters.
fn remote_model_name(model: &str) -> Result<&str, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Choose a model".into());
    }
    if model.chars().count() > REMOTE_MODEL_MAX_CHARS {
        return Err("Model name is too long".into());
    }
    Ok(model)
}

/// The longest model name a browser may set.
const REMOTE_MODEL_MAX_CHARS: usize = 200;

/// Whether the sidebar lists a chat: an empty new chat is listed once it has a message or a run.
fn listed(conv: &Conversation, runs: &HashMap<Uuid, Run>) -> bool {
    !conv.messages.is_empty() || runs.contains_key(&conv.id)
}

fn theme_name(theme: &Theme) -> &'static str {
    match theme {
        Theme::System => "system",
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

fn chat_summary(
    conv: &Conversation,
    runs: &HashMap<Uuid, Run>,
    projects: &[Project],
) -> ChatSummary {
    ChatSummary {
        id: conv.id,
        title: conv.title.clone(),
        pinned: conv.pinned,
        updated_at: conv.updated_at,
        running: runs.contains_key(&conv.id),
        approvals: runs.get(&conv.id).map_or(0, |r| r.approvals.len()),
        project: conv
            .project
            .and_then(|pid| projects.iter().find(|p| p.id == pid))
            .map(|p| p.name.clone()),
    }
}

fn message_view(m: &ChatMessage) -> MessageView {
    MessageView {
        role: m.role,
        content: m.content.clone(),
        reasoning: m.reasoning.clone(),
        tool_calls: m
            .tool_calls
            .iter()
            .map(|call| ToolCallView {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                label: tools::display_name(&call.name),
            })
            .collect(),
        tool_call_id: m.tool_call_id.clone(),
        tool_name: m.tool_name.clone(),
        is_error: m.is_error,
        cancelled: m.cancelled,
        // The text and data of a file stay in the app.
        attachments: m
            .attachments
            .iter()
            .map(|a| AttachmentView {
                name: a.name.clone(),
                mime: a.mime.clone(),
            })
            .collect(),
        stats: m.stats.as_ref().map(crate::ui::stats_view::summary),
        sandbox: m.sandbox.as_ref().map(|s| s.image.clone()),
    }
}

fn approval_view(p: &PendingApproval) -> ApprovalView {
    ApprovalView {
        call_id: p.call.id.clone(),
        name: p.call.name.clone(),
        label: tools::display_name(&p.call.name),
        arguments: p.call.arguments.clone(),
        server_name: p.server_name.clone(),
        can_always_tool: !crate::approval::always_asks(&p.call.name),
        can_always_server: p.origin.is_some(),
    }
}

/// A change marker for one chat as the browsers see it. Streaming text changes the last
/// message's length, so that is hashed rather than the whole text.
fn chat_fingerprint(conv: &Conversation, run: Option<&Run>, error: Option<&String>) -> u64 {
    let mut h = DefaultHasher::new();
    conv.messages.len().hash(&mut h);
    if let Some(last) = conv.messages.last() {
        last.content.len().hash(&mut h);
        last.reasoning.as_ref().map(String::len).hash(&mut h);
    }
    conv.title.hash(&mut h);
    conv.pinned.hash(&mut h);
    conv.updated_at.hash(&mut h);
    conv.workspace.hash(&mut h);
    run.is_some().hash(&mut h);
    if let Some(run) = run {
        run.approvals.len().hash(&mut h);
        run.running_tool.hash(&mut h);
    }
    error.hash(&mut h);
    h.finish()
}

/// A change marker for the chat list and the models, as the browsers see them.
fn list_fingerprint(
    conversations: &[Conversation],
    runs: &HashMap<Uuid, Run>,
    settings: &Settings,
    projects: &[Project],
) -> u64 {
    let mut h = DefaultHasher::new();
    for conv in conversations.iter().filter(|c| listed(c, runs)) {
        conv.id.hash(&mut h);
        conv.title.hash(&mut h);
        conv.pinned.hash(&mut h);
        conv.updated_at.hash(&mut h);
        conv.project.hash(&mut h);
        let run = runs.get(&conv.id);
        run.is_some().hash(&mut h);
        run.map_or(0, |r| r.approvals.len()).hash(&mut h);
    }
    for project in projects {
        project.id.hash(&mut h);
        project.name.hash(&mut h);
    }
    settings.active_provider.hash(&mut h);
    settings.general.send_with_enter.hash(&mut h);
    theme_name(&settings.general.theme).hash(&mut h);
    for p in &settings.providers {
        p.name.hash(&mut h);
        p.model.hash(&mut h);
        p.models.hash(&mut h);
    }
    h.finish()
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

impl ChatApp {
    /// Keeps the tray icon in step with "Keep running" (Settings → Background), hides the window
    /// instead of closing it while that is on, and acts on the tray's clicks and menu choices.
    fn handle_tray(&mut self, ctx: &egui::Context) {
        let keep = self.settings.general.background.keep_running && tray::SUPPORTED;
        if keep {
            if self.tray.is_none() && !self.tray_tried {
                self.tray_tried = true;
                self.tray = tray::Tray::new(self.window_icon_dark.unwrap_or(false));
            }
        } else {
            self.tray_tried = false;
            if self.tray.take().is_some() && self.hidden_to_tray {
                // Turned off while hidden: show the window again, or nothing could.
                self.show_from_tray(ctx);
            }
        }

        // Quit and the update restart arrive as a close request too (egui sends its Close
        // command as one), so `on_close` lets those through.
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested
            && tray::on_close(keep, self.tray.is_some(), self.quitting) == tray::CloseAction::Hide
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden_to_tray = true;
            if !self.tray_notified {
                self.tray_notified = true;
                background::notify(&self.rt, tray::hidden_notice().to_owned());
            }
        }

        let commands = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for command in commands {
            match command {
                tray::Command::Show => self.show_from_tray(ctx),
                tray::Command::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        if self.hidden_to_tray {
            // A hidden window may not be redrawn, so wake up now and then to poll the tray.
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn show_from_tray(&mut self, ctx: &egui::Context) {
        self.hidden_to_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

impl eframe::App for ChatApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.window_focused = ctx.input(|i| i.focused);
        self.handle_tray(ctx);
        self.handle_events();
        self.reap_finished_runs();
        self.check_time_limits(ctx);
        self.run_schedules(ctx);
        self.start_queued(ctx);
        if let Some(id) = self.current {
            self.unread.remove(&id);
        }
        self.publish_remote();
        self.handle_shortcuts(ctx);
        self.track_place();
        self.handle_dropped_files(ctx);
        if let Some(sandbox) = &self.sandbox {
            // Wake once a minute even with no input, so idle containers are reaped on time.
            ctx.request_repaint_after(Duration::from_secs(60));
            if self.sandbox_reaped.elapsed() >= Duration::from_secs(60) {
                self.sandbox_reaped = Instant::now();
                sandbox.reap_idle(Duration::from_secs(15 * 60));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Where the live preview of a web page goes this frame, if anywhere.
        let mut web_area: Option<(egui::Rect, String)> = None;

        let dark = ui.visuals().dark_mode;
        if self.window_icon_dark != Some(dark) {
            self.window_icon_dark = Some(dark);
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(theme::window_icon(dark))));
        }
        let p = theme::palette(dark);
        // The sidebar runs the full height (its top is the title row); the canvas's own title
        // row spans the rest.
        let mut sidebar_open = self.sidebar_open;
        egui::Panel::left("sidebar")
            .resizable(false)
            .show_separator_line(false)
            .exact_size(shell::SIDEBAR_WIDTH)
            .frame(
                egui::Frame::new()
                    .fill(p.shelf)
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show_collapsible(ui, &mut sidebar_open, |ui| self.sidebar(ui));
        // Before the top bar, whose panel button needs the open chat's artifacts.
        let current = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id));
        self.artifacts.refresh(current);
        egui::Panel::top("top-bar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.paper))
            .show(ui, |ui| self.top_bar(ui));

        if self.page == Page::Projects {
            egui::CentralPanel::no_frame()
                .frame(egui::Frame::new().fill(p.paper))
                .show(ui, |ui| self.projects_page(ui));
        } else if self.page == Page::Artifacts {
            egui::CentralPanel::no_frame()
                .frame(egui::Frame::new().fill(p.paper))
                .show(ui, |ui| self.artifacts_page(ui));
        } else if self.page == Page::Scheduled {
            egui::CentralPanel::no_frame()
                .frame(egui::Frame::new().fill(p.paper))
                .show(ui, |ui| self.scheduled_page(ui));
        } else {
            let remote_images = self.settings.general.load_remote_images;
            let preview = &self.settings.general.html_preview;
            // Linux: the page opens in its own window, from the panel's button.
            #[cfg(target_os = "linux")]
            let own_window = preview.enabled.then(crate::ui::linux_preview::is_open);
            #[cfg(not(target_os = "linux"))]
            let own_window = None;
            let live_html = if !preview.enabled {
                Err("Live preview is off (Settings → General); web pages open in your browser.")
            } else if crate::ui::webview::OWN_WINDOW {
                Err(
                    "Web pages open in their own window (needs WebKitGTK); otherwise in your browser.",
                )
            } else if !crate::ui::webview::SUPPORTED {
                Err("Web pages open in your browser: this build has no web view.")
            } else if self.html_preview.error.is_some() {
                Err("The web view couldn't start, so web pages open in your browser.")
            } else {
                Ok(())
            };
            if let Some(view) = self
                .artifact
                .as_mut()
                .filter(|v| Some(v.conv) == self.current)
            {
                let mut actions = Vec::new();
                let width = ui.available_width();
                let panel = egui::Panel::right("artifact")
                    .resizable(true)
                    .default_size(width * 0.45)
                    .size_range(300.0..=(width * 0.7).max(300.0))
                    .frame(
                        egui::Frame::new()
                            .fill(ui.visuals().panel_fill)
                            .inner_margin(egui::Margin::symmetric(14, 0)),
                    )
                    .show(ui, |ui| {
                        artifact_panel::show(
                            ui,
                            &mut self.md_cache,
                            &mut self.drawings,
                            view,
                            &self.artifacts.list,
                            remote_images,
                            live_html,
                            own_window,
                            self.settings.general.save_as,
                            &mut actions,
                        )
                    });
                web_area = web_area.or(panel.inner);
                // The window follows the page the panel shows.
                #[cfg(target_os = "linux")]
                if let Some(a) = self
                    .artifacts
                    .list
                    .iter()
                    .find(|a| (a.message, a.index) == (view.message, view.index))
                    .filter(|a| a.kind == crate::artifacts::Kind::Html)
                {
                    crate::ui::linux_preview::follow(
                        &a.source,
                        &self.settings.general.html_preview,
                    );
                }
                self.apply_actions(actions, &ctx);
            }
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
        self.project_windows(&ctx);
        self.changed_files_window(&ctx);
        self.commit_dialog_window(&ctx);
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
            let mut keep = true;
            let mut actions = sui.take_queued();
            actions.extend(self.settings_modal(&ctx, &mut sui));
            let mut install_update = false;
            for action in actions {
                match action {
                    SettingsAction::Apply(s) => {
                        self.apply_settings(*s, &ctx);
                        // Keep fields the user is still editing, but pick up normalisation.
                        sui.draft.active_provider = self.settings.active_provider;
                    }
                    SettingsAction::Close => keep = false,
                    SettingsAction::RunScheduledTask(task) => {
                        self.run_now(*task, &ctx, Some(&mut sui.draft.general.schedules));
                    }
                    SettingsAction::Memory(change) => self.apply_memory_change(change),
                    SettingsAction::CheckForUpdates(url) => self.spawn_update_check(true, url),
                    SettingsAction::InstallUpdate => install_update = true,
                    SettingsAction::CheckSandbox(cfg) => {
                        sui.sandbox_busy = true;
                        sui.sandbox_status = None;
                        self.spawn_sandbox(async move { crate::sandbox::check(&cfg).await });
                    }
                    SettingsAction::PrepareSandbox(cfg) => {
                        sui.sandbox_busy = true;
                        sui.sandbox_status = None;
                        self.spawn_sandbox(async move { crate::sandbox::prepare(&cfg).await });
                    }
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
            } else {
                self.settings_opened = None;
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
        self.focus_ring(&ctx);

        // The web view is a native window above egui: show it only while nothing could be
        // drawn over its area.
        let covered = self.settings_ui.is_some()
            || self.palette.open
            || self.viewing.is_some()
            || self.prompt_form.is_some()
            || self.changed_files_open
            || ctx.any_popup_open()
            || ctx.input(|i| !i.raw.hovered_files.is_empty());
        match web_area.filter(|_| !covered) {
            Some((area, html)) => self.html_preview.show(
                frame,
                area,
                ctx.pixels_per_point(),
                &html,
                &self.settings.general.html_preview,
            ),
            None => self.html_preview.hide(),
        }
    }
}

impl ChatApp {
    /// Outline the focused widget in the accent colour while focus moves with the keyboard:
    /// most of the app's buttons have no frame, so focus would otherwise be invisible. A
    /// pointer press hides it again.
    fn focus_ring(&mut self, ctx: &egui::Context) {
        let (tabbed, pointer) =
            ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.pointer.any_pressed()));
        if pointer {
            self.keyboard_nav = false;
        } else if tabbed {
            self.keyboard_nav = true;
        }
        if !self.keyboard_nav {
            return;
        }
        let Some(rect) = ctx
            .memory(|m| m.focused())
            .and_then(|id| ctx.read_response(id))
            .map(|r| r.rect)
        else {
            return;
        };
        let accent = theme::palette(ctx.global_style().visuals.dark_mode).signal;
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("focus-ring"),
        ))
        .rect_stroke(
            rect.expand(2.0),
            6,
            egui::Stroke::new(2.0, accent),
            egui::StrokeKind::Outside,
        );
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

#[cfg(test)]
mod remote_tests {
    use super::*;
    use crate::attach::Attachment;
    use crate::types::ToolCall;

    fn conv_with_reply(text: &str) -> Conversation {
        let mut conv = Conversation::new();
        conv.messages.push(ChatMessage::new(Role::Assistant, text));
        conv
    }

    /// A run that has nothing going on but one approval waiting for `call_id`.
    fn run_awaiting(call_id: &str) -> Run {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let task = rt.spawn(std::future::pending::<()>());
        let call = ToolCall {
            id: call_id.into(),
            name: "fetch".into(),
            arguments: "{}".into(),
        };
        let (respond, _answer) = tokio::sync::oneshot::channel();
        Run {
            task,
            approvals: vec![PendingApproval::new(call, None, None, respond)],
            running_tool: None,
            live_stats: None,
            subagents: HashMap::new(),
            compacting: false,
            compact_only: false,
            started: Instant::now(),
        }
    }

    #[test]
    fn chat_fingerprint_changes_when_a_reply_streams() {
        let mut conv = conv_with_reply("Hel");
        let before = chat_fingerprint(&conv, None, None);
        assert_eq!(before, chat_fingerprint(&conv, None, None));
        conv.messages.last_mut().unwrap().content.push_str("lo");
        assert_ne!(before, chat_fingerprint(&conv, None, None));
    }

    #[test]
    fn chat_fingerprint_changes_when_an_approval_is_added() {
        let conv = conv_with_reply("Working on it");
        let mut run = run_awaiting("call_1");
        let waiting = chat_fingerprint(&conv, Some(&run), None);
        run.approvals.clear();
        assert_ne!(waiting, chat_fingerprint(&conv, Some(&run), None));
        assert_ne!(
            chat_fingerprint(&conv, None, None),
            chat_fingerprint(&conv, None, Some(&"boom".to_owned()))
        );
    }

    #[test]
    fn list_fingerprint_changes_when_a_chat_is_pinned() {
        let settings = Settings::default();
        let mut conv = conv_with_reply("hi");
        let runs = HashMap::new();
        let before = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[]);
        conv.pinned = true;
        let after = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[]);
        assert_ne!(before, after);
    }

    #[test]
    fn list_fingerprint_changes_when_a_chat_moves_to_a_project() {
        let settings = Settings::default();
        let mut conv = conv_with_reply("hi");
        let runs = HashMap::new();
        let project = Project::new("Site", &settings.general.projects);
        let before = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[]);
        conv.project = Some(project.id);
        let after = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[project]);
        assert_ne!(before, after);
    }

    #[test]
    fn chat_notices_hold_back_a_quick_change_and_deliver_it_after_the_interval() {
        let (a, t0) = (Uuid::new_v4(), Instant::now());
        let (mut seen, mut sent) = (HashMap::new(), HashMap::new());
        // A new chat is notified at once.
        let (notify, wait) = chat_notices(&[(a, 1)], &mut seen, &mut sent, t0);
        assert_eq!((notify, wait), (vec![a], None));
        // A change 50 ms later is held back for the rest of the interval, keeping the old
        // fingerprint so it still counts as changed.
        let at = t0 + Duration::from_millis(50);
        let (notify, wait) = chat_notices(&[(a, 2)], &mut seen, &mut sent, at);
        assert!(notify.is_empty());
        assert_eq!(wait, Some(Duration::from_millis(150)));
        assert_eq!(seen.get(&a), Some(&1));
        // A frame before the interval ends still holds it back.
        let (notify, _) = chat_notices(
            &[(a, 2)],
            &mut seen,
            &mut sent,
            t0 + Duration::from_millis(199),
        );
        assert!(notify.is_empty());
        // Once the interval has passed, the held-back change is delivered.
        let (notify, wait) =
            chat_notices(&[(a, 2)], &mut seen, &mut sent, t0 + REMOTE_CHAT_INTERVAL);
        assert_eq!((notify, wait), (vec![a], None));
        assert_eq!(seen.get(&a), Some(&2));
        // Unchanged chats are not notified.
        let (notify, wait) =
            chat_notices(&[(a, 2)], &mut seen, &mut sent, t0 + Duration::from_secs(5));
        assert_eq!((notify, wait), (vec![], None));
    }

    #[test]
    fn chat_notices_send_a_deleted_chat_at_once_and_forget_it() {
        let (a, b, t0) = (Uuid::new_v4(), Uuid::new_v4(), Instant::now());
        let (mut seen, mut sent) = (HashMap::new(), HashMap::new());
        chat_notices(&[(a, 1), (b, 1)], &mut seen, &mut sent, t0);
        // `b` changes and is held back, then is deleted in the same frame: it still goes out.
        let quick = t0 + Duration::from_millis(10);
        let (notify, _) = chat_notices(&[(a, 1), (b, 2)], &mut seen, &mut sent, quick);
        assert!(notify.is_empty());
        let (notify, _) = chat_notices(&[(a, 1)], &mut seen, &mut sent, quick);
        assert_eq!(notify, vec![b]);
        assert!(!seen.contains_key(&b) && !sent.contains_key(&b));
    }

    #[test]
    fn compaction_marker_goes_before_the_kept_messages() {
        let mut cancelled = ChatMessage::new(Role::User, "stopped");
        cancelled.cancelled = true;
        let mut marker = ChatMessage::new(Role::System, "old summary");
        marker.compaction = Some(crate::types::CompactionInfo {
            messages: 2,
            tokens_before: 0,
            manual: false,
        });
        let messages = vec![
            ChatMessage::new(Role::User, "a"),
            marker,
            ChatMessage::new(Role::Assistant, "b"),
            cancelled,
            ChatMessage::new(Role::User, "c"),
            ChatMessage::new(Role::Assistant, "d"),
        ];
        // The cancelled prompt and the old marker don't count.
        assert_eq!(compaction_marker_at(&messages, 2), 4);
        assert_eq!(compaction_marker_at(&messages, 3), 2);
        assert_eq!(compaction_marker_at(&messages, 0), 6);
        assert_eq!(compaction_marker_at(&messages, 4), 0);
    }

    #[test]
    fn regenerate_keeps_the_chat_when_there_is_no_user_message() {
        let mut messages = vec![ChatMessage::new(Role::Assistant, "hi")];
        let off = BranchSettings {
            enabled: false,
            ..BranchSettings::default()
        };
        assert!(branches::prepare_regenerate(&mut messages, &off).is_err());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn regenerate_cuts_back_to_the_last_user_message() {
        let mut messages = vec![
            ChatMessage::new(Role::User, "first"),
            ChatMessage::new(Role::Assistant, "answer"),
            ChatMessage::new(Role::User, "second"),
            ChatMessage::new(Role::Assistant, "old reply"),
            ChatMessage::new(Role::Tool, "result"),
        ];
        messages[2].cancelled = true;
        let off = BranchSettings {
            enabled: false,
            ..BranchSettings::default()
        };
        branches::prepare_regenerate(&mut messages, &off).unwrap();
        let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["first", "answer", "second"]);
        assert!(!messages[2].cancelled);
    }

    #[test]
    fn remote_model_names_are_trimmed_and_limited_to_200_characters() {
        assert_eq!(remote_model_name("  gpt-x "), Ok("gpt-x"));
        assert_eq!(remote_model_name("  "), Err("Choose a model".to_owned()));
        let max = "m".repeat(200);
        assert_eq!(remote_model_name(&max), Ok(max.as_str()));
        let long = "m".repeat(201);
        assert_eq!(
            remote_model_name(&long),
            Err("Model name is too long".to_owned())
        );
        // Counted in characters, not bytes.
        let accents = "é".repeat(200);
        assert_eq!(remote_model_name(&accents), Ok(accents.as_str()));
    }

    #[test]
    fn tool_calls_and_approvals_carry_the_desktop_label() {
        let mut msg = ChatMessage::new(Role::Assistant, "");
        msg.tool_calls.push(ToolCall {
            id: "call_1".into(),
            name: crate::tools::FETCH.into(),
            arguments: "{}".into(),
        });
        let view = message_view(&msg);
        assert_eq!(view.tool_calls[0].label, "Web fetch");
        assert_eq!(view.tool_calls[0].name, crate::tools::FETCH);
        let run = run_awaiting("call_2");
        let approval = approval_view(&run.approvals[0]);
        assert_eq!(approval.name, "fetch");
        assert_eq!(approval.label, "fetch");
    }

    #[test]
    fn message_view_never_carries_attachment_text_or_data() {
        let mut msg = ChatMessage::new(Role::User, "see the file");
        msg.attachments.push(Attachment {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            text: Some("TOP SECRET TEXT".into()),
            data: Some("U0VDUkVUIERBVEE=".into()),
        });
        let view = message_view(&msg);
        assert_eq!(view.attachments[0].name, "notes.txt");
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("notes.txt"));
        assert!(!json.contains("TOP SECRET TEXT"));
        assert!(!json.contains("U0VDUkVUIERBVEE="));
    }
}

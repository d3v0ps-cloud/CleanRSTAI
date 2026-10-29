//! Persistent application settings. Everything here is editable from the UI.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::commands::{SlashCommand, default_commands};
use crate::secrets::SecretStore;

pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("net", "bukn", "CleanRSTAI")
}

pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cleanrstai"))
}

pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cleanrstai"))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// What kind of server a provider is, so the app can use its extra stats.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any OpenAI-compatible API: standard `usage` plus client-side timing.
    #[default]
    Generic,
    /// llama.cpp `llama-server`: native prefill/decode timings and context size.
    LlamaCpp,
    /// LiteLLM proxy: served model and cost.
    LiteLlm,
    /// Ollama: loaded models, their size in GPU memory and context size.
    Ollama,
    /// LM Studio: load state, quantization and loaded context size.
    LmStudio,
    /// vLLM: context size and load (running and waiting requests, KV cache, prefix cache).
    Vllm,
    /// OpenRouter: the provider that served each request, cost, context and key credit.
    OpenRouter,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 7] = [
        Self::Generic,
        Self::LlamaCpp,
        Self::LiteLlm,
        Self::Ollama,
        Self::LmStudio,
        Self::Vllm,
        Self::OpenRouter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Generic => "Generic OpenAI-compatible",
            Self::LlamaCpp => "llama.cpp server",
            Self::LiteLlm => "LiteLLM proxy",
            Self::Ollama => "Ollama",
            Self::LmStudio => "LM Studio",
            Self::Vllm => "vLLM",
            Self::OpenRouter => "OpenRouter",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Generic => "Token usage from the API; speeds measured by the app.",
            Self::LlamaCpp => {
                "Prefill/decode speeds, context size and slot usage reported by llama.cpp."
            }
            Self::LiteLlm => "Shows the model that served each request and its cost.",
            Self::Ollama => {
                "Whether the model is loaded, its size in GPU memory, quantization and context size."
            }
            Self::LmStudio => {
                "Whether the model is loaded, its quantization and loaded context size."
            }
            Self::Vllm => {
                "Context size, and the server's load: running and waiting requests, KV cache use."
            }
            Self::OpenRouter => {
                "The provider that served each request, its cost, the model's context and your credit."
            }
        }
    }
}

/// The API a provider speaks.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderApi {
    /// OpenAI-compatible `/chat/completions`, which almost every server offers.
    #[default]
    ChatCompletions,
    /// OpenAI's `/responses`.
    Responses,
    /// Anthropic's `/messages`, with prompt caching and extended thinking.
    Anthropic,
}

impl ProviderApi {
    pub const ALL: [ProviderApi; 3] = [Self::ChatCompletions, Self::Responses, Self::Anthropic];

    pub fn label(self) -> &'static str {
        match self {
            Self::ChatCompletions => "Chat Completions (OpenAI-compatible)",
            Self::Responses => "OpenAI Responses",
            Self::Anthropic => "Anthropic Messages",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ChatCompletions => {
                "POST /chat/completions. Works with OpenAI and almost every other server."
            }
            Self::Responses => {
                "POST /responses, OpenAI's newer API: reasoning summaries and reasoning kept \
                 between tool calls. Stop sequences aren't supported."
            }
            Self::Anthropic => {
                "POST /messages with an x-api-key header: prompt caching and extended thinking. \
                 Base URL https://api.anthropic.com/v1."
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    /// Base URL of the OpenAI-compatible API, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Models discovered via `GET /models`, cached for the model picker.
    pub models: Vec<String>,
    /// Reasoning and sampling options for every model of this provider. Flattened, so the
    /// `temperature` and `max_tokens` of older settings files still load.
    #[serde(flatten)]
    pub options: crate::model_options::ModelOptions,
    /// Options for single models (by name), over the provider's.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub model_options: std::collections::BTreeMap<String, crate::model_options::ModelOptions>,
    /// Which API the provider speaks.
    pub api: ProviderApi,
    /// Anthropic Messages API: mark the system prompt, tools and latest message for caching.
    pub prompt_caching: bool,
    pub stream: bool,
    pub extra_headers: Vec<KeyValue>,
    pub timeout_secs: u64,
    pub kind: ProviderKind,
    /// Ask for token usage in streamed responses (`stream_options.include_usage`).
    pub request_usage: bool,
    /// The model's context size in tokens, for when the server doesn't report it. Used for
    /// compaction and the context-use figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            models: Vec::new(),
            options: Default::default(),
            model_options: Default::default(),
            api: ProviderApi::default(),
            prompt_caching: true,
            stream: true,
            extra_headers: Vec::new(),
            timeout_secs: 300,
            kind: ProviderKind::Generic,
            request_usage: true,
            context_tokens: None,
        }
    }
}

impl ProviderConfig {
    /// The options for the current model: the provider's, with the model's own over them.
    pub fn resolved_options(&self) -> crate::model_options::ModelOptions {
        match self.model_options.get(self.model.trim()) {
            Some(m) => m.over(&self.options),
            None => self.options.clone(),
        }
    }

    /// This provider with a chat's options over its own, for that chat's requests.
    pub fn with_chat_options(&self, chat: &crate::model_options::ModelOptions) -> Self {
        let mut p = self.clone();
        p.options = chat.over(&self.resolved_options());
        p.model_options.clear();
        p
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<KeyValue>,
        #[serde(default)]
        cwd: String,
    },
    /// MCP "Streamable HTTP" transport.
    Http {
        url: String,
        #[serde(default)]
        headers: Vec<KeyValue>,
        /// OAuth client ID for servers that don't support automatic registration.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        oauth_client_id: String,
    },
}

impl Default for McpTransportConfig {
    fn default() -> Self {
        Self::Stdio {
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransportConfig,
    /// Tools the user switched off; they are not offered to the model.
    pub disabled_tools: Vec<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "new-server".into(),
            enabled: true,
            transport: McpTransportConfig::default(),
            disabled_tools: Vec::new(),
        }
    }
}

impl McpServerConfig {
    /// Whether a change from `other` requires reconnecting.
    pub fn needs_reconnect(&self, other: &McpServerConfig) -> bool {
        self.enabled != other.enabled || self.transport != other.transport
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// How the model's reasoning ("Thinking") is shown in the chat.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningDisplay {
    /// Collapsed; click to expand.
    #[default]
    Collapsed,
    /// Always expanded.
    Expanded,
    /// Expanded while the model is reasoning, collapsed once the answer starts.
    WhileStreaming,
}

impl ReasoningDisplay {
    pub const ALL: [ReasoningDisplay; 3] = [Self::Collapsed, Self::Expanded, Self::WhileStreaming];

    pub fn label(self) -> &'static str {
        match self {
            Self::Collapsed => "Collapsed",
            Self::Expanded => "Expanded",
            Self::WhileStreaming => "Expanded while thinking",
        }
    }
}

/// Which built-in tools are offered to the model by default.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BuiltinToolSettings {
    /// Current date, time and time zone.
    pub time: bool,
    /// Exact arithmetic.
    pub calculator: bool,
    /// Break a request into a task checklist and track it.
    pub tasks: bool,
    /// Fetch web URLs (asks for approval, like MCP tools).
    pub fetch: bool,
    /// Let fetch reach loopback, private and link-local addresses (local services, cloud
    /// metadata endpoints). Off by default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fetch_private_networks: bool,
    /// Read, search, edit and run commands in a chat's workspace folder, when it has one.
    pub workspace: bool,
    /// Search the web through the service in `search`.
    pub web_search: bool,
    pub search: WebSearchSettings,
}

impl Default for BuiltinToolSettings {
    fn default() -> Self {
        Self {
            time: true,
            calculator: true,
            tasks: true,
            fetch: true,
            fetch_private_networks: false,
            workspace: true,
            web_search: true,
            search: WebSearchSettings::default(),
        }
    }
}

/// The service the web search tool asks.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchBackend {
    /// DuckDuckGo's HTML results page; no account or key.
    #[default]
    DuckDuckGo,
    /// A SearXNG instance with the JSON format turned on.
    Searxng,
    /// The Brave Search API (needs a key).
    Brave,
    /// The Tavily search API (needs a key).
    Tavily,
    /// The Kagi Search API (needs a key).
    Kagi,
}

impl SearchBackend {
    pub const ALL: [SearchBackend; 5] = [
        Self::DuckDuckGo,
        Self::Searxng,
        Self::Brave,
        Self::Tavily,
        Self::Kagi,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Searxng => "SearXNG",
            Self::Brave => "Brave Search API",
            Self::Tavily => "Tavily",
            Self::Kagi => "Kagi Search API",
        }
    }

    pub fn needs_key(self) -> bool {
        matches!(self, Self::Brave | Self::Tavily | Self::Kagi)
    }
}

/// Most results one web search returns.
pub const MAX_SEARCH_RESULTS: u32 = 20;

/// The web search tool's service and options.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WebSearchSettings {
    pub backend: SearchBackend,
    /// Base URL of the SearXNG instance, e.g. `https://searx.example.org`.
    pub searxng_url: String,
    /// API key for Brave, Tavily or Kagi. Kept in the keychain.
    pub api_key: String,
    /// Results per search, 1–20.
    pub max_results: u32,
    /// Ask the service to filter adult results.
    pub safe_search: bool,
    /// Ask before each search (the query leaves this computer).
    pub ask: bool,
    /// Subagents may search too (when they may use the built-in tools).
    pub subagents: bool,
}

impl Default for WebSearchSettings {
    fn default() -> Self {
        Self {
            backend: SearchBackend::DuckDuckGo,
            searxng_url: String::new(),
            api_key: String::new(),
            max_results: 5,
            safe_search: true,
            ask: true,
            subagents: true,
        }
    }
}

/// Summarising the older part of a chat when it nears the model's context size.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CompactionSettings {
    /// Compact automatically. "Compact now" works either way.
    pub enabled: bool,
    /// Compact once the request would fill this share of the context, in percent (50–95).
    pub threshold_percent: u32,
    /// Messages at the end of the chat kept word for word (2–50).
    pub keep_recent: u32,
    /// Instructions for the model that writes the summary.
    pub prompt: String,
    /// Provider profile that writes the summary; empty means the chat's provider.
    pub provider_id: String,
    /// Model override for the summary; empty means the provider's selected model.
    pub model: String,
    /// Context size to assume when the server doesn't report one; 0 compacts automatically
    /// only when it does.
    pub context_tokens: u32,
    /// Shorten long results of older tool calls in what is sent (the chat keeps them).
    pub trim_tool_results: bool,
    /// Characters an older tool result keeps when trimmed (200–100,000).
    pub trim_chars: u32,
}

pub const DEFAULT_COMPACTION_PROMPT: &str = "Summarise the conversation below so another \
assistant can continue it without seeing it. Keep the user's goals and instructions, decisions \
made, facts and names that matter, files read or changed and what changed in them, commands run \
and their outcome, open questions and the next steps. Leave out pleasantries. Write it as a \
compact list of notes, not a story.";

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: 80,
            keep_recent: 6,
            prompt: DEFAULT_COMPACTION_PROMPT.into(),
            provider_id: String::new(),
            model: String::new(),
            context_tokens: 0,
            trim_tool_results: false,
            trim_chars: 2000,
        }
    }
}

/// Which changes "Review changes" sends to the model.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewScope {
    /// Staged, unstaged and new files, against the last commit.
    #[default]
    Uncommitted,
    /// Everything on the current branch since it forked from its base.
    Branch,
}

pub const DEFAULT_REVIEW_PROMPT: &str = "Review the changes in the attached diff. Look for \
bugs, security problems, unclear code and missing tests. List each finding by severity (high, \
medium or low) with its file and line, and say why it matters and how to fix it. Leave out \
praise and a restatement of what the change does. If you find nothing worth reporting, say so \
plainly.";

/// How much "Review changes" reports beyond the prompt's own list.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStrictness {
    /// Only problems that would cause bugs, data loss or security issues.
    Lenient,
    /// The review prompt as written.
    #[default]
    Normal,
    /// Also style, naming, readability, missing tests and documentation, and risky assumptions.
    Strict,
}

impl ReviewStrictness {
    /// The words added to the review prompt for this level; empty for `Normal`.
    pub fn instruction(self) -> &'static str {
        match self {
            Self::Lenient => {
                "Only report problems that would cause bugs, data loss or security issues; skip \
                 style and minor suggestions."
            }
            Self::Normal => "",
            Self::Strict => {
                "Be thorough: also report style, naming, readability, missing tests and \
                 documentation, and any risky assumption."
            }
        }
    }
}

/// "Review changes": the working folder's git diff, sent to a model with a review prompt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReviewSettings {
    /// Show "Review changes" in the tools menu and the command palette.
    pub enabled: bool,
    /// Instructions for the model that reviews the diff.
    pub prompt: String,
    pub scope: ReviewScope,
    /// Branch the "branch" scope diffs against; empty picks `origin/HEAD`, `main` or `master`.
    pub base_branch: String,
    /// Provider profile that writes the review; empty means the chat's provider.
    pub provider_id: String,
    /// Model override for the review; empty means the provider's selected model.
    pub model: String,
    /// Largest diff sent, in KB (16–4096); a longer one is cut.
    pub max_diff_kb: u32,
    /// How far beyond the prompt the review reports.
    pub strictness: ReviewStrictness,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            prompt: DEFAULT_REVIEW_PROMPT.into(),
            scope: ReviewScope::default(),
            base_branch: String::new(),
            provider_id: String::new(),
            model: String::new(),
            max_diff_kb: 200,
            strictness: ReviewStrictness::default(),
        }
    }
}

/// Skills: folders of instructions the model loads with `load_skill` (see `skills`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SkillSettings {
    /// List the skills in the system prompt and offer the `load_skill` tool.
    pub enabled: bool,
    /// Also load `.cleanrstai/skills/*/SKILL.md` from the chat's working folder; a workspace
    /// skill replaces a user skill of the same name.
    pub workspace_skills: bool,
    /// Names of skills that are switched off, from either folder.
    pub disabled: Vec<String>,
    /// Most text one load returns, in KB (1–1024).
    pub max_kb: u32,
}

impl Default for SkillSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace_skills: true,
            disabled: Vec::new(),
            max_kb: 64,
        }
    }
}

/// Instruction files (`AGENTS.md`, `CLAUDE.md`, …) read from a chat's workspace folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InstructionSettings {
    pub enabled: bool,
    /// File names to look for, in order.
    pub file_names: Vec<String>,
    /// Read every listed file found in a folder, not only the first.
    pub all_files: bool,
    /// Also read them from the folders above the workspace (outermost first).
    pub parent_folders: bool,
    /// Most text read in all, in KB (1–1024).
    pub max_kb: u32,
}

impl Default for InstructionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            file_names: vec!["AGENTS.md".into(), "CLAUDE.md".into()],
            all_files: false,
            parent_folders: false,
            max_kb: 64,
        }
    }
}

/// Saved prompts typed as `/name` in the message box (see `commands`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SlashCommandSettings {
    /// Expand `/name` commands when a message is sent, and suggest them while typing.
    pub enabled: bool,
    /// Commands from Settings, by name. Names are unique after normalizing.
    pub commands: Vec<SlashCommand>,
    /// Also load `.cleanrstai/commands/*.md` from the chat's working folder.
    pub workspace_commands: bool,
}

impl Default for SlashCommandSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            commands: default_commands(),
            workspace_commands: true,
        }
    }
}

/// What the app keeps around the workspace's `write_file` and `edit_file` calls.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FileEditSettings {
    /// Show the change as a diff on the approval prompt and in the tool card.
    pub diff_preview: bool,
    /// Keep each file's previous content, so a turn's edits can be undone.
    pub checkpoints: bool,
    /// Files larger than this (in KB) get no checkpoint (16–65,536).
    pub max_file_kb: u32,
    /// Checkpoints are kept for this many of the latest turns; 0 keeps them all.
    pub keep_turns: u32,
}

impl Default for FileEditSettings {
    fn default() -> Self {
        Self {
            diff_preview: true,
            checkpoints: true,
            max_file_kb: 1024,
            keep_turns: 20,
        }
    }
}

/// The live preview of web page artifacts in the artifact panel.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HtmlPreviewSettings {
    /// Show web pages live in the panel (where the platform has a web view).
    pub enabled: bool,
    /// Let the page run its scripts.
    pub scripts: bool,
    /// Let the page load anything from the network (scripts, styles, images, requests).
    pub network: bool,
}

impl Default for HtmlPreviewSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            scripts: true,
            network: false,
        }
    }
}

/// Replies running in the background: notifications, how many run at once, and a time limit
/// (Settings → Background).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackgroundSettings {
    /// Send a desktop notification when a reply finishes, fails or needs approval (per chat
    /// too: `feature:notify`).
    pub notify: bool,
    /// Notify only while the CleanRSTAI window is in the background.
    pub notify_only_unfocused: bool,
    /// Replies that run at once; more wait in a queue (1–32).
    pub max_parallel: u32,
    /// Stop a reply after this many minutes, counted from when it starts; 0 for no limit
    /// (0–1440).
    pub time_limit_mins: u32,
    /// Closing the window hides it to the menu bar (macOS) or system tray (Windows) and keeps
    /// replies and MCP connections running. Not available on Linux yet.
    pub keep_running: bool,
}

/// Most replies that run at once.
pub const MAX_PARALLEL_RUNS: u32 = 32;
/// Longest time limit for a reply, in minutes (a day).
pub const MAX_TIME_LIMIT_MINS: u32 = 1440;

impl Default for BackgroundSettings {
    fn default() -> Self {
        Self {
            notify: true,
            notify_only_unfocused: true,
            max_parallel: 4,
            time_limit_mins: 0,
            keep_running: false,
        }
    }
}

/// The `delegate` tool: hands tasks to subagents that work on their own and report back.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SubagentSettings {
    pub enabled: bool,
    /// System prompt for every subagent.
    pub system_prompt: String,
    /// Provider profile subagents use; empty means the chat's current provider.
    pub provider_id: String,
    /// Model override; empty means the provider's selected model.
    pub model: String,
    /// Let the model pick another provider profile or model for individual tasks.
    pub allow_provider_choice: bool,
    /// Subagents may use the built-in tools (time, calculator, web fetch). Their fetches run
    /// without asking.
    pub builtin_tools: bool,
    /// Subagents may use the chat's MCP tools. Their calls run without asking.
    pub mcp_tools: bool,
    /// Subagents may use the chat's workspace tools, with shell commands in their own
    /// container of the Docker sandbox (only while the sandbox is on). Their calls run
    /// without asking.
    pub workspace: bool,
    /// MCP servers (by id) subagents may not use, even when on in the chat.
    pub excluded_servers: Vec<String>,
    /// How many subagents run at the same time.
    pub max_parallel: u32,
    /// Most tasks one `delegate` call may hand out.
    pub max_tasks: u32,
    /// Tool-call rounds each subagent may make before it must answer.
    pub max_rounds: u32,
    /// Time limit for each subagent.
    pub timeout_secs: u64,
}

impl Default for SubagentSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            system_prompt: "You are a subagent working on one task delegated by another \
                            assistant. Complete the task on your own and reply with a \
                            concise, complete result. You cannot ask follow-up questions."
                .into(),
            provider_id: String::new(),
            model: String::new(),
            allow_provider_choice: true,
            builtin_tools: true,
            mcp_tools: false,
            workspace: false,
            excluded_servers: Vec::new(),
            max_parallel: 3,
            max_tasks: 8,
            max_rounds: 10,
            timeout_secs: 300,
        }
    }
}

/// Which network interfaces the browser remote-access server listens on.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteBind {
    /// 127.0.0.1 only.
    #[default]
    Local,
    /// 0.0.0.0, reachable from the local network.
    Network,
}

/// How the browser remote-access server serves HTTPS.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTls {
    /// A self-signed certificate generated once and cached in the config folder.
    #[default]
    SelfSigned,
    /// PEM certificate and key files chosen by the user.
    Files,
    /// Plain HTTP. Passwords and chats travel unencrypted.
    Off,
}

/// Port used when the remote-access port is unset (0).
pub const REMOTE_DEFAULT_PORT: u16 = 8484;

/// Browser remote access, served by the app itself.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RemoteSettings {
    /// Start the browser remote-access server with the app.
    pub enabled: bool,
    /// Listen on this computer only, or on the local network too.
    pub bind: RemoteBind,
    /// TCP port the server listens on.
    pub port: u16,
    /// Password for logging in from a browser. Kept in the keychain; the server won't start
    /// while it is empty.
    pub password: String,
    /// HTTPS mode.
    pub tls: RemoteTls,
    /// PEM certificate file, used when `tls` is `Files`.
    pub cert_path: String,
    /// PEM private key file, used when `tls` is `Files`.
    pub key_path: String,
    /// A Cloudflare Tunnel that makes the server reachable from outside the network.
    pub tunnel: TunnelSettings,
}

/// Which kind of Cloudflare Tunnel `cloudflared` runs.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelMode {
    /// A temporary `trycloudflare.com` address, no Cloudflare account needed.
    #[default]
    Quick,
    /// A tunnel in the user's Cloudflare account, on their own domain.
    Named,
}

/// A Cloudflare Tunnel run with `cloudflared`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TunnelSettings {
    pub enabled: bool,
    pub mode: TunnelMode,
    /// Quick tunnels: email addresses or `*@domain` patterns, comma-separated, that Cloudflare
    /// lets through with a one-time PIN. Empty lets anyone with the address reach the login.
    pub allowed_emails: String,
    /// Named tunnels: the tunnel's token from the Cloudflare dashboard. Kept in the keychain.
    pub token: String,
    /// Named tunnels: the public hostname of the tunnel's route, e.g. `chat.example.com`.
    /// Optional: cloudflared also reports the tunnel's hostnames once it is connected.
    pub hostname: String,
    /// Path to `cloudflared`; empty finds it on `PATH`.
    pub cloudflared: String,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: RemoteBind::default(),
            port: REMOTE_DEFAULT_PORT,
            password: String::new(),
            tls: RemoteTls::default(),
            cert_path: String::new(),
            key_path: String::new(),
            tunnel: TunnelSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeneralSettings {
    pub system_prompt: String,
    pub theme: Theme,
    pub zoom: f32,
    /// Enter sends, Shift+Enter inserts a newline. When false, Ctrl/Cmd+Enter sends.
    pub send_with_enter: bool,
    /// Run MCP tool calls without asking first.
    pub auto_approve_tools: bool,
    /// The permission mode new chats start in (each chat can change its own).
    #[serde(default)]
    pub default_approval_mode: crate::approval::ApprovalMode,
    pub max_tool_rounds: u32,
    pub tool_timeout_secs: u64,
    /// Show prefill/decode speed and token counts under each response.
    pub show_stats: bool,
    /// Keep API keys and other secrets in the OS keychain instead of settings.json.
    pub use_keychain: bool,
    /// Look for a newer release on GitHub at startup.
    pub check_for_updates: bool,
    /// Where to look for releases (see `update::release_api_url`). Only written to the file
    /// once changed, so everyone else follows the built-in default.
    #[serde(skip_serializing_if = "is_default_update_url")]
    pub update_url: String,
    pub reasoning_display: ReasoningDisplay,
    /// Load images that replies link to on the web. Off by default: a reply could otherwise
    /// send data to any server just by being shown.
    pub load_remote_images: bool,
    /// Open the artifact panel when a reply makes a web page, drawing, document or long code.
    pub open_artifacts: bool,
    /// Live preview of web page artifacts.
    pub html_preview: HtmlPreviewSettings,
    pub builtin_tools: BuiltinToolSettings,
    /// Summarising older turns when the context fills up.
    pub compaction: CompactionSettings,
    /// `AGENTS.md` and similar files from the workspace.
    pub instructions: InstructionSettings,
    /// "Review changes": the workspace's git diff sent to a model.
    pub review: ReviewSettings,
    /// Diffs and undo for workspace file edits.
    pub file_edits: FileEditSettings,
    /// Reading scanned PDFs (no text layer) with a vision model or tesseract.
    #[serde(default)]
    pub ocr: crate::ocr::OcrSettings,
    /// Slash commands: saved prompts typed as `/name` (see `commands`).
    pub slash_commands: SlashCommandSettings,
    /// Skills the model loads on demand (see `skills`).
    pub skills: SkillSettings,
    /// Browser remote access (see `remote`).
    pub remote: RemoteSettings,
    /// Replies running in the background: notifications, parallel limit and time limit.
    #[serde(default)]
    pub background: BackgroundSettings,
    /// `cleanrstai exec`: one request from the command line (Settings → Background).
    #[serde(default)]
    pub exec: crate::exec::ExecSettings,
    /// Commands run around tool calls and when a reply is done (see `hooks`).
    #[serde(default)]
    pub hooks: crate::hooks::HookSettings,
    /// Prompts that run on a schedule into a chat (Settings → Background).
    #[serde(default)]
    pub schedules: crate::schedule::ScheduleSettings,
}

fn is_default_update_url(url: &String) -> bool {
    url == crate::update::LATEST_RELEASE_URL
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            system_prompt: "You are a helpful assistant.".into(),
            theme: Theme::System,
            zoom: 1.0,
            send_with_enter: true,
            auto_approve_tools: false,
            default_approval_mode: Default::default(),
            max_tool_rounds: 20,
            tool_timeout_secs: 120,
            show_stats: true,
            use_keychain: true,
            check_for_updates: true,
            update_url: crate::update::LATEST_RELEASE_URL.into(),
            reasoning_display: ReasoningDisplay::default(),
            load_remote_images: false,
            open_artifacts: true,
            html_preview: HtmlPreviewSettings::default(),
            builtin_tools: BuiltinToolSettings::default(),
            compaction: CompactionSettings::default(),
            instructions: InstructionSettings::default(),
            review: ReviewSettings::default(),
            file_edits: FileEditSettings::default(),
            ocr: Default::default(),
            slash_commands: SlashCommandSettings::default(),
            skills: SkillSettings::default(),
            remote: RemoteSettings::default(),
            background: BackgroundSettings::default(),
            exec: Default::default(),
            hooks: Default::default(),
            schedules: Default::default(),
        }
    }
}

/// Where the sandbox image comes from.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxImageSource {
    /// An image named in `SandboxSettings::image`, pulled or found on this computer.
    #[default]
    Image,
    /// A Dockerfile in `SandboxSettings::dockerfile`, built by the app.
    Dockerfile,
}

/// When the app pulls the sandbox image.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPull {
    /// Pull only when the image isn't on this computer.
    #[default]
    Missing,
    /// Pull before the first container of each app session.
    Always,
    /// Use only an image already on this computer.
    Never,
}

/// Network access for sandbox containers.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxNetwork {
    /// No network access.
    #[default]
    None,
    /// Normal Docker bridge network.
    Bridge,
    /// Only the outbound connections that `SandboxSettings::rules` allow.
    Custom,
}

/// The protocol a sandbox network rule allows.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetProtocol {
    #[default]
    Tcp,
    Udp,
    /// ICMP (ping, traceroute replies); takes no ports.
    Icmp,
    /// TCP and UDP on the rule's ports, or every protocol (ICMP too) when it has no ports.
    Any,
}

/// One outbound connection a custom sandbox network allows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SandboxNetRule {
    /// A domain (its subdomains included; `*.` is optional), an IPv4 address or CIDR range,
    /// or `*` for any address.
    pub host: String,
    pub protocol: NetProtocol,
    /// Ports and ranges separated by commas, such as `80, 443, 8000-8100`. Empty allows all.
    pub ports: String,
}

/// Most rules a custom sandbox network takes.
pub const MAX_SANDBOX_NET_RULES: usize = 256;

/// Image used when the sandbox image is unset.
pub const DEFAULT_SANDBOX_IMAGE: &str = "python:3.13-slim";

/// Whether `image` can be passed to `docker` as an image reference: not empty, no leading `-`
/// (it would read as an option), and no whitespace or control characters.
pub fn valid_sandbox_image(image: &str) -> bool {
    !image.is_empty()
        && !image.starts_with('-')
        && !image.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Dockerfile offered as the example for a custom sandbox image.
pub const DEFAULT_SANDBOX_DOCKERFILE: &str = "FROM python:3.13-slim\n\
RUN apt-get update \\\n && apt-get install -y --no-install-recommends git curl ca-certificates \\\n && rm -rf /var/lib/apt/lists/*\n";

/// The larger example: Ubuntu with Python, Node.js, build tools and network tools.
pub const FULL_SANDBOX_DOCKERFILE: &str = include_str!("../assets/sandbox/full.Dockerfile");

/// Docker sandbox for the workspace shell tool.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SandboxSettings {
    /// Run shell commands in a Docker container instead of on this computer.
    pub enabled: bool,
    /// Whether `image` or `dockerfile` provides the container image.
    pub source: SandboxImageSource,
    /// Image name and tag used when `source` is `Image`.
    pub image: String,
    /// Dockerfile built when `source` is `Dockerfile`.
    pub dockerfile: String,
    /// When to pull the image (`Image` source only).
    pub pull: SandboxPull,
    /// Network access for the container.
    pub network: SandboxNetwork,
    /// The outbound connections allowed when `network` is `Custom`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<SandboxNetRule>,
    /// CPU limit per container, in CPUs.
    pub cpus: f32,
    /// Memory limit per container, in MB.
    pub memory_mb: u32,
    /// Maximum number of processes per container.
    pub pids: u32,
    /// Mount the workspace folder read-only.
    pub read_only: bool,
    /// Path to the `docker` program. Empty finds `docker` on PATH.
    pub docker: String,
}

impl SandboxSettings {
    /// A copy with the values clamped or filled in the way [`Settings::normalize`] does: the
    /// settings the sandbox actually uses, and the ones Settings → Docker sends to Docker.
    pub fn normalized(&self) -> Self {
        let mut sb = self.clone();
        sb.cpus = if sb.cpus.is_finite() {
            sb.cpus.clamp(0.25, 64.0)
        } else {
            2.0
        };
        sb.memory_mb = sb.memory_mb.clamp(256, 262_144);
        sb.pids = sb.pids.clamp(32, 65_536);
        let image = sb.image.trim().to_owned();
        sb.image = if valid_sandbox_image(&image) {
            image
        } else {
            DEFAULT_SANDBOX_IMAGE.into()
        };
        sb.rules.truncate(MAX_SANDBOX_NET_RULES);
        for rule in &mut sb.rules {
            rule.host = rule.host.trim().to_ascii_lowercase();
            rule.ports = rule.ports.trim().to_owned();
        }
        sb
    }
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            source: SandboxImageSource::Image,
            image: DEFAULT_SANDBOX_IMAGE.into(),
            dockerfile: DEFAULT_SANDBOX_DOCKERFILE.into(),
            pull: SandboxPull::Missing,
            network: SandboxNetwork::None,
            rules: Vec::new(),
            cpus: 2.0,
            memory_mb: 2048,
            pids: 512,
            read_only: false,
            docker: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub providers: Vec<ProviderConfig>,
    pub active_provider: usize,
    pub mcp_servers: Vec<McpServerConfig>,
    pub general: GeneralSettings,
    pub subagents: SubagentSettings,
    /// Docker sandbox for the workspace shell tool.
    pub sandbox: SandboxSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            providers: vec![ProviderConfig::default()],
            active_provider: 0,
            mcp_servers: Vec::new(),
            general: GeneralSettings::default(),
            subagents: SubagentSettings::default(),
            sandbox: SandboxSettings::default(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    /// Load settings, filling secrets back in from the OS keychain.
    pub fn load() -> LoadedSettings {
        Self::load_from(&Self::path(), &*crate::secrets::keychain())
    }

    pub fn load_from(path: &std::path::Path, store: &dyn SecretStore) -> LoadedSettings {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => {
                return LoadedSettings {
                    settings: Self::default(),
                    notice: None,
                    needs_migration: false,
                };
            }
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&text)
            .and_then(|v| Ok((serde_json::from_value::<Settings>(v.clone())?, v)));
        let (mut settings, raw) = match parsed {
            Ok(x) => x,
            Err(e) => {
                log::error!("Failed to parse {}: {e}", path.display());
                // Keep the broken file around rather than silently overwriting it later.
                let _ = std::fs::copy(path, path.with_extension("json.bak"));
                return LoadedSettings {
                    settings: Self::default(),
                    notice: Some(format!(
                        "settings.json could not be read ({e}); started with defaults. The old file was kept as settings.json.bak."
                    )),
                    needs_migration: false,
                };
            }
        };
        settings.normalize();
        let in_keychain = raw
            .get(KEY_IN_KEYCHAIN)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut notice = None;
        if in_keychain {
            let failed = crate::secrets::internalize(&mut settings, store, &stored_accounts(&raw));
            if !failed.is_empty() {
                notice = Some(format!(
                    "{} secret(s) couldn't be read from the system keychain; re-enter them in Settings.",
                    failed.len()
                ));
            }
        }
        let needs_migration = !in_keychain
            && settings.general.use_keychain
            && crate::secrets::has_any_secret(&settings);
        LoadedSettings {
            settings,
            notice,
            needs_migration,
        }
    }

    /// Save settings. Returns a warning if secrets had to stay in the file.
    pub fn save(&self) -> Result<Option<String>> {
        self.save_to(&Self::path(), &*crate::secrets::keychain())
    }

    pub fn save_to(
        &self,
        path: &std::path::Path,
        store: &dyn SecretStore,
    ) -> Result<Option<String>> {
        let previous = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| stored_accounts(&v))
            .unwrap_or_default();

        let mut on_disk = self.clone();
        let mut accounts = Vec::new();
        let mut warning = None;
        if self.general.use_keychain {
            match crate::secrets::externalize(&mut on_disk, store) {
                Ok(a) => accounts = a,
                Err(e) => {
                    warning = Some(format!(
                        "Couldn't use the system keychain ({e}); secrets are saved in settings.json instead. Turn off \"Store API keys and secrets in the system keychain\" in Settings → General to stop this message."
                    ));
                    on_disk = self.clone();
                }
            }
        }
        let mut doc = serde_json::to_value(&on_disk)?;
        doc[KEY_IN_KEYCHAIN] = serde_json::json!(self.general.use_keychain && warning.is_none());
        doc[KEY_ACCOUNTS] = serde_json::json!(accounts);
        write_private(path, serde_json::to_string_pretty(&doc)?.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        crate::secrets::remove_stale(store, &previous, &accounts);
        Ok(warning)
    }

    pub fn normalize(&mut self) {
        if self.providers.is_empty() {
            self.providers.push(ProviderConfig::default());
        }
        if self.active_provider >= self.providers.len() {
            self.active_provider = 0;
        }
        self.general.zoom = self.general.zoom.clamp(0.5, 3.0);
        if self.general.remote.port == 0 {
            self.general.remote.port = REMOTE_DEFAULT_PORT;
        }
        let url = self.general.update_url.trim();
        self.general.update_url = if url.is_empty() {
            crate::update::LATEST_RELEASE_URL.into()
        } else {
            url.to_owned()
        };
        let c = &mut self.general.compaction;
        c.threshold_percent = c.threshold_percent.clamp(50, 95);
        c.keep_recent = c.keep_recent.clamp(2, 50);
        c.trim_chars = c.trim_chars.clamp(200, 100_000);
        if c.prompt.trim().is_empty() {
            c.prompt = DEFAULT_COMPACTION_PROMPT.into();
        }
        if !self.providers.iter().any(|p| p.id == c.provider_id) {
            c.provider_id.clear();
        }
        self.general
            .ocr
            .normalize(|id| self.providers.iter().any(|p| p.id == id));
        let rv = &mut self.general.review;
        rv.max_diff_kb = rv.max_diff_kb.clamp(16, 4096);
        rv.base_branch = rv.base_branch.trim().to_owned();
        if rv.prompt.trim().is_empty() {
            rv.prompt = DEFAULT_REVIEW_PROMPT.into();
        }
        if !self.providers.iter().any(|p| p.id == rv.provider_id) {
            rv.provider_id.clear();
        }
        let ins = &mut self.general.instructions;
        ins.max_kb = ins.max_kb.clamp(1, 1024);
        ins.file_names = ins
            .file_names
            .iter()
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty() && !n.contains(['/', '\\']) && n != "." && n != "..")
            .collect();
        let fe = &mut self.general.file_edits;
        fe.max_file_kb = fe.max_file_kb.clamp(16, 65_536);
        // Names are trimmed; commands that can't be typed, and repeats of a name, are dropped.
        let mut names = std::collections::HashSet::new();
        let commands = std::mem::take(&mut self.general.slash_commands.commands);
        self.general.slash_commands.commands = commands
            .into_iter()
            .map(|mut c| {
                c.name = c.name.trim().to_owned();
                c
            })
            .filter(|c| crate::commands::valid_name(&c.name) && names.insert(c.name.clone()))
            .collect();
        let skills = &mut self.general.skills;
        skills.max_kb = skills.max_kb.clamp(1, 1024);
        // Names are trimmed; blanks and repeats are dropped.
        let mut disabled: Vec<String> = Vec::new();
        for name in skills.disabled.iter().map(|n| n.trim().to_owned()) {
            if !name.is_empty() && !disabled.contains(&name) {
                disabled.push(name);
            }
        }
        skills.disabled = disabled;
        let search = &mut self.general.builtin_tools.search;
        search.max_results = search.max_results.clamp(1, MAX_SEARCH_RESULTS);
        search.searxng_url = search.searxng_url.trim().trim_end_matches('/').to_owned();
        let sa = &mut self.subagents;
        sa.max_parallel = sa.max_parallel.clamp(1, 16);
        sa.max_tasks = sa.max_tasks.clamp(1, 32);
        sa.max_rounds = sa.max_rounds.clamp(1, 100);
        sa.timeout_secs = sa.timeout_secs.clamp(10, 3600);
        let bg = &mut self.general.background;
        bg.max_parallel = bg.max_parallel.clamp(1, MAX_PARALLEL_RUNS);
        bg.time_limit_mins = bg.time_limit_mins.min(MAX_TIME_LIMIT_MINS);
        self.general.exec.mcp_wait_secs = self
            .general
            .exec
            .mcp_wait_secs
            .min(crate::exec::MAX_MCP_WAIT_SECS);
        crate::hooks::normalize(&mut self.general.hooks);
        self.general.schedules.normalize();
        self.sandbox = self.sandbox.normalized();
        if !self.providers.iter().any(|p| p.id == sa.provider_id) {
            sa.provider_id.clear();
        }
        let servers = &self.mcp_servers;
        sa.excluded_servers
            .retain(|id| servers.iter().any(|s| &s.id == id));
    }

    /// The provider profile subagents use when a task doesn't name one.
    pub fn subagent_provider(&self) -> ProviderConfig {
        let mut p = self
            .providers
            .iter()
            .find(|p| p.id == self.subagents.provider_id)
            .unwrap_or_else(|| self.provider())
            .clone();
        if !self.subagents.model.trim().is_empty() {
            p.model = self.subagents.model.trim().to_owned();
        }
        p
    }

    /// The provider profile that writes compaction summaries for a chat on `chat`.
    pub fn compaction_provider(&self, chat: &ProviderConfig) -> ProviderConfig {
        let c = &self.general.compaction;
        let mut p = self
            .providers
            .iter()
            .find(|p| p.id == c.provider_id)
            .unwrap_or(chat)
            .clone();
        if !c.model.trim().is_empty() {
            p.model = c.model.trim().to_owned();
        }
        p
    }

    /// The provider profile that writes a code review for a chat on `chat`.
    pub fn review_provider(&self, chat: &ProviderConfig) -> ProviderConfig {
        let r = &self.general.review;
        let mut p = self
            .providers
            .iter()
            .find(|p| p.id == r.provider_id)
            .unwrap_or(chat)
            .clone();
        if !r.model.trim().is_empty() {
            p.model = r.model.trim().to_owned();
        }
        p
    }

    pub fn provider(&self) -> &ProviderConfig {
        &self.providers[self.active_provider.min(self.providers.len() - 1)]
    }

    pub fn provider_mut(&mut self) -> &mut ProviderConfig {
        let idx = self.active_provider.min(self.providers.len() - 1);
        &mut self.providers[idx]
    }
}

const KEY_IN_KEYCHAIN: &str = "secrets_in_keychain";
const KEY_ACCOUNTS: &str = "keychain_accounts";

fn stored_accounts(raw: &serde_json::Value) -> Vec<String> {
    raw.get(KEY_ACCOUNTS)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub struct LoadedSettings {
    pub settings: Settings,
    /// Something the user should know (keychain read failures, unreadable file).
    pub notice: Option<String>,
    /// Secrets are still in plain text but should move to the keychain; save once to migrate.
    pub needs_migration: bool,
}

/// Write a file atomically; on Unix restrict it to the current user since it may hold API keys.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        // Created owner-only, so the secret is never readable by others, not even briefly.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Parse a Claude Desktop / VS Code style `{"mcpServers": {...}}` JSON blob.
pub fn import_mcp_json(text: &str) -> Result<Vec<McpServerConfig>> {
    let value: serde_json::Value = serde_json::from_str(text).context("invalid JSON")?;
    let servers = value
        .get("mcpServers")
        .or_else(|| value.get("servers"))
        .unwrap_or(&value)
        .as_object()
        .context("expected an object of servers (e.g. {\"mcpServers\": {...}})")?;

    let kv = |v: Option<&serde_json::Value>| -> Vec<KeyValue> {
        v.and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| KeyValue {
                        key: k.clone(),
                        value: v
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut out = Vec::new();
    for (name, cfg) in servers {
        let transport = if let Some(command) = cfg.get("command").and_then(|c| c.as_str()) {
            McpTransportConfig::Stdio {
                command: command.to_owned(),
                args: cfg
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                env: kv(cfg.get("env")),
                cwd: cfg
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            }
        } else if let Some(url) = cfg.get("url").and_then(|u| u.as_str()) {
            McpTransportConfig::Http {
                url: url.to_owned(),
                headers: kv(cfg.get("headers")),
                oauth_client_id: String::new(),
            }
        } else {
            anyhow::bail!("server '{name}' has neither 'command' nor 'url'");
        };
        out.push(McpServerConfig {
            name: name.clone(),
            enabled: !cfg
                .get("disabled")
                .and_then(|d| d.as_bool())
                .unwrap_or(false),
            transport,
            ..Default::default()
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hook_settings_normalise_with_the_rest_of_the_settings() {
        let mut s = Settings::default();
        assert!(!s.general.hooks.enabled);
        s.general.hooks.hooks.push(crate::hooks::Hook {
            command: "cargo fmt".into(),
            timeout_secs: 0,
            ..Default::default()
        });
        s.normalize();
        assert_eq!(s.general.hooks.hooks[0].timeout_secs, 1);
        assert_eq!(s.general.hooks.hooks[0].name, "hook 1");
    }

    #[test]
    fn skill_settings_load_from_older_files_and_are_kept_tidy() {
        let old: GeneralSettings = serde_json::from_str(r#"{"max_tool_rounds": 5}"#).unwrap();
        assert_eq!(old.skills, SkillSettings::default());
        assert!(old.skills.enabled && old.skills.workspace_skills);

        let partial: SkillSettings =
            serde_json::from_str(r#"{"disabled": ["x"], "max_kb": 8}"#).unwrap();
        assert!(partial.enabled && partial.workspace_skills);
        assert_eq!(partial.disabled, ["x"]);
        assert_eq!(partial.max_kb, 8);

        let mut s = Settings::default();
        s.general.skills.max_kb = 0;
        s.general.skills.disabled = vec![" a ".into(), "".into(), "a".into(), "b".into()];
        s.normalize();
        assert_eq!(s.general.skills.max_kb, 1);
        assert_eq!(s.general.skills.disabled, ["a", "b"]);
    }

    #[test]
    fn review_strictness_and_provider_context_size_load_from_older_files() {
        let old: ReviewSettings = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert_eq!(old.strictness, ReviewStrictness::Normal);
        let strict: ReviewSettings = serde_json::from_str(r#"{"strictness": "strict"}"#).unwrap();
        assert_eq!(strict.strictness, ReviewStrictness::Strict);
        assert!(
            serde_json::to_string(&ReviewSettings::default())
                .unwrap()
                .contains(r#""strictness":"normal""#)
        );

        let p = ProviderConfig::default();
        assert_eq!(p.context_tokens, None);
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("context_tokens"), "{json}");
        let p: ProviderConfig = serde_json::from_str(r#"{"context_tokens": 8192}"#).unwrap();
        assert_eq!(p.context_tokens, Some(8192));
    }

    use super::*;

    #[test]
    fn imports_claude_desktop_format() {
        let json = r#"{"mcpServers": {
            "fs": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"], "env": {"A": "1"}},
            "remote": {"url": "https://example.com/mcp", "headers": {"Authorization": "Bearer x"}}
        }}"#;
        let servers = import_mcp_json(json).unwrap();
        assert_eq!(servers.len(), 2);
        let fs = servers.iter().find(|s| s.name == "fs").unwrap();
        match &fs.transport {
            McpTransportConfig::Stdio {
                command, args, env, ..
            } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
                assert_eq!(env[0].key, "A");
            }
            _ => panic!("expected stdio"),
        }
        let remote = servers.iter().find(|s| s.name == "remote").unwrap();
        assert!(matches!(remote.transport, McpTransportConfig::Http { .. }));
    }

    #[test]
    fn keychain_save_load_migrate_and_fallback() {
        use crate::secrets::tests::{MemoryStore, sample};
        let dir = std::env::temp_dir().join(format!("cleanrstai-cfg-{}", uuid::Uuid::new_v4()));
        let path = dir.join("settings.json");
        let store = MemoryStore::default();

        // A legacy plain-text file needs migrating.
        let legacy = sample();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
        let loaded = Settings::load_from(&path, &store);
        assert!(loaded.needs_migration);
        assert_eq!(loaded.settings, legacy);

        // Saving moves secrets out of the file; loading puts them back.
        assert_eq!(legacy.save_to(&path, &store).unwrap(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("sk-1") && text.contains("\"secrets_in_keychain\": true"),
            "{text}"
        );
        let loaded = Settings::load_from(&path, &store);
        assert!(!loaded.needs_migration && loaded.notice.is_none());
        assert_eq!(loaded.settings, legacy);

        // Removing a provider deletes its keychain entry.
        let mut fewer = legacy.clone();
        fewer.providers = vec![ProviderConfig::default()];
        fewer.save_to(&path, &store).unwrap();
        assert!(
            !store
                .items
                .lock()
                .unwrap()
                .contains_key("provider:p1:api_key")
        );

        // A missing keychain entry is reported.
        legacy.save_to(&path, &store).unwrap();
        store.items.lock().unwrap().clear();
        assert!(Settings::load_from(&path, &store).notice.is_some());

        // A failing keychain falls back to plain text with a warning.
        let broken = MemoryStore {
            fail: true,
            ..Default::default()
        };
        assert!(legacy.save_to(&path, &broken).unwrap().is_some());
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-1"));

        // Turning the keychain off writes plain text and clears stored entries.
        let mut off = legacy.clone();
        off.general.use_keychain = false;
        off.save_to(&path, &store).unwrap(); // previous save had no accounts
        legacy.save_to(&path, &store).unwrap();
        assert!(!store.items.lock().unwrap().is_empty());
        off.save_to(&path, &store).unwrap();
        assert!(store.items.lock().unwrap().is_empty());
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_roundtrip_with_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert_eq!(s.general.zoom, 1.5);
        assert!(s.general.send_with_enter);
        assert_eq!(s.providers.len(), 1);
        assert_eq!(s.general.update_url, crate::update::LATEST_RELEASE_URL);
    }

    #[test]
    fn settings_without_sandbox_load_defaults() {
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert_eq!(s.sandbox, SandboxSettings::default());
        assert!(!s.sandbox.enabled);
        assert_eq!(s.sandbox.image, "python:3.13-slim");
        assert_eq!(s.sandbox.pull, SandboxPull::Missing);
        assert_eq!(s.sandbox.network, SandboxNetwork::None);
    }

    #[test]
    fn sandbox_normalize_clamps_and_fills_image() {
        let mut s = Settings::default();
        s.sandbox.cpus = 0.0;
        s.sandbox.memory_mb = 1;
        s.sandbox.pids = 1_000_000;
        s.sandbox.image = "  ".into();
        s.normalize();
        assert_eq!(s.sandbox.cpus, 0.25);
        assert_eq!(s.sandbox.memory_mb, 256);
        assert_eq!(s.sandbox.pids, 65_536);
        assert_eq!(s.sandbox.image, "python:3.13-slim");

        s.sandbox.cpus = 500.0;
        s.sandbox.memory_mb = 10_000_000;
        s.sandbox.pids = 1;
        s.sandbox.cpus = f32::NAN;
        s.normalize();
        assert_eq!(s.sandbox.cpus, 2.0);
        s.sandbox.cpus = 500.0;
        s.normalize();
        assert_eq!(s.sandbox.cpus, 64.0);
        assert_eq!(s.sandbox.memory_mb, 262_144);
        assert_eq!(s.sandbox.pids, 32);
    }

    #[test]
    fn valid_sandbox_image_rejects_empty_option_like_and_control_text() {
        for ok in [
            "python:3.13-slim",
            "ghcr.io/me/tools:1.2",
            "registry:5000/app@sha256:abc",
            "cleanrstai-sandbox:0123456789ab",
        ] {
            assert!(valid_sandbox_image(ok), "{ok}");
        }
        for bad in ["", "-x", "--privileged", "a b", "a\tb", "a\nb", "a\u{7}b"] {
            assert!(!valid_sandbox_image(bad), "{bad:?}");
        }
    }

    #[test]
    fn sandbox_normalize_resets_an_invalid_image_after_trim() {
        for bad in ["-x", "a b", "a\nb", "   "] {
            let sb = SandboxSettings {
                image: bad.into(),
                ..SandboxSettings::default()
            };
            assert_eq!(sb.normalized().image, DEFAULT_SANDBOX_IMAGE, "{bad:?}");
        }
        let padded = SandboxSettings {
            image: "  ghcr.io/me/tools:1.2 \n".into(),
            ..SandboxSettings::default()
        };
        assert_eq!(padded.normalized().image, "ghcr.io/me/tools:1.2");
        let mut s = Settings::default();
        s.sandbox.image = "--entrypoint".into();
        s.normalize();
        assert_eq!(s.sandbox.image, DEFAULT_SANDBOX_IMAGE);
    }

    #[test]
    fn sandbox_normalized_returns_a_clamped_copy_and_keeps_the_draft() {
        let draft = SandboxSettings {
            cpus: 500.0,
            memory_mb: 1,
            pids: 1_000_000,
            image: " ".into(),
            ..SandboxSettings::default()
        };
        let clean = draft.normalized();
        assert_eq!(clean.cpus, 64.0);
        assert_eq!(clean.memory_mb, 256);
        assert_eq!(clean.pids, 65_536);
        assert_eq!(clean.image, DEFAULT_SANDBOX_IMAGE);
        // The draft itself is untouched.
        assert_eq!(draft.cpus, 500.0);
        assert_eq!(draft.image, " ");
        let nan = SandboxSettings {
            cpus: f32::NAN,
            ..SandboxSettings::default()
        };
        assert_eq!(nan.normalized().cpus, 2.0);
    }

    #[test]
    fn sandbox_settings_roundtrip_snake_case() {
        let sb = SandboxSettings {
            source: SandboxImageSource::Dockerfile,
            network: SandboxNetwork::Bridge,
            pull: SandboxPull::Never,
            ..SandboxSettings::default()
        };
        let json = serde_json::to_string(&sb).unwrap();
        assert!(json.contains(r#""source":"dockerfile""#));
        assert!(json.contains(r#""network":"bridge""#));
        assert!(json.contains(r#""pull":"never""#));
        let back: SandboxSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sb);
    }

    #[test]
    fn settings_without_remote_load_defaults() {
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert_eq!(s.general.remote, RemoteSettings::default());
        assert!(!s.general.remote.enabled);
        assert_eq!(s.general.remote.port, 8484);
    }

    #[test]
    fn new_feature_settings_load_with_defaults_and_normalise() {
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert!(s.general.compaction.enabled);
        assert!(s.general.instructions.enabled);
        assert_eq!(
            s.general.instructions.file_names,
            ["AGENTS.md", "CLAUDE.md"]
        );
        assert!(s.general.file_edits.diff_preview && s.general.file_edits.checkpoints);
        assert!(s.general.html_preview.enabled && !s.general.html_preview.network);
        assert!(s.general.builtin_tools.web_search);
        assert_eq!(
            s.general.builtin_tools.search.backend,
            SearchBackend::DuckDuckGo
        );

        let mut s = Settings::default();
        s.general.compaction.threshold_percent = 5;
        s.general.compaction.keep_recent = 0;
        s.general.compaction.prompt = " ".into();
        s.general.compaction.provider_id = "gone".into();
        s.general.instructions.file_names = vec![
            " AGENTS.md ".into(),
            "".into(),
            "../x.md".into(),
            "..".into(),
        ];
        s.general.instructions.max_kb = 0;
        s.general.builtin_tools.search.max_results = 99;
        s.general.builtin_tools.search.searxng_url = " https://searx.example/ ".into();
        s.normalize();
        assert_eq!(s.general.compaction.threshold_percent, 50);
        assert_eq!(s.general.compaction.keep_recent, 2);
        assert_eq!(s.general.compaction.prompt, DEFAULT_COMPACTION_PROMPT);
        assert!(s.general.compaction.provider_id.is_empty());
        assert_eq!(s.general.instructions.file_names, ["AGENTS.md"]);
        assert_eq!(s.general.instructions.max_kb, 1);
        assert_eq!(s.general.builtin_tools.search.max_results, 20);
        assert_eq!(
            s.general.builtin_tools.search.searxng_url,
            "https://searx.example"
        );
    }

    #[test]
    fn older_provider_sampling_settings_still_load() {
        let p: ProviderConfig = serde_json::from_value(serde_json::json!({
            "name": "Old", "model": "m", "temperature": 0.5, "max_tokens": 100}))
        .unwrap();
        assert_eq!(p.options.temperature, Some(0.5));
        assert_eq!(p.options.max_tokens, Some(100));
        assert_eq!(p.api, ProviderApi::ChatCompletions);
        assert!(p.prompt_caching);
        let p: ProviderConfig =
            serde_json::from_value(serde_json::json!({"temperature": null})).unwrap();
        assert_eq!(p.options.temperature, None);
        let back: ProviderConfig =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn options_layer_provider_model_and_chat() {
        use crate::model_options::{ModelOptions, ReasoningEffort};
        let mut p = ProviderConfig {
            model: "big".into(),
            options: ModelOptions {
                temperature: Some(0.1),
                top_p: Some(0.5),
                ..Default::default()
            },
            ..Default::default()
        };
        p.model_options.insert(
            "big".into(),
            ModelOptions {
                temperature: Some(0.2),
                reasoning_effort: Some(ReasoningEffort::Low),
                ..Default::default()
            },
        );
        let r = p.resolved_options();
        assert_eq!(
            (r.temperature, r.top_p, r.reasoning_effort),
            (Some(0.2), Some(0.5), Some(ReasoningEffort::Low))
        );
        let chat = ModelOptions {
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        };
        let c = p.with_chat_options(&chat).resolved_options();
        assert_eq!(
            (c.temperature, c.reasoning_effort),
            (Some(0.2), Some(ReasoningEffort::High))
        );
        p.model = "small".into();
        assert_eq!(p.resolved_options().temperature, Some(0.1));
    }

    #[test]
    fn compaction_provider_falls_back_to_the_chat() {
        let mut s = Settings::default();
        let chat = s.providers[0].clone();
        assert_eq!(s.compaction_provider(&chat).model, chat.model);
        let other = ProviderConfig {
            id: "p2".into(),
            model: "big".into(),
            ..Default::default()
        };
        s.providers.push(other);
        s.general.compaction.provider_id = "p2".into();
        assert_eq!(s.compaction_provider(&chat).model, "big");
        s.general.compaction.model = " small ".into();
        assert_eq!(s.compaction_provider(&chat).model, "small");
    }

    #[test]
    fn review_provider_and_limits_normalize() {
        let mut s = Settings::default();
        let chat = s.providers[0].clone();
        assert_eq!(s.review_provider(&chat).model, chat.model);
        s.providers.push(ProviderConfig {
            id: "p2".into(),
            model: "big".into(),
            ..Default::default()
        });
        s.general.review.provider_id = "p2".into();
        assert_eq!(s.review_provider(&chat).model, "big");
        s.general.review.model = " reviewer ".into();
        assert_eq!(s.review_provider(&chat).model, "reviewer");

        s.general.review.max_diff_kb = 1;
        s.general.review.base_branch = " dev ".into();
        s.general.review.prompt = " ".into();
        s.normalize();
        assert_eq!(s.general.review.max_diff_kb, 16);
        assert_eq!(s.general.review.base_branch, "dev");
        assert_eq!(s.general.review.prompt, DEFAULT_REVIEW_PROMPT);
        assert_eq!(s.general.review.provider_id, "p2");

        s.general.review.provider_id = "gone".into();
        s.general.review.max_diff_kb = 1_000_000;
        s.normalize();
        assert_eq!(s.general.review.provider_id, "");
        assert_eq!(s.general.review.max_diff_kb, 4096);
    }

    #[test]
    fn default_approval_mode_defaults_and_round_trips() {
        let s: GeneralSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            s.default_approval_mode,
            crate::approval::ApprovalMode::Default
        );
        let g = GeneralSettings {
            default_approval_mode: crate::approval::ApprovalMode::ReadOnly,
            ..Default::default()
        };
        let back: GeneralSettings =
            serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(
            back.default_approval_mode,
            crate::approval::ApprovalMode::ReadOnly
        );
    }

    #[test]
    fn review_settings_default_when_missing_from_file() {
        let s: GeneralSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.review, ReviewSettings::default());
        assert_eq!(s.review.scope, ReviewScope::Uncommitted);
    }

    #[test]
    fn remote_port_zero_normalizes_to_default() {
        let mut s = Settings::default();
        s.general.remote.port = 0;
        s.normalize();
        assert_eq!(s.general.remote.port, 8484);
    }

    #[test]
    fn background_settings_load_from_older_files_and_normalise() {
        // Settings written before Background existed load with the defaults.
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert_eq!(s.general.background, BackgroundSettings::default());
        assert!(s.general.background.notify);
        assert!(s.general.background.notify_only_unfocused);
        assert_eq!(s.general.background.max_parallel, 4);
        assert_eq!(s.general.background.time_limit_mins, 0);
        assert!(!s.general.background.keep_running);

        // Settings from before "keep running" existed (a block without the field) load it off.
        let s: Settings = serde_json::from_str(
            r#"{"general": {"background": {"notify": false, "time_limit_mins": 3}}}"#,
        )
        .unwrap();
        assert!(!s.general.background.keep_running);
        assert!(!s.general.background.notify);
        assert_eq!(s.general.background.time_limit_mins, 3);

        // A partial block keeps the rest at their defaults.
        let s: Settings =
            serde_json::from_str(r#"{"general": {"background": {"max_parallel": 2}}}"#).unwrap();
        assert_eq!(s.general.background.max_parallel, 2);
        assert!(s.general.background.notify);

        let mut s = Settings::default();
        s.general.background.max_parallel = 0;
        s.general.background.time_limit_mins = 5000;
        s.normalize();
        assert_eq!(s.general.background.max_parallel, 1);
        assert_eq!(s.general.background.time_limit_mins, MAX_TIME_LIMIT_MINS);
        s.general.background.max_parallel = 100;
        s.normalize();
        assert_eq!(s.general.background.max_parallel, MAX_PARALLEL_RUNS);
    }

    #[test]
    fn update_url_is_saved_only_when_changed() {
        let mut s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("update_url"), "{json}");
        s.general.update_url = "me/fork".into();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.general.update_url, "me/fork");
        // Clearing the field goes back to the default.
        s.general.update_url = "  ".into();
        s.normalize();
        assert_eq!(s.general.update_url, crate::update::LATEST_RELEASE_URL);
    }

    #[test]
    fn write_private_replaces_the_file_and_keeps_it_owner_only() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-config-{}", uuid::Uuid::new_v4()));
        let path = dir.join("secret.json");
        write_private(&path, b"first").unwrap();
        write_private(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(!path.with_extension("tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

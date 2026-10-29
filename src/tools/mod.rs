//! Tools offered to the model: the built-in tools (time, calculator, task list, web fetch,
//! subagents)
//! plus MCP tools, filtered by what the user switched on for the current chat.

pub mod calc;
pub mod fetch;
pub mod search;
pub mod workspace;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, FixedOffset, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{BuiltinToolSettings, Settings};
use crate::llm::ToolSpec;
use crate::mcp::ToolOutput;
use crate::mcp::manager::{McpManager, ToolRouter};
use crate::skills::{self, Skill};

pub const TIME: &str = "builtin__get_time";
pub const CALCULATE: &str = "builtin__calculate";
pub const TASKS: &str = "builtin__update_tasks";
pub const FETCH: &str = "builtin__fetch";
pub const WEB_SEARCH: &str = "builtin__web_search";
pub const DELEGATE: &str = "builtin__delegate";
pub const LOAD_SKILL: &str = "builtin__load_skill";

/// Built-in tools in display order, with the key used for per-chat switches.
pub const BUILTINS: [(&str, &str, &str); 7] = [
    (TIME, "time", "Time & date"),
    (CALCULATE, "calculator", "Calculator"),
    (TASKS, "tasks", "Task list"),
    (FETCH, "fetch", "Web fetch"),
    (WEB_SEARCH, "web_search", "Web search"),
    (DELEGATE, "subagents", "Subagents"),
    (LOAD_SKILL, "skills", "Skills"),
];

pub fn is_builtin(function: &str) -> bool {
    BUILTINS.iter().any(|(name, ..)| *name == function)
        || workspace::TOOLS.iter().any(|(name, _)| *name == function)
}

/// Label for a tool call card: built-ins by name, MCP tools as `server › tool`.
pub fn display_name(function: &str) -> String {
    if let Some((.., label)) = BUILTINS.iter().find(|(name, ..)| *name == function) {
        return (*label).to_owned();
    }
    match workspace::TOOLS.iter().find(|(name, _)| *name == function) {
        Some((_, label)) => (*label).to_owned(),
        None => McpManager::display_name(function),
    }
}

// ------------------------------------------------------------------ per-chat switches

/// Switches the user flipped for one chat, on top of the defaults from Settings.
/// Keys: `server:<id>`, `tool:<server id>:<tool>`, `builtin:<key>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ToolOverrides(pub BTreeMap<String, bool>);

impl ToolOverrides {
    pub fn server_key(id: &str) -> String {
        format!("server:{id}")
    }

    pub fn tool_key(server_id: &str, tool: &str) -> String {
        format!("tool:{server_id}:{tool}")
    }

    pub fn builtin_key(key: &str) -> String {
        format!("builtin:{key}")
    }

    /// A chat feature that isn't a tool: one of [`FEATURES`].
    pub fn feature_key(key: &str) -> String {
        format!("feature:{key}")
    }

    pub fn get(&self, key: &str) -> Option<bool> {
        self.0.get(key).copied()
    }

    /// Record a switch; one that matches the default is dropped, so a later change in
    /// Settings still applies.
    pub fn set(&mut self, key: String, on: bool, default: bool) {
        if on == default {
            self.0.remove(&key);
        } else {
            self.0.insert(key, on);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub const COMPACTION: &str = "compaction";
pub const INSTRUCTIONS: &str = "instructions";
pub const CHECKPOINTS: &str = "checkpoints";
pub const NOTIFY: &str = "notify";
pub const HOOKS: &str = "hooks";

/// Chat features with a per-chat switch, in display order: (key, label, description).
pub const FEATURES: [(&str, &str, &str); 5] = [
    (
        COMPACTION,
        "Context compaction",
        "Summarise older turns when the context fills up",
    ),
    (
        INSTRUCTIONS,
        "Project instructions",
        "Read AGENTS.md and similar files from the workspace",
    ),
    (
        CHECKPOINTS,
        "Undo for file edits",
        "Keep files' previous content so a turn's edits can be undone",
    ),
    (
        NOTIFY,
        "Notifications",
        "Send a desktop notification when this chat's reply finishes, fails or needs approval",
    ),
    (
        HOOKS,
        "Hooks",
        "Run the commands set under Skills & hooks around tool calls and when a reply is done. \
         Hooks run commands on your computer (or in the sandbox, when it's on)",
    ),
];

/// What one chat may use: Settings defaults plus that chat's overrides.
#[derive(Clone, Debug, Default)]
pub struct ToolSelection {
    pub overrides: ToolOverrides,
    pub builtins: BuiltinToolSettings,
    pub subagents: bool,
    /// Tools switched off in Settings, per server id.
    disabled: HashMap<String, Vec<String>>,
    /// Defaults of the [`FEATURES`], from Settings.
    features: [bool; 5],
    /// The chat has skills to offer (see [`ToolSelection::with_skills`]).
    skills_available: bool,
    /// The provider's own web search replaces the built-in `web_search` tool this request.
    native_search: bool,
}

impl ToolSelection {
    pub fn new(settings: &Settings, overrides: &ToolOverrides) -> Self {
        let g = &settings.general;
        Self {
            overrides: overrides.clone(),
            builtins: g.builtin_tools.clone(),
            subagents: settings.subagents.enabled,
            disabled: settings
                .mcp_servers
                .iter()
                .map(|s| (s.id.clone(), s.disabled_tools.clone()))
                .collect(),
            features: [
                g.compaction.enabled,
                g.instructions.enabled,
                g.file_edits.checkpoints,
                g.background.notify,
                g.hooks.enabled,
            ],
            skills_available: false,
            native_search: false,
        }
    }

    /// This selection for a request the provider's own search answers: the built-in
    /// `web_search` tool is left out. Subagents start from the selection without this.
    pub fn with_native_search(mut self, on: bool) -> Self {
        self.native_search = on;
        self
    }

    /// Default for a chat feature, by its key ([`COMPACTION`], …).
    pub fn feature_default(&self, key: &str) -> bool {
        FEATURES
            .iter()
            .position(|(k, ..)| *k == key)
            .is_some_and(|i| self.features[i])
    }

    /// Whether the chat has skills to list: the `skills` built-in is on by default only then,
    /// and `load_skill` is offered only then. Off unless set, so subagents and tests don't
    /// offer it by accident.
    pub fn with_skills(mut self, available: bool) -> Self {
        self.skills_available = available;
        self
    }

    /// This selection under a chat's permissions: Read only (and Plan) turns off the workspace
    /// tools that write files or run commands (for subagents too, which start from this
    /// selection).
    pub fn with_mode(mut self, mode: crate::approval::ApprovalMode) -> Self {
        if mode.is_read_only() {
            for (key, _) in workspace::TOOLS {
                if workspace::changes_things(key) {
                    self.overrides
                        .0
                        .insert(ToolOverrides::builtin_key(key), false);
                }
            }
        }
        self
    }

    pub fn feature_on(&self, key: &str) -> bool {
        self.overrides
            .get(&ToolOverrides::feature_key(key))
            .unwrap_or_else(|| self.feature_default(key))
    }

    pub fn server_on(&self, id: &str) -> bool {
        self.overrides
            .get(&ToolOverrides::server_key(id))
            .unwrap_or(true)
    }

    pub fn tool_default(&self, server_id: &str, tool: &str) -> bool {
        !self
            .disabled
            .get(server_id)
            .is_some_and(|d| d.iter().any(|t| t == tool))
    }

    pub fn tool_on(&self, server_id: &str, tool: &str) -> bool {
        self.server_on(server_id)
            && self
                .overrides
                .get(&ToolOverrides::tool_key(server_id, tool))
                .unwrap_or_else(|| self.tool_default(server_id, tool))
    }

    /// Default for a built-in, by its switch key (`time`, `calculator`, …).
    pub fn builtin_default(&self, key: &str) -> bool {
        match key {
            "time" => self.builtins.time,
            "calculator" => self.builtins.calculator,
            "tasks" => self.builtins.tasks,
            "fetch" => self.builtins.fetch,
            "web_search" => self.builtins.web_search,
            ws if workspace::TOOLS.iter().any(|(name, _)| *name == ws) => self.builtins.workspace,
            "subagents" => self.subagents,
            "skills" => self.skills_available,
            _ => false,
        }
    }

    pub fn builtin_on(&self, key: &str) -> bool {
        self.overrides
            .get(&ToolOverrides::builtin_key(key))
            .unwrap_or_else(|| self.builtin_default(key))
    }
}

// ------------------------------------------------------------------ toolbox

/// The tools for one model request and how to run them.
#[derive(Clone, Default)]
pub struct Toolbox {
    pub specs: Vec<ToolSpec>,
    mcp: ToolRouter,
    /// For the fetch tool.
    http: Option<reqwest::Client>,
    /// Fetch may reach loopback, private and link-local addresses.
    fetch_private: bool,
    /// The chat's workspace, for the workspace tools.
    workspace: Option<workspace::Workspace>,
    /// The web search service and its options.
    search: crate::config::WebSearchSettings,
    /// The skills `load_skill` reads, and the most text one load returns.
    skills: Arc<Vec<Skill>>,
    skills_max_bytes: usize,
}

impl Toolbox {
    /// Built-ins that `selection` allows, then the allowed MCP tools. `delegate` is the
    /// description for the subagent tool, or `None` to leave it out.
    pub fn new(mut mcp: ToolRouter, selection: &ToolSelection, delegate: Option<String>) -> Self {
        mcp.retain(|server, tool| selection.tool_on(server, tool));
        let mut specs = Vec::new();
        for (name, key, _) in BUILTINS {
            if !selection.builtin_on(key) || (name == WEB_SEARCH && selection.native_search) {
                continue;
            }
            if name == DELEGATE {
                if let Some(desc) = &delegate {
                    specs.push(delegate_spec(desc));
                }
            } else if name == WEB_SEARCH {
                specs.push(ToolSpec {
                    name: WEB_SEARCH.into(),
                    description: search::spec_description(&selection.builtins.search),
                    parameters: search::spec_parameters(),
                });
            } else {
                specs.push(builtin_spec(name));
            }
        }
        // A built-in name wins over an MCP tool that happens to sanitise to the same name.
        specs.extend(mcp.specs.iter().filter(|s| !is_builtin(&s.name)).cloned());
        Self {
            specs,
            mcp,
            http: None,
            fetch_private: selection.builtins.fetch_private_networks,
            workspace: None,
            search: selection.builtins.search.clone(),
            skills: Arc::default(),
            skills_max_bytes: 0,
        }
    }

    /// The skills `load_skill` can read (offered when the selection allows it), cut to
    /// `max_kb` per load.
    pub fn with_skills(mut self, skills: Arc<Vec<Skill>>, max_kb: u32) -> Self {
        self.skills = skills;
        self.skills_max_bytes = max_kb as usize * 1024;
        self
    }

    /// Offer the workspace tools that `selection` allows, working in `ws`.
    pub fn with_workspace(mut self, ws: workspace::Workspace, selection: &ToolSelection) -> Self {
        // After the built-ins, before the MCP tools.
        let at = self
            .specs
            .iter()
            .take_while(|s| is_builtin(&s.name))
            .count();
        let specs = workspace::TOOLS
            .iter()
            .filter(|(name, _)| selection.builtin_on(name))
            .map(|(name, _)| ws.spec(name));
        self.specs.splice(at..at, specs);
        self.workspace = Some(ws);
        self
    }

    /// The HTTP client the fetch tool uses.
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    pub fn has(&self, function: &str) -> bool {
        self.specs.iter().any(|s| s.name == function)
    }

    /// The MCP server id and tool name behind `function`; `None` for built-ins.
    pub fn origin(&self, function: &str) -> Option<(&str, &str)> {
        if is_builtin(function) {
            return None;
        }
        self.mcp.origin(function)
    }

    /// The local built-ins (time, calculator, tasks) and the read-only workspace tools skip
    /// approval; `fetch` reaches the network, `delegate` starts subagents, and writing,
    /// editing and running commands change things, so they ask like MCP tools do.
    pub fn needs_approval(function: &str) -> bool {
        !is_builtin(function)
            || function == DELEGATE
            || function == FETCH
            || function == WEB_SEARCH
            || workspace::changes_things(function)
    }

    /// Whether a call to `function` asks first in this toolbox: [`Self::needs_approval`],
    /// except that web search follows its own setting.
    pub fn asks(&self, function: &str) -> bool {
        if function == WEB_SEARCH {
            return self.search.ask;
        }
        Self::needs_approval(function)
    }

    /// Run a built-in (except `delegate`, which the agent handles) or an MCP tool.
    pub async fn call(&self, function: &str, arguments: &str, timeout: Duration) -> ToolOutput {
        if !self.has(function) {
            return error_output(format!(
                "Error: the tool '{function}' is not available in this chat."
            ));
        }
        if let Some(ws) = &self.workspace
            && workspace::TOOLS.iter().any(|(name, _)| *name == function)
        {
            return match parse_args(arguments) {
                Ok(args) => ws.call(function, &args, timeout).await,
                Err(e) => error_output(format!("Error: {e:#}")),
            };
        }
        if function == LOAD_SKILL {
            let result = match parse_args(arguments) {
                Ok(args) => skills::call(&self.skills, &args, self.skills_max_bytes),
                Err(e) => Err(format!("{e:#}")),
            };
            return match result {
                Ok(text) => text_output(text),
                Err(e) => error_output(format!("Error: {e}")),
            };
        }
        if function == FETCH {
            let Some(http) = &self.http else {
                return error_output("Error: web fetch isn't available here.");
            };
            let args = match parse_args(arguments) {
                Ok(args) => args,
                Err(e) => return error_output(format!("Error: {e:#}")),
            };
            return match fetch::run(http, &args, timeout, self.fetch_private).await {
                Ok(out) => out,
                Err(e) => error_output(format!("Error: {e:#}")),
            };
        }
        if function == WEB_SEARCH {
            let Some(http) = &self.http else {
                return error_output("Error: web search isn't available here.");
            };
            let result = match parse_args(arguments) {
                Ok(args) => search::run(http, &self.search, &args, timeout).await,
                Err(e) => Err(e),
            };
            return match result {
                Ok(text) => text_output(text),
                Err(e) => error_output(format!("Error: {e:#}")),
            };
        }
        if is_builtin(function) {
            return run_builtin(function, arguments);
        }
        match self.mcp.call(function, arguments, timeout).await {
            Ok(out) => out,
            Err(e) => error_output(format!("Error: {e:#}")),
        }
    }
}

pub(crate) fn text_output(text: impl Into<String>) -> ToolOutput {
    ToolOutput {
        text: text.into(),
        is_error: false,
        images: Vec::new(),
        sandbox: None,
        edit: None,
    }
}

pub fn error_output(text: impl Into<String>) -> ToolOutput {
    ToolOutput {
        text: text.into(),
        is_error: true,
        images: Vec::new(),
        sandbox: None,
        edit: None,
    }
}

// ------------------------------------------------------------------ built-in tools

fn builtin_spec(name: &str) -> ToolSpec {
    match name {
        TIME => ToolSpec {
            name: TIME.into(),
            description: "Get the current date, time, weekday and time zone of the user's \
                computer, plus UTC. Use it whenever the answer depends on today's date or the \
                current time. Optionally also give the time at another UTC offset."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "utc_offset": {
                        "type": "string",
                        "description": "Also report the time at this offset from UTC, e.g. \"+05:30\", \"-8\" or \"UTC+2\"."
                    }
                }
            }),
        },
        CALCULATE => ToolSpec {
            name: CALCULATE.into(),
            description: "Evaluate a math expression exactly instead of doing arithmetic in \
                your head. Supports + - * / % ^ (power), parentheses, factorial (!), pi, e, \
                and functions: sqrt, cbrt, abs, sin, cos, tan, asin, acos, atan, atan2, ln, \
                log (base 10, or log(x, base)), log2, exp, floor, ceil, round(x, digits), \
                min, max, sum, avg, pow, hypot, deg, rad. Trigonometry uses radians."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The expression, e.g. \"(1250 * 1.08^5) - 1250\"."
                    }
                },
                "required": ["expression"]
            }),
        },
        TASKS => ToolSpec {
            name: TASKS.into(),
            description: "Plan and track multi-step work. For a request that needs several \
                steps, first break it down into short, actionable tasks and call this with the \
                whole list. Then work through them, calling this again with the full updated \
                list whenever a task starts or finishes. The user sees the list as a checklist."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "description": "The complete task list, in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string", "description": "What to do, in a few words."},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
                            },
                            "required": ["title", "status"]
                        }
                    }
                },
                "required": ["tasks"]
            }),
        },
        FETCH => ToolSpec {
            name: FETCH.into(),
            description: fetch::spec_description(),
            parameters: fetch::spec_parameters(),
        },
        LOAD_SKILL => ToolSpec {
            name: LOAD_SKILL.into(),
            description: "Load a skill: a folder of instructions listed under \"Skills \
                available\" in the system prompt. When a task matches a skill, call this with \
                its name and follow the text it returns. Other files in the skill's folder \
                are named at the end of the text; pass one as `file` to read it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "The skill's name, as listed."},
                    "file": {"type": "string", "description": "Optional: a file in the skill's folder, by relative path, instead of SKILL.md."}
                },
                "required": ["name"]
            }),
        },
        other => unreachable!("not a built-in tool: {other}"),
    }
}

fn delegate_spec(description: &str) -> ToolSpec {
    ToolSpec {
        name: DELEGATE.into(),
        description: description.to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "description": "Independent tasks, one subagent each. Subagents run in parallel and cannot see this conversation, so each task must be self-contained.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "task": {"type": "string", "description": "What the subagent should do, with all the context it needs."},
                            "provider": {"type": "string", "description": "Optional provider profile name to run this task on."},
                            "model": {"type": "string", "description": "Optional model to use for this task."}
                        },
                        "required": ["task"]
                    }
                }
            },
            "required": ["tasks"]
        }),
    }
}

fn parse_args(arguments: &str) -> Result<Value> {
    if arguments.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(arguments).context("tool arguments are not valid JSON")
}

fn run_builtin(function: &str, arguments: &str) -> ToolOutput {
    let result = parse_args(arguments).and_then(|args| match function {
        TIME => time_tool(&args, Utc::now()),
        CALCULATE => {
            let expr = args
                .get("expression")
                .and_then(Value::as_str)
                .context("missing \"expression\"")?;
            let v = calc::evaluate(expr)?;
            Ok(format!("{expr} = {}", calc::format_number(v)))
        }
        TASKS => {
            let tasks = parse_tasks(arguments)?;
            Ok(task_summary(&tasks))
        }
        other => bail!("'{other}' is not a built-in tool"),
    });
    match result {
        Ok(text) => text_output(text),
        Err(e) => error_output(format!("Error: {e:#}")),
    }
}

/// Parse `+05:30`, `-8`, `UTC+2`, `GMT-03:00` into an offset.
fn parse_offset(s: &str) -> Result<FixedOffset> {
    let t = s.trim().to_ascii_uppercase();
    let t = t
        .strip_prefix("UTC")
        .or_else(|| t.strip_prefix("GMT"))
        .unwrap_or(&t)
        .trim();
    if t.is_empty() || t == "Z" {
        return Ok(FixedOffset::east_opt(0).unwrap());
    }
    let (sign, rest) = match t.as_bytes()[0] {
        b'+' => (1, &t[1..]),
        b'-' => (-1, &t[1..]),
        _ => (1, t),
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        None if rest.len() == 4 => (&rest[..2], &rest[2..]),
        None => (rest, "0"),
    };
    let h: i32 = h
        .trim()
        .parse()
        .with_context(|| format!("invalid UTC offset '{s}'"))?;
    let m: i32 = m
        .trim()
        .parse()
        .with_context(|| format!("invalid UTC offset '{s}'"))?;
    if h > 14 || m >= 60 {
        bail!("invalid UTC offset '{s}'");
    }
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
        .with_context(|| format!("invalid UTC offset '{s}'"))
}

fn describe(t: &DateTime<FixedOffset>) -> String {
    format!(
        "{} (UTC{})",
        t.format("%A, %-d %B %Y, %H:%M:%S"),
        t.format("%:z")
    )
}

fn time_tool(args: &Value, now: DateTime<Utc>) -> Result<String> {
    let local: DateTime<FixedOffset> = now.with_timezone(&Local).fixed_offset();
    let mut out = format!(
        "Local time: {}\nISO 8601: {}\nUTC: {}\nUnix timestamp: {}",
        describe(&local),
        local.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        now.timestamp()
    );
    if let Ok(tz) = std::env::var("TZ")
        && !tz.trim().is_empty()
    {
        out.push_str(&format!("\nTime zone: {}", tz.trim()));
    }
    if let Some(offset) = args
        .get("utc_offset")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        let at = now.with_timezone(&parse_offset(offset)?);
        out.push_str(&format!("\nAt UTC{}: {}", at.format("%:z"), describe(&at)));
    }
    Ok(out)
}

// ------------------------------------------------------------------ task list

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    #[serde(alias = "todo", alias = "not_started", alias = "open")]
    Pending,
    #[serde(
        alias = "in-progress",
        alias = "active",
        alias = "doing",
        alias = "started"
    )]
    InProgress,
    #[serde(alias = "completed", alias = "complete", alias = "finished")]
    Done,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub title: String,
    #[serde(default)]
    pub status: TaskStatus,
}

/// The task list from an `update_tasks` call's arguments.
pub fn parse_tasks(arguments: &str) -> Result<Vec<Task>> {
    #[derive(Deserialize)]
    struct Args {
        tasks: Vec<Task>,
    }
    let args: Args = serde_json::from_str(arguments).context(
        "expected {\"tasks\": [{\"title\": …, \"status\": \"pending\" | \"in_progress\" | \"done\"}]}",
    )?;
    let tasks: Vec<Task> = args
        .tasks
        .into_iter()
        .filter(|t| !t.title.trim().is_empty())
        .collect();
    if tasks.is_empty() {
        bail!("the task list is empty");
    }
    Ok(tasks)
}

pub fn task_summary(tasks: &[Task]) -> String {
    let done = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let mut out = format!("Task list updated: {done} of {} done.", tasks.len());
    if let Some(next) = tasks.iter().find(|t| t.status != TaskStatus::Done) {
        out.push_str(&format!(" Next: {}.", next.title.trim()));
    } else {
        out.push_str(" All tasks are done.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpServerConfig;

    #[test]
    fn overrides_fall_back_to_settings() {
        let mut settings = Settings::default();
        settings.general.builtin_tools.calculator = false;
        settings.mcp_servers.push(McpServerConfig {
            id: "s1".into(),
            disabled_tools: vec!["rm".into()],
            ..Default::default()
        });
        let mut ov = ToolOverrides::default();
        let sel = ToolSelection::new(&settings, &ov);
        assert!(sel.builtin_on("time"));
        assert!(!sel.builtin_on("calculator"));
        assert!(sel.tool_on("s1", "ls"));
        assert!(!sel.tool_on("s1", "rm"));

        ov.set(ToolOverrides::builtin_key("calculator"), true, false);
        ov.set(ToolOverrides::tool_key("s1", "rm"), true, false);
        ov.set(ToolOverrides::builtin_key("time"), true, true); // matches default: not stored
        assert_eq!(ov.0.len(), 2);
        let sel = ToolSelection::new(&settings, &ov);
        assert!(sel.builtin_on("calculator"));
        assert!(sel.tool_on("s1", "rm"));

        // Switching a server off hides all its tools; its tool switches are kept.
        ov.set(ToolOverrides::server_key("s1"), false, true);
        let sel = ToolSelection::new(&settings, &ov);
        assert!(!sel.tool_on("s1", "ls"));
        assert!(!sel.tool_on("s1", "rm"));
    }

    #[test]
    fn toolbox_respects_selection() {
        let mut settings = Settings::default();
        settings.general.builtin_tools.tasks = false;
        let sel = ToolSelection::new(&settings, &ToolOverrides::default());
        let tb = Toolbox::new(ToolRouter::default(), &sel, Some("delegate".into()));
        let names: Vec<&str> = tb.specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, [TIME, CALCULATE, FETCH, WEB_SEARCH, DELEGATE]);
        let tb = Toolbox::new(ToolRouter::default(), &sel, None);
        assert!(!tb.has(DELEGATE));
        // Web search asks first unless its own setting says not to.
        assert!(tb.asks(WEB_SEARCH));
        settings.general.builtin_tools.search.ask = false;
        let quiet = ToolSelection::new(&settings, &ToolOverrides::default());
        let tb_quiet = Toolbox::new(ToolRouter::default(), &quiet, None);
        assert!(!tb_quiet.asks(WEB_SEARCH));
        assert!(tb_quiet.asks(FETCH));
        assert!(Toolbox::needs_approval(DELEGATE));
        assert!(Toolbox::needs_approval(FETCH));
        assert!(!Toolbox::needs_approval(CALCULATE));
        assert!(Toolbox::needs_approval("server__tool"));
    }

    #[tokio::test]
    async fn load_skill_is_offered_only_with_skills_and_reads_them() {
        let dir =
            std::env::temp_dir().join(format!("cleanrstai-tools-skill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("one")).unwrap();
        std::fs::write(
            dir.join("one/SKILL.md"),
            "---\ndescription: first\n---\nDo it.",
        )
        .unwrap();
        let skills = crate::skills::discover(
            &[(dir.clone(), crate::skills::SkillSource::User)],
            &crate::config::SkillSettings::default(),
        );
        let settings = Settings::default();
        let sel = ToolSelection::new(&settings, &ToolOverrides::default());
        assert!(!Toolbox::new(ToolRouter::default(), &sel, None).has(LOAD_SKILL));

        let sel = sel.with_skills(true);
        let tb = Toolbox::new(ToolRouter::default(), &sel, None).with_skills(Arc::new(skills), 64);
        assert!(tb.has(LOAD_SKILL));
        assert!(!Toolbox::needs_approval(LOAD_SKILL));
        let out = tb
            .call(LOAD_SKILL, r#"{"name": "one"}"#, Duration::from_secs(5))
            .await;
        assert!(!out.is_error);
        assert!(out.text.contains("Do it."));
        let missing = tb
            .call(LOAD_SKILL, r#"{"name": "two"}"#, Duration::from_secs(5))
            .await;
        assert!(missing.is_error);

        let mut ov = ToolOverrides::default();
        ov.set(ToolOverrides::builtin_key("skills"), false, true);
        let sel = ToolSelection::new(&settings, &ov).with_skills(true);
        assert!(!Toolbox::new(ToolRouter::default(), &sel, None).has(LOAD_SKILL));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn fetch_runs_through_the_toolbox() {
        let (base, _) = crate::testutil::serve(|_| {
            crate::testutil::Response::with_type(200, "text/plain", "hello from the web")
        })
        .await;
        let args = json!({"url": format!("{base}/x")}).to_string();
        let t = Duration::from_secs(5);
        // By default the local mock server is out of reach.
        let sel = ToolSelection::new(&Settings::default(), &ToolOverrides::default());
        let tb = Toolbox::new(ToolRouter::default(), &sel, None).with_http(reqwest::Client::new());
        let out = tb.call(FETCH, &args, t).await;
        assert!(
            out.is_error && out.text.contains("public internet"),
            "{}",
            out.text
        );
        let mut settings = Settings::default();
        settings.general.builtin_tools.fetch_private_networks = true;
        let sel = ToolSelection::new(&settings, &ToolOverrides::default());
        // Without an HTTP client (never the case in the app) it fails cleanly.
        let tb = Toolbox::new(ToolRouter::default(), &sel, None);
        assert!(tb.call(FETCH, &args, t).await.is_error);
        let tb = tb.with_http(reqwest::Client::new());
        let out = tb.call(FETCH, &args, t).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.ends_with("hello from the web"), "{}", out.text);
        // Switched off for the chat: refused.
        let mut ov = ToolOverrides::default();
        ov.set(ToolOverrides::builtin_key("fetch"), false, true);
        let sel = ToolSelection::new(&Settings::default(), &ov);
        let tb = Toolbox::new(ToolRouter::default(), &sel, None).with_http(reqwest::Client::new());
        assert!(
            tb.call(FETCH, &args, t)
                .await
                .text
                .contains("not available")
        );
    }

    #[tokio::test]
    async fn runs_builtins() {
        let sel = ToolSelection::new(&Settings::default(), &ToolOverrides::default());
        let tb = Toolbox::new(ToolRouter::default(), &sel, None);
        let t = Duration::from_secs(5);
        let out = tb
            .call(CALCULATE, r#"{"expression": "2 + 2 * 10"}"#, t)
            .await;
        assert!(!out.is_error);
        assert_eq!(out.text, "2 + 2 * 10 = 22");
        let out = tb.call(CALCULATE, r#"{"expression": "1/0"}"#, t).await;
        assert!(out.is_error);
        assert!(out.text.contains("division by zero"));
        let out = tb.call(TIME, "{}", t).await;
        assert!(out.text.contains("UTC: "), "{}", out.text);
        let out = tb
            .call(
                TASKS,
                r#"{"tasks": [{"title": "Plan", "status": "completed"}, {"title": "Build", "status": "in_progress"}]}"#,
                t,
            )
            .await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "Task list updated: 1 of 2 done. Next: Build.");
        let out = tb.call(DELEGATE, "{}", t).await;
        assert!(out.is_error);
        let out = tb.call("other__tool", "{}", t).await;
        assert!(out.is_error);
    }

    #[test]
    fn features_follow_settings_and_chat_switches() {
        let mut settings = Settings::default();
        let sel = ToolSelection::new(&settings, &ToolOverrides::default());
        assert!(sel.feature_on(COMPACTION) && sel.feature_on(INSTRUCTIONS));
        assert!(sel.feature_on(CHECKPOINTS));
        // Hooks default to the master switch in Settings, which is off.
        assert!(!sel.feature_on(HOOKS));
        settings.general.hooks.enabled = true;
        assert!(ToolSelection::new(&settings, &ToolOverrides::default()).feature_on(HOOKS));
        settings.general.compaction.enabled = false;
        settings.general.file_edits.checkpoints = false;
        let mut ov = ToolOverrides::default();
        ov.set(ToolOverrides::feature_key(INSTRUCTIONS), false, true);
        ov.set(ToolOverrides::feature_key(CHECKPOINTS), true, false);
        ov.set(ToolOverrides::feature_key(HOOKS), false, true);
        let sel = ToolSelection::new(&settings, &ov);
        assert!(!sel.feature_on(COMPACTION));
        assert!(!sel.feature_on(INSTRUCTIONS));
        assert!(sel.feature_on(CHECKPOINTS));
        assert!(!sel.feature_on(HOOKS));
        assert!(!sel.feature_default(CHECKPOINTS));
        assert!(!sel.feature_on("unknown"));
    }

    #[test]
    fn time_tool_reports_offsets() {
        let now = DateTime::parse_from_rfc3339("2026-10-22T09:15:03Z")
            .unwrap()
            .with_timezone(&Utc);
        let out = time_tool(&json!({"utc_offset": "+05:30"}), now).unwrap();
        assert!(out.contains("UTC: 2026-10-22T09:15:03Z"), "{out}");
        assert!(out.contains("Unix timestamp: 1792660503"), "{out}");
        assert!(
            out.contains("At UTC+05:30: Thursday, 22 October 2026, 14:45:03"),
            "{out}"
        );
        assert!(time_tool(&json!({"utc_offset": "nonsense"}), now).is_err());
        assert_eq!(parse_offset("UTC-8").unwrap().local_minus_utc(), -8 * 3600);
        assert_eq!(parse_offset("GMT+0130").unwrap().local_minus_utc(), 5400);
        assert_eq!(parse_offset("Z").unwrap().local_minus_utc(), 0);
    }

    #[test]
    fn parses_task_lists() {
        let tasks = parse_tasks(
            r#"{"tasks": [{"title": "a", "status": "todo"}, {"title": " "}, {"title": "b", "status": "done"}]}"#,
        )
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].status, TaskStatus::Pending);
        assert_eq!(tasks[1].status, TaskStatus::Done);
        assert!(parse_tasks(r#"{"tasks": []}"#).is_err());
        assert!(parse_tasks("{}").is_err());
        assert_eq!(
            task_summary(&tasks[1..]),
            "Task list updated: 1 of 1 done. All tasks are done."
        );
    }

    #[test]
    fn provider_search_replaces_the_builtin_web_search_tool() {
        let settings = Settings::default();
        let sel = ToolSelection::new(&settings, &ToolOverrides::default());
        let tb = Toolbox::new(ToolRouter::default(), &sel, None);
        assert!(tb.has(WEB_SEARCH));
        let native = sel.clone().with_native_search(true);
        let tb = Toolbox::new(ToolRouter::default(), &native, None);
        assert!(!tb.has(WEB_SEARCH));
        assert!(tb.has(TIME) && tb.has(FETCH), "the other built-ins stay");
        // Subagents start from the selection without the flag, so they keep the tool.
        assert!(Toolbox::new(ToolRouter::default(), &sel, None).has(WEB_SEARCH));
    }
}

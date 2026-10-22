//! Tools offered to the model: the built-in tools (time, calculator, task list, subagents)
//! plus MCP tools, filtered by what the user switched on for the current chat.

pub mod calc;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, FixedOffset, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{BuiltinToolSettings, Settings};
use crate::llm::ToolSpec;
use crate::mcp::ToolOutput;
use crate::mcp::manager::{McpManager, ToolRouter};

pub const TIME: &str = "builtin__get_time";
pub const CALCULATE: &str = "builtin__calculate";
pub const TASKS: &str = "builtin__update_tasks";
pub const DELEGATE: &str = "builtin__delegate";

/// Built-in tools in display order, with the key used for per-chat switches.
pub const BUILTINS: [(&str, &str, &str); 4] = [
    (TIME, "time", "Time & date"),
    (CALCULATE, "calculator", "Calculator"),
    (TASKS, "tasks", "Task list"),
    (DELEGATE, "subagents", "Subagents"),
];

pub fn is_builtin(function: &str) -> bool {
    BUILTINS.iter().any(|(name, ..)| *name == function)
}

/// Label for a tool call card: built-ins by name, MCP tools as `server › tool`.
pub fn display_name(function: &str) -> String {
    match BUILTINS.iter().find(|(name, ..)| *name == function) {
        Some((.., label)) => (*label).to_owned(),
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

/// What one chat may use: Settings defaults plus that chat's overrides.
#[derive(Clone, Debug, Default)]
pub struct ToolSelection {
    pub overrides: ToolOverrides,
    pub builtins: BuiltinToolSettings,
    pub subagents: bool,
    /// Tools switched off in Settings, per server id.
    disabled: HashMap<String, Vec<String>>,
}

impl ToolSelection {
    pub fn new(settings: &Settings, overrides: &ToolOverrides) -> Self {
        Self {
            overrides: overrides.clone(),
            builtins: settings.general.builtin_tools.clone(),
            subagents: settings.subagents.enabled,
            disabled: settings
                .mcp_servers
                .iter()
                .map(|s| (s.id.clone(), s.disabled_tools.clone()))
                .collect(),
        }
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
            "subagents" => self.subagents,
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
}

impl Toolbox {
    /// Built-ins that `selection` allows, then the allowed MCP tools. `delegate` is the
    /// description for the subagent tool, or `None` to leave it out.
    pub fn new(mut mcp: ToolRouter, selection: &ToolSelection, delegate: Option<String>) -> Self {
        mcp.retain(|server, tool| selection.tool_on(server, tool));
        let mut specs = Vec::new();
        for (name, key, _) in BUILTINS {
            if !selection.builtin_on(key) {
                continue;
            }
            if name == DELEGATE {
                if let Some(desc) = &delegate {
                    specs.push(delegate_spec(desc));
                }
            } else {
                specs.push(builtin_spec(name));
            }
        }
        // A built-in name wins over an MCP tool that happens to sanitise to the same name.
        specs.extend(mcp.specs.iter().filter(|s| !is_builtin(&s.name)).cloned());
        Self { specs, mcp }
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

    /// Built-ins other than `delegate` are local and harmless, so they skip approval.
    pub fn needs_approval(function: &str) -> bool {
        !is_builtin(function) || function == DELEGATE
    }

    /// Run a built-in (except `delegate`, which the agent handles) or an MCP tool.
    pub async fn call(&self, function: &str, arguments: &str, timeout: Duration) -> ToolOutput {
        if !self.has(function) {
            return error_output(format!(
                "Error: the tool '{function}' is not available in this chat."
            ));
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

fn text_output(text: impl Into<String>) -> ToolOutput {
    ToolOutput {
        text: text.into(),
        is_error: false,
        images: Vec::new(),
    }
}

pub fn error_output(text: impl Into<String>) -> ToolOutput {
    ToolOutput {
        text: text.into(),
        is_error: true,
        images: Vec::new(),
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
        assert_eq!(names, [TIME, CALCULATE, DELEGATE]);
        let tb = Toolbox::new(ToolRouter::default(), &sel, None);
        assert!(!tb.has(DELEGATE));
        assert!(Toolbox::needs_approval(DELEGATE));
        assert!(!Toolbox::needs_approval(CALCULATE));
        assert!(Toolbox::needs_approval("server__tool"));
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
}

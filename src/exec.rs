//! `cleanrstai exec`: one request from the command line, with the saved settings and the same
//! request the app would build for a new chat. Replies stream to stdout and tool calls go to
//! stderr (or one JSON object at the end with `--json`). Nothing is ever asked interactively:
//! a tool call that the chat's approval mode doesn't allow is denied, and the model sees that.
//!
//! The argument parsing, the approval decision and the output shape are pure functions; `drive`
//! runs one request and is what the tests exercise against a mock provider.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::agent::{self, AgentRequest, AppEvent, ChatEvent};
use crate::approval::ApprovalMode;
use crate::config::{ProviderConfig, Settings};
use crate::mcp::manager::{McpManager, ServerStatus};
use crate::sandbox::Sandbox;
use crate::stats::RequestStats;
use crate::types::{ChatMessage, Conversation, Role, ToolCall};

/// Longest wait for MCP servers to connect before the request starts, in seconds.
pub const MAX_MCP_WAIT_SECS: u32 = 120;

/// The `cleanrstai exec` settings (Settings → Background, "Command line").
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ExecSettings {
    /// Whether `cleanrstai exec` runs at all. It only runs when the user types the command.
    pub enabled: bool,
    /// The permission mode of the request, unless `--mode` says otherwise.
    pub approval_mode: ApprovalMode,
    /// Start the MCP servers from Settings for the request.
    pub mcp: bool,
    /// How long to wait for MCP servers to connect before the request starts (0–120).
    pub mcp_wait_secs: u32,
    /// Save each run's chat, so it shows in the app's chat list, even without `--save`.
    pub save_chats: bool,
}

impl Default for ExecSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            approval_mode: ApprovalMode::ReadOnly,
            mcp: true,
            mcp_wait_secs: 10,
            save_chats: false,
        }
    }
}

pub const USAGE: &str = "\
Usage: cleanrstai exec [OPTIONS] PROMPT
       cleanrstai exec [OPTIONS] -         read the prompt from stdin
       git diff | cleanrstai exec \"review this\" -

Runs one request with the saved settings and prints the reply to stdout. Tool calls are shown
on stderr. Nothing is asked: a tool call the mode doesn't allow is denied.

Options:
  --provider NAME     provider profile to use (default: the active one)
  --model MODEL       model to use instead of the provider's
  --workspace DIR     working folder for the file, search and shell tools
  --mode MODE         read-only, default, auto, allow-all or plan
                      (default: the mode in Settings → Background)
  --json              print one JSON object at the end instead of streaming
  --no-mcp            don't start MCP servers
  --save              save the chat so it shows in the app's chat list
  -h, --help          show this help

Exit status: 0 the reply finished, 1 the provider or agent failed, 2 bad arguments or
cleanrstai exec is off in Settings.
";

/// What `cleanrstai exec` was asked to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecArgs {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub workspace: Option<PathBuf>,
    /// `None` takes the mode from Settings.
    pub mode: Option<ApprovalMode>,
    pub json: bool,
    pub no_mcp: bool,
    pub save: bool,
    /// The prompt words, joined by spaces. Empty when only stdin was given.
    pub text: String,
    /// A `-` among the arguments: stdin is added to the prompt.
    pub read_stdin: bool,
}

/// Parses the arguments after `exec`. Flags may come before or after the prompt; `--` ends
/// the flags, and `-` reads stdin.
pub fn parse_args(args: &[String]) -> Result<ExecArgs, String> {
    let mut out = ExecArgs::default();
    let mut words: Vec<&str> = Vec::new();
    let mut literal = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if literal || arg == "-" {
            if arg == "-" && !literal {
                out.read_stdin = true;
            } else {
                words.push(arg);
            }
            continue;
        }
        if arg == "--" {
            literal = true;
            continue;
        }
        if !arg.starts_with('-') {
            words.push(arg);
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (arg, None),
        };
        match name {
            "--json" | "--no-mcp" | "--save" => {
                if inline.is_some() {
                    return Err(format!("{name} takes no value"));
                }
                match name {
                    "--json" => out.json = true,
                    "--no-mcp" => out.no_mcp = true,
                    _ => out.save = true,
                }
            }
            "--provider" | "--model" | "--workspace" | "--mode" => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let value = args
                            .get(i)
                            .cloned()
                            .ok_or_else(|| format!("{name} needs a value"))?;
                        i += 1;
                        value
                    }
                };
                if value.is_empty() {
                    return Err(format!("{name} needs a value"));
                }
                match name {
                    "--provider" => out.provider = Some(value),
                    "--model" => out.model = Some(value),
                    "--workspace" => out.workspace = Some(PathBuf::from(value)),
                    _ => out.mode = Some(parse_mode(&value)?),
                }
            }
            _ => return Err(format!("unknown option {name}")),
        }
    }
    out.text = words.join(" ");
    if out.text.trim().is_empty() && !out.read_stdin {
        return Err("no prompt: give the prompt text, or - to read it from stdin".into());
    }
    Ok(out)
}

/// The `--mode` values: the approval modes of the chat's tools menu, spelled for a shell.
pub fn parse_mode(value: &str) -> Result<ApprovalMode, String> {
    match value {
        "read-only" => Ok(ApprovalMode::ReadOnly),
        "default" => Ok(ApprovalMode::Default),
        "auto" => Ok(ApprovalMode::AutoWorkspace),
        "allow-all" => Ok(ApprovalMode::AllowAll),
        "plan" => Ok(ApprovalMode::Plan),
        other => Err(format!(
            "unknown mode '{other}': use read-only, default, auto, allow-all or plan"
        )),
    }
}

/// Whether `--help` or `-h` asks for the usage (before any `--`).
pub fn wants_help(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| a.as_str() != "--")
        .any(|a| a == "--help" || a == "-h")
}

/// The prompt: the words, then stdin after a blank line when there is stdin.
pub fn compose_prompt(text: &str, stdin: Option<&str>) -> String {
    let stdin = stdin.map(str::trim_end).filter(|s| !s.trim().is_empty());
    match (text.trim(), stdin) {
        ("", Some(s)) => s.trim_start_matches('\n').to_owned(),
        (t, Some(s)) => format!("{t}\n\n{s}"),
        (t, None) => t.to_owned(),
    }
}

/// The request as exec runs it: only `--mode` (or the exec setting) decides what runs unasked,
/// so the GUI's "Run all tools without asking" never lets a read-only run call tools.
fn headless(mut req: AgentRequest) -> AgentRequest {
    req.general.auto_approve_tools = false;
    req
}

/// Whether a tool call that would ask for approval runs anyway, with nobody to ask. The agent
/// asks only for the calls the mode doesn't run unasked, so the rest are denied here.
/// `sandboxed`: shell commands run in the Docker sandbox.
pub fn headless_allows(mode: ApprovalMode, function: &str, sandboxed: bool) -> bool {
    mode.runs_unasked(function, sandboxed)
}

/// Applies `--provider` and `--model` to the settings: the named profile (by id, or by name
/// without regard to case) becomes the active one, and `--model` replaces its model.
pub fn pick_provider(
    settings: &mut Settings,
    name: Option<&str>,
    model: Option<&str>,
) -> Result<(), String> {
    if let Some(name) = name {
        let index = find_provider(&settings.providers, name).ok_or_else(|| {
            let names: Vec<&str> = settings.providers.iter().map(|p| p.name.as_str()).collect();
            format!(
                "no provider named '{name}' (Settings → Providers has: {})",
                names.join(", ")
            )
        })?;
        settings.active_provider = index;
    }
    if let Some(model) = model {
        settings.provider_mut().model = model.to_owned();
    }
    Ok(())
}

fn find_provider(providers: &[ProviderConfig], name: &str) -> Option<usize> {
    providers.iter().position(|p| p.id == name).or_else(|| {
        providers
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    })
}

/// One tool call as the output reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolRecord {
    pub name: String,
    /// The arguments as the model wrote them (a JSON string).
    pub arguments: String,
    pub result: String,
    pub is_error: bool,
}

/// What a run produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// All the reply text, across the rounds of tool calls.
    pub reply: String,
    pub tool_calls: Vec<ToolRecord>,
    /// The stats of the last request that answered.
    pub stats: Option<RequestStats>,
    pub error: Option<String>,
}

/// The `--json` object: `reply`, `tool_calls`, `stats` and `error` (null when none).
pub fn report_json(report: &Report) -> serde_json::Value {
    serde_json::json!({
        "reply": report.reply,
        "tool_calls": report.tool_calls,
        "stats": report.stats.as_ref().map_or(serde_json::Value::Null, |s| {
            serde_json::to_value(s).unwrap_or(serde_json::Value::Null)
        }),
        "error": report.error,
    })
}

/// Where a run's output goes while it runs.
pub trait Sink {
    /// Reply text as it arrives.
    fn reply_text(&mut self, text: &str);
    /// A line for the person running the command: tool calls, denials, errors.
    fn note(&mut self, line: &str);
}

/// Streams to the terminal: reply text to stdout (not with `--json`), notes to stderr.
struct Terminal {
    stream: bool,
}

impl Sink for Terminal {
    fn reply_text(&mut self, text: &str) {
        if self.stream {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
    }

    fn note(&mut self, line: &str) {
        let _ = writeln!(std::io::stderr(), "{line}");
    }
}

/// Runs `req` to its end. `conv` gets the messages the run makes (the caller saves it, if
/// wanted), and `sink` the output as it happens. Approvals are answered by [`headless_allows`].
pub async fn drive(
    req: AgentRequest,
    http: reqwest::Client,
    mcp: Arc<McpManager>,
    conv: &mut Conversation,
    sink: &mut dyn Sink,
) -> Report {
    let (mode, sandboxed) = (req.approval_mode, req.sandbox.is_some());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let agent = tokio::spawn(agent::run(req, http, mcp, tx, || {}));
    let mut report = Report::default();
    let mut calls: HashMap<String, ToolCall> = HashMap::new();
    while let Some(AppEvent::Chat { event, .. }) = rx.recv().await {
        match event {
            ChatEvent::ContentDelta(text) => {
                sink.reply_text(&text);
                report.reply.push_str(&text);
            }
            ChatEvent::AssistantFinal(msg) => {
                for call in &msg.tool_calls {
                    calls.insert(call.id.clone(), call.clone());
                }
                if msg.stats.is_some() {
                    report.stats = msg.stats.clone();
                }
                conv.messages.push(*msg);
            }
            ChatEvent::ToolApproval { call, respond, .. } => {
                let allowed = headless_allows(mode, &call.name, sandboxed);
                let _ = respond.send(allowed);
                if !allowed {
                    sink.note(&format!("denied: {} (use --mode to allow)", call.name));
                }
            }
            ChatEvent::ToolRunning(id) => {
                if let Some(call) = calls.get(&id) {
                    sink.note(&format!("→ {}", call.name));
                }
            }
            ChatEvent::ToolResult(msg) => {
                sink.note(if msg.is_error { "← error" } else { "← ok" });
                let id = msg.tool_call_id.clone().unwrap_or_default();
                report.tool_calls.push(ToolRecord {
                    name: msg.tool_name.clone().unwrap_or_default(),
                    arguments: calls
                        .get(&id)
                        .map(|c| c.arguments.clone())
                        .unwrap_or_default(),
                    result: msg.content.clone(),
                    is_error: msg.is_error,
                });
                conv.messages.push(*msg);
            }
            ChatEvent::Error(error) => {
                sink.note(&format!("error: {error}"));
                report.error.get_or_insert(error);
            }
            ChatEvent::Done => break,
            _ => {}
        }
    }
    if agent.await.is_err() {
        report
            .error
            .get_or_insert_with(|| "the request stopped unexpectedly".into());
    }
    report
}

/// Waits until no MCP server is still connecting, or `limit` has passed.
async fn wait_for_servers(mcp: &McpManager, limit: Duration) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline
        && mcp
            .snapshot()
            .iter()
            .any(|s| s.status == ServerStatus::Connecting)
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for server in mcp.snapshot() {
        if let ServerStatus::Error(e) = server.status {
            let _ = writeln!(std::io::stderr(), "mcp: {}: {e}", server.id);
        }
    }
}

/// Windows builds use the windows subsystem, which starts without a console. Attach to the
/// console that started us, so the reply and the notes reach the shell.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: AttachConsole takes no pointers; without a parent console it just fails.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// `cleanrstai exec ARGS` (`args` are the arguments after `exec`). Returns the exit status.
pub fn run(args: &[String]) -> i32 {
    attach_parent_console();
    if wants_help(args) {
        print!("{USAGE}");
        return 0;
    }
    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "cleanrstai exec: {e}\nTry 'cleanrstai exec --help'."
            );
            return 2;
        }
    };
    let loaded = Settings::load();
    if let Some(notice) = &loaded.notice {
        let _ = writeln!(std::io::stderr(), "cleanrstai: {notice}");
    }
    let mut settings = loaded.settings;
    if !settings.general.exec.enabled {
        let _ = writeln!(
            std::io::stderr(),
            "cleanrstai exec is off (Settings → Background)"
        );
        return 2;
    }
    let stdin = parsed.read_stdin.then(|| {
        let mut text = String::new();
        let _ = std::io::stdin().read_to_string(&mut text);
        text
    });
    let prompt = compose_prompt(&parsed.text, stdin.as_deref());
    if prompt.trim().is_empty() {
        let _ = writeln!(std::io::stderr(), "cleanrstai exec: the prompt is empty");
        return 2;
    }
    if let Err(e) = pick_provider(
        &mut settings,
        parsed.provider.as_deref(),
        parsed.model.as_deref(),
    ) {
        let _ = writeln!(std::io::stderr(), "cleanrstai exec: {e}");
        return 2;
    }
    let workspace = match &parsed.workspace {
        None => None,
        Some(dir) => match std::fs::canonicalize(dir) {
            Ok(path) if path.is_dir() => Some(path),
            _ => {
                let _ = writeln!(
                    std::io::stderr(),
                    "cleanrstai exec: {} is not a folder",
                    dir.display()
                );
                return 2;
            }
        },
    };

    let mode = parsed.mode.unwrap_or(settings.general.exec.approval_mode);
    let mut conv = Conversation::new();
    conv.permissions.mode = mode;
    conv.workspace = workspace;
    conv.messages.push(ChatMessage::new(Role::User, prompt));
    conv.auto_title();
    conv.updated_at = chrono::Utc::now();
    let save = parsed.save || settings.general.exec.save_chats;
    if save {
        conv.save();
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("cleanrstai-exec")
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "cleanrstai exec: couldn't start: {e}");
            return 1;
        }
    };
    let http = crate::llm::build_http_client();
    let mut sink = Terminal {
        stream: !parsed.json,
    };
    let report = runtime.block_on(async {
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        if !parsed.no_mcp && settings.general.exec.mcp {
            mcp.sync(&settings.mcp_servers);
            let wait = u64::from(settings.general.exec.mcp_wait_secs);
            wait_for_servers(&mcp, Duration::from_secs(wait)).await;
        }
        let sandbox = settings
            .sandbox
            .enabled
            .then(|| Sandbox::new(settings.sandbox.clone()));
        let req = headless(AgentRequest::for_chat(
            &settings,
            &conv,
            sandbox.as_ref(),
            false,
            false,
            None,
        ));
        drive(req, http, mcp, &mut conv, &mut sink).await
    });
    runtime.shutdown_timeout(Duration::from_millis(500));
    if save {
        conv.save();
    }

    let mut stdout = std::io::stdout().lock();
    if parsed.json {
        let _ = writeln!(stdout, "{}", report_json(&report));
    } else if !report.reply.is_empty() && !report.reply.ends_with('\n') {
        let _ = writeln!(stdout);
    }
    let _ = stdout.flush();
    if report.error.is_some() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};
    use uuid::Uuid;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prompt_words_are_joined_and_flags_can_come_after() {
        let p = parse_args(&words(&["review", "this", "--json"])).unwrap();
        assert_eq!(p.text, "review this");
        assert!(p.json);
        assert!(!p.read_stdin);
        assert_eq!(p.mode, None);
    }

    #[test]
    fn every_flag_parses() {
        let p = parse_args(&words(&[
            "--provider",
            "Local",
            "--model=qwen",
            "--workspace",
            "/work",
            "--mode",
            "allow-all",
            "--json",
            "--no-mcp",
            "--save",
            "hello",
        ]))
        .unwrap();
        assert_eq!(p.provider.as_deref(), Some("Local"));
        assert_eq!(p.model.as_deref(), Some("qwen"));
        assert_eq!(p.workspace, Some(PathBuf::from("/work")));
        assert_eq!(p.mode, Some(ApprovalMode::AllowAll));
        assert!(p.json && p.no_mcp && p.save);
        assert_eq!(p.text, "hello");
    }

    #[test]
    fn a_dash_reads_stdin_and_joins_the_prompt() {
        let p = parse_args(&words(&["--mode", "read-only", "review this", "-"])).unwrap();
        assert!(p.read_stdin);
        assert_eq!(p.text, "review this");
        assert_eq!(p.mode, Some(ApprovalMode::ReadOnly));
        let only = parse_args(&words(&["-"])).unwrap();
        assert!(only.read_stdin);
        assert_eq!(only.text, "");
    }

    #[test]
    fn double_dash_ends_the_flags() {
        let p = parse_args(&words(&["--", "--json", "-x"])).unwrap();
        assert!(!p.json);
        assert_eq!(p.text, "--json -x");
        // A lone `-` after `--` is prompt text too.
        let p = parse_args(&words(&["--", "-"])).unwrap();
        assert!(!p.read_stdin);
        assert_eq!(p.text, "-");
    }

    #[test]
    fn bad_arguments_are_errors() {
        assert!(parse_args(&words(&[])).unwrap_err().contains("no prompt"));
        assert!(
            parse_args(&words(&["--json"]))
                .unwrap_err()
                .contains("no prompt")
        );
        assert!(
            parse_args(&words(&["--mode", "bogus", "x"]))
                .unwrap_err()
                .contains("bogus")
        );
        assert!(
            parse_args(&words(&["--model"]))
                .unwrap_err()
                .contains("--model needs a value")
        );
        assert!(
            parse_args(&words(&["--model=", "x"]))
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            parse_args(&words(&["--json=yes", "x"]))
                .unwrap_err()
                .contains("takes no value")
        );
        assert!(
            parse_args(&words(&["--nope", "x"]))
                .unwrap_err()
                .contains("--nope")
        );
    }

    #[test]
    fn modes_are_spelled_for_a_shell() {
        assert_eq!(parse_mode("read-only"), Ok(ApprovalMode::ReadOnly));
        assert_eq!(parse_mode("default"), Ok(ApprovalMode::Default));
        assert_eq!(parse_mode("auto"), Ok(ApprovalMode::AutoWorkspace));
        assert_eq!(parse_mode("allow-all"), Ok(ApprovalMode::AllowAll));
        assert_eq!(parse_mode("plan"), Ok(ApprovalMode::Plan));
        assert!(parse_mode("Read Only").is_err());
        assert!(parse_mode("Plan").is_err());
    }

    #[test]
    fn help_is_asked_before_a_double_dash() {
        assert!(wants_help(&words(&["--help"])));
        assert!(wants_help(&words(&["hi", "-h"])));
        assert!(!wants_help(&words(&["--", "--help"])));
        assert!(!wants_help(&words(&["hi"])));
    }

    #[test]
    fn stdin_follows_the_words() {
        assert_eq!(
            compose_prompt("review this", Some("diff\n")),
            "review this\n\ndiff"
        );
        assert_eq!(compose_prompt("", Some("\nonly stdin\n")), "only stdin");
        assert_eq!(compose_prompt("just words", None), "just words");
        assert_eq!(compose_prompt("words", Some("  \n")), "words");
    }

    #[test]
    fn headless_approval_follows_the_mode() {
        use ApprovalMode::*;
        // Read only and the default mode deny every call that asks.
        assert!(!headless_allows(ReadOnly, "write_file", true));
        assert!(!headless_allows(Default, "srv__tool", false));
        // Auto runs workspace writes, and shell commands only in the sandbox.
        assert!(headless_allows(AutoWorkspace, "edit_file", false));
        assert!(headless_allows(AutoWorkspace, "exec_shell_command", true));
        assert!(!headless_allows(AutoWorkspace, "exec_shell_command", false));
        assert!(headless_allows(AllowAll, "srv__tool", false));
    }

    #[test]
    fn provider_flags_pick_the_profile_and_model() {
        let mut settings = Settings::default();
        settings.providers.push(ProviderConfig {
            id: "second-id".into(),
            name: "Local Server".into(),
            model: "old".into(),
            ..Default::default()
        });

        pick_provider(&mut settings, Some("local server"), Some("qwen")).unwrap();
        assert_eq!(settings.active_provider, 1);
        assert_eq!(settings.provider().model, "qwen");

        pick_provider(&mut settings, Some("second-id"), None).unwrap();
        assert_eq!(settings.active_provider, 1);

        let err = pick_provider(&mut settings, Some("missing"), None).unwrap_err();
        assert!(err.contains("missing") && err.contains("Local Server"));
        pick_provider(&mut settings, None, None).unwrap();
        assert_eq!(settings.active_provider, 1);
    }

    #[test]
    fn json_output_has_the_four_keys() {
        let report = Report {
            reply: "hi".into(),
            tool_calls: vec![ToolRecord {
                name: "read_file".into(),
                arguments: r#"{"path":"a"}"#.into(),
                result: "text".into(),
                is_error: false,
            }],
            stats: None,
            error: None,
        };
        let v = report_json(&report);
        assert_eq!(v["reply"], "hi");
        assert_eq!(v["tool_calls"][0]["name"], "read_file");
        assert_eq!(v["tool_calls"][0]["arguments"], r#"{"path":"a"}"#);
        assert_eq!(v["tool_calls"][0]["result"], "text");
        assert_eq!(v["tool_calls"][0]["is_error"], false);
        assert!(v["stats"].is_null());
        assert!(v["error"].is_null());
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 4);

        let failed = Report {
            error: Some("boom".into()),
            ..Default::default()
        };
        assert_eq!(report_json(&failed)["error"], "boom");
    }

    #[test]
    fn exec_settings_default_and_load_from_older_files() {
        let loaded: Settings =
            serde_json::from_str(r#"{"general": {"system_prompt": "x"}}"#).unwrap();
        assert_eq!(loaded.general.exec, ExecSettings::default());
        assert!(loaded.general.exec.enabled);
        assert_eq!(loaded.general.exec.approval_mode, ApprovalMode::ReadOnly);
        assert_eq!(loaded.general.exec.mcp_wait_secs, 10);

        let mut settings = Settings::default();
        settings.general.exec.mcp_wait_secs = 500;
        settings.normalize();
        assert_eq!(settings.general.exec.mcp_wait_secs, MAX_MCP_WAIT_SECS);
    }

    // ------------------------------------------------------------ end to end

    /// Collects what a run prints.
    #[derive(Default)]
    struct Collect {
        text: String,
        notes: Vec<String>,
    }

    impl Sink for Collect {
        fn reply_text(&mut self, text: &str) {
            self.text.push_str(text);
        }
        fn note(&mut self, line: &str) {
            self.notes.push(line.to_owned());
        }
    }

    /// A mock OpenAI-compatible server: it answers `first` as its first reply, and `text`
    /// once a tool result is in the request (or for a plain prompt, every time).
    async fn mock(first: Option<(&'static str, &'static str)>, text: &'static str) -> String {
        let (base, _) = serve(move |req| {
            let body = req.json();
            let has_tool_result = body["messages"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
            match first {
                Some((name, args)) if !has_tool_result => Response::json(serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [
                        {"id": "call_1", "type": "function", "function": {"name": name, "arguments": args}}
                    ]}, "finish_reason": "tool_calls"}]
                })),
                _ => Response::json(serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": text},
                                 "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 4}
                })),
            }
        })
        .await;
        base
    }

    /// Default settings pointed at the mock, with `workspace` as the chat's folder.
    fn request_for(
        base: &str,
        mode: ApprovalMode,
        workspace: Option<PathBuf>,
    ) -> (AgentRequest, Conversation) {
        let mut settings = Settings::default();
        let provider = settings.provider_mut();
        provider.base_url = format!("{base}/v1");
        provider.model = "mock".into();
        provider.stream = false;
        let mut conv = Conversation::new();
        conv.permissions.mode = mode;
        conv.workspace = workspace;
        conv.messages.push(ChatMessage::new(Role::User, "go"));
        let req = AgentRequest::for_chat(&settings, &conv, None, false, false, None);
        (req, conv)
    }

    fn temp_folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cleanrstai-exec-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_prompt_gets_the_reply_text() {
        let base = mock(None, "Hello from the mock.").await;
        let (req, mut conv) = request_for(&base, ApprovalMode::ReadOnly, None);
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let mut sink = Collect::default();
        let report = drive(req, http, mcp, &mut conv, &mut sink).await;

        assert_eq!(report.reply, "Hello from the mock.");
        assert_eq!(sink.text, report.reply);
        assert_eq!(report.error, None);
        assert!(report.tool_calls.is_empty());
        assert!(report.stats.is_some());
        // The conversation keeps the prompt and the answer, ready to save.
        assert_eq!(conv.messages.len(), 2);
        assert_eq!(conv.messages[1].content, "Hello from the mock.");
    }

    #[test]
    fn headless_runs_ignore_auto_approve() {
        let mut settings = crate::config::Settings::default();
        settings.general.auto_approve_tools = true;
        let conv = Conversation::with_permission_mode(ApprovalMode::ReadOnly);
        let req = headless(AgentRequest::for_chat(
            &settings, &conv, None, false, false, None,
        ));
        assert!(!req.general.auto_approve_tools);
    }

    #[test]
    fn plan_mode_request_offers_no_writes() {
        let dir = temp_folder();
        let (req, _) = request_for(
            "http://127.0.0.1:9",
            parse_mode("plan").unwrap(),
            Some(dir.clone()),
        );
        assert_eq!(req.approval_mode, ApprovalMode::Plan);
        assert!(
            !req.selection
                .builtin_on(crate::tools::workspace::WRITE_FILE)
        );
        assert!(!req.selection.builtin_on(crate::tools::workspace::SHELL));
        assert!(req.selection.builtin_on(crate::tools::workspace::READ_FILE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_write_in_read_only_mode_is_denied() {
        let dir = temp_folder();
        let args = r#"{"path": "made.txt", "content": "hi"}"#;
        let base = mock(Some((crate::tools::workspace::WRITE_FILE, args)), "Done.").await;
        let (req, mut conv) = request_for(&base, ApprovalMode::ReadOnly, Some(dir.clone()));
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let mut sink = Collect::default();
        let report = drive(req, http, mcp, &mut conv, &mut sink).await;

        assert!(!dir.join("made.txt").exists(), "the write must not happen");
        assert!(
            sink.notes
                .contains(&"denied: write_file (use --mode to allow)".to_owned()),
            "{:?}",
            sink.notes
        );
        assert_eq!(report.tool_calls.len(), 1);
        assert!(report.tool_calls[0].is_error);
        assert_eq!(report.tool_calls[0].name, "write_file");
        assert_eq!(report.reply, "Done.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn allow_all_runs_the_write() {
        let dir = temp_folder();
        let args = r#"{"path": "made.txt", "content": "hi"}"#;
        let base = mock(Some((crate::tools::workspace::WRITE_FILE, args)), "Done.").await;
        let (req, mut conv) = request_for(&base, ApprovalMode::AllowAll, Some(dir.clone()));
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let mut sink = Collect::default();
        let report = drive(req, http, mcp, &mut conv, &mut sink).await;

        assert_eq!(std::fs::read_to_string(dir.join("made.txt")).unwrap(), "hi");
        assert!(!sink.notes.iter().any(|n| n.starts_with("denied")));
        assert!(!report.tool_calls[0].is_error);
        assert!(sink.notes.contains(&"→ write_file".to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

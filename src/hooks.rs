//! Hooks: commands from Settings that run around the model's tool calls and when a reply
//! finishes. They come only from Settings, never from the workspace or the model.
//!
//! - `before_tool` runs before a call (after its approval). A non-zero exit or a timeout
//!   blocks the call: its result is an error with the hook's output.
//! - `after_tool` runs after a call. Its output is appended to the result the model sees.
//! - `reply_done` runs once a reply finishes without an error. Its output is a notice.
//!
//! A hook runs through the platform shell (`sh -c`, `cmd /C` on Windows) in the chat's
//! workspace folder, or in the chat's sandbox container when the workspace's commands run
//! there. A chat without a workspace runs them in the home folder.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use uuid::Uuid;

use crate::sandbox::Place;
use crate::tools::workspace::{kill_command, shell_command};

pub const DEFAULT_TIMEOUT_SECS: u32 = 30;
pub const MAX_TIMEOUT_SECS: u32 = 600;
/// Most output kept from one hook run (stdout and stderr together).
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
/// Most of a tool's result given to a hook in CLEANRSTAI_TOOL_RESULT.
const MAX_RESULT_BYTES: usize = 64 * 1024;
/// Most of a call's arguments given in CLEANRSTAI_TOOL_ARGS. Linux refuses to start a process
/// with one environment string over 128 KiB, and a large file write would be one.
const MAX_ARGS_BYTES: usize = 32 * 1024;
const CUT_NOTE: &str = "\n[… cut]";

/// When a hook runs.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Before a tool call runs, after its approval. A failure blocks the call.
    #[default]
    BeforeTool,
    /// After a tool call. Its output is added to the result.
    AfterTool,
    /// When a reply finishes without an error. Its output is shown as a notice.
    ReplyDone,
}

impl HookEvent {
    pub const ALL: [HookEvent; 3] = [Self::BeforeTool, Self::AfterTool, Self::ReplyDone];

    /// The value of `CLEANRSTAI_EVENT`.
    pub fn key(self) -> &'static str {
        match self {
            Self::BeforeTool => "before_tool",
            Self::AfterTool => "after_tool",
            Self::ReplyDone => "reply_done",
        }
    }

    /// The event's name in Settings.
    pub fn label(self) -> &'static str {
        match self {
            Self::BeforeTool => "Before a tool runs",
            Self::AfterTool => "After a tool runs",
            Self::ReplyDone => "When a reply is done",
        }
    }
}

/// One command from Settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Hook {
    /// Shown in blocked results and notices.
    pub name: String,
    pub event: HookEvent,
    /// Comma-separated glob patterns matched against the routed tool name, e.g.
    /// `write_file, edit_file, github__*`. Empty matches every tool. Ignored for `reply_done`.
    pub tools: String,
    /// Run with the platform shell.
    pub command: String,
    pub enabled: bool,
    /// Killed after this many seconds (1–600).
    pub timeout_secs: u32,
}

impl Default for Hook {
    fn default() -> Self {
        Self {
            name: String::new(),
            event: HookEvent::default(),
            tools: String::new(),
            command: String::new(),
            enabled: true,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// The hooks from Settings, and the master switch (a chat's switch defaults to it).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HookSettings {
    pub enabled: bool,
    pub hooks: Vec<Hook>,
}

/// Tidies hooks read from Settings: names and tool patterns are trimmed (a blank name becomes
/// "hook N"), timeouts are clamped, and hooks without a command are dropped.
pub fn normalize(settings: &mut HookSettings) {
    settings.hooks.retain(|h| !h.command.trim().is_empty());
    for (i, hook) in settings.hooks.iter_mut().enumerate() {
        hook.name = hook.name.trim().to_owned();
        if hook.name.is_empty() {
            hook.name = format!("hook {}", i + 1);
        }
        hook.tools = hook.tools.trim().to_owned();
        hook.timeout_secs = hook.timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    }
}

/// Whether `tool` matches the comma-separated glob `patterns` (`write_file, github__*`).
/// Empty patterns match every tool. A pattern that isn't a valid glob matches only itself.
pub fn matches(patterns: &str, tool: &str) -> bool {
    let mut any = false;
    for pattern in patterns.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        any = true;
        let hit = match globset::Glob::new(pattern) {
            Ok(glob) => glob.compile_matcher().is_match(tool),
            Err(_) => pattern == tool,
        };
        if hit {
            return true;
        }
    }
    !any
}

/// What a hook is run for.
struct Call<'a> {
    event: HookEvent,
    tool: &'a str,
    args: &'a str,
    /// The tool's result, for an after-tool hook.
    result: Option<&'a str>,
}

/// The hooks one chat runs, and where. Built once per run by `agent::run`.
pub struct Runner {
    hooks: Vec<Hook>,
    chat: Uuid,
    /// The chat's workspace folder: where hooks run, and CLEANRSTAI_WORKSPACE.
    workspace: Option<PathBuf>,
    /// The chat's sandbox container, when the workspace's commands run in it.
    place: Option<Place>,
}

/// How one hook run ended.
struct Outcome {
    /// Exit status zero, in time.
    ok: bool,
    /// Output (stdout then stderr, capped), or why it failed. Empty when it passed silently.
    output: String,
}

impl Outcome {
    fn failed(output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
        }
    }

    /// A finished run: a failure with no output says its exit code.
    fn finished(ok: bool, output: String, code: Option<i32>) -> Self {
        let output = if output.is_empty() && !ok {
            match code {
                Some(code) => format!("exit code {code}"),
                None => "killed by a signal".to_owned(),
            }
        } else {
            output
        };
        Self { ok, output }
    }
}

impl Runner {
    /// The enabled hooks of `settings`, when `enabled` (the chat's switch) is on.
    pub fn new(
        settings: &HookSettings,
        enabled: bool,
        chat: Uuid,
        workspace: Option<PathBuf>,
        place: Option<Place>,
    ) -> Self {
        let hooks = if enabled {
            settings
                .hooks
                .iter()
                .filter(|h| h.enabled && !h.command.trim().is_empty())
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Self {
            hooks,
            chat,
            workspace,
            place,
        }
    }

    fn matching<'a>(
        &'a self,
        event: HookEvent,
        tool: &'a str,
    ) -> impl Iterator<Item = &'a Hook> + 'a {
        self.hooks.iter().filter(move |h| {
            h.event == event && (event == HookEvent::ReplyDone || matches(&h.tools, tool))
        })
    }

    /// Runs the before-tool hooks for a call to `tool`. `Some` is the error the call returns
    /// instead of running: the first hook that failed, with its output.
    pub async fn before_tool(&self, tool: &str, args: &str) -> Option<String> {
        for hook in self.matching(HookEvent::BeforeTool, tool) {
            let call = Call {
                event: HookEvent::BeforeTool,
                tool,
                args,
                result: None,
            };
            let outcome = self.run(hook, &call).await;
            if !outcome.ok {
                return Some(format!(
                    "Blocked by hook \"{}\": {}",
                    hook.name, outcome.output
                ));
            }
        }
        None
    }

    /// Runs the after-tool hooks for a call to `tool`, which gave `result`. Each hook's output
    /// comes back under its name, to append to the result.
    pub async fn after_tool(&self, tool: &str, args: &str, result: &str) -> String {
        let mut text = String::new();
        for hook in self.matching(HookEvent::AfterTool, tool) {
            let call = Call {
                event: HookEvent::AfterTool,
                tool,
                args,
                result: Some(result),
            };
            let outcome = self.run(hook, &call).await;
            if !outcome.output.is_empty() {
                text.push_str(&format!("\n\n[hook {}]\n{}", hook.name, outcome.output));
            }
        }
        text
    }

    /// Runs the reply-done hooks. Each one that printed something (or failed) gives a notice.
    pub async fn reply_done(&self) -> Vec<String> {
        let mut notices = Vec::new();
        for hook in self.matching(HookEvent::ReplyDone, "") {
            let call = Call {
                event: HookEvent::ReplyDone,
                tool: "",
                args: "",
                result: None,
            };
            let outcome = self.run(hook, &call).await;
            if !outcome.output.is_empty() {
                notices.push(format!("Hook \"{}\": {}", hook.name, outcome.output));
            }
        }
        notices
    }

    async fn run(&self, hook: &Hook, call: &Call<'_>) -> Outcome {
        let timeout = Duration::from_secs(u64::from(hook.timeout_secs.clamp(1, MAX_TIMEOUT_SECS)));
        match (&self.place, &self.workspace) {
            (Some(place), Some(root)) => {
                let env = env_vars(call, self.chat, "/workspace");
                run_in_sandbox(place, root, &hook.command, &env, timeout).await
            }
            _ => {
                let dir = self.workspace.clone().or_else(home_dir);
                let Some(dir) = dir else {
                    return Outcome::failed("there is no folder to run it in");
                };
                let workspace = self
                    .workspace
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let env = env_vars(call, self.chat, &workspace);
                run_on_host(&hook.command, &dir, &env, timeout).await
            }
        }
    }
}

/// The CLEANRSTAI_* variables a hook runs with.
fn env_vars(call: &Call<'_>, chat: Uuid, workspace: &str) -> Vec<(String, String)> {
    let mut vars = vec![
        ("CLEANRSTAI_EVENT", call.event.key().to_owned()),
        ("CLEANRSTAI_TOOL", env_value(call.tool, usize::MAX)),
        ("CLEANRSTAI_TOOL_ARGS", env_value(call.args, MAX_ARGS_BYTES)),
        ("CLEANRSTAI_CHAT_ID", chat.to_string()),
        ("CLEANRSTAI_WORKSPACE", env_value(workspace, usize::MAX)),
    ];
    if let Some(result) = call.result {
        vars.push((
            "CLEANRSTAI_TOOL_RESULT",
            env_value(result, MAX_RESULT_BYTES),
        ));
    }
    vars.into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// A value for an environment variable: NUL bytes (which can't be passed) dropped, and cut to
/// `max` bytes.
fn env_value(text: &str, max: usize) -> String {
    cut(&text.replace('\0', ""), max)
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character boundary.
fn prefix(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text`, or its start cut so that the result, with a note, is at most `max` bytes.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    format!(
        "{}{CUT_NOTE}",
        prefix(text, max.saturating_sub(CUT_NOTE.len()))
    )
}

/// One output stream: its bytes, and whether some were dropped before them.
type Stream<'a> = (&'a [u8], bool);

/// The output streams as one text (stdout first), trimmed and capped. A stream that dropped
/// bytes is marked as cut, even when what was kept is short.
fn output_text(streams: &[Stream<'_>]) -> String {
    let text = streams
        .iter()
        .map(|(bytes, _)| String::from_utf8_lossy(bytes).trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if streams.iter().any(|(_, dropped)| *dropped) {
        format!(
            "{}{CUT_NOTE}",
            prefix(&text, MAX_OUTPUT_BYTES.saturating_sub(CUT_NOTE.len()))
        )
    } else {
        cut(&text, MAX_OUTPUT_BYTES)
    }
}

/// Reads a stream to its end, keeping at most `MAX_OUTPUT_BYTES` and whether it dropped any.
/// The rest is read and dropped, so the process never blocks on a full pipe.
async fn read_capped<R: AsyncRead + Unpin>(stream: Option<R>) -> (Vec<u8>, bool) {
    let Some(mut stream) = stream else {
        return (Vec::new(), false);
    };
    let mut kept = Vec::new();
    let mut dropped = false;
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_OUTPUT_BYTES.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
                dropped |= n > room;
            }
        }
    }
    (kept, dropped)
}

fn home_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Runs `command` through the platform shell in `dir`, killed after `timeout`.
async fn run_on_host(
    command: &str,
    dir: &Path,
    env: &[(String, String)],
    timeout: Duration,
) -> Outcome {
    let mut cmd = shell_command(command);
    cmd.current_dir(dir)
        .envs(
            env.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so a timeout stops everything the hook started.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Outcome::failed(format!("couldn't start the hook: {e}")),
    };
    let pid = child.id();
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let finished = async {
        let (out, err) = tokio::join!(read_capped(stdout), read_capped(stderr));
        (out, err, child.wait().await)
    };
    match tokio::time::timeout(timeout, finished).await {
        Ok((out, err, status)) => {
            let code = status.as_ref().ok().and_then(|s| s.code());
            let ok = status.is_ok_and(|s| s.success());
            let output = output_text(&[(&out.0, out.1), (&err.0, err.1)]);
            Outcome::finished(ok, output, code)
        }
        Err(_) => {
            if let Some(pid) = pid {
                kill_command(pid);
            }
            Outcome::failed(format!("timed out after {} s", timeout.as_secs()))
        }
    }
}

/// Runs `command` in the chat's sandbox container, with `root` at /workspace.
async fn run_in_sandbox(
    place: &Place,
    root: &Path,
    command: &str,
    env: &[(String, String)],
    timeout: Duration,
) -> Outcome {
    match place.exec_env(root, command, env, timeout).await {
        Err(e) => Outcome::failed(format!("couldn't run the hook in the sandbox: {e}")),
        Ok(run) if run.timed_out => Outcome::failed(format!(
            "timed out after {} s; its container was stopped and removed",
            timeout.as_secs()
        )),
        Ok(run) => {
            let ok = run.exit_code == Some(0);
            let output = output_text(&[
                (&run.stdout, run.stdout_cut > 0),
                (&run.stderr, run.stderr_cut > 0),
            ]);
            Outcome::finished(ok, output, run.exit_code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GeneralSettings, Settings};

    fn hook(name: &str, event: HookEvent, tools: &str, command: &str) -> Hook {
        Hook {
            name: name.into(),
            event,
            tools: tools.into(),
            command: command.into(),
            ..Default::default()
        }
    }

    #[test]
    fn tool_patterns_are_comma_separated_globs() {
        assert!(matches("", "anything"));
        assert!(matches(" , ", "anything"));
        assert!(matches("write_file, edit_file", "edit_file"));
        assert!(!matches("write_file, edit_file", "read_file"));
        assert!(matches("github__*", "github__create_issue"));
        assert!(!matches("github__*", "fetch"));
        assert!(matches("read_*,write_file", "write_file"));
        assert!(!matches("read_*", "write_file"));
        // A pattern that isn't a valid glob matches only itself.
        assert!(matches("a[", "a["));
        assert!(!matches("a[", "ab"));
    }

    #[test]
    fn env_carries_the_call_and_caps_its_text() {
        let chat = Uuid::nil();
        let call = Call {
            event: HookEvent::AfterTool,
            tool: "write_file",
            args: "{\"path\":\"a\"}",
            result: Some("done"),
        };
        let env = env_vars(&call, chat, "/home/me/project");
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("CLEANRSTAI_EVENT"), Some("after_tool"));
        assert_eq!(get("CLEANRSTAI_TOOL"), Some("write_file"));
        assert_eq!(get("CLEANRSTAI_TOOL_ARGS"), Some("{\"path\":\"a\"}"));
        assert_eq!(get("CLEANRSTAI_TOOL_RESULT"), Some("done"));
        assert_eq!(get("CLEANRSTAI_CHAT_ID"), Some(chat.to_string().as_str()));
        assert_eq!(get("CLEANRSTAI_WORKSPACE"), Some("/home/me/project"));

        let before = Call {
            event: HookEvent::BeforeTool,
            tool: "edit_file",
            args: "{}",
            result: None,
        };
        let env = env_vars(&before, chat, "");
        assert!(env.iter().all(|(k, _)| k != "CLEANRSTAI_TOOL_RESULT"));

        let big = "x".repeat(MAX_RESULT_BYTES * 2);
        let env = env_vars(
            &Call {
                result: Some(big.as_str()),
                ..before
            },
            chat,
            "",
        );
        let result = &env
            .iter()
            .find(|(k, _)| k == "CLEANRSTAI_TOOL_RESULT")
            .unwrap()
            .1;
        assert!(result.len() <= MAX_RESULT_BYTES);
        assert!(result.ends_with("[… cut]"));
    }

    #[test]
    fn cut_keeps_char_boundaries_and_the_limit() {
        let text = "é".repeat(100);
        let short = cut(&text, 50);
        assert!(short.len() <= 50, "{}", short.len());
        assert!(short.ends_with("[… cut]"));
        assert_eq!(cut("short", 50), "short");
        assert_eq!(env_value("a\0b", 10), "ab");
    }

    #[test]
    fn hooks_load_from_older_files_and_are_kept_tidy() {
        let old: GeneralSettings = serde_json::from_str(r#"{"max_tool_rounds": 5}"#).unwrap();
        assert_eq!(old.hooks, HookSettings::default());
        assert!(!old.hooks.enabled);

        let partial: Hook = serde_json::from_str(r#"{"command": "cargo fmt"}"#).unwrap();
        assert!(partial.enabled);
        assert_eq!(partial.event, HookEvent::BeforeTool);
        assert_eq!(partial.timeout_secs, DEFAULT_TIMEOUT_SECS);

        let event: HookEvent = serde_json::from_str("\"after_tool\"").unwrap();
        assert_eq!(event, HookEvent::AfterTool);

        let mut settings = HookSettings {
            enabled: true,
            hooks: vec![
                Hook {
                    name: "  ".into(),
                    timeout_secs: 0,
                    tools: " write_file ".into(),
                    ..hook("", HookEvent::AfterTool, "", "cargo fmt")
                },
                hook("blank", HookEvent::BeforeTool, "", "  "),
                Hook {
                    timeout_secs: 9_999,
                    ..hook("big", HookEvent::ReplyDone, "", "true")
                },
            ],
        };
        normalize(&mut settings);
        assert_eq!(settings.hooks.len(), 2);
        assert_eq!(settings.hooks[0].name, "hook 1");
        assert_eq!(settings.hooks[0].timeout_secs, 1);
        assert_eq!(settings.hooks[0].tools, "write_file");
        assert_eq!(settings.hooks[1].timeout_secs, MAX_TIMEOUT_SECS);
    }

    #[test]
    fn the_chat_switch_turns_all_hooks_off() {
        let settings = HookSettings {
            enabled: true,
            hooks: vec![
                hook("on", HookEvent::BeforeTool, "", "true"),
                Hook {
                    enabled: false,
                    ..hook("off", HookEvent::BeforeTool, "", "true")
                },
            ],
        };
        let id = Uuid::nil();
        assert_eq!(Runner::new(&settings, true, id, None, None).hooks.len(), 1);
        assert!(
            Runner::new(&settings, false, id, None, None)
                .hooks
                .is_empty()
        );
        // Settings' default is off, so a chat with no switch runs none.
        assert!(!Settings::default().general.hooks.enabled);
    }

    #[cfg(unix)]
    fn runner(hooks: Vec<Hook>, dir: &Path) -> Runner {
        let settings = HookSettings {
            enabled: true,
            hooks,
        };
        Runner::new(&settings, true, Uuid::nil(), Some(dir.to_path_buf()), None)
    }

    #[cfg(unix)]
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cleanrstai-hooks-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_before_hook_blocks_only_the_tools_it_names() {
        let dir = temp_dir();
        let r = runner(
            vec![hook(
                "guard",
                HookEvent::BeforeTool,
                "write_file",
                "echo nope; exit 3",
            )],
            &dir,
        );
        let blocked = r.before_tool("write_file", "{}").await.unwrap();
        assert_eq!(blocked, "Blocked by hook \"guard\": nope");
        assert_eq!(r.before_tool("read_file", "{}").await, None);

        let silent = runner(
            vec![hook("guard", HookEvent::BeforeTool, "", "exit 3")],
            &dir,
        );
        assert_eq!(
            silent.before_tool("read_file", "{}").await.unwrap(),
            "Blocked by hook \"guard\": exit code 3"
        );

        let passing = runner(vec![hook("ok", HookEvent::BeforeTool, "", "true")], &dir);
        assert_eq!(passing.before_tool("write_file", "{}").await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn after_hook_output_is_appended_and_failures_too() {
        let dir = temp_dir();
        let r = runner(
            vec![
                hook("fmt", HookEvent::AfterTool, "write_file", "echo formatted"),
                hook("lint", HookEvent::AfterTool, "", "echo boom >&2; exit 1"),
                hook("quiet", HookEvent::AfterTool, "", "true"),
                hook("other", HookEvent::AfterTool, "edit_file", "echo never"),
            ],
            &dir,
        );
        let text = r.after_tool("write_file", "{}", "wrote").await;
        assert_eq!(text, "\n\n[hook fmt]\nformatted\n\n[hook lint]\nboom");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hooks_run_in_the_workspace_with_the_call_in_their_environment() {
        let dir = temp_dir();
        let r = runner(
            vec![hook(
                "env",
                HookEvent::AfterTool,
                "",
                "printf '%s|%s|%s|%s' \"$CLEANRSTAI_EVENT\" \"$CLEANRSTAI_TOOL\" \"$CLEANRSTAI_TOOL_RESULT\" \"$CLEANRSTAI_TOOL_ARGS\"; echo; pwd -P",
            )],
            &dir,
        );
        let text = r.after_tool("edit_file", "{\"a\":1}", "ok").await;
        let expected = format!("after_tool|edit_file|ok|{{\"a\":1}}\n{}", dir.display());
        assert_eq!(text, format!("\n\n[hook env]\n{expected}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_runs_too_long_is_stopped_and_blocks() {
        let dir = temp_dir();
        let mut slow = hook("slow", HookEvent::BeforeTool, "", "sleep 30");
        slow.timeout_secs = 1;
        let r = runner(vec![slow], &dir);
        let started = std::time::Instant::now();
        let blocked = r.before_tool("write_file", "{}").await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(blocked, "Blocked by hook \"slow\": timed out after 1 s");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_output_is_capped() {
        let dir = temp_dir();
        let r = runner(
            vec![hook(
                "loud",
                HookEvent::AfterTool,
                "",
                "head -c 100000 /dev/zero | tr '\\0' a",
            )],
            &dir,
        );
        let text = r.after_tool("read_file", "{}", "").await;
        assert!(text.len() <= MAX_OUTPUT_BYTES + "\n\n[hook loud]\n".len());
        assert!(text.contains("[… cut]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reply_done_hooks_give_notices_for_their_output() {
        let dir = temp_dir();
        let r = runner(
            vec![
                hook("say", HookEvent::ReplyDone, "write_file", "echo finished"),
                hook("silent", HookEvent::ReplyDone, "", "true"),
                hook("broken", HookEvent::ReplyDone, "", "exit 2"),
                hook("tool", HookEvent::BeforeTool, "", "echo not for replies"),
            ],
            &dir,
        );
        // Tool patterns don't apply to a reply.
        assert_eq!(
            r.reply_done().await,
            ["Hook \"say\": finished", "Hook \"broken\": exit code 2"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

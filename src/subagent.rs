//! Subagents: the `delegate` tool hands independent tasks to agents that each run their own
//! model/tool loop, in parallel, and report back to the main conversation.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

use crate::config::{ProviderConfig, Settings, SubagentSettings};
use crate::llm;
use crate::mcp::ToolOutput;
use crate::mcp::manager::{McpManager, ToolRouter};
use crate::stats::RequestStats;
use crate::tools::{self, ToolOverrides, ToolSelection, Toolbox};
use crate::types::{ChatMessage, Role, ToolCall};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubagentState {
    #[default]
    Queued,
    Running,
    Done,
    Failed,
}

/// One subagent's task and outcome, shown in the chat and saved with the tool result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SubagentReport {
    pub task: String,
    /// Provider profile name.
    pub provider: String,
    pub model: String,
    pub state: SubagentState,
    /// What it is doing right now; only meaningful while running.
    #[serde(skip)]
    pub activity: String,
    /// The final answer, or the error.
    pub result: String,
    /// Tools it called, by display name.
    pub tools_used: Vec<String>,
    /// One entry per model request.
    pub stats: Vec<RequestStats>,
}

#[derive(Deserialize)]
struct TaskSpec {
    task: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Applies a change to the shared reports and passes the new state to the UI.
type Publish<'a> = dyn Fn(&dyn Fn(&mut Vec<SubagentReport>)) + Sync + 'a;

/// Everything a `delegate` call needs, captured when the chat request starts.
#[derive(Clone)]
pub struct Delegation {
    pub settings: SubagentSettings,
    providers: Vec<ProviderConfig>,
    default_provider: ProviderConfig,
    /// Tools subagents may use: derived from the chat's selection.
    selection: ToolSelection,
    tool_timeout: Duration,
}

impl Delegation {
    /// `None` when subagents are switched off for this chat.
    pub fn new(settings: &Settings, chat: &ToolSelection) -> Option<Self> {
        if !chat.builtin_on("subagents") {
            return None;
        }
        let sa = &settings.subagents;
        // Subagents never get the task list or `delegate` (no recursion); the time and
        // calculator tools only when allowed for subagents and on in this chat.
        let mut selection = chat.clone();
        let force = |sel: &mut ToolSelection, key: &str, on: bool| {
            sel.overrides.0.insert(ToolOverrides::builtin_key(key), on);
        };
        for key in ["time", "calculator"] {
            let on = sa.builtin_tools && chat.builtin_on(key);
            force(&mut selection, key, on);
        }
        force(&mut selection, "tasks", false);
        force(&mut selection, "subagents", false);
        for id in &sa.excluded_servers {
            selection
                .overrides
                .0
                .insert(ToolOverrides::server_key(id), false);
        }
        Some(Self {
            settings: sa.clone(),
            providers: settings.providers.clone(),
            default_provider: settings.subagent_provider(),
            selection,
            tool_timeout: Duration::from_secs(settings.general.tool_timeout_secs.max(5)),
        })
    }

    #[cfg(test)]
    pub fn set_default_provider(&mut self, p: ProviderConfig) {
        self.default_provider = p;
    }

    pub fn tool_description(&self) -> String {
        let sa = &self.settings;
        let mut abilities = Vec::new();
        if self.selection.builtin_on("time") || self.selection.builtin_on("calculator") {
            abilities.push("the time and calculator tools");
        }
        if sa.mcp_tools {
            abilities.push("this chat's MCP tools");
        }
        let abilities = if abilities.is_empty() {
            "no tools".to_owned()
        } else {
            abilities.join(" and ")
        };
        let mut d = format!(
            "Delegate tasks to subagents: independent AI agents that each work on one task on \
             their own and report back with the result. Use this to split a large request into \
             independent parts that can run in parallel, such as researching several topics or \
             drafting several sections, then combine their results. Subagents can use {abilities}. \
             They cannot see this conversation or ask questions, so make every task \
             self-contained. At most {} tasks per call; {} run at a time.",
            sa.max_tasks, sa.max_parallel
        );
        if sa.allow_provider_choice && self.providers.len() > 1 {
            let list: Vec<String> = self
                .providers
                .iter()
                .map(|p| {
                    if p.model.trim().is_empty() {
                        format!("\"{}\"", p.name)
                    } else {
                        format!("\"{}\" (model {})", p.name, p.model.trim())
                    }
                })
                .collect();
            d.push_str(&format!(
                " By default tasks run on \"{}\" with model {}. To run a task on another \
                 provider, set its `provider` to one of: {}. For example, send simple tasks to a \
                 fast or local model.",
                self.default_provider.name,
                self.default_provider.model.trim(),
                list.join(", ")
            ));
        }
        d
    }

    /// The provider and model for one task.
    fn resolve(&self, provider: Option<&str>, model: Option<&str>) -> Result<ProviderConfig> {
        let choose = self.settings.allow_provider_choice;
        let mut p = match provider.map(str::trim).filter(|s| !s.is_empty()) {
            Some(name) if choose => self
                .providers
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name) || p.id == name)
                .cloned()
                .with_context(|| {
                    let names: Vec<&str> = self.providers.iter().map(|p| p.name.as_str()).collect();
                    format!(
                        "unknown provider '{name}' (available: {})",
                        names.join(", ")
                    )
                })?,
            _ => self.default_provider.clone(),
        };
        if choose && let Some(m) = model.map(str::trim).filter(|s| !s.is_empty()) {
            p.model = m.to_owned();
        }
        Ok(p)
    }

    /// Run a `delegate` call. `update` receives every progress change, for the UI.
    pub async fn run(
        &self,
        http: &reqwest::Client,
        mcp: &McpManager,
        arguments: &str,
        update: &(dyn Fn(&[SubagentReport]) + Send + Sync),
    ) -> (ToolOutput, Vec<SubagentReport>) {
        let specs = match parse_tasks(arguments, self.settings.max_tasks as usize) {
            Ok(s) => s,
            Err(e) => return (tools::error_output(format!("Error: {e:#}")), Vec::new()),
        };
        let reports = Mutex::new(
            specs
                .iter()
                .map(|s| SubagentReport {
                    task: s.task.trim().to_owned(),
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        );
        let publish = |change: &dyn Fn(&mut Vec<SubagentReport>)| {
            let snapshot = {
                let mut r = reports.lock().unwrap();
                change(&mut r);
                r.clone()
            };
            update(&snapshot);
        };
        publish(&|_| {});

        let parallel = self.settings.max_parallel.max(1) as usize;
        futures_util::stream::iter(0..specs.len())
            .map(|i| self.run_one(http, mcp, i, &specs[i], &publish))
            .buffer_unordered(parallel)
            .collect::<Vec<()>>()
            .await;

        let reports = reports.into_inner().unwrap();
        (summarize(&reports), reports)
    }

    async fn run_one(
        &self,
        http: &reqwest::Client,
        mcp: &McpManager,
        i: usize,
        spec: &TaskSpec,
        publish: &Publish<'_>,
    ) {
        let provider = match self.resolve(spec.provider.as_deref(), spec.model.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                publish(&|r| {
                    r[i].state = SubagentState::Failed;
                    r[i].result = format!("{e:#}");
                });
                return;
            }
        };
        publish(&|r| {
            r[i].provider = provider.name.clone();
            r[i].model = provider.model.trim().to_owned();
            r[i].state = SubagentState::Running;
            r[i].activity = "Starting…".into();
        });
        let timeout = Duration::from_secs(self.settings.timeout_secs.max(1));
        let outcome = tokio::time::timeout(
            timeout,
            self.agent_loop(http, mcp, i, &spec.task, &provider, publish),
        )
        .await
        .unwrap_or_else(|_| bail!("timed out after {} s", timeout.as_secs()));
        publish(&|r| {
            r[i].activity.clear();
            match &outcome {
                Ok(text) => {
                    r[i].state = SubagentState::Done;
                    r[i].result = text.clone();
                }
                Err(e) => {
                    r[i].state = SubagentState::Failed;
                    r[i].result = format!("{e:#}");
                }
            }
        });
    }

    async fn agent_loop(
        &self,
        http: &reqwest::Client,
        mcp: &McpManager,
        i: usize,
        task: &str,
        provider: &ProviderConfig,
        publish: &Publish<'_>,
    ) -> Result<String> {
        let mut history = vec![ChatMessage::new(Role::User, task.trim())];
        let max_rounds = self.settings.max_rounds.max(1);
        for round in 0.. {
            let toolbox = if round < max_rounds {
                let router = if self.settings.mcp_tools {
                    mcp.router()
                } else {
                    ToolRouter::default()
                };
                Toolbox::new(router, &self.selection, None)
            } else {
                Toolbox::default()
            };
            publish(&|r| r[i].activity = "Thinking…".into());
            let messages = llm::to_api_messages(&self.settings.system_prompt, &history);
            let turn =
                llm::chat_completion(http, provider, messages, &toolbox.specs, |_| {}).await?;
            publish(&|r| r[i].stats.push(turn.stats.clone()));

            let mut msg = ChatMessage::new(Role::Assistant, turn.content.clone());
            msg.tool_calls = turn.tool_calls.clone();
            history.push(msg);
            if turn.tool_calls.is_empty() {
                if turn.content.trim().is_empty() {
                    bail!("the model returned an empty answer");
                }
                return Ok(turn.content);
            }
            if round >= max_rounds {
                bail!("stopped after {max_rounds} rounds of tool calls");
            }
            for call in &turn.tool_calls {
                let name = tools::display_name(&call.name);
                publish(&|r| {
                    r[i].activity = format!("Using {name}…");
                    r[i].tools_used.push(name.clone());
                });
                let out = toolbox
                    .call(&call.name, &call.arguments, self.tool_timeout)
                    .await;
                history.push(tool_message(call, out));
            }
        }
        unreachable!()
    }
}

fn tool_message(call: &ToolCall, out: ToolOutput) -> ChatMessage {
    let mut msg = ChatMessage::tool_result(call, out.text, out.is_error);
    msg.attachments = out.images;
    msg
}

fn parse_tasks(arguments: &str, max: usize) -> Result<Vec<TaskSpec>> {
    #[derive(Deserialize)]
    struct Args {
        tasks: Vec<TaskSpec>,
    }
    let args: Args =
        serde_json::from_str(arguments).context("expected {\"tasks\": [{\"task\": \"…\"}]}")?;
    let tasks: Vec<TaskSpec> = args
        .tasks
        .into_iter()
        .filter(|t| !t.task.trim().is_empty())
        .collect();
    if tasks.is_empty() {
        bail!("no tasks given");
    }
    if tasks.len() > max {
        bail!(
            "{} tasks given, but at most {max} are allowed per call; group them or call again",
            tasks.len()
        );
    }
    Ok(tasks)
}

/// The tool result the main agent sees: each subagent's answer, in task order.
fn summarize(reports: &[SubagentReport]) -> ToolOutput {
    let mut text = String::new();
    for (i, r) in reports.iter().enumerate() {
        let status = if r.state == SubagentState::Done {
            ""
        } else {
            " (failed)"
        };
        text.push_str(&format!(
            "## Subagent {}{status}\nTask: {}\n",
            i + 1,
            r.task
        ));
        if !r.provider.is_empty() {
            text.push_str(&format!("Ran on: {} · {}\n", r.provider, r.model));
        }
        text.push('\n');
        text.push_str(r.result.trim());
        text.push_str("\n\n");
    }
    ToolOutput {
        text: text.trim_end().to_owned(),
        is_error: reports.iter().all(|r| r.state != SubagentState::Done),
        images: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};
    use serde_json::json;

    fn settings_with(base_url: &str) -> Settings {
        let mut s = Settings::default();
        s.providers[0].name = "Main".into();
        s.providers[0].base_url = base_url.into();
        s.providers[0].model = "main-model".into();
        s.providers[0].stream = false;
        let mut fast = s.providers[0].clone();
        fast.id = "fast-id".into();
        fast.name = "Fast".into();
        fast.model = "small-model".into();
        s.providers.push(fast);
        s
    }

    fn reply(content: &str) -> Response {
        Response::json(json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }))
    }

    #[test]
    fn respects_chat_selection() {
        let s = Settings::default();
        let mut ov = ToolOverrides::default();
        assert!(Delegation::new(&s, &ToolSelection::new(&s, &ov)).is_some());
        ov.set(ToolOverrides::builtin_key("subagents"), false, true);
        assert!(Delegation::new(&s, &ToolSelection::new(&s, &ov)).is_none());

        let mut s = s;
        s.subagents.excluded_servers = vec!["srv".into()];
        let d = Delegation::new(&s, &ToolSelection::new(&s, &ToolOverrides::default())).unwrap();
        assert!(!d.selection.tool_on("srv", "any"));
        assert!(d.selection.tool_on("other", "any"));
        assert!(d.selection.builtin_on("calculator"));
        assert!(!d.selection.builtin_on("tasks"));
        assert!(!d.selection.builtin_on("subagents"));
    }

    #[tokio::test]
    async fn runs_tasks_in_parallel_on_chosen_providers() {
        let (base, log) = serve(|req| {
            let body = req.json();
            let model = body["model"].as_str().unwrap_or_default().to_owned();
            let task = body["messages"]
                .as_array()
                .and_then(|m| m.iter().find(|m| m["role"] == "user"))
                .and_then(|m| m["content"].as_str())
                .unwrap_or_default()
                .to_owned();
            // The second task uses the calculator once, then answers.
            let used_tool = body["messages"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
            if task.contains("multiply") && !used_tool {
                return Response::json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [
                        {"id": "c1", "type": "function", "function": {"name": tools::CALCULATE, "arguments": "{\"expression\": \"6*7\"}"}}
                    ]}, "finish_reason": "tool_calls"}]
                }));
            }
            if task.contains("multiply") {
                let result = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["role"] == "tool")
                    .unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                return reply(&format!("{model}: {result}"));
            }
            reply(&format!("{model} did: {task}"))
        })
        .await;
        let s = settings_with(&format!("{base}/v1"));
        let d = Delegation::new(&s, &ToolSelection::new(&s, &ToolOverrides::default())).unwrap();
        assert!(
            d.tool_description()
                .contains("\"Fast\" (model small-model)")
        );

        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let updates = Mutex::new(0usize);
        let args = json!({"tasks": [
            {"task": "summarise A"},
            {"task": "multiply 6 by 7", "provider": "fast"},
            {"task": "use a missing provider", "provider": "nope"},
        ]})
        .to_string();
        let (out, reports) = d
            .run(&http, &mcp, &args, &|_| *updates.lock().unwrap() += 1)
            .await;

        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].state, SubagentState::Done);
        assert_eq!(reports[0].result, "main-model did: summarise A");
        assert_eq!(reports[0].provider, "Main");
        assert_eq!(reports[1].state, SubagentState::Done);
        assert_eq!(reports[1].result, "small-model: 6*7 = 42");
        assert_eq!(reports[1].tools_used, ["Calculator"]);
        assert_eq!(reports[1].stats.len(), 2);
        assert_eq!(reports[2].state, SubagentState::Failed);
        assert!(reports[2].result.contains("unknown provider 'nope'"));
        assert!(!out.is_error);
        assert!(out.text.contains("## Subagent 2\nTask: multiply 6 by 7"));
        assert!(out.text.contains("## Subagent 3 (failed)"));
        assert!(*updates.lock().unwrap() >= 6);
        // Subagents never see the task list or delegate tools.
        for req in log.lock().unwrap().iter() {
            let tools: Vec<String> = req.json()["tools"]
                .as_array()
                .map(|t| {
                    t.iter()
                        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                !tools
                    .iter()
                    .any(|t| t == tools::TASKS || t == tools::DELEGATE)
            );
        }
    }

    #[tokio::test]
    async fn rejects_bad_arguments() {
        let s = Settings::default();
        let mut sa = s.clone();
        sa.subagents.max_tasks = 1;
        let d = Delegation::new(&sa, &ToolSelection::new(&sa, &ToolOverrides::default())).unwrap();
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (out, reports) = d.run(&http, &mcp, "{}", &|_| {}).await;
        assert!(out.is_error && reports.is_empty());
        let two = json!({"tasks": [{"task": "a"}, {"task": "b"}]}).to_string();
        let (out, _) = d.run(&http, &mcp, &two, &|_| {}).await;
        assert!(out.text.contains("at most 1"), "{}", out.text);
    }

    #[tokio::test]
    async fn times_out() {
        // A server that accepts the request and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        let mut s = settings_with(&format!("{base}/v1"));
        s.subagents.timeout_secs = 10;
        let mut d =
            Delegation::new(&s, &ToolSelection::new(&s, &ToolOverrides::default())).unwrap();
        d.settings.timeout_secs = 1; // below the settings clamp, for a fast test
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let args = json!({"tasks": [{"task": "slow"}]}).to_string();
        let (out, reports) = d.run(&http, &mcp, &args, &|_| {}).await;
        assert!(out.is_error, "{}", out.text);
        assert!(
            reports[0].result.contains("timed out"),
            "{}",
            reports[0].result
        );
    }
}

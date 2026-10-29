//! One "generation": stream the model's answer, run any MCP tool calls it makes, feed
//! the results back, and repeat until the model answers without tools.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::compaction::{self, Compaction};
use crate::config::{GeneralSettings, ProviderConfig};
use crate::llm::{self, Delta};
use crate::mcp::manager::McpManager;
use crate::subagent::{Delegation, SubagentReport};
use crate::tools::workspace::{EditOptions, Workspace};
use crate::tools::{self, ToolSelection, Toolbox};
use crate::types::{ChatMessage, CompactionInfo, Role, ToolCall};

pub enum ChatEvent {
    /// A new, empty assistant message begins.
    AssistantStart,
    ContentDelta(String),
    ReasoningDelta(String),
    /// A live snapshot of the in-flight request's stats, for the readout above the input box.
    StatsLive(Box<crate::stats::RequestStats>),
    /// Replaces the in-progress assistant message with its final form (incl. tool calls).
    AssistantFinal(Box<ChatMessage>),
    ToolApproval {
        call: ToolCall,
        /// The MCP server id and tool name behind the call; `None` for built-ins.
        origin: Option<(String, String)>,
        /// For a workspace file write or edit: the change it would make, as a diff.
        preview: Option<String>,
        respond: oneshot::Sender<bool>,
    },
    /// The older messages are being summarised.
    Compacting,
    /// The messages before the last `kept` ones were summarised as `summary`, by a request
    /// with these stats.
    Compacted {
        kept: usize,
        summary: String,
        info: CompactionInfo,
        stats: Box<crate::stats::RequestStats>,
    },
    /// The project instruction files added to the system prompt.
    Instructions(Vec<String>),
    ToolRunning(String),
    /// Live progress of the subagents started by a `delegate` call.
    SubagentProgress {
        call_id: String,
        reports: Vec<SubagentReport>,
    },
    ToolResult(Box<ChatMessage>),
    Error(String),
    Done,
}

/// What a provider's server reported about its model, for the details beside the model
/// picker. `key` is the provider id and model.
#[derive(Default)]
pub struct ServerReport {
    pub key: (String, String),
    /// llama.cpp: the model and server, or None when it didn't answer like llama.cpp.
    pub info: Option<crate::llm::LlamaServerInfo>,
    /// A llama.cpp router's models and their load state (None for a single-model server).
    pub router: Option<Vec<crate::llm::RouterModel>>,
    /// LiteLLM: the model as the proxy lists it, and the key's spend.
    pub litellm: Option<crate::llm::LiteLlmModel>,
    pub litellm_key: Option<crate::llm::LiteLlmKey>,
    /// Ollama, LM Studio, vLLM and OpenRouter: what the server reports, as rows.
    pub details: Option<crate::servers::ServerDetails>,
}

pub enum AppEvent {
    /// A request from a browser connected to the remote access server.
    Remote(crate::remote::RemoteCommand),
    /// The working folder's diff for "Review changes", or why it couldn't be read.
    ReviewReady {
        conv_id: Uuid,
        result: Result<crate::review::ReviewDiff, String>,
    },
    /// Result of a GitHub Releases check. `explicit` when the user asked for it.
    UpdateChecked {
        result: Result<crate::update::UpdateCheck, String>,
        explicit: bool,
    },
    /// Bytes of the update downloaded so far, and the total when known.
    UpdateProgress {
        done: u64,
        total: Option<u64>,
    },
    /// The update was put in place (the path to relaunch), or why it wasn't.
    UpdateInstalled(Result<std::path::PathBuf, String>),
    /// Result of a Settings → Docker test, pull or build: a message, or why it failed.
    SandboxChecked(Result<String, String>),
    Chat {
        conv_id: Uuid,
        event: ChatEvent,
    },
    ModelsFetched {
        provider_id: String,
        result: Result<Vec<String>, String>,
    },
    /// A resource read from an MCP server, to attach to the next message.
    McpResourceRead {
        name: String,
        result: Result<Vec<crate::attach::Attachment>, String>,
    },
    /// A file read (and, for documents, its text extracted) for the next message.
    FileAttached {
        result: Result<crate::attach::Attachment, String>,
    },
    /// Messages from an MCP prompt, to insert into the chat.
    McpPromptReady {
        name: String,
        result: Result<Vec<crate::mcp::PromptMessage>, String>,
    },
    /// What the active provider's server reported (llama.cpp or LiteLLM).
    ServerInfo(Box<ServerReport>),
    /// A router load or unload finished (the error, if it failed).
    ModelLoadChanged(Result<(), String>),
    KindDetected {
        provider_id: String,
        kind: crate::config::ProviderKind,
        /// Whether the user asked (Detect button) rather than it running alongside a fetch.
        explicit: bool,
    },
}

pub struct AgentRequest {
    pub conv_id: Uuid,
    pub provider: ProviderConfig,
    pub general: GeneralSettings,
    pub history: Vec<ChatMessage>,
    /// Tools this chat may use.
    pub selection: ToolSelection,
    /// Subagent setup, when the `delegate` tool is on for this chat.
    pub delegation: Option<Delegation>,
    /// The chat's workspace folder, for the workspace tools.
    pub workspace: Option<std::path::PathBuf>,
    /// The Docker sandbox that runs the workspace shell tool, while it's on.
    pub sandbox: Option<crate::sandbox::Sandbox>,
    /// The summary of the messages before `history`, from an earlier compaction.
    pub summary: Option<String>,
    /// How to compact the chat; `None` when compaction is off for it.
    pub compaction: Option<Compaction>,
    /// Only compact the chat now ("Compact now"), without asking the model anything else.
    pub compact_only: bool,
    /// The chat's permission mode: which calls run without asking.
    pub approval_mode: crate::approval::ApprovalMode,
}

impl AgentRequest {
    /// The request for a reply in `conv`, built from the settings the way the app does: the
    /// chat's tools and permissions, its messages after the latest compaction, and the provider
    /// with the chat's model options. `review` uses the review provider; `compact_only` only
    /// summarises. `sandbox` is the Docker sandbox, which a chat with `no_sandbox` skips.
    pub fn for_chat(
        settings: &crate::config::Settings,
        conv: &crate::types::Conversation,
        sandbox: Option<&crate::sandbox::Sandbox>,
        review: bool,
        compact_only: bool,
    ) -> Self {
        let permissions = &conv.permissions;
        let selection =
            ToolSelection::new(settings, &conv.tool_overrides).with_mode(permissions.mode);
        // After a compaction only its summary and the messages since are sent.
        let marker = crate::types::last_compaction(&conv.messages);
        let summary = marker.map(|i| conv.messages[i].content.clone());
        let from = marker.map_or(0, |i| i + 1);
        let chat_provider = settings.provider().clone();
        let provider = if review {
            settings.review_provider(&chat_provider)
        } else {
            chat_provider
        };
        let compaction =
            (compact_only || selection.feature_on(tools::COMPACTION)).then(|| Compaction {
                settings: settings.general.compaction.clone(),
                provider: settings.compaction_provider(&provider),
                auto: selection.feature_on(tools::COMPACTION),
            });
        Self {
            conv_id: conv.id,
            provider: provider.with_chat_options(&conv.model_options),
            general: settings.general.clone(),
            history: conv.messages[from..]
                .iter()
                .filter(|m| !m.cancelled && m.compaction.is_none())
                .cloned()
                .collect(),
            delegation: Delegation::new(settings, &selection),
            selection,
            workspace: conv.workspace.clone(),
            sandbox: sandbox.filter(|_| !permissions.no_sandbox).cloned(),
            summary,
            compaction,
            compact_only,
            approval_mode: permissions.mode,
        }
    }
}

pub type EventSender = mpsc::UnboundedSender<AppEvent>;

pub async fn run(
    req: AgentRequest,
    http: reqwest::Client,
    mcp: Arc<McpManager>,
    tx: EventSender,
    repaint: impl Fn() + Send + Sync,
) {
    let conv_id = req.conv_id;
    let emit = |event: ChatEvent| {
        let _ = tx.send(AppEvent::Chat { conv_id, event });
        repaint();
    };

    let workspace = match req.workspace.as_deref().map(Workspace::open).transpose() {
        Ok(ws) => ws,
        Err(e) => {
            emit(ChatEvent::Error(format!(
                "{e:#}. Choose another working folder below the message box."
            )));
            emit(ChatEvent::Done);
            return;
        }
    };
    let edits = &req.general.file_edits;
    let workspace = workspace.map(|ws| {
        ws.with_edit_options(EditOptions {
            diff: edits.diff_preview,
            checkpoint: req.selection.feature_on(tools::CHECKPOINTS),
            max_checkpoint_bytes: u64::from(edits.max_file_kb) * 1024,
        })
    });
    // Shell commands run in the chat's sandbox container, when the sandbox is on.
    let workspace = match (workspace, &req.sandbox) {
        (Some(ws), Some(sandbox)) => Some(ws.with_sandbox(sandbox.clone(), req.conv_id)),
        (ws, _) => ws,
    };
    let mut system_prompt = match &workspace {
        Some(ws) => format!(
            "{}\n\n{}",
            req.general.system_prompt.trim(),
            ws.prompt_note()
        ),
        None => req.general.system_prompt.clone(),
    };
    // AGENTS.md and the like, from the workspace.
    if let Some(ws) = &workspace
        && req.selection.feature_on(tools::INSTRUCTIONS)
    {
        let (root, settings) = (ws.path().to_path_buf(), req.general.instructions.clone());
        let loaded =
            tokio::task::spawn_blocking(move || crate::instructions::load(&root, &settings))
                .await
                .ok()
                .flatten();
        if let Some(loaded) = loaded {
            system_prompt = format!("{}\n\n{}", system_prompt.trim_end(), loaded.prompt);
            emit(ChatEvent::Instructions(loaded.files));
        }
    }
    // Skills: listed in the system prompt and loaded with `load_skill`, when the chat allows
    // them (subagents are switched off in their own selection).
    let switched_off = req
        .selection
        .overrides
        .get(&tools::ToolOverrides::builtin_key("skills"))
        == Some(false);
    let mut skill_files = Vec::new();
    if req.general.skills.enabled && !switched_off {
        let (settings, root) = (
            req.general.skills.clone(),
            workspace.as_ref().map(|ws| ws.path().to_path_buf()),
        );
        skill_files = tokio::task::spawn_blocking(move || {
            crate::skills::for_chat(&settings, root.as_deref())
        })
        .await
        .unwrap_or_default();
    }
    let skills = Arc::new(skill_files);
    let selection = req.selection.clone().with_skills(!skills.is_empty());
    let skills_on = selection.builtin_on("skills");
    if skills_on {
        system_prompt = format!(
            "{}\n\n{}",
            system_prompt.trim_end(),
            crate::skills::prompt_section(&skills)
        );
    }
    // Subagents may get the workspace too, with commands in containers of their own.
    let delegation = req.delegation.map(|d| d.with_workspace(workspace.as_ref()));
    let mut history = req.history;
    let max_rounds = req.general.max_tool_rounds.max(1);
    let tool_timeout = Duration::from_secs(req.general.tool_timeout_secs.max(5));
    let mut asked_context_after_reply = false;
    let (mut context_size, slot_usage) =
        if req.provider.kind == crate::config::ProviderKind::LlamaCpp {
            tokio::join!(
                llm::llama_cpp_context_size(&http, &req.provider),
                llm::llama_cpp_slot_usage(&http, &req.provider)
            )
        } else {
            (None, None)
        };
    // Ollama, LM Studio, vLLM and OpenRouter report the context size the model runs with.
    if context_size.is_none() {
        context_size = crate::servers::context_size(&http, &req.provider).await;
    }
    // LiteLLM lists the model's prices and context window.
    let litellm = if req.provider.kind == crate::config::ProviderKind::LiteLlm {
        llm::litellm_model_info(&http, &req.provider).await
    } else {
        None
    };
    if let Some(n) = litellm.as_ref().and_then(|m| m.max_input_tokens) {
        context_size = Some(n);
    }
    let prices = litellm.and_then(|m| m.prices);
    // The provider's own context size, when the server reports none.
    let server_reported = context_size.is_some();
    if context_size.is_none() {
        context_size = req.provider.context_tokens.map(u64::from);
    }
    let mut summary = req.summary;
    let compaction = req.compaction;
    // Set once a compaction failed, so the rest of the run doesn't try again every round.
    let mut compaction_failed = false;

    if req.compact_only {
        match &compaction {
            Some(c) => {
                let tokens =
                    compaction::estimate_tokens(&system_prompt, &history, &[], summary.is_none());
                if let Err(e) =
                    compact(c, &http, &mut history, &mut summary, tokens, true, &emit).await
                {
                    emit(ChatEvent::Error(e));
                }
            }
            None => emit(ChatEvent::Error("Compaction isn't available.".into())),
        }
        emit(ChatEvent::Done);
        return;
    }

    for round in 0.. {
        // Re-read the router every round so servers that finished connecting are picked up.
        let toolbox = if round < max_rounds {
            let toolbox = Toolbox::new(
                mcp.router(),
                &selection,
                delegation.as_ref().map(Delegation::tool_description),
            )
            .with_http(http.clone());
            let toolbox = match &workspace {
                Some(ws) => toolbox.with_workspace(ws.clone(), &selection),
                None => toolbox,
            };
            if skills_on {
                toolbox.with_skills(skills.clone(), req.general.skills.max_kb)
            } else {
                toolbox
            }
        } else {
            Toolbox::default()
        };
        // Summarise older messages when this request would fill most of the context.
        if let Some(c) = compaction.as_ref().filter(|c| c.auto && !compaction_failed) {
            let size = context_size
                .filter(|&n| n > 0)
                .or(Some(u64::from(c.settings.context_tokens)).filter(|&n| n > 0));
            let tokens = compaction::estimate_tokens(
                &system_prompt,
                &history,
                &toolbox.specs,
                summary.is_none(),
            );
            if let Some(size) = size
                && tokens * 100 >= size * u64::from(c.settings.threshold_percent)
                && compaction::split_point(&history, c.settings.keep_recent as usize).is_some()
                && let Err(e) =
                    compact(c, &http, &mut history, &mut summary, tokens, false, &emit).await
            {
                compaction_failed = true;
                emit(ChatEvent::Error(e));
            }
        }
        let mut sent = history.clone();
        if let Some(c) = compaction.as_ref().filter(|c| c.settings.trim_tool_results) {
            compaction::trim_tool_results(
                &mut sent,
                c.settings.keep_recent as usize,
                c.settings.trim_chars as usize,
            );
        }
        let system = match &summary {
            Some(s) => {
                if sent.first().is_some_and(|m| m.role != Role::User) {
                    sent.insert(0, ChatMessage::new(Role::User, compaction::CONTINUE_NOTE));
                }
                compaction::system_with_summary(&system_prompt, s)
            }
            None => system_prompt.clone(),
        };
        let messages = llm::to_api_messages(&system, &sent);

        emit(ChatEvent::AssistantStart);
        let result = llm::chat_completion(
            &http,
            &req.provider,
            messages,
            &toolbox.specs,
            |d| match d {
                Delta::Content(s) => emit(ChatEvent::ContentDelta(s.to_owned())),
                Delta::Reasoning(s) => emit(ChatEvent::ReasoningDelta(s.to_owned())),
            },
            |s| emit(ChatEvent::StatsLive(Box::new(s))),
        )
        .await;

        let turn = match result {
            Ok(t) => t,
            Err(e) => {
                emit(ChatEvent::Error(format!("{e:#}")));
                break;
            }
        };

        let mut msg = ChatMessage::new(Role::Assistant, turn.content);
        if !turn.reasoning.is_empty() {
            msg.reasoning = Some(turn.reasoning);
        }
        msg.tool_calls = turn.tool_calls;
        msg.native = turn.native;
        let mut stats = turn.stats;
        // Only llama.cpp sends `timings` (source Server). Ask for the context size after such
        // a reply too: when Server type is left on Generic, or when a router had not loaded
        // the model yet at the start.
        if !server_reported
            && !asked_context_after_reply
            && stats.source == crate::stats::StatsSource::Server
        {
            asked_context_after_reply = true;
            if let Some(n) = llm::llama_cpp_context_size(&http, &req.provider).await {
                context_size = Some(n);
            }
        }
        stats.context_size = context_size;
        stats.slot_usage = slot_usage;
        // LiteLLM reports no cost for streamed replies: estimate it from its prices.
        if let Some(pr) = prices {
            pr.fill_in(&mut stats);
        }
        msg.stats = Some(stats);
        let calls = msg.tool_calls.clone();
        history.push(msg.clone());
        emit(ChatEvent::AssistantFinal(Box::new(msg)));

        if calls.is_empty() {
            if turn.finish_reason.as_deref() == Some("length") {
                emit(ChatEvent::Error(
                    "The response was cut off (max tokens reached).".into(),
                ));
            }
            break;
        }
        if round >= max_rounds {
            emit(ChatEvent::Error(format!(
                "Stopped after {max_rounds} rounds of tool calls."
            )));
            break;
        }

        for call in &calls {
            let approved = if req.general.auto_approve_tools
                || !toolbox.asks(&call.name)
                || req
                    .approval_mode
                    .runs_unasked(&call.name, req.sandbox.is_some())
            {
                true
            } else {
                let preview = match &workspace {
                    Some(ws) if req.general.file_edits.diff_preview => {
                        let (ws, name, args) =
                            (ws.clone(), call.name.clone(), call.arguments.clone());
                        tokio::task::spawn_blocking(move || {
                            let args = serde_json::from_str(&args).ok()?;
                            ws.preview(&name, &args)
                        })
                        .await
                        .ok()
                        .flatten()
                    }
                    _ => None,
                };
                let (respond, rx) = oneshot::channel();
                emit(ChatEvent::ToolApproval {
                    call: call.clone(),
                    origin: toolbox
                        .origin(&call.name)
                        .map(|(server, tool)| (server.to_owned(), tool.to_owned())),
                    preview,
                    respond,
                });
                rx.await.unwrap_or(false)
            };
            let result = if approved {
                emit(ChatEvent::ToolRunning(call.id.clone()));
                match (
                    &delegation,
                    call.name == tools::DELEGATE && toolbox.has(&call.name),
                ) {
                    (Some(delegation), true) => {
                        let progress = |reports: &[SubagentReport]| {
                            emit(ChatEvent::SubagentProgress {
                                call_id: call.id.clone(),
                                reports: reports.to_vec(),
                            })
                        };
                        let (out, reports) = delegation
                            .run(&http, &mcp, &call.arguments, &progress)
                            .await;
                        let mut msg = ChatMessage::tool_result(call, out.text, out.is_error);
                        msg.subagents = reports;
                        msg
                    }
                    _ => {
                        let out = toolbox
                            .call(&call.name, &call.arguments, tool_timeout)
                            .await;
                        let mut msg = ChatMessage::tool_result(call, out.text, out.is_error);
                        msg.attachments = out.images;
                        msg.sandbox = out.sandbox;
                        msg.edit = out.edit;
                        msg
                    }
                }
            } else {
                ChatMessage::tool_result(call, "The user declined to run this tool.", true)
            };
            history.push(result.clone());
            emit(ChatEvent::ToolResult(Box::new(result)));
        }
    }
    emit(ChatEvent::Done);
}

/// Summarise the messages before the kept ones, and drop them from `history`. `tokens` is the
/// estimate that set it off, kept on the marker. `manual` when started with "Compact now".
async fn compact(
    c: &Compaction,
    http: &reqwest::Client,
    history: &mut Vec<ChatMessage>,
    summary: &mut Option<String>,
    tokens: u64,
    manual: bool,
    emit: &impl Fn(ChatEvent),
) -> Result<(), String> {
    let Some(split) = compaction::split_point(history, c.settings.keep_recent as usize) else {
        return Err("Nothing to compact yet: the chat is shorter than the messages kept.".into());
    };
    emit(ChatEvent::Compacting);
    let (mut new, stats) = compaction::summarise(http, c, summary.as_deref(), &history[..split])
        .await
        .map_err(|e| format!("Couldn't compact the chat: {e:#}"))?;
    if let Some(tasks) = compaction::carried_tasks(&history[..split], &history[split..]) {
        new = format!("{new}\n\n{tasks}");
    }
    history.drain(..split);
    *summary = Some(new.clone());
    emit(ChatEvent::Compacted {
        kept: history.len(),
        summary: new,
        info: CompactionInfo {
            messages: split,
            tokens_before: tokens,
            manual,
        },
        stats: Box::new(stats),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    //! End-to-end: a mock OpenAI-compatible server that asks for a tool call, routed to
    //! the reference MCP "everything" server. Needs `npx` + network, so it's ignored by
    //! default: `cargo test -- --ignored`.
    use super::*;
    use crate::config::{McpServerConfig, McpTransportConfig};
    use crate::testutil::{Request, Response, serve};

    async fn mock_llm() -> (String, Arc<std::sync::Mutex<Vec<Request>>>) {
        let (base, log) = serve(|r| {
            let has_tool_result = r.json()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool");
            let chunks: Vec<&str> = if has_tool_result {
                vec![
                    r#"{"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"The tool "}}]}"#,
                    r#"{"choices":[{"delta":{"content":"said hi."},"finish_reason":"stop"}]}"#,
                ]
            } else {
                vec![
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"everything__echo","arguments":""}}]}}]}"#,
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"message\":"}}]}}]}"#,
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"hi\"}"}}]},"finish_reason":"tool_calls"}]}"#,
                ]
            };
            let chunks: Vec<serde_json::Value> = chunks
                .iter()
                .map(|c| serde_json::from_str(c).unwrap())
                .collect();
            Response::sse(&chunks)
        })
        .await;
        (format!("{base}/v1"), log)
    }

    async fn wait_connected(mcp: &McpManager) {
        for _ in 0..600 {
            let snap = mcp.snapshot();
            match &snap[0].status {
                crate::mcp::manager::ServerStatus::Connected => return,
                crate::mcp::manager::ServerStatus::Error(e) => panic!("MCP error: {e}"),
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
        panic!("MCP server did not connect");
    }

    async fn run_agent_against(transport: McpTransportConfig) {
        let http = llm::build_http_client();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        mcp.sync(&[McpServerConfig {
            name: "everything".into(),
            transport,
            ..Default::default()
        }]);
        wait_connected(&mcp).await;
        assert!(
            mcp.router()
                .specs
                .iter()
                .any(|s| s.name == "everything__echo")
        );

        let (base_url, bodies) = mock_llm().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let req = AgentRequest {
            conv_id: Uuid::new_v4(),
            provider: ProviderConfig {
                base_url,
                model: "mock".into(),
                ..Default::default()
            },
            general: GeneralSettings {
                auto_approve_tools: true,
                ..Default::default()
            },
            history: vec![ChatMessage::new(Role::User, "say hi via the tool")],
            selection: ToolSelection::default(),
            delegation: None,
            workspace: None,
            sandbox: None,
            summary: None,
            compaction: None,
            compact_only: false,
            approval_mode: Default::default(),
        };
        run(req, http, mcp, tx, || {}).await;

        let mut content = String::new();
        let mut tool_result = None;
        let mut errors = Vec::new();
        while let Ok(AppEvent::Chat { event, .. }) = rx.try_recv() {
            match event {
                ChatEvent::ContentDelta(s) => content.push_str(&s),
                ChatEvent::ToolResult(m) => tool_result = Some(*m),
                ChatEvent::Error(e) => errors.push(e),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        let tool_result = tool_result.expect("tool was not called");
        assert!(!tool_result.is_error, "{}", tool_result.content);
        assert!(
            tool_result.content.contains("hi"),
            "{}",
            tool_result.content
        );
        assert_eq!(content, "The tool said hi.");

        let bodies: Vec<serde_json::Value> =
            bodies.lock().unwrap().iter().map(Request::json).collect();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0]["tools"].as_array().unwrap().len() > 3);
        let second = bodies[1]["messages"].as_array().unwrap();
        assert_eq!(
            second[2]["tool_calls"][0]["function"]["arguments"],
            "{\"message\":\"hi\"}"
        );
    }

    /// A mock model that calls `tool` with `args` on its first turn, then answers with the
    /// tool result it was given.
    async fn tool_calling_llm(
        tool: &'static str,
        args: &'static str,
    ) -> (String, Arc<std::sync::Mutex<Vec<crate::testutil::Request>>>) {
        use crate::testutil::{Response, serve};
        serve(move |req| {
            let body = req.json();
            let msgs = body["messages"].as_array().cloned().unwrap_or_default();
            if let Some(result) = msgs.iter().find(|m| m["role"] == "tool") {
                return Response::json(serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": format!("Tool said: {}", result["content"].as_str().unwrap())}, "finish_reason": "stop"}]
                }));
            }
            Response::json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": tool, "arguments": args}}
                ]}, "finish_reason": "tool_calls"}]
            }))
        })
        .await
    }

    fn request(
        base: &str,
        settings: &crate::config::Settings,
        overrides: &crate::tools::ToolOverrides,
    ) -> AgentRequest {
        let selection = ToolSelection::new(settings, overrides);
        AgentRequest {
            conv_id: Uuid::new_v4(),
            provider: ProviderConfig {
                base_url: format!("{base}/v1"),
                model: "mock".into(),
                stream: false,
                ..Default::default()
            },
            general: settings.general.clone(),
            history: vec![ChatMessage::new(Role::User, "go")],
            delegation: Delegation::new(settings, &selection),
            selection,
            workspace: None,
            sandbox: None,
            summary: None,
            compaction: None,
            compact_only: false,
            approval_mode: Default::default(),
        }
    }

    #[tokio::test]
    async fn workspace_tools_read_freely_and_ask_before_writing() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-agent-ws-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "remember the milk\n").unwrap();
        let settings = crate::config::Settings::default();
        let http = reqwest::Client::new();
        let run_with = |tool: &'static str, args: &'static str| {
            let (dir, settings, http) = (dir.clone(), settings.clone(), http.clone());
            async move {
                let (base, log) = tool_calling_llm(tool, args).await;
                let mut req = request(&base, &settings, &Default::default());
                req.workspace = Some(dir);
                let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
                let (tx, mut rx) = mpsc::unbounded_channel();
                let agent = tokio::spawn(run(req, http, mcp, tx, || {}));
                let mut approvals = 0;
                let mut result = None;
                while let Some(AppEvent::Chat { event, .. }) = rx.recv().await {
                    match event {
                        ChatEvent::ToolApproval { respond, .. } => {
                            approvals += 1;
                            respond.send(true).unwrap();
                        }
                        ChatEvent::ToolResult(m) => result = Some(*m),
                        ChatEvent::Done => break,
                        _ => {}
                    }
                }
                agent.await.unwrap();
                (approvals, result.unwrap(), log.lock().unwrap()[0].json())
            }
        };

        let (approvals, result, first) =
            run_with(tools::workspace::READ_FILE, r#"{"path": "notes.txt"}"#).await;
        assert_eq!(approvals, 0);
        assert!(
            result.content.contains("remember the milk"),
            "{}",
            result.content
        );
        let names: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"exec_shell_command"), "{names:?}");
        let system = first["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("(the workspace)"), "{system}");

        let (approvals, result, _) = run_with(
            tools::workspace::WRITE_FILE,
            r#"{"path": "out/hello.txt", "content": "hi"}"#,
        )
        .await;
        assert_eq!(approvals, 1);
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(dir.join("out/hello.txt")).unwrap(),
            "hi"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn context_size_after_a_llama_cpp_reply_on_a_generic_provider() {
        let (base, _) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/props?model=mock&autoload=false") => Response::json(serde_json::json!({
                "default_generation_settings": {"n_ctx": 4096}, "build_info": "b1"})),
            ("POST", "/v1/chat/completions") => Response::json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "hi"},
                             "finish_reason": "stop"}],
                "timings": {"prompt_n": 90, "cache_n": 10, "prompt_ms": 50.0,
                            "prompt_per_second": 1800.0, "predicted_n": 2,
                            "predicted_ms": 40.0, "predicted_per_second": 50.0}})),
            _ => Response::status(404),
        })
        .await;
        let settings = crate::config::Settings::default();
        let req = request(&base, &settings, &Default::default());
        assert_eq!(req.provider.kind, crate::config::ProviderKind::Generic);
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(req, http, mcp, tx, || {}).await;

        let mut stats = None;
        while let Ok(AppEvent::Chat { event, .. }) = rx.try_recv() {
            if let ChatEvent::AssistantFinal(m) = event {
                stats = m.stats;
            }
        }
        let stats = stats.unwrap();
        assert_eq!(stats.context_use(), Some((102, 4096)));
    }

    #[tokio::test]
    async fn the_provider_context_size_stands_in_when_the_server_reports_none() {
        let (base, _) = serve(|_| {
            Response::json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "hi"},
                             "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 100, "completion_tokens": 2}
            }))
        })
        .await;
        let settings = crate::config::Settings::default();
        let mut req = request(&base, &settings, &Default::default());
        assert_eq!(req.provider.context_tokens, None);
        req.provider.context_tokens = Some(4096);
        let events = drive(req).await;
        let stats = events
            .iter()
            .find_map(|e| match e {
                ChatEvent::AssistantFinal(m) => m.stats.clone(),
                _ => None,
            })
            .expect("a reply");
        assert_eq!(stats.context_use(), Some((102, 4096)));
    }

    #[tokio::test]
    async fn litellm_reply_without_a_cost_gets_an_estimate() {
        let (base, _) = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/model/info") => Response::json(serde_json::json!({"data": [
                {"model_name": "mock", "model_info": {
                    "input_cost_per_token": 0.000001, "output_cost_per_token": 0.000002,
                    "max_input_tokens": 8000}}]})),
            ("POST", "/v1/chat/completions") => Response::json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "hi"},
                             "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1000, "completion_tokens": 500}})),
            _ => Response::status(404),
        })
        .await;
        let settings = crate::config::Settings::default();
        let mut req = request(&base, &settings, &Default::default());
        req.provider.kind = crate::config::ProviderKind::LiteLlm;
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(req, http, mcp, tx, || {}).await;

        let mut stats = None;
        while let Ok(AppEvent::Chat { event, .. }) = rx.try_recv() {
            if let ChatEvent::AssistantFinal(m) = event {
                stats = m.stats;
            }
        }
        let stats = stats.unwrap();
        assert!(stats.cost_estimated);
        // 1,000 × $1/M + 500 × $2/M
        assert!((stats.cost_usd.unwrap() - 0.002).abs() < 1e-12);
        // LiteLLM's listed context window gives context use.
        assert_eq!(stats.context_use(), Some((1500, 8000)));
    }

    #[tokio::test]
    async fn builtin_tools_run_without_approval() {
        let (base, log) = tool_calling_llm(tools::CALCULATE, r#"{"expression": "6*7"}"#).await;
        let settings = crate::config::Settings::default();
        assert!(!settings.general.auto_approve_tools);
        let req = request(&base, &settings, &Default::default());
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(req, http, mcp, tx, || {}).await;

        let mut approvals = 0;
        let mut result = None;
        while let Ok(AppEvent::Chat { event, .. }) = rx.try_recv() {
            match event {
                ChatEvent::ToolApproval { .. } => approvals += 1,
                ChatEvent::ToolResult(m) => result = Some(*m),
                _ => {}
            }
        }
        assert_eq!(approvals, 0);
        assert_eq!(result.unwrap().content, "6*7 = 42");
        let first = log.lock().unwrap()[0].json();
        let names: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                tools::TIME,
                tools::CALCULATE,
                tools::TASKS,
                tools::FETCH,
                tools::WEB_SEARCH,
                tools::DELEGATE
            ]
        );
    }

    #[tokio::test]
    async fn chat_switches_hide_tools() {
        let (base, log) = tool_calling_llm(tools::CALCULATE, r#"{"expression": "1+1"}"#).await;
        let settings = crate::config::Settings::default();
        let mut ov = crate::tools::ToolOverrides::default();
        ov.set(
            crate::tools::ToolOverrides::builtin_key("calculator"),
            false,
            true,
        );
        ov.set(
            crate::tools::ToolOverrides::builtin_key("subagents"),
            false,
            true,
        );
        let req = request(&base, &settings, &ov);
        assert!(req.delegation.is_none());
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        run(req, http, mcp, tx, || {}).await;

        let first = log.lock().unwrap()[0].json();
        let names: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [tools::TIME, tools::TASKS, tools::FETCH, tools::WEB_SEARCH]
        );
        // A call to a tool that is off for this chat is refused, not run.
        let mut result = None;
        while let Ok(AppEvent::Chat { event, .. }) = rx.try_recv() {
            if let ChatEvent::ToolResult(m) = event {
                result = Some(*m);
            }
        }
        let result = result.unwrap();
        assert!(result.is_error);
        assert!(
            result.content.contains("not available"),
            "{}",
            result.content
        );
    }

    #[tokio::test]
    async fn delegate_asks_first_and_reports_subagents() {
        let (base, _) = tool_calling_llm(
            tools::DELEGATE,
            r#"{"tasks": [{"task": "first"}, {"task": "second"}]}"#,
        )
        .await;
        let settings = crate::config::Settings::default();
        let mut req = request(&base, &settings, &Default::default());
        // Subagents talk to the same mock; it answers them directly (no tool messages).
        if let Some(d) = &mut req.delegation {
            d.set_default_provider(req.provider.clone());
        }
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = tokio::spawn(run(req, http, mcp, tx, || {}));

        let mut progress = 0;
        let mut result = None;
        while let Some(AppEvent::Chat { event, .. }) = rx.recv().await {
            match event {
                ChatEvent::ToolApproval { call, respond, .. } => {
                    assert_eq!(call.name, tools::DELEGATE);
                    respond.send(true).unwrap();
                }
                ChatEvent::SubagentProgress { reports, .. } => {
                    assert_eq!(reports.len(), 2);
                    progress += 1;
                }
                ChatEvent::ToolResult(m) => result = Some(*m),
                ChatEvent::Done => break,
                _ => {}
            }
        }
        agent.await.unwrap();
        assert!(progress >= 3);
        let result = result.unwrap();
        assert_eq!(result.subagents.len(), 2);
        assert!(
            result
                .subagents
                .iter()
                .all(|r| r.state == crate::subagent::SubagentState::Done)
        );
        assert!(
            result.content.contains("## Subagent 2\nTask: second"),
            "{}",
            result.content
        );
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cleanrstai-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Collect a run's events, answering approvals with `true`.
    async fn drive(req: AgentRequest) -> Vec<ChatEvent> {
        let http = reqwest::Client::new();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = tokio::spawn(run(req, http, mcp, tx, || {}));
        let mut events = Vec::new();
        while let Some(AppEvent::Chat { event, .. }) = rx.recv().await {
            let done = matches!(event, ChatEvent::Done);
            match event {
                ChatEvent::ToolApproval {
                    call,
                    origin,
                    preview,
                    respond,
                } => {
                    respond.send(true).unwrap();
                    let (tx, _) = oneshot::channel();
                    events.push(ChatEvent::ToolApproval {
                        call,
                        origin,
                        preview,
                        respond: tx,
                    });
                }
                other => events.push(other),
            }
            if done {
                break;
            }
        }
        agent.await.unwrap();
        events
    }

    #[tokio::test]
    async fn project_instructions_join_the_system_prompt_unless_switched_off() {
        let dir = temp_dir("agent-ins");
        std::fs::write(dir.join("AGENTS.md"), "Always run cargo fmt.").unwrap();
        let (base, log) = tool_calling_llm(tools::CALCULATE, r#"{"expression": "1"}"#).await;
        let settings = crate::config::Settings::default();
        let mut req = request(&base, &settings, &Default::default());
        req.workspace = Some(dir.clone());
        let events = drive(req).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ChatEvent::Instructions(f) if f == &["AGENTS.md"]))
        );
        let system = log.lock().unwrap()[0].json()["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(system.contains("Always run cargo fmt."), "{system}");

        let (base, log) = tool_calling_llm(tools::CALCULATE, r#"{"expression": "1"}"#).await;
        let mut ov = crate::tools::ToolOverrides::default();
        ov.set(
            crate::tools::ToolOverrides::feature_key(tools::INSTRUCTIONS),
            false,
            true,
        );
        let mut req = request(&base, &settings, &ov);
        req.workspace = Some(dir.clone());
        let events = drive(req).await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ChatEvent::Instructions(_)))
        );
        let system = log.lock().unwrap()[0].json()["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!system.contains("cargo fmt"), "{system}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn workspace_skills_are_listed_and_offered_unless_switched_off() {
        let dir = temp_dir("agent-skill");
        let skill = crate::skills::workspace_root(&dir).join("notes");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\ndescription: Take meeting notes\n---\nWrite the notes.",
        )
        .unwrap();
        let settings = crate::config::Settings::default();
        let (base, log) = tool_calling_llm(tools::CALCULATE, r#"{"expression": "1"}"#).await;
        let mut req = request(&base, &settings, &Default::default());
        req.workspace = Some(dir.clone());
        drive(req).await;
        let first = log.lock().unwrap()[0].json();
        let system = first["messages"][0]["content"].as_str().unwrap().to_owned();
        assert!(system.contains("- notes: Take meeting notes"), "{system}");
        let names: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&tools::LOAD_SKILL), "{names:?}");

        let (base, log) = tool_calling_llm(tools::CALCULATE, r#"{"expression": "1"}"#).await;
        let mut ov = crate::tools::ToolOverrides::default();
        ov.set(
            crate::tools::ToolOverrides::builtin_key("skills"),
            false,
            true,
        );
        let mut req = request(&base, &settings, &ov);
        req.workspace = Some(dir.clone());
        drive(req).await;
        let first = log.lock().unwrap()[0].json();
        let system = first["messages"][0]["content"].as_str().unwrap().to_owned();
        assert!(!system.contains("Skills available"), "{system}");
        let has_tool = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == tools::LOAD_SKILL);
        assert!(!has_tool);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn file_edits_show_a_diff_before_approval_and_keep_a_checkpoint() {
        let dir = temp_dir("agent-edit");
        std::fs::write(dir.join("a.txt"), "hello world\n").unwrap();
        let (base, _) = tool_calling_llm(
            tools::workspace::EDIT_FILE,
            r#"{"path": "a.txt", "old_string": "world", "new_string": "there"}"#,
        )
        .await;
        let settings = crate::config::Settings::default();
        let mut req = request(&base, &settings, &Default::default());
        req.workspace = Some(dir.clone());
        let events = drive(req).await;
        let preview = events
            .iter()
            .find_map(|e| match e {
                ChatEvent::ToolApproval { preview, .. } => preview.clone(),
                _ => None,
            })
            .expect("the approval carries a diff");
        assert!(
            preview.contains("-hello world\n+hello there\n"),
            "{preview}"
        );
        let result = events
            .iter()
            .find_map(|e| match e {
                ChatEvent::ToolResult(m) => Some(m.clone()),
                _ => None,
            })
            .unwrap();
        let edit = result.edit.expect("the result records the edit");
        assert!(edit.checkpoint);
        assert_eq!(edit.before.as_deref(), Some("hello world\n"));
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "hello there\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mock model that answers "ok", or writes a summary when asked for one.
    async fn summarising_llm() -> (String, Arc<std::sync::Mutex<Vec<Request>>>) {
        serve(|r| {
            let body = r.json();
            let system = body["messages"][0]["content"].as_str().unwrap_or("");
            let text = if system.starts_with("Summarise the conversation") {
                "SUMMARY-NOTES"
            } else {
                "ok"
            };
            Response::json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 700, "completion_tokens": 30}
            }))
        })
        .await
    }

    fn long_history(turns: usize) -> Vec<ChatMessage> {
        let mut h = Vec::new();
        for i in 0..turns {
            h.push(ChatMessage::new(
                Role::User,
                format!("question {i} {}", "x".repeat(400)),
            ));
            h.push(ChatMessage::new(Role::Assistant, format!("answer {i}")));
        }
        h.push(ChatMessage::new(Role::User, "latest question"));
        h
    }

    fn with_compaction(req: &mut AgentRequest, settings: &crate::config::Settings) {
        req.compaction = Some(Compaction {
            settings: settings.general.compaction.clone(),
            provider: req.provider.clone(),
            auto: true,
        });
    }

    #[tokio::test]
    async fn compacts_when_the_context_fills_up_and_sends_the_summary() {
        let (base, log) = summarising_llm().await;
        let mut settings = crate::config::Settings::default();
        settings.general.compaction.context_tokens = 1000;
        settings.general.compaction.keep_recent = 3;
        let mut req = request(&base, &settings, &Default::default());
        req.history = long_history(6);
        with_compaction(&mut req, &settings);
        let events = drive(req).await;
        let (kept, summary, stats) = events
            .iter()
            .find_map(|e| match e {
                ChatEvent::Compacted {
                    kept,
                    summary,
                    info,
                    stats,
                } => {
                    assert_eq!(info.messages, 10);
                    assert!(!info.manual);
                    Some((*kept, summary.clone(), stats.clone()))
                }
                _ => None,
            })
            .expect("compacted");
        assert_eq!((kept, summary.as_str()), (3, "SUMMARY-NOTES"));
        // The summary request's usage travels with the event, for the chat's totals.
        assert_eq!(
            (stats.prompt_tokens, stats.completion_tokens),
            (Some(700), Some(30))
        );
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2, "one summary request, then the reply");
        let reply = log[1].json();
        let msgs = reply["messages"].as_array().unwrap();
        let system = msgs[0]["content"].as_str().unwrap();
        assert!(
            system.contains("<summary_of_earlier_conversation>\nSUMMARY-NOTES"),
            "{system}"
        );
        // The system prompt, then the kept messages; they start with the user's, so no note.
        assert_eq!(msgs.len(), 4);
        assert_eq!(
            msgs[1]["content"],
            format!("question 5 {}", "x".repeat(400))
        );
        assert_eq!(msgs[3]["content"], "latest question");
    }

    #[tokio::test]
    async fn no_compaction_below_the_threshold_or_when_switched_off() {
        let (base, log) = summarising_llm().await;
        let mut settings = crate::config::Settings::default();
        settings.general.compaction.context_tokens = 100_000;
        let mut req = request(&base, &settings, &Default::default());
        req.history = long_history(6);
        with_compaction(&mut req, &settings);
        let events = drive(req).await;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ChatEvent::Compacted { .. }))
        );
        assert_eq!(log.lock().unwrap().len(), 1);

        // Off for the chat (no compaction), however full the context.
        let (base, log) = summarising_llm().await;
        settings.general.compaction.context_tokens = 100;
        let mut req = request(&base, &settings, &Default::default());
        req.history = long_history(6);
        drive(req).await;
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn compact_now_only_summarises_and_a_short_chat_says_so() {
        let (base, log) = summarising_llm().await;
        let settings = crate::config::Settings::default();
        let mut req = request(&base, &settings, &Default::default());
        req.history = long_history(6);
        req.summary = Some("OLDER".into());
        req.compact_only = true;
        with_compaction(&mut req, &settings);
        let events = drive(req).await;
        assert!(events.iter().any(
            |e| matches!(e, ChatEvent::Compacted { info, kept, .. } if info.manual && *kept == 6)
        ));
        let (len, asked) = {
            let log = log.lock().unwrap();
            let asked = log[0].json()["messages"][1]["content"]
                .as_str()
                .unwrap()
                .to_owned();
            (log.len(), asked)
        };
        assert_eq!(len, 1, "no reply, only the summary");
        assert!(asked.contains("<earlier_summary>\nOLDER"), "{asked}");

        let (base, _) = summarising_llm().await;
        let mut req = request(&base, &settings, &Default::default());
        req.compact_only = true;
        with_compaction(&mut req, &settings);
        let events = drive(req).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ChatEvent::Error(m) if m.starts_with("Nothing to compact")))
        );
    }

    /// Both e2e tests start the same package with `npx -y`; running those installs at the
    /// same time on a cold npm cache can race, so the tests take turns.
    static NPX_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_stdio_mcp() {
        let _guard = NPX_LOCK.lock().await;
        run_agent_against(McpTransportConfig::Stdio {
            command: "npx".into(),
            args: vec![
                "-y".into(),
                "@modelcontextprotocol/server-everything".into(),
            ],
            env: vec![],
            cwd: String::new(),
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_streamable_http_mcp() {
        let _guard = NPX_LOCK.lock().await;
        // Pick a free port rather than hard-coding one.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let stderr_path = std::env::temp_dir().join(format!("cleanrstai-mcp-http-{port}.log"));
        let mut child = tokio::process::Command::new(which::which("npx").expect("npx not found"))
            .args([
                "-y",
                "@modelcontextprotocol/server-everything",
                "streamableHttp",
            ])
            .env("PORT", port.to_string())
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            // A file rather than a pipe: on Windows a pipe held open by the orphaned `node`
            // would keep the runtime's blocking reader (and so the test) alive forever.
            .stderr(std::fs::File::create(&stderr_path).unwrap())
            .spawn()
            .unwrap();
        // A cold `npx` download can take a while on CI runners.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                panic!("MCP HTTP server exited before listening: {status}\nstderr:\n{stderr}");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "MCP HTTP server did not start listening on port {port} within 180s"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        run_agent_against(McpTransportConfig::Http {
            url: format!("http://127.0.0.1:{port}/mcp"),
            headers: vec![],
            oauth_client_id: String::new(),
        })
        .await;
        if let Some(pid) = child.id() {
            crate::mcp::stdio::kill_process_tree(pid);
        }
        let _ = child.kill().await;
        let _ = std::fs::remove_file(&stderr_path);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_mcp_tool_returns_image() {
        let _guard = NPX_LOCK.lock().await;
        let http = llm::build_http_client();
        let mcp = McpManager::new(tokio::runtime::Handle::current(), http);
        mcp.sync(&[McpServerConfig {
            name: "everything".into(),
            transport: McpTransportConfig::Stdio {
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-everything".into(),
                ],
                env: vec![],
                cwd: String::new(),
            },
            ..Default::default()
        }]);
        wait_connected(&mcp).await;
        let out = mcp
            .router()
            .call("everything__get-tiny-image", "{}", Duration::from_secs(60))
            .await
            .unwrap();
        assert!(!out.images.is_empty(), "{}", out.text);
        let img = &out.images[0];
        assert!(img.is_image());
        let bytes = img.bytes().unwrap();
        let decoded = crate::attach::Attachment::from_bytes("x".into(), bytes).unwrap();
        assert_eq!(decoded.mime, img.mime, "data matches the declared type");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn e2e_mcp_resources_and_prompts() {
        let _guard = NPX_LOCK.lock().await;
        let mcp = McpManager::new(tokio::runtime::Handle::current(), llm::build_http_client());
        mcp.sync(&[McpServerConfig {
            id: "ev".into(),
            name: "everything".into(),
            transport: McpTransportConfig::Stdio {
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-everything".into(),
                ],
                env: vec![],
                cwd: String::new(),
            },
            ..Default::default()
        }]);
        wait_connected(&mcp).await;
        let view = mcp.snapshot().remove(0);
        assert!(!view.resources.is_empty(), "server lists resources");
        assert!(!view.prompts.is_empty(), "server lists prompts");
        let client = mcp.client("ev").unwrap();

        let text_res = view
            .resources
            .iter()
            .find(|r| {
                r.mime_type
                    .as_deref()
                    .is_some_and(|m| m.starts_with("text/"))
            })
            .unwrap_or(&view.resources[0]);
        let read = client.read_resource(&text_res.uri).await.unwrap();
        assert!(!read.is_empty());

        // Some prompts validate their arguments, so try each one with simple values and
        // require that prompts both with and without arguments work.
        let (mut with_args, mut without_args) = (0, 0);
        for prompt in &view.prompts {
            let args: std::collections::BTreeMap<String, String> = prompt
                .arguments
                .iter()
                .map(|a| (a.name.clone(), "1".to_owned()))
                .collect();
            if let Ok(msgs) = client.get_prompt(&prompt.name, &args).await
                && !msgs.is_empty()
            {
                if prompt.arguments.is_empty() {
                    without_args += 1;
                } else {
                    with_args += 1;
                }
            }
        }
        assert!(
            with_args > 0 && without_args > 0,
            "{with_args} / {without_args}"
        );
    }
}

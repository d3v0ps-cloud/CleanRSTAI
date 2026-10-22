//! One "generation": stream the model's answer, run any MCP tool calls it makes, feed
//! the results back, and repeat until the model answers without tools.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::config::{GeneralSettings, ProviderConfig};
use crate::llm::{self, Delta};
use crate::mcp::manager::McpManager;
use crate::subagent::{Delegation, SubagentReport};
use crate::tools::{self, ToolSelection, Toolbox};
use crate::types::{ChatMessage, Role, ToolCall};

pub enum ChatEvent {
    /// A new, empty assistant message begins.
    AssistantStart,
    ContentDelta(String),
    ReasoningDelta(String),
    /// Replaces the in-progress assistant message with its final form (incl. tool calls).
    AssistantFinal(Box<ChatMessage>),
    ToolApproval {
        call: ToolCall,
        /// The MCP server id and tool name behind the call; `None` for built-ins.
        origin: Option<(String, String)>,
        respond: oneshot::Sender<bool>,
    },
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

pub enum AppEvent {
    /// Result of a GitHub Releases check. `explicit` when the user asked for it.
    UpdateChecked {
        result: Result<crate::update::UpdateCheck, String>,
        explicit: bool,
    },
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
    /// Messages from an MCP prompt, to insert into the chat.
    McpPromptReady {
        name: String,
        result: Result<Vec<crate::mcp::PromptMessage>, String>,
    },
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

    let mut history = req.history;
    let max_rounds = req.general.max_tool_rounds.max(1);
    let tool_timeout = Duration::from_secs(req.general.tool_timeout_secs.max(5));
    let context_size = if req.provider.kind == crate::config::ProviderKind::LlamaCpp {
        llm::llama_cpp_context_size(&http, &req.provider).await
    } else {
        None
    };

    for round in 0.. {
        // Re-read the router every round so servers that finished connecting are picked up.
        let toolbox = if round < max_rounds {
            Toolbox::new(
                mcp.router(),
                &req.selection,
                req.delegation.as_ref().map(Delegation::tool_description),
            )
        } else {
            Toolbox::default()
        };
        let messages = llm::to_api_messages(&req.general.system_prompt, &history);

        emit(ChatEvent::AssistantStart);
        let result =
            llm::chat_completion(
                &http,
                &req.provider,
                messages,
                &toolbox.specs,
                |d| match d {
                    Delta::Content(s) => emit(ChatEvent::ContentDelta(s.to_owned())),
                    Delta::Reasoning(s) => emit(ChatEvent::ReasoningDelta(s.to_owned())),
                },
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
        let mut stats = turn.stats;
        stats.context_size = context_size;
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
            let approved = if req.general.auto_approve_tools || !Toolbox::needs_approval(&call.name)
            {
                true
            } else {
                let (respond, rx) = oneshot::channel();
                emit(ChatEvent::ToolApproval {
                    call: call.clone(),
                    origin: toolbox
                        .origin(&call.name)
                        .map(|(server, tool)| (server.to_owned(), tool.to_owned())),
                    respond,
                });
                rx.await.unwrap_or(false)
            };
            let result = if approved {
                emit(ChatEvent::ToolRunning(call.id.clone()));
                match (
                    &req.delegation,
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

#[cfg(test)]
mod tests {
    //! End-to-end: a mock OpenAI-compatible server that asks for a tool call, routed to
    //! the reference MCP "everything" server. Needs `npx` + network, so it's ignored by
    //! default: `cargo test -- --ignored`.
    use super::*;
    use crate::config::{McpServerConfig, McpTransportConfig};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn mock_llm() -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bodies: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Arc::default();
        let seen = bodies.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let body = loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len: usize = text[..h]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            break serde_json::from_slice::<serde_json::Value>(
                                &buf[h + 4..h + 4 + len],
                            )
                            .unwrap();
                        }
                    }
                };
                let has_tool_result = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["role"] == "tool");
                seen.lock().unwrap().push(body);
                let chunks: Vec<String> = if has_tool_result {
                    vec![
                        r#"{"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#.into(),
                        r#"{"choices":[{"delta":{"content":"The tool "}}]}"#.into(),
                        r#"{"choices":[{"delta":{"content":"said hi."},"finish_reason":"stop"}]}"#
                            .into(),
                    ]
                } else {
                    vec![
                        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"everything__echo","arguments":""}}]}}]}"#.into(),
                        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"message\":"}}]}}]}"#.into(),
                        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"hi\"}"}}]},"finish_reason":"tool_calls"}]}"#.into(),
                    ]
                };
                let mut sse = String::new();
                for c in chunks {
                    sse.push_str(&format!("data: {c}\n\n"));
                }
                sse.push_str("data: [DONE]\n\n");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                    sse.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{addr}/v1"), bodies)
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

        let bodies = bodies.lock().unwrap();
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
        }
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
            [tools::TIME, tools::CALCULATE, tools::TASKS, tools::DELEGATE]
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
        assert_eq!(names, [tools::TIME, tools::TASKS]);
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

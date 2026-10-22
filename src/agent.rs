//! One "generation": stream the model's answer, run any MCP tool calls it makes, feed
//! the results back, and repeat until the model answers without tools.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::config::{GeneralSettings, ProviderConfig};
use crate::llm::{self, Delta};
use crate::mcp::manager::McpManager;
use crate::types::{ChatMessage, Role, ToolCall};

pub enum ChatEvent {
    /// A new, empty assistant message begins.
    AssistantStart,
    ContentDelta(String),
    ReasoningDelta(String),
    /// Replaces the in-progress assistant message with its final form (incl. tool calls).
    AssistantFinal(ChatMessage),
    ToolApproval {
        call: ToolCall,
        respond: oneshot::Sender<bool>,
    },
    ToolRunning(String),
    ToolResult(ChatMessage),
    Error(String),
    Done,
}

pub enum AppEvent {
    Chat {
        conv_id: Uuid,
        event: ChatEvent,
    },
    ModelsFetched {
        provider_id: String,
        result: Result<Vec<String>, String>,
    },
}

pub struct AgentRequest {
    pub conv_id: Uuid,
    pub provider: ProviderConfig,
    pub general: GeneralSettings,
    pub history: Vec<ChatMessage>,
}

pub type EventSender = mpsc::UnboundedSender<AppEvent>;

pub async fn run(
    req: AgentRequest,
    http: reqwest::Client,
    mcp: Arc<McpManager>,
    tx: EventSender,
    repaint: impl Fn(),
) {
    let conv_id = req.conv_id;
    let emit = |event: ChatEvent| {
        let _ = tx.send(AppEvent::Chat { conv_id, event });
        repaint();
    };

    let mut history = req.history;
    let max_rounds = req.general.max_tool_rounds.max(1);
    let tool_timeout = Duration::from_secs(req.general.tool_timeout_secs.max(5));

    for round in 0.. {
        // Re-read the router every round so servers that finished connecting are picked up.
        let router = mcp.router();
        let tools = if round < max_rounds {
            router.specs.as_slice()
        } else {
            &[]
        };
        let messages = llm::to_api_messages(&req.general.system_prompt, &history);

        emit(ChatEvent::AssistantStart);
        let result = llm::chat_completion(&http, &req.provider, messages, tools, |d| match d {
            Delta::Content(s) => emit(ChatEvent::ContentDelta(s.to_owned())),
            Delta::Reasoning(s) => emit(ChatEvent::ReasoningDelta(s.to_owned())),
        })
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
        let calls = msg.tool_calls.clone();
        history.push(msg.clone());
        emit(ChatEvent::AssistantFinal(msg));

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
            let approved = if req.general.auto_approve_tools {
                true
            } else {
                let (respond, rx) = oneshot::channel();
                emit(ChatEvent::ToolApproval {
                    call: call.clone(),
                    respond,
                });
                rx.await.unwrap_or(false)
            };
            let result = if approved {
                emit(ChatEvent::ToolRunning(call.id.clone()));
                match router.call(&call.name, &call.arguments, tool_timeout).await {
                    Ok(out) => ChatMessage::tool_result(call, out.text, out.is_error),
                    Err(e) => ChatMessage::tool_result(call, format!("Error: {e:#}"), true),
                }
            } else {
                ChatMessage::tool_result(call, "The user declined to run this tool.", true)
            };
            history.push(result.clone());
            emit(ChatEvent::ToolResult(result));
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
        };
        run(req, http, mcp, tx, || {}).await;

        let mut content = String::new();
        let mut tool_result = None;
        let mut errors = Vec::new();
        while let Ok(AppEvent::Chat { event, .. }) = rx.try_recv() {
            match event {
                ChatEvent::ContentDelta(s) => content.push_str(&s),
                ChatEvent::ToolResult(m) => tool_result = Some(m),
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
        })
        .await;
        if let Some(pid) = child.id() {
            crate::mcp::stdio::kill_process_tree(pid);
        }
        let _ = child.kill().await;
        let _ = std::fs::remove_file(&stderr_path);
    }
}

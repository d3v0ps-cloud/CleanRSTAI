//! Browser remote access: a small web server inside the app turns browser requests into
//! commands that `ChatApp` handles on the UI thread, exactly like a click. This module holds
//! the types both sides share; the server itself lives in `server.rs`.

mod auth;
mod server;
mod tls;

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::EventSender;
use crate::approval::Approval;
use crate::config::{REMOTE_DEFAULT_PORT, RemoteBind, RemoteSettings, RemoteTls};
use crate::types::Role;

use auth::Auth;

/// A request from a browser, sent to the app as `AppEvent::Remote`. Each carries a oneshot
/// sender for the answer.
pub enum RemoteCommand {
    State {
        reply: oneshot::Sender<StateView>,
    },
    Chat {
        id: Uuid,
        reply: oneshot::Sender<Option<ChatView>>,
    },
    NewChat {
        reply: oneshot::Sender<Uuid>,
    },
    Send {
        chat: Option<Uuid>,
        text: String,
        reply: oneshot::Sender<Result<Uuid, String>>,
    },
    Stop {
        chat: Uuid,
        reply: oneshot::Sender<()>,
    },
    Regenerate {
        chat: Uuid,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Approve {
        chat: Uuid,
        call_id: String,
        answer: Approval,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Rename {
        chat: Uuid,
        title: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Pin {
        chat: Uuid,
        pinned: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Delete {
        chat: Uuid,
        reply: oneshot::Sender<()>,
    },
    SetModel {
        provider: usize,
        model: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// Pushed from the app to every connected browser over server-sent events.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteEvent {
    /// The chat list or the models changed: refetch `/api/state`.
    List,
    /// That chat changed: refetch `/api/chats/{id}` if it is open.
    Chat { id: Uuid },
}

/// What the browser shows in the sidebar and model picker.
#[derive(Clone, Debug, Serialize)]
pub struct StateView {
    /// Pinned first, then most recently updated. Only chats with messages or a run.
    pub chats: Vec<ChatSummary>,
    pub providers: Vec<ProviderView>,
    pub active_provider: usize,
    pub send_with_enter: bool,
    /// "system" | "light" | "dark"
    pub theme: String,
    pub user_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatSummary {
    pub id: Uuid,
    pub title: String,
    pub pinned: bool,
    pub updated_at: DateTime<Utc>,
    pub running: bool,
    pub approvals: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderView {
    pub name: String,
    pub model: String,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatView {
    pub id: Uuid,
    pub title: String,
    pub pinned: bool,
    pub messages: Vec<MessageView>,
    pub running: bool,
    pub running_tool: Option<String>,
    pub approvals: Vec<ApprovalView>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MessageView {
    /// "system" | "user" | "assistant" | "tool"
    pub role: Role,
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCallView>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub is_error: bool,
    pub cancelled: bool,
    /// Names only, never the data.
    pub attachments: Vec<AttachmentView>,
    /// A short one-line summary, or None.
    pub stats: Option<String>,
}

/// A tool call the model made, with the name the desktop shows for it.
#[derive(Clone, Debug, Serialize)]
pub struct ToolCallView {
    pub id: String,
    /// The function name the model used, e.g. `builtin__fetch`.
    pub name: String,
    /// JSON-encoded arguments, exactly as produced by the model.
    pub arguments: String,
    /// The desktop's label, e.g. "Web fetch".
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttachmentView {
    pub name: String,
    pub mime: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ApprovalView {
    pub call_id: String,
    pub name: String,
    /// The desktop's label for the tool, e.g. "Web fetch".
    pub label: String,
    pub arguments: String,
    pub server_name: Option<String>,
    pub can_always_server: bool,
}

/// Whether the remote-access server runs, and where.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RemoteStatus {
    #[default]
    Off,
    Running {
        urls: Vec<String>,
        /// True when the server speaks plain HTTP (TLS is off).
        plain_http: bool,
    },
    Failed(String),
}

/// A running server, owned by `ChatApp`. Dropping it stops the server.
pub struct RemoteHandle {
    config: RemoteSettings,
    urls: Vec<String>,
    events: broadcast::Sender<RemoteEvent>,
    task: Option<JoinHandle<()>>,
}

impl RemoteHandle {
    pub(crate) fn new(
        config: RemoteSettings,
        urls: Vec<String>,
        events: broadcast::Sender<RemoteEvent>,
        task: JoinHandle<()>,
    ) -> Self {
        Self {
            config,
            urls,
            events,
            task: Some(task),
        }
    }

    /// The settings the server was started with.
    pub fn config(&self) -> &RemoteSettings {
        &self.config
    }

    /// The addresses the server reports, e.g. `https://127.0.0.1:8484/`.
    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    /// Tells connected browsers about a change. Does nothing when none are connected.
    pub fn notify(&self, event: RemoteEvent) {
        let _ = self.events.send(event);
    }

    /// A receiver for the events the browsers get.
    #[cfg(test)]
    pub fn subscribe(&self) -> broadcast::Receiver<RemoteEvent> {
        self.events.subscribe()
    }

    /// Stops the server task. Connected browsers see their event streams end.
    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for RemoteHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts the server for `cfg` and returns once it is listening, so a busy port or a bad
/// certificate is reported at once. The server runs as a task on `rt`. `tx` carries browser
/// requests to the app; `repaint` wakes the UI thread for each one.
pub fn start(
    rt: &tokio::runtime::Handle,
    cfg: &RemoteSettings,
    tx: EventSender,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> Result<RemoteHandle, String> {
    if cfg.password.trim().is_empty() {
        return Err("Set a password to start the server".into());
    }
    let tls = tls::server_config(cfg)?;
    let ip = match cfg.bind {
        RemoteBind::Local => Ipv4Addr::LOCALHOST,
        RemoteBind::Network => Ipv4Addr::UNSPECIFIED,
    };
    let listen_error = |e: std::io::Error| format!("Can't listen on port {}: {e}", cfg.port);
    let std_listener = bind_with_retry(ip, cfg.port).map_err(listen_error)?;
    std_listener.set_nonblocking(true).map_err(listen_error)?;
    let listener = {
        let _guard = rt.enter();
        tokio::net::TcpListener::from_std(std_listener).map_err(listen_error)?
    };
    let port = listener.local_addr().map_err(listen_error)?.port();

    let shown = RemoteSettings {
        port,
        ..cfg.clone()
    };
    let urls = urls(&shown);
    let (events, _) = broadcast::channel(256);
    let auth = Auth::new(&cfg.password, tls.is_some());
    let hsts = server::hsts_enabled(cfg);
    let task = rt.spawn(server::serve(
        listener,
        tls,
        auth,
        tx,
        repaint,
        events.clone(),
        hsts,
    ));
    Ok(RemoteHandle::new(cfg.clone(), urls, events, task))
}

/// Binds the port. A server that was just stopped (its handle dropped) releases the port only
/// once its task is cancelled, a moment later, so a busy port is retried for up to half a second.
fn bind_with_retry(ip: Ipv4Addr, port: u16) -> std::io::Result<std::net::TcpListener> {
    let mut attempts = 0;
    loop {
        match std::net::TcpListener::bind((ip, port)) {
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempts < 10 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// The addresses to show for these settings: always the loopback address, and for
/// `RemoteBind::Network` also this computer's LAN address when one is found.
pub fn urls(cfg: &RemoteSettings) -> Vec<String> {
    let scheme = if cfg.tls == RemoteTls::Off {
        "http"
    } else {
        "https"
    };
    let port = match cfg.port {
        0 => REMOTE_DEFAULT_PORT,
        port => port,
    };
    let mut hosts = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    if cfg.bind == RemoteBind::Network
        && let Some(lan) = lan_ipv4()
    {
        hosts.push(IpAddr::V4(lan));
    }
    hosts
        .into_iter()
        .map(|host| match host {
            IpAddr::V4(ip) => format!("{scheme}://{ip}:{port}/"),
            IpAddr::V6(ip) => format!("{scheme}://[{ip}]:{port}/"),
        })
        .collect()
}

/// This computer's LAN IPv4 address, found by asking the OS which local address it would
/// route from. Nothing is sent. None when there is no route or only a loopback address.
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_local_self_signed_is_loopback_https() {
        let cfg = RemoteSettings {
            enabled: true,
            tls: RemoteTls::SelfSigned,
            ..RemoteSettings::default()
        };
        assert_eq!(urls(&cfg), vec!["https://127.0.0.1:8484/".to_string()]);
    }

    #[test]
    fn urls_plain_http_when_tls_off() {
        let cfg = RemoteSettings {
            tls: RemoteTls::Off,
            ..RemoteSettings::default()
        };
        assert_eq!(urls(&cfg), vec!["http://127.0.0.1:8484/".to_string()]);
    }

    #[test]
    fn urls_use_the_configured_port() {
        let cfg = RemoteSettings {
            port: 9000,
            ..RemoteSettings::default()
        };
        assert_eq!(urls(&cfg), vec!["https://127.0.0.1:9000/".to_string()]);
    }

    #[test]
    fn urls_network_adds_lan_address_when_found() {
        let cfg = RemoteSettings {
            bind: RemoteBind::Network,
            ..RemoteSettings::default()
        };
        let found = urls(&cfg);
        assert_eq!(found[0], "https://127.0.0.1:8484/");
        match lan_ipv4() {
            Some(lan) => assert_eq!(found[1], format!("https://{lan}:8484/")),
            None => assert_eq!(found.len(), 1),
        }
    }

    #[test]
    fn events_serialize_with_a_type_tag() {
        let id = Uuid::nil();
        assert_eq!(
            serde_json::to_string(&RemoteEvent::List).unwrap(),
            r#"{"type":"list"}"#
        );
        assert_eq!(
            serde_json::to_string(&RemoteEvent::Chat { id }).unwrap(),
            format!(r#"{{"type":"chat","id":"{id}"}}"#)
        );
    }

    #[test]
    fn dropping_the_handle_aborts_the_task() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (tx, _) = broadcast::channel(4);
            let task = tokio::spawn(std::future::pending::<()>());
            let abort = task.abort_handle();
            let handle = RemoteHandle::new(RemoteSettings::default(), Vec::new(), tx, task);
            let mut rx = handle.subscribe();
            drop(handle);
            tokio::task::yield_now().await;
            assert!(abort.is_finished());
            // The event stream ends once the sender is gone.
            assert!(rx.recv().await.is_err());
        });
    }
}

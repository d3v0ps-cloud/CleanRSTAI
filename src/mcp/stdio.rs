//! stdio transport: newline-delimited JSON-RPC over a child process's stdin/stdout.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config::KeyValue;

const STDERR_LINES: usize = 200;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct StdioTransport {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Pending,
    closed: Arc<Mutex<Option<String>>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    tasks: Vec<JoinHandle<()>>,
    child: Child,
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // On Windows `npx` runs as `cmd.exe` → `node`; killing only the direct child would
        // leave `node` running and holding our pipes open.
        if let Some(pid) = self.child.id() {
            kill_process_tree(pid);
        }
        // Aborting the readers closes our pipe ends, so grandchildren (e.g. the `node`
        // started by `npx`) see EOF and exit even though only the direct child is killed.
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Kill a process and all its descendants. Only needed on Windows: elsewhere closing the
/// pipes is enough for well-behaved servers, and the direct child is killed on drop.
pub(crate) fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// Find the executable, honouring a PATH override from the server's env settings.
/// On Windows this also resolves `npx` → `npx.cmd` via PATHEXT.
fn resolve_program(command: &str, env: &[KeyValue], cwd: &str) -> PathBuf {
    let path_override = env
        .iter()
        .find(|kv| kv.key.eq_ignore_ascii_case("PATH"))
        .map(|kv| kv.value.clone());
    let cwd = if cwd.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(cwd)
    };
    let search = path_override.or_else(|| std::env::var("PATH").ok());
    which::which_in(command, search, cwd).unwrap_or_else(|_| PathBuf::from(command))
}

impl StdioTransport {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &[KeyValue],
        cwd: &str,
        on_notify: super::NotifyFn,
    ) -> Result<Self> {
        let command = command.trim();
        if command.is_empty() {
            anyhow::bail!("no command configured");
        }
        let program = resolve_program(command, env, cwd);
        let mut cmd = Command::new(&program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for kv in env.iter().filter(|kv| !kv.key.trim().is_empty()) {
            cmd.env(kv.key.trim(), &kv.value);
        }
        if !cwd.trim().is_empty() {
            cmd.current_dir(cwd.trim());
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to start '{}'", program.display()))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr_pipe = child.stderr.take().context("no stderr")?;

        let stdin = Arc::new(tokio::sync::Mutex::new(stdin));
        let pending: Pending = Arc::default();
        let closed: Arc<Mutex<Option<String>>> = Arc::default();
        let stderr: Arc<Mutex<VecDeque<String>>> = Arc::default();

        let stderr_task = {
            let stderr = stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr_pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    log::debug!("mcp stderr: {line}");
                    let mut buf = stderr.lock().unwrap();
                    if buf.len() >= STDERR_LINES {
                        buf.pop_front();
                    }
                    buf.push_back(line);
                }
            })
        };

        let stdout_task = {
            let (stdin, pending, closed) = (stdin.clone(), pending.clone(), closed.clone());
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                loop {
                    let line = match lines.next_line().await {
                        Ok(Some(l)) => l,
                        Ok(None) => break,
                        Err(e) => {
                            log::warn!("mcp stdout read error: {e}");
                            break;
                        }
                    };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(line) else {
                        log::debug!("mcp: ignoring non-JSON stdout line: {line}");
                        continue;
                    };
                    if let Some(id) = super::response_id(&msg) {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(msg);
                        }
                    } else if msg.get("id").is_some() && msg.get("method").is_some() {
                        let reply = super::reply_to_server_request(&msg);
                        let mut w = stdin.lock().await;
                        let _ = write_line(&mut w, &reply).await;
                    } else {
                        log::debug!("mcp notification: {line}");
                        if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                            on_notify(method);
                        }
                    }
                }
                *closed.lock().unwrap() = Some("server process exited".into());
                // Dropping the senders wakes every waiter with an error.
                pending.lock().unwrap().clear();
            })
        };

        Ok(Self {
            stdin,
            pending,
            closed,
            stderr,
            tasks: vec![stdout_task, stderr_task],
            child,
        })
    }

    fn closed_reason(&self) -> Option<String> {
        self.closed.lock().unwrap().clone()
    }

    pub async fn send(&self, msg: &Value) -> Result<()> {
        if let Some(reason) = self.closed_reason() {
            return Err(anyhow!(reason));
        }
        let mut w = self.stdin.lock().await;
        write_line(&mut w, msg)
            .await
            .context("writing to server stdin")
    }

    pub async fn request(&self, id: u64, msg: &Value) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.send(msg).await {
            self.forget(id);
            return Err(e);
        }
        rx.await.map_err(|_| {
            anyhow!(
                self.closed_reason()
                    .unwrap_or_else(|| "connection closed".into())
            )
        })
    }

    pub fn forget(&self, id: u64) {
        self.pending.lock().unwrap().remove(&id);
    }

    pub fn stderr_lines(&self) -> Vec<String> {
        self.stderr.lock().unwrap().iter().cloned().collect()
    }
}

async fn write_line(w: &mut ChildStdin, msg: &Value) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(msg)?;
    bytes.push(b'\n');
    w.write_all(&bytes).await?;
    w.flush().await
}

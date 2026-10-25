//! A Cloudflare Tunnel for remote access: runs `cloudflared tunnel run` with the tunnel's
//! token, follows its log for the connection state and the public hostnames, and starts it
//! again when it stops. The tunnel's public hostname and the local service it points at are
//! set up in the Cloudflare dashboard. Dropping the handle stops cloudflared.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::config::TunnelSettings;

/// The tunnel puts the server on the internet, so it needs a password at least this long.
pub const MIN_PASSWORD_CHARS: usize = 12;
const RETRY_FIRST: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);
const MAX_ERROR_CHARS: usize = 300;

/// The tunnel's public hostnames, lowercase. The server accepts requests for these names and
/// reads the visitor's address from Cloudflare's header on them.
pub type TunnelHosts = Arc<RwLock<Vec<String>>>;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum TunnelStatus {
    #[default]
    Off,
    /// cloudflared is running but has no connection to Cloudflare yet; the last error it
    /// logged, if any.
    Connecting(Option<String>),
    Connected {
        connections: usize,
        hosts: Vec<String>,
    },
    /// cloudflared stopped. With `retrying` it starts again shortly.
    Failed { error: String, retrying: bool },
}

/// A running tunnel, owned by `ChatApp`.
pub struct TunnelHandle {
    config: TunnelSettings,
    status: Arc<Mutex<TunnelStatus>>,
    hosts: TunnelHosts,
    task: JoinHandle<()>,
}

impl TunnelHandle {
    /// The settings the tunnel was started with.
    pub fn config(&self) -> &TunnelSettings {
        &self.config
    }

    pub fn status(&self) -> TunnelStatus {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        // Aborting drops the child process, which kills it (`kill_on_drop`).
        self.task.abort();
        self.hosts.write().unwrap().clear();
    }
}

/// Starts cloudflared for `cfg`. Fails at once when the token is missing, the password is too
/// short or cloudflared can't be found; everything later shows in `TunnelHandle::status`.
pub fn start(
    rt: &tokio::runtime::Handle,
    cfg: &TunnelSettings,
    password: &str,
    hosts: TunnelHosts,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> Result<TunnelHandle, String> {
    let token = cfg.token.trim().to_owned();
    if token.is_empty() {
        return Err("Paste the tunnel's token to start it".into());
    }
    if password.trim().chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!(
            "The tunnel puts the server on the internet: use a password of at least \
             {MIN_PASSWORD_CHARS} characters"
        ));
    }
    let program = find_cloudflared(&cfg.cloudflared)?;
    let configured: Vec<String> = host_of(&cfg.hostname).into_iter().collect();
    *hosts.write().unwrap() = configured.clone();
    let status = Arc::new(Mutex::new(TunnelStatus::Connecting(None)));
    let task = rt.spawn(supervise(
        program,
        token,
        configured,
        status.clone(),
        hosts.clone(),
        repaint,
    ));
    Ok(TunnelHandle {
        config: cfg.clone(),
        status,
        hosts,
        task,
    })
}

fn find_cloudflared(path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if !path.is_empty() {
        let path = PathBuf::from(path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!("cloudflared not found at {}", path.display()))
        };
    }
    which::which("cloudflared").map_err(|_| {
        "cloudflared isn't installed: install it from Cloudflare, or set its path".into()
    })
}

/// Runs cloudflared until the task is aborted, starting it again after it stops unless the
/// token was refused.
async fn supervise(
    program: PathBuf,
    token: String,
    configured: Vec<String>,
    status: Arc<Mutex<TunnelStatus>>,
    hosts: TunnelHosts,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let set = |s: TunnelStatus| {
        let mut current = status.lock().unwrap();
        if *current != s {
            *current = s;
            repaint();
        }
    };
    let mut delay = RETRY_FIRST;
    loop {
        set(TunnelStatus::Connecting(None));
        let (error, connected) = run_once(&program, &token, &configured, &set, &hosts).await;
        *hosts.write().unwrap() = configured.clone();
        let retrying = !token_refused(&error);
        set(TunnelStatus::Failed { error, retrying });
        if !retrying {
            return;
        }
        if connected {
            delay = RETRY_FIRST;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

/// One cloudflared process, from start to exit. Returns why it stopped and whether it ever
/// connected.
async fn run_once(
    program: &PathBuf,
    token: &str,
    configured: &[String],
    set: &impl Fn(TunnelStatus),
    hosts: &TunnelHosts,
) -> (String, bool) {
    let mut cmd = Command::new(program);
    // The token goes in the environment, not the arguments, so other users can't read it
    // from the process list.
    cmd.args(["tunnel", "--no-autoupdate", "run"])
        .env("TUNNEL_TOKEN", token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // `kill_on_drop` covers a normal quit. On Linux the kernel also stops cloudflared when the
    // app is killed outright, so a tunnel never outlives it.
    #[cfg(target_os = "linux")]
    // SAFETY: the closure runs in the forked child before exec and only calls prctl, which is
    // async-signal-safe and touches no memory shared with the parent.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return (format!("Can't start {}: {e}", program.display()), false),
    };
    let (lines_tx, mut lines) = mpsc::unbounded_channel();
    if let Some(out) = child.stdout.take() {
        forward_lines(out, lines_tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        forward_lines(err, lines_tx);
    }
    let mut log = LogState::default();
    let mut connected = false;
    while let Some(line) = lines.recv().await {
        log::debug!("cloudflared: {line}");
        if !log.feed(&line) {
            continue;
        }
        let mut all = configured.to_vec();
        for host in &log.hosts {
            if !all.contains(host) {
                all.push(host.clone());
            }
        }
        *hosts.write().unwrap() = all.clone();
        if log.connections.is_empty() {
            set(TunnelStatus::Connecting(log.last_error.clone()));
        } else {
            connected = true;
            set(TunnelStatus::Connected {
                connections: log.connections.len(),
                hosts: all,
            });
        }
    }
    let exit = child.wait().await;
    let error = log.last_error.unwrap_or_else(|| match exit {
        Ok(code) => format!("cloudflared stopped ({code})"),
        Err(e) => format!("cloudflared stopped: {e}"),
    });
    (error, connected)
}

fn forward_lines(pipe: impl AsyncRead + Unpin + Send + 'static, tx: mpsc::UnboundedSender<String>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

/// Errors after which starting again won't help: the token is malformed or not accepted.
fn token_refused(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("token is not valid") || e.contains("unauthorized") || e.contains("invalid tunnel")
}

/// What cloudflared's log has said so far.
#[derive(Default)]
struct LogState {
    /// Indexes of the connections to Cloudflare that are up.
    connections: BTreeSet<String>,
    /// Public hostnames from the tunnel's configuration.
    hosts: Vec<String>,
    last_error: Option<String>,
}

impl LogState {
    /// Reads one log line. True when it changed the state.
    ///
    /// Lines look like `2026-10-25T21:17:25Z INF Registered tunnel connection connIndex=0 …`.
    /// A line without a timestamp and level is cloudflared's own complaint before it starts,
    /// such as "Provided Tunnel token is not valid."
    fn feed(&mut self, line: &str) -> bool {
        let line = line.trim();
        if line.is_empty() {
            return false;
        }
        let Some((level, message)) = split_level(line) else {
            if line.starts_with("See '") {
                return false;
            }
            self.last_error = Some(clip(line));
            return true;
        };
        if message.starts_with("Registered tunnel connection") {
            if let Some(index) = field(message, "connIndex") {
                self.last_error = None;
                return self.connections.insert(index.to_owned());
            }
        } else if message.starts_with("Unregistered tunnel connection")
            || message.starts_with("Connection terminated")
            || message.starts_with("Lost connection")
        {
            if let Some(index) = field(message, "connIndex") {
                return self.connections.remove(index);
            }
        } else if message.starts_with("Updated to new configuration") {
            let hosts = config_hostnames(message);
            if hosts != self.hosts {
                self.hosts = hosts;
                return true;
            }
        } else if level == "ERR" && !is_hint(message) {
            self.last_error = Some(clip(message));
            return true;
        }
        false
    }
}

/// Splits `<timestamp> <LEVEL> <message>`.
fn split_level(line: &str) -> Option<(&str, &str)> {
    let (stamp, rest) = line.split_once(' ')?;
    if !stamp.as_bytes().first().is_some_and(u8::is_ascii_digit) || !stamp.contains('T') {
        return None;
    }
    let (level, message) = rest.split_once(' ')?;
    matches!(level, "DBG" | "INF" | "WRN" | "ERR" | "FTL").then_some((level, message.trim()))
}

/// The follow-up lines cloudflared prints under an error ("Please try the following…",
/// numbered steps), which say less than the error itself.
fn is_hint(message: &str) -> bool {
    message.starts_with("Please try")
        || message.starts_with("Run your")
        || message
            .split_once('.')
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The value of `key=value` in a log message.
fn field<'a>(message: &'a str, key: &str) -> Option<&'a str> {
    message.split_whitespace().find_map(|part| {
        part.strip_prefix(key)?
            .strip_prefix('=')
            .filter(|v| !v.is_empty())
    })
}

/// The `hostname`s in the tunnel configuration that cloudflared logs as escaped JSON:
/// `config="{\"ingress\":[{\"hostname\":\"chat.example.com\", …`.
fn config_hostnames(message: &str) -> Vec<String> {
    let unescaped = message.replace("\\\"", "\"");
    let mut hosts = Vec::new();
    let mut rest = unescaped.as_str();
    while let Some(at) = rest.find("\"hostname\"") {
        rest = &rest[at + "\"hostname\"".len()..];
        let value = rest
            .trim_start()
            .strip_prefix(':')
            .map(str::trim_start)
            .and_then(|v| v.strip_prefix('"'))
            .and_then(|v| v.split_once('"'))
            .map(|(v, _)| v);
        if let Some(host) = value.and_then(host_of)
            && !hosts.contains(&host)
        {
            hosts.push(host);
        }
    }
    hosts
}

/// The bare, lowercase host name of `chat.example.com`, `https://chat.example.com/` or
/// `chat.example.com:443`. None when empty.
pub fn host_of(address: &str) -> Option<String> {
    let mut host = address.trim();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = host.strip_prefix(scheme) {
            host = rest;
        }
    }
    let host = host.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(name, _)| name);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn clip(text: &str) -> String {
    let mut s: String = text.chars().take(MAX_ERROR_CHARS).collect();
    if text.chars().count() > MAX_ERROR_CHARS {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTERED: &str = "2026-10-25T21:20:01Z INF Registered tunnel connection \
        connIndex=0 connection=5a0e7f1c-1111-2222-3333-444455556666 event=0 ip=198.41.200.13 \
        location=lhr01 protocol=quic";
    const CONFIG: &str = r#"2026-10-25T21:20:01Z INF Updated to new configuration config="{\"ingress\":[{\"hostname\":\"Chat.Example.com\", \"originRequest\":{\"noTLSVerify\":true}, \"service\":\"https://localhost:8484\"}, {\"service\":\"http_status:404\"}], \"warp-routing\":{\"enabled\":false}}" version=2"#;

    #[test]
    fn registered_connections_count_and_clear_the_error() {
        let mut log = LogState::default();
        assert!(log.feed("2026-10-25T21:20:00Z ERR edge discovery: the DNS query failed event=0"));
        assert!(
            log.last_error
                .as_deref()
                .unwrap()
                .starts_with("edge discovery")
        );
        assert!(log.feed(REGISTERED));
        assert_eq!(log.connections.len(), 1);
        assert_eq!(log.last_error, None);
        assert!(log.feed(&REGISTERED.replace("connIndex=0", "connIndex=1")));
        assert_eq!(log.connections.len(), 2);
        assert!(
            log.feed(
                "2026-10-25T21:30:00Z WRN Connection terminated error=\"timeout\" connIndex=1"
            )
        );
        assert_eq!(log.connections.len(), 1);
        assert!(log.feed("2026-10-25T21:31:00Z ERR Unregistered tunnel connection connIndex=0"));
        assert!(log.connections.is_empty());
    }

    #[test]
    fn hostnames_come_from_the_logged_configuration() {
        let mut log = LogState::default();
        assert!(log.feed(CONFIG));
        assert_eq!(log.hosts, vec!["chat.example.com".to_string()]);
        // The same configuration again changes nothing.
        assert!(!log.feed(CONFIG));
    }

    #[test]
    fn hints_under_an_error_do_not_replace_it() {
        let mut log = LogState::default();
        log.feed("2026-10-25T21:17:37Z ERR edge discovery: error looking up Cloudflare edge IPs");
        assert!(!log.feed("2026-10-25T21:17:37Z ERR Please try the following things event=0"));
        assert!(!log.feed("2026-10-25T21:17:37Z ERR   1. ensure that argotunnel.com is returning"));
        assert!(log.last_error.unwrap().starts_with("edge discovery"));
    }

    #[test]
    fn a_plain_complaint_before_start_is_the_error() {
        let mut log = LogState::default();
        assert!(log.feed("Provided Tunnel token is not valid."));
        assert!(!log.feed("See 'cloudflared tunnel run --help'."));
        let error = log.last_error.unwrap();
        assert_eq!(error, "Provided Tunnel token is not valid.");
        assert!(token_refused(&error));
        assert!(!token_refused("edge discovery: the DNS query failed"));
    }

    #[test]
    fn info_and_warning_lines_are_ignored() {
        let mut log = LogState::default();
        assert!(!log.feed("2026-10-25T21:17:25Z INF Starting tunnel tunnelID=1111"));
        assert!(!log.feed("2026-10-25T21:17:25Z WRN ICMP proxy feature is disabled"));
        assert_eq!(log.last_error, None);
    }

    #[test]
    fn host_of_strips_scheme_path_and_port() {
        assert_eq!(
            host_of("chat.example.com").as_deref(),
            Some("chat.example.com")
        );
        assert_eq!(
            host_of(" https://Chat.Example.com/app?x=1 ").as_deref(),
            Some("chat.example.com")
        );
        assert_eq!(
            host_of("chat.example.com:443").as_deref(),
            Some("chat.example.com")
        );
        assert_eq!(host_of("  "), None);
    }

    #[test]
    fn start_checks_token_password_and_program() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let hosts = TunnelHosts::default();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let cfg = TunnelSettings {
            enabled: true,
            token: "tok".into(),
            cloudflared: "/no/such/cloudflared".into(),
            ..TunnelSettings::default()
        };
        let err = |cfg: &TunnelSettings, pw: &str| {
            start(rt.handle(), cfg, pw, hosts.clone(), repaint.clone())
                .err()
                .unwrap()
        };
        let no_token = TunnelSettings {
            token: " ".into(),
            ..cfg.clone()
        };
        assert!(err(&no_token, "a long enough password").contains("token"));
        assert!(err(&cfg, "short").contains("at least 12"));
        assert!(err(&cfg, "a long enough password").contains("/no/such/cloudflared"));
    }

    /// A stand-in for cloudflared: a shell script that prints log lines, then waits.
    #[cfg(unix)]
    #[test]
    fn runs_cloudflared_and_follows_its_log() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("cleanrstai-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("cloudflared");
        let config = CONFIG.replace('\'', "'\\''");
        let registered = REGISTERED;
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 [ \"$*\" = 'tunnel --no-autoupdate run' ] || exit 3\n\
                 [ \"$TUNNEL_TOKEN\" = 'secret-token' ] || exit 4\n\
                 echo '{config}' >&2\n\
                 echo '{registered}' >&2\n\
                 sleep 30\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let hosts = TunnelHosts::default();
        let cfg = TunnelSettings {
            enabled: true,
            token: "secret-token".into(),
            hostname: "https://typed.example.com/".into(),
            cloudflared: script.display().to_string(),
        };
        let handle = start(
            rt.handle(),
            &cfg,
            "a long enough password",
            hosts.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !matches!(handle.status(), TunnelStatus::Connected { .. }) {
            assert!(
                std::time::Instant::now() < deadline,
                "status: {:?}",
                handle.status()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            handle.status(),
            TunnelStatus::Connected {
                connections: 1,
                hosts: vec!["typed.example.com".into(), "chat.example.com".into()],
            }
        );
        assert_eq!(hosts.read().unwrap().len(), 2);
        drop(handle);
        assert!(hosts.read().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

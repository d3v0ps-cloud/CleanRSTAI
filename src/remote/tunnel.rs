//! A Cloudflare Tunnel for remote access. A quick tunnel runs `cloudflared tunnel --url` and
//! gets a random `trycloudflare.com` address; a named tunnel runs `cloudflared tunnel run` with
//! the tunnel's token, its hostname published through the API (`cloudflare.rs`) or in the
//! dashboard. Either way the app follows cloudflared's log for the connection state and the
//! public hostnames, and starts it again when it stops. Dropping the handle stops cloudflared.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::cloudflare;
use crate::config::{RemoteSettings, RemoteTls, TunnelMode, TunnelSettings};

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

/// Whether a named tunnel's hostname was published through the Cloudflare API.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PublishStatus {
    /// Not asked for: a quick tunnel, or no API token.
    #[default]
    Off,
    Working,
    /// What was published, e.g. `https://chat.example.com/ → https://127.0.0.1:8484`.
    Done(String),
    Failed(String),
}

/// A running tunnel, owned by `ChatApp`.
pub struct TunnelHandle {
    config: TunnelSettings,
    service: String,
    status: Arc<Mutex<TunnelStatus>>,
    publish: Arc<Mutex<PublishStatus>>,
    hosts: TunnelHosts,
    task: JoinHandle<()>,
}

impl TunnelHandle {
    /// The settings the tunnel was started with.
    pub fn config(&self) -> &TunnelSettings {
        &self.config
    }

    /// The local address cloudflared sends requests to.
    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn status(&self) -> TunnelStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn publish_status(&self) -> PublishStatus {
        self.publish.lock().unwrap().clone()
    }
}

impl Drop for TunnelHandle {
    fn drop(&mut self) {
        // Aborting drops the child process, which kills it (`kill_on_drop`).
        self.task.abort();
        self.hosts.write().unwrap().clear();
    }
}

/// Where cloudflared sends the tunnel's requests: this server on the loopback address.
pub fn service_url(remote: &RemoteSettings) -> String {
    let scheme = if remote.tls == RemoteTls::Off {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://127.0.0.1:{}", remote.port)
}

/// How to run cloudflared.
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    /// A named tunnel's token, passed in `TUNNEL_TOKEN`.
    token: Option<String>,
    /// Hostnames known before cloudflared reports any.
    hosts: Vec<String>,
}

/// Starts cloudflared for the tunnel in `remote`, which reaches the server those settings
/// describe. Fails at once when something needed is missing or the password is too short;
/// everything later shows in `TunnelHandle::status` and `publish_status`.
pub fn start(
    rt: &tokio::runtime::Handle,
    http: reqwest::Client,
    remote: &RemoteSettings,
    hosts: TunnelHosts,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> Result<TunnelHandle, String> {
    let cfg = &remote.tunnel;
    if remote.password.trim().chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!(
            "The tunnel puts the server on the internet: use a password of at least \
             {MIN_PASSWORD_CHARS} characters"
        ));
    }
    let service = service_url(remote);
    let https = remote.tls != RemoteTls::Off;
    let mut publish = None;
    let launch = match cfg.mode {
        TunnelMode::Quick => {
            let mut args: Vec<String> = ["tunnel", "--no-autoupdate", "--url", &service]
                .map(String::from)
                .into();
            if https {
                // The certificate is self-signed, or made for another name than 127.0.0.1.
                args.push("--no-tls-verify".into());
            }
            let emails = allowed_emails(&cfg.allowed_emails);
            if !emails.is_empty() {
                args.extend(["--allowed-mail".into(), emails]);
            }
            Launch {
                program: find_cloudflared(&cfg.cloudflared)?,
                args,
                token: None,
                hosts: Vec::new(),
            }
        }
        TunnelMode::Named => {
            let token = cfg.token.trim();
            if token.is_empty() {
                return Err("Paste the tunnel's token to start it".into());
            }
            let hostname = host_of(&cfg.hostname);
            if !cfg.api_token.trim().is_empty() {
                let Some(hostname) = hostname.clone() else {
                    return Err("Enter the public hostname to publish".into());
                };
                publish = Some(cloudflare::Publish {
                    api: cloudflare::API.into(),
                    api_token: cfg.api_token.clone(),
                    tunnel_token: token.to_owned(),
                    hostname,
                    service: service.clone(),
                });
            }
            Launch {
                program: find_cloudflared(&cfg.cloudflared)?,
                args: ["tunnel", "--no-autoupdate", "run"]
                    .map(String::from)
                    .into(),
                token: Some(token.to_owned()),
                hosts: hostname.into_iter().collect(),
            }
        }
    };
    *hosts.write().unwrap() = launch.hosts.clone();
    let status = Arc::new(Mutex::new(TunnelStatus::Connecting(None)));
    let publish_status = Arc::new(Mutex::new(if publish.is_some() {
        PublishStatus::Working
    } else {
        PublishStatus::Off
    }));
    let task = rt.spawn(supervise(
        launch,
        publish.map(|job| (http, job, publish_status.clone())),
        status.clone(),
        hosts.clone(),
        repaint,
    ));
    Ok(TunnelHandle {
        config: cfg.clone(),
        service,
        status,
        publish: publish_status,
        hosts,
        task,
    })
}

/// `a@x.com; *@y.com` → `a@x.com,*@y.com`.
fn allowed_emails(list: &str) -> String {
    list.split([',', ';', ' ', '\n'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect::<Vec<_>>()
        .join(",")
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

/// Publishes the hostname first when asked to, then runs cloudflared until the task is
/// aborted, starting it again after it stops unless the token was refused.
async fn supervise(
    launch: Launch,
    publish: Option<(
        reqwest::Client,
        cloudflare::Publish,
        Arc<Mutex<PublishStatus>>,
    )>,
    status: Arc<Mutex<TunnelStatus>>,
    hosts: TunnelHosts,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    if let Some((http, job, publish_status)) = publish {
        let outcome = match cloudflare::publish(&http, &job).await {
            Ok(done) => PublishStatus::Done(done),
            Err(e) => {
                log::warn!("publishing the tunnel's hostname failed: {e}");
                PublishStatus::Failed(e)
            }
        };
        *publish_status.lock().unwrap() = outcome;
        repaint();
    }
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
        let (error, connected) = run_once(&launch, &set, &hosts).await;
        *hosts.write().unwrap() = launch.hosts.clone();
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
    launch: &Launch,
    set: &impl Fn(TunnelStatus),
    hosts: &TunnelHosts,
) -> (String, bool) {
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // The token goes in the environment, not the arguments, so other users can't read it
    // from the process list.
    if let Some(token) = &launch.token {
        cmd.env("TUNNEL_TOKEN", token);
    }
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
        Err(e) => {
            let error = format!("Can't start {}: {e}", launch.program.display());
            return (error, false);
        }
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
        let mut all = launch.hosts.clone();
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
        if let Some(host) = quick_tunnel_host(message) {
            if self.hosts.contains(&host) {
                return false;
            }
            self.hosts.push(host);
            return true;
        }
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

/// The address in the banner a quick tunnel prints:
/// `|  https://seasonal-deck-organisms-sf.trycloudflare.com  |`.
fn quick_tunnel_host(message: &str) -> Option<String> {
    let at = message.find("https://")?;
    let url = message[at..]
        .split(|c: char| c.is_whitespace() || c == '|')
        .next()?;
    let host = host_of(url)?;
    host.ends_with(".trycloudflare.com").then_some(host)
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
    fn quick_tunnel_address_comes_from_the_banner() {
        let mut log = LogState::default();
        assert!(
            !log.feed(
                "2026-10-26T06:44:20Z INF +--------------------------------------------------+"
            )
        );
        assert!(!log.feed(
            "2026-10-26T06:44:20Z INF |  Your quick Tunnel has been created! Visit it at (it \
             may take some time to be reachable):  |"
        ));
        assert!(log.feed(
            "2026-10-26T06:44:20Z INF |  https://seasonal-deck-organisms-sf.trycloudflare.com \
                     |"
        ));
        assert_eq!(
            log.hosts,
            vec!["seasonal-deck-organisms-sf.trycloudflare.com".to_string()]
        );
        // Cloudflare's terms link on the first line is not a tunnel address.
        let mut other = LogState::default();
        assert!(!other.feed(
            "2026-10-26T06:44:17Z INF Thank you for trying Cloudflare Tunnel. … \
             (https://www.cloudflare.com/website-terms/) …"
        ));
        assert!(other.hosts.is_empty());
    }

    #[test]
    fn allowed_emails_are_joined_with_commas() {
        assert_eq!(
            allowed_emails(" a@x.com; *@y.com\nb@z.org,, "),
            "a@x.com,*@y.com,b@z.org"
        );
        assert_eq!(allowed_emails("  "), "");
    }

    fn remote(tunnel: TunnelSettings) -> RemoteSettings {
        RemoteSettings {
            enabled: true,
            password: "a long enough password".into(),
            tunnel,
            ..RemoteSettings::default()
        }
    }

    #[test]
    fn start_checks_token_password_hostname_and_program() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let err = |remote: &RemoteSettings| {
            start(
                rt.handle(),
                reqwest::Client::new(),
                remote,
                TunnelHosts::default(),
                Arc::new(|| {}),
            )
            .err()
            .unwrap()
        };
        let named = TunnelSettings {
            enabled: true,
            mode: TunnelMode::Named,
            token: "tok".into(),
            cloudflared: "/no/such/cloudflared".into(),
            ..TunnelSettings::default()
        };
        let no_token = TunnelSettings {
            token: " ".into(),
            ..named.clone()
        };
        assert!(err(&remote(no_token)).contains("token"));
        let short = RemoteSettings {
            password: "short".into(),
            ..remote(named.clone())
        };
        assert!(err(&short).contains("at least 12"));
        let publish_without_host = TunnelSettings {
            api_token: "api".into(),
            ..named.clone()
        };
        assert!(err(&remote(publish_without_host)).contains("hostname"));
        assert!(err(&remote(named)).contains("/no/such/cloudflared"));
    }

    #[test]
    fn the_service_is_this_server_on_the_loopback_address() {
        let mut r = RemoteSettings {
            port: 9000,
            ..RemoteSettings::default()
        };
        assert_eq!(service_url(&r), "https://127.0.0.1:9000");
        r.tls = RemoteTls::Off;
        assert_eq!(service_url(&r), "http://127.0.0.1:9000");
    }

    /// Writes a stand-in for cloudflared: a shell script that checks how it was started, prints
    /// `lines` as cloudflared would, then waits.
    #[cfg(unix)]
    fn fake_cloudflared(dir: &std::path::Path, expect_args: &str, token: &str, lines: &[&str]) {
        use std::os::unix::fs::PermissionsExt;
        let echo: String = lines
            .iter()
            .map(|l| format!("echo '{}' >&2\n", l.replace('\'', "'\\''")))
            .collect();
        let script = dir.join("cloudflared");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 [ \"$*\" = '{expect_args}' ] || {{ echo \"bad args: $*\" >&2; exit 3; }}\n\
                 [ \"$TUNNEL_TOKEN\" = '{token}' ] || {{ echo 'bad token' >&2; exit 4; }}\n\
                 {echo}sleep 30\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn wait_connected(handle: &TunnelHandle) -> TunnelStatus {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let status = handle.status();
            if matches!(status, TunnelStatus::Connected { .. }) {
                return status;
            }
            assert!(std::time::Instant::now() < deadline, "status: {status:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_named_tunnel_and_follows_its_log() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        fake_cloudflared(
            &dir,
            "tunnel --no-autoupdate run",
            "secret-token",
            &[CONFIG, REGISTERED],
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let hosts = TunnelHosts::default();
        let cfg = TunnelSettings {
            enabled: true,
            mode: TunnelMode::Named,
            token: "secret-token".into(),
            hostname: "https://typed.example.com/".into(),
            cloudflared: dir.join("cloudflared").display().to_string(),
            ..TunnelSettings::default()
        };
        let handle = start(
            rt.handle(),
            reqwest::Client::new(),
            &remote(cfg),
            hosts.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        assert_eq!(
            wait_connected(&handle),
            TunnelStatus::Connected {
                connections: 1,
                hosts: vec!["typed.example.com".into(), "chat.example.com".into()],
            }
        );
        assert_eq!(handle.publish_status(), PublishStatus::Off);
        assert_eq!(hosts.read().unwrap().len(), 2);
        drop(handle);
        assert!(hosts.read().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_quick_tunnel_to_this_server() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        fake_cloudflared(
            &dir,
            "tunnel --no-autoupdate --url https://127.0.0.1:8484 --no-tls-verify \
             --allowed-mail me@example.com,*@team.example",
            "",
            &[
                "2026-10-26T06:44:20Z INF |  https://brave-new-words.trycloudflare.com  |",
                REGISTERED,
            ],
        );
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let hosts = TunnelHosts::default();
        let cfg = TunnelSettings {
            enabled: true,
            mode: TunnelMode::Quick,
            allowed_emails: "me@example.com, *@team.example".into(),
            // A named tunnel's token is not used, nor passed on.
            token: "unused".into(),
            cloudflared: dir.join("cloudflared").display().to_string(),
            ..TunnelSettings::default()
        };
        let handle = start(
            rt.handle(),
            reqwest::Client::new(),
            &remote(cfg),
            hosts.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        assert_eq!(
            wait_connected(&handle),
            TunnelStatus::Connected {
                connections: 1,
                hosts: vec!["brave-new-words.trycloudflare.com".into()],
            }
        );
        assert_eq!(handle.service(), "https://127.0.0.1:8484");
        drop(handle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

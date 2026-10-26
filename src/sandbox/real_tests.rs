//! Tests of the Docker sandbox against a real Docker daemon. They are `#[ignore]` and do
//! nothing unless `CLEANRSTAI_DOCKER_TESTS=1`. Run them with:
//!
//! `CLEANRSTAI_DOCKER_TESTS=1 cargo test real_tests -- --ignored --nocapture`
//!
//! `CLEANRSTAI_DOCKER_IMAGE` picks the image (default `python:3.13-slim`); it needs `sh`, and
//! `python3` for the network test.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::{Sandbox, SandboxRun};
use crate::config::{DEFAULT_SANDBOX_IMAGE, SandboxNetwork, SandboxSettings};

const ENV_ENABLE: &str = "CLEANRSTAI_DOCKER_TESTS";
const ENV_IMAGE: &str = "CLEANRSTAI_DOCKER_IMAGE";
const LONG: Duration = Duration::from_secs(60);

/// False (and a note on stderr) unless the real-Docker tests are enabled.
fn enabled() -> bool {
    if std::env::var(ENV_ENABLE).as_deref() == Ok("1") {
        return true;
    }
    eprintln!("skipped: set {ENV_ENABLE}=1 to run the real Docker tests");
    false
}

fn settings(network: SandboxNetwork, read_only: bool) -> SandboxSettings {
    SandboxSettings {
        enabled: true,
        image: std::env::var(ENV_IMAGE).unwrap_or_else(|_| DEFAULT_SANDBOX_IMAGE.into()),
        network,
        read_only,
        ..SandboxSettings::default()
    }
}

/// A new empty folder under the system temp dir, deleted on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cleanrstai-real-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create scratch folder");
        Self(std::fs::canonicalize(&dir).expect("canonicalize scratch folder"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn exec(sandbox: &Sandbox, chat: Uuid, root: &Path, command: &str) -> SandboxRun {
    sandbox
        .exec(chat, root, command, LONG)
        .await
        .unwrap_or_else(|e| panic!("exec of {command:?} failed: {e}"))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Asserts that no container with this exact name is left (running or stopped).
fn assert_removed(name: &str) {
    let out = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name=^{name}$"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .expect("run docker ps");
    let left = text(&out.stdout);
    assert!(
        left.trim().is_empty(),
        "container {name} is still there: {left:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn workspace_file_is_visible_at_workspace() {
    if !enabled() {
        return;
    }
    let ws = Scratch::new("ws");
    std::fs::write(ws.0.join("hello.txt"), "hello from the host\n").unwrap();
    let sandbox = Sandbox::new(settings(SandboxNetwork::None, false));
    let chat = Uuid::new_v4();

    let run = exec(
        &sandbox,
        chat,
        &ws.0,
        "cat /workspace/hello.txt && cat /etc/hostname",
    )
    .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", text(&run.stderr));
    let out = text(&run.stdout);
    assert!(out.starts_with("hello from the host\n"), "stdout: {out:?}");
    assert!(
        out.lines().count() >= 2,
        "no hostname after the file: {out:?}"
    );
    let name = run.container.clone();
    drop(sandbox);
    assert_removed(&name);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn host_folder_outside_the_workspace_is_invisible() {
    if !enabled() {
        return;
    }
    let ws = Scratch::new("ws");
    let outside = Scratch::new("outside");
    std::fs::write(outside.0.join("secret.txt"), "host only").unwrap();
    let sandbox = Sandbox::new(settings(SandboxNetwork::None, false));
    let chat = Uuid::new_v4();
    let dir = outside.0.display();

    let ls = exec(&sandbox, chat, &ws.0, &format!("ls {dir}")).await;
    assert_ne!(
        ls.exit_code,
        Some(0),
        "ls of the host folder succeeded: {}",
        text(&ls.stdout)
    );
    assert!(
        !text(&ls.stdout).contains("secret.txt"),
        "listed: {}",
        text(&ls.stdout)
    );

    let cat = exec(&sandbox, chat, &ws.0, &format!("cat {dir}/secret.txt")).await;
    assert_ne!(
        cat.exit_code,
        Some(0),
        "read the host file: {}",
        text(&cat.stdout)
    );
    assert!(
        !text(&cat.stdout).contains("host only"),
        "read the host file"
    );

    let name = ls.container.clone();
    drop(sandbox);
    assert_removed(&name);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn network_none_blocks_outbound_connections() {
    if !enabled() {
        return;
    }
    let ws = Scratch::new("ws");
    let sandbox = Sandbox::new(settings(SandboxNetwork::None, false));
    let chat = Uuid::new_v4();

    let connect = exec(
        &sandbox,
        chat,
        &ws.0,
        "python3 -c \"import socket; socket.create_connection(('1.1.1.1', 53), 2)\"",
    )
    .await;
    assert_ne!(
        connect.exit_code,
        Some(0),
        "connected: {}",
        text(&connect.stdout)
    );
    let err = text(&connect.stderr);
    assert!(err.contains("Errno"), "expected a socket error, got: {err}");

    // Only the loopback interface exists.
    let ifaces = exec(
        &sandbox,
        chat,
        &ws.0,
        "tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '",
    )
    .await;
    assert_eq!(
        ifaces.exit_code,
        Some(0),
        "stderr: {}",
        text(&ifaces.stderr)
    );
    assert_eq!(
        text(&ifaces.stdout).trim(),
        "lo",
        "interfaces: {}",
        text(&ifaces.stdout)
    );

    let name = connect.container.clone();
    drop(sandbox);
    assert_removed(&name);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn files_written_by_the_command_belong_to_the_host_user() {
    use std::os::unix::fs::MetadataExt;

    if !enabled() {
        return;
    }
    let ws = Scratch::new("ws");
    let sandbox = Sandbox::new(settings(SandboxNetwork::None, false));
    let chat = Uuid::new_v4();

    let run = exec(&sandbox, chat, &ws.0, "echo made > /workspace/made.txt").await;
    assert_eq!(run.exit_code, Some(0), "stderr: {}", text(&run.stderr));

    // SAFETY: getuid takes no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    let meta = std::fs::metadata(ws.0.join("made.txt")).expect("file made by the command");
    assert_eq!(
        meta.uid(),
        uid,
        "file owned by uid {} not {uid}",
        meta.uid()
    );

    let name = run.container.clone();
    drop(sandbox);
    assert_removed(&name);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn timeout_kills_the_command_and_its_container() {
    if !enabled() {
        return;
    }
    let ws = Scratch::new("ws");
    let sandbox = Sandbox::new(settings(SandboxNetwork::None, false));
    let chat = Uuid::new_v4();

    let started = Instant::now();
    let run = sandbox
        .exec(chat, &ws.0, "sleep 30", Duration::from_secs(2))
        .await
        .unwrap_or_else(|e| panic!("exec failed: {e}"));
    let elapsed = started.elapsed();

    assert!(run.timed_out, "not reported as timed out");
    assert_eq!(run.exit_code, None);
    assert!(elapsed < Duration::from_secs(20), "took {elapsed:?}");
    assert_removed(&run.container);

    // The chat gets a fresh container for its next command.
    let next = exec(&sandbox, chat, &ws.0, "echo ok").await;
    assert_eq!(next.exit_code, Some(0), "stderr: {}", text(&next.stderr));
    assert_eq!(text(&next.stdout).trim(), "ok");

    let name = next.container.clone();
    drop(sandbox);
    assert_removed(&name);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn read_only_workspace_refuses_writes() {
    if !enabled() {
        return;
    }
    let ws = Scratch::new("ws");
    std::fs::write(ws.0.join("keep.txt"), "kept").unwrap();
    let sandbox = Sandbox::new(settings(SandboxNetwork::None, true));
    let chat = Uuid::new_v4();

    let read = exec(&sandbox, chat, &ws.0, "cat /workspace/keep.txt").await;
    assert_eq!(read.exit_code, Some(0), "stderr: {}", text(&read.stderr));
    assert_eq!(text(&read.stdout), "kept");

    let write = exec(&sandbox, chat, &ws.0, "echo x > /workspace/new.txt").await;
    assert_ne!(
        write.exit_code,
        Some(0),
        "write succeeded in a read-only folder"
    );
    assert!(
        !ws.0.join("new.txt").exists(),
        "new.txt appeared on the host"
    );

    let name = read.container.clone();
    drop(sandbox);
    assert_removed(&name);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon and internet DNS; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn custom_network_allows_only_its_rules() {
    use crate::config::{NetProtocol, SandboxNetRule};
    if !enabled() {
        return;
    }
    // The host as containers on the default bridge see it.
    let out = Command::new("docker")
        .args([
            "network",
            "inspect",
            "bridge",
            "-f",
            "{{(index .IPAM.Config 0).Gateway}}",
        ])
        .output()
        .expect("run docker network inspect");
    let host = text(&out.stdout).trim().to_owned();
    let allowed = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let blocked = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let (allowed_port, blocked_port) = (
        allowed.local_addr().unwrap().port(),
        blocked.local_addr().unwrap().port(),
    );

    let ws = Scratch::new("net");
    let sandbox = Sandbox::new(SandboxSettings {
        rules: vec![
            SandboxNetRule {
                host: host.clone(),
                protocol: NetProtocol::Tcp,
                ports: allowed_port.to_string(),
            },
            SandboxNetRule {
                host: "pypi.org".into(),
                protocol: NetProtocol::Tcp,
                ports: "443".into(),
            },
            SandboxNetRule {
                host: host.clone(),
                protocol: NetProtocol::Icmp,
                ports: String::new(),
            },
        ],
        ..settings(SandboxNetwork::Custom, false)
    });
    let chat = Uuid::new_v4();
    let script = format!(
        r#"python3 - <<'PY'
import socket
def t(name, f):
    try:
        f(); print(name, "ok")
    except Exception as e:
        print(name, "fail", type(e).__name__)
t("allowed-port", lambda: socket.create_connection(("{host}", {allowed_port}), timeout=5))
t("blocked-port", lambda: socket.create_connection(("{host}", {blocked_port}), timeout=5))
t("allowed-domain", lambda: socket.gethostbyname("pypi.org"))
t("other-domain", lambda: socket.gethostbyname("example.com"))
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(3)
t("direct-dns", lambda: (s.sendto(b"\0" * 12, ("8.8.8.8", 53)), s.recvfrom(512)))
def ping(addr):
    p = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_ICMP); p.settimeout(3)
    p.sendto(b"\x08\0\0\0\0\0\0\x01ping", (addr, 0))
    p.recvfrom(64)
t("allowed-ping", lambda: ping("{host}"))
t("other-ping", lambda: ping("8.8.8.8"))
PY"#
    );
    let run = exec(&sandbox, chat, &ws.0, &script).await;
    let out = text(&run.stdout);
    for line in [
        "allowed-port ok",
        "blocked-port fail",
        "allowed-domain ok",
        "other-domain fail",
        "direct-dns fail",
        "allowed-ping ok",
        "other-ping fail",
    ] {
        assert!(
            out.lines()
                .any(|l| l == line || l.starts_with(&format!("{line} "))),
            "missing {line:?} in {out}{}",
            text(&run.stderr)
        );
    }

    let gateway = super::network::gateway_name(&run.container);
    sandbox.remove_chat(chat);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let out = Command::new("docker")
            .args(["ps", "-aq", "--filter", &format!("name=^{gateway}$")])
            .output()
            .expect("run docker ps");
        if text(&out.stdout).trim().is_empty() || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_removed(&run.container);
    assert_removed(&gateway);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker daemon; set CLEANRSTAI_DOCKER_TESTS=1"]
async fn subagents_share_the_chats_custom_network() {
    use crate::config::{NetProtocol, SandboxNetRule};
    if !enabled() {
        return;
    }
    let out = Command::new("docker")
        .args([
            "network",
            "inspect",
            "bridge",
            "-f",
            "{{(index .IPAM.Config 0).Gateway}}",
        ])
        .output()
        .expect("run docker network inspect");
    let host = text(&out.stdout).trim().to_owned();
    let allowed = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let blocked = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let (allowed_port, blocked_port) = (
        allowed.local_addr().unwrap().port(),
        blocked.local_addr().unwrap().port(),
    );
    let ws = Scratch::new("subs");
    let sandbox = Sandbox::new(SandboxSettings {
        rules: vec![SandboxNetRule {
            host: host.clone(),
            protocol: NetProtocol::Tcp,
            ports: allowed_port.to_string(),
        }],
        ..settings(SandboxNetwork::Custom, false)
    });
    let chat = Uuid::new_v4();
    let probe = |tag: &str| {
        format!(
            r#"echo {tag} > {tag}.txt; python3 - <<'PY'
import socket
for port in ({allowed_port}, {blocked_port}):
    try:
        socket.create_connection(("{host}", port), timeout=5); print(port, "ok")
    except Exception as e:
        print(port, "fail")
PY"#
        )
    };
    let (a, b) = (sandbox.subagent(chat), sandbox.subagent(chat));
    let (pa, pb) = (probe("a"), probe("b"));
    let (ra, rb) = tokio::join!(a.exec(&ws.0, &pa, LONG), b.exec(&ws.0, &pb, LONG));
    let expected = format!("{allowed_port} ok\n{blocked_port} fail\n");
    for run in [ra.unwrap(), rb.unwrap()] {
        assert_eq!(text(&run.stdout), expected, "{}", text(&run.stderr));
    }
    // Both wrote to the shared folder, and the chat's own container sees their files.
    let main = exec(&sandbox, chat, &ws.0, "cat a.txt b.txt").await;
    assert_eq!(text(&main.stdout), "a\nb\n");

    let gateway = super::chat_gateway(chat);
    let sub = a.name.clone();
    drop(a);
    drop(b);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let out = Command::new("docker")
            .args(["ps", "-aq", "--filter", &format!("name=^{sub}$")])
            .output()
            .expect("run docker ps");
        if text(&out.stdout).trim().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_removed(&sub);
    // The chat's container still has its network.
    let again = exec(&sandbox, chat, &ws.0, "echo still").await;
    assert_eq!(text(&again.stdout), "still\n");
    sandbox.remove_chat(chat);
    std::thread::sleep(Duration::from_secs(3));
    assert_removed(&main.container);
    assert_removed(&gateway);
}

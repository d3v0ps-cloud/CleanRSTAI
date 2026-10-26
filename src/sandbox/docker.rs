//! The `docker` command line: argument builders and small async runners. The sandbox talks to
//! Docker only through this program, never through a socket or a library.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::config::{
    SandboxImageSource, SandboxNetwork, SandboxPull, SandboxSettings, valid_sandbox_image,
};

/// Label set on every sandbox container, so leftovers can be found after a crash.
pub const LABEL: &str = "net.bukn.cleanrstai.sandbox=1";
/// The part of the error from [`docker`] when the wait ran out; callers match on it.
pub(crate) const TIMED_OUT: &str = "didn't finish within";

const NOT_INSTALLED: &str = "Docker isn't installed: install Docker Desktop or Docker Engine, or set its location in Settings → Docker.";
const NOT_RUNNING: &str =
    "Docker isn't running: start Docker Desktop (or the Docker service) and try again.";
/// Label holding the pid of the app that created a container, so [`reap_orphans`] leaves the
/// containers of other running apps alone.
const PID_KEY: &str = "net.bukn.cleanrstai.sandbox.pid";
/// How long the blocking `docker ps` and `docker rm` of [`reap_orphans`] may run.
const REAP_TIMEOUT: Duration = Duration::from_secs(30);
/// Output cap for [`docker_capped`]: what the app keeps of a command's output.
#[allow(dead_code)] // Until the sandbox runs commands through it.
pub(crate) const OUTPUT_CAP: usize = 4 * 1024 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);
const BUILD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Spawn attempts when the executable is busy, and the pause between them.
const SPAWN_TRIES: u32 = 5;
const SPAWN_PAUSE: Duration = Duration::from_millis(20);

/// The `docker` program: the configured value if set, otherwise the first `docker` on PATH.
/// A bare name (no `/` or `\`) is looked up on PATH; anything else is a path.
pub fn program(cfg: &SandboxSettings) -> Result<PathBuf, String> {
    let set = cfg.docker.trim();
    if set.is_empty() {
        return which::which("docker").map_err(|_| NOT_INSTALLED.into());
    }
    if !has_separator(set) {
        return which::which(set).map_err(|_| format!("docker not found on PATH: {set}"));
    }
    let path = PathBuf::from(set);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("docker not found at {}", path.display()))
    }
}

/// Whether a configured program value names a path rather than a bare program name.
fn has_separator(value: &str) -> bool {
    value.contains(['/', '\\'])
}

/// The image commands run in: the typed image, or the tag of the Dockerfile's build.
pub fn image_ref(cfg: &SandboxSettings) -> String {
    match cfg.source {
        SandboxImageSource::Image => cfg.image.trim().to_owned(),
        SandboxImageSource::Dockerfile => {
            let digest = Sha256::digest(cfg.dockerfile.as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("cleanrstai-sandbox:{}", &hex[..12])
        }
    }
}

/// The label that records the pid of this app, for [`reap_orphans`].
pub fn pid_label() -> String {
    format!("{PID_KEY}={}", std::process::id())
}

/// Arguments of `docker run` that create a chat's container (without the program).
pub fn run_args(cfg: &SandboxSettings, name: &str, root: &Path, image: &str) -> Vec<String> {
    let network = match cfg.network {
        SandboxNetwork::None => "none",
        SandboxNetwork::Bridge => "bridge",
    };
    let memory = format!("{}m", cfg.memory_mb);
    let mut args = strings(&[
        "run", "-d", "--init", "--name", name, "--label", LABEL, "--label",
    ]);
    args.push(pid_label());
    args.push("--mount".into());
    args.push(mount_arg(cfg, root));
    args.extend(strings(&[
        "-w",
        "/workspace",
        "--network",
        network,
        "--cpus",
    ]));
    args.push(cfg.cpus.to_string());
    args.extend(strings(&["--memory", &memory, "--memory-swap", &memory]));
    args.push("--pids-limit".into());
    args.push(cfg.pids.to_string());
    args.extend(strings(&[
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
    ]));
    args.extend(user_args());
    // The image's own ENTRYPOINT would otherwise run before `sleep infinity`.
    args.extend(strings(&["-e", "HOME=/tmp", "--entrypoint", ""]));
    args.push(image.into());
    args.extend(strings(&["sleep", "infinity"]));
    args
}

/// Arguments of `docker exec` that run `command` with `sh -c` in the chat's container.
pub fn exec_args(name: &str, command: &str) -> Vec<String> {
    strings(&["exec", "-w", "/workspace", name, "sh", "-c", command])
}

/// The `--mount` value that binds the folder at /workspace. The `src=` pair is CSV-quoted
/// (inner quotes doubled) when the path has a comma or a quote.
fn mount_arg(cfg: &SandboxSettings, root: &Path) -> String {
    let pair = format!("src={}", mount_path(&root.display().to_string()));
    let pair = if pair.contains([',', '"']) {
        format!("\"{}\"", pair.replace('"', "\"\""))
    } else {
        pair
    };
    let mut mount = format!("type=bind,{pair},dst=/workspace");
    if cfg.read_only {
        mount.push_str(",readonly");
    }
    mount
}

/// A Windows verbatim path (`\\?\C:\dir` or `\\?\UNC\server\share`) as docker takes it:
/// the prefix is dropped. Other paths are returned unchanged.
fn mount_path(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.to_owned()
    }
}

/// Runs the container as the host user, so files written to the folder belong to them.
#[cfg(target_os = "linux")]
fn user_args() -> Vec<String> {
    // SAFETY: getuid and getgid take no arguments and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    vec!["--user".into(), format!("{uid}:{gid}")]
}

#[cfg(not(target_os = "linux"))]
fn user_args() -> Vec<String> {
    Vec::new()
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

/// Runs `docker` with `args`, feeding `stdin` when given, and waits at most `timeout`. The
/// child is killed if the wait is abandoned.
pub(crate) async fn docker(
    program: &Path,
    args: &[String],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = spawn_async(&mut cmd)
        .await
        .map_err(|e| format!("Can't run docker: {e}"))?;
    let pipe = child.stdin.take();
    let run = async move {
        if let (Some(mut pipe), Some(text)) = (pipe, stdin) {
            // A docker that exits early closes the pipe; its exit status says why.
            let _ = pipe.write_all(text.as_bytes()).await;
        }
        child.wait_with_output().await
    };
    match tokio::time::timeout(timeout, run).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("Can't run docker: {e}")),
        Err(_) => Err(timed_out(args, timeout)),
    }
}

/// The error text for a `docker` call that ran out of time.
fn timed_out(args: &[String], timeout: Duration) -> String {
    format!(
        "docker {} {TIMED_OUT} {} s",
        args.first().map_or("", String::as_str),
        timeout.as_secs()
    )
}

/// What [`docker_capped`] got back: each stream keeps its first and last `cap / 2` bytes.
#[allow(dead_code)] // Until the sandbox runs commands through it.
#[derive(Debug)]
pub(crate) struct Capped {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Bytes dropped from the middle of stdout.
    pub stdout_cut: usize,
    /// Bytes dropped from the middle of stderr.
    pub stderr_cut: usize,
}

/// Like [`docker`] without stdin, but both output streams are read to the end (so the child
/// never blocks on a full pipe) and only the first and last `cap / 2` bytes of each are kept.
#[allow(dead_code)] // Until the sandbox runs commands through it.
pub(crate) async fn docker_capped(
    program: &Path,
    args: &[String],
    timeout: Duration,
    cap: usize,
) -> Result<Capped, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = spawn_async(&mut cmd)
        .await
        .map_err(|e| format!("Can't run docker: {e}"))?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let run = async move {
        let (out, err, status) = tokio::join!(
            capture_stream(stdout, cap / 2),
            capture_stream(stderr, cap / 2),
            child.wait()
        );
        Ok::<_, std::io::Error>(((out, err), status?))
    };
    match tokio::time::timeout(timeout, run).await {
        Ok(Ok(((out, err), status))) => Ok(Capped {
            status,
            stdout: out.0,
            stdout_cut: out.1,
            stderr: err.0,
            stderr_cut: err.1,
        }),
        Ok(Err(e)) => Err(format!("Can't run docker: {e}")),
        Err(_) => Err(timed_out(args, timeout)),
    }
}

/// Reads a pipe to its end, keeping the first and last `half` bytes. Returns the kept bytes
/// and how many were dropped.
#[allow(dead_code)] // Until the sandbox runs commands through it.
async fn capture_stream<R: AsyncRead + Unpin>(pipe: Option<R>, half: usize) -> (Vec<u8>, usize) {
    let mut kept = Keep::new(half);
    if let Some(mut pipe) = pipe {
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => kept.push(&buf[..n]),
            }
        }
    }
    kept.finish()
}

/// The head and tail of a byte stream, with a count of what was dropped between them.
#[allow(dead_code)] // Until the sandbox runs commands through it.
struct Keep {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    half: usize,
    cut: usize,
}

#[allow(dead_code)] // Until the sandbox runs commands through it.
impl Keep {
    fn new(half: usize) -> Self {
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            half,
            cut: 0,
        }
    }

    fn push(&mut self, data: &[u8]) {
        let room = self.half - self.head.len();
        let (head, rest) = data.split_at(room.min(data.len()));
        self.head.extend_from_slice(head);
        // Only the last `half` bytes of what is left can stay in the tail.
        let skipped = rest.len().saturating_sub(self.half);
        self.cut += skipped;
        let rest = &rest[skipped..];
        let evict = (self.tail.len() + rest.len()).saturating_sub(self.half);
        self.tail.drain(..evict);
        self.cut += evict;
        self.tail.extend(rest);
    }

    fn finish(self) -> (Vec<u8>, usize) {
        let mut out = self.head;
        out.extend(self.tail);
        (out, self.cut)
    }
}

/// The last non-empty line docker wrote to stderr, for error messages.
pub(crate) fn last_line(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no error message")
        .to_owned()
}

/// The Docker server version, or the "isn't running" sentence.
pub async fn server_version(program: &Path) -> Result<String, String> {
    let args = strings(&["version", "--format", "{{.Server.Version}}"]);
    let out = docker(program, &args, None, VERSION_TIMEOUT).await?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(NOT_RUNNING.into())
    }
}

/// Whether `image` exists on this computer (`docker image inspect`).
pub async fn image_present(program: &Path, image: &str) -> Result<bool, String> {
    let args = strings(&["image", "inspect", "--", image]);
    let out = docker(program, &args, None, INSPECT_TIMEOUT).await?;
    Ok(out.status.success())
}

async fn pull(program: &Path, image: &str) -> Result<(), String> {
    let args = strings(&["pull", "--", image]);
    let out = docker(program, &args, None, BUILD_TIMEOUT)
        .await
        .map_err(|e| format!("Couldn't pull {image}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Couldn't pull {image}: {}", last_line(&out)))
    }
}

async fn build(
    program: &Path,
    cfg: &SandboxSettings,
    tag: &str,
    force: bool,
) -> Result<(), String> {
    let mut args = strings(&["build"]);
    if force {
        args.extend(strings(&["--pull", "--no-cache"]));
    }
    args.extend(strings(&["-t", tag, "-"]));
    let out = docker(program, &args, Some(&cfg.dockerfile), BUILD_TIMEOUT)
        .await
        .map_err(|e| format!("Couldn't build the sandbox image: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Couldn't build the sandbox image: {}",
            last_line(&out)
        ))
    }
}

/// Makes the sandbox image ready and returns its reference. `force` pulls or rebuilds
/// whatever the settings say. `pulled_this_session` records an `Always` pull, so the caller
/// pulls only once.
pub async fn ensure_image(
    program: &Path,
    cfg: &SandboxSettings,
    force: bool,
    pulled_this_session: &mut bool,
) -> Result<String, String> {
    let image = image_ref(cfg);
    check_image_name(&image)?;
    match cfg.source {
        SandboxImageSource::Dockerfile => {
            if force || !image_present(program, &image).await? {
                build(program, cfg, &image, force).await?;
            }
            Ok(image)
        }
        SandboxImageSource::Image => {
            let pull_now = if force {
                true
            } else {
                match cfg.pull {
                    SandboxPull::Missing => !image_present(program, &image).await?,
                    SandboxPull::Always => !*pulled_this_session,
                    SandboxPull::Never => {
                        if !image_present(program, &image).await? {
                            return Err(format!(
                                "The image {image} isn't on this computer: pull it, or choose \"Pull when missing\" in Settings → Docker."
                            ));
                        }
                        false
                    }
                }
            };
            if pull_now {
                if let Err(e) = pull(program, &image).await {
                    // An `Always` pull that fails (offline, say) keeps the copy already here.
                    let present = image_present(program, &image).await.unwrap_or(false);
                    if force || cfg.pull != SandboxPull::Always || !present {
                        return Err(e);
                    }
                    log::warn!("{e}. Using the {image} already on this computer.");
                }
                *pulled_this_session = true;
            }
            Ok(image)
        }
    }
}

/// The error for an image name that `docker` would not take as an image reference.
fn check_image_name(image: &str) -> Result<(), String> {
    if valid_sandbox_image(image) {
        Ok(())
    } else {
        Err(format!("The image \"{image}\" isn't valid."))
    }
}

/// For Settings → Docker "Test": the Docker version and whether the image is ready.
pub async fn check(cfg: &SandboxSettings) -> Result<String, String> {
    let image = image_ref(cfg);
    check_image_name(&image)?;
    let program = program(cfg)?;
    let version = server_version(&program).await?;
    let present = image_present(&program, &image).await?;
    let state = match (cfg.source, present) {
        (SandboxImageSource::Image, true) => format!("image {image} is present"),
        (SandboxImageSource::Image, false) => format!("image {image} isn't on this computer yet"),
        (SandboxImageSource::Dockerfile, true) => format!("image {image} is built"),
        (SandboxImageSource::Dockerfile, false) => format!("image {image} isn't built yet"),
    };
    Ok(format!("Docker {version} is running · {state}"))
}

/// For "Pull now" / "Build now": pulls the image or rebuilds the Dockerfile, even when it is
/// already there.
pub async fn prepare(cfg: &SandboxSettings) -> Result<String, String> {
    check_image_name(&image_ref(cfg))?;
    let program = program(cfg)?;
    server_version(&program).await?;
    let mut pulled = false;
    let image = ensure_image(&program, cfg, true, &mut pulled).await?;
    Ok(match cfg.source {
        SandboxImageSource::Image => format!("Pulled {image}"),
        SandboxImageSource::Dockerfile => format!("Built {image}"),
    })
}

/// Removes the containers labelled [`LABEL`] that no running app made: the ones whose
/// [`PID_KEY`] label is not this process and is not a live process (or has no pid label).
/// Blocking, with a 30 s limit on each `docker` call; errors are only logged.
pub fn reap_orphans(program: &Path) {
    let filter = format!("label={LABEL}");
    let listed = output_sync_timeout(
        std_command(program).args([
            "ps",
            "-aq",
            "--filter",
            &filter,
            "--format",
            r#"{{.ID}} {{.Label "net.bukn.cleanrstai.sandbox.pid"}}"#,
        ]),
        REAP_TIMEOUT,
    );
    let stdout = match listed {
        Ok(out) if out.status.success() => out.stdout,
        Ok(out) => {
            log::debug!("docker ps failed: {}", last_line(&out));
            return;
        }
        Err(e) => {
            log::debug!("can't run docker ps: {e}");
            return;
        }
    };
    let ours = std::process::id();
    let ids: Vec<String> = String::from_utf8_lossy(&stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next()?;
            let pid = parts.next().and_then(|p| p.parse::<u32>().ok());
            let orphan = pid != Some(ours) && !pid.is_some_and(process_alive);
            orphan.then(|| id.to_owned())
        })
        .collect();
    if ids.is_empty() {
        return;
    }
    match output_sync_timeout(
        std_command(program).arg("rm").arg("-f").args(&ids),
        REAP_TIMEOUT,
    ) {
        Ok(out) if !out.status.success() => {
            log::debug!("docker rm failed: {}", last_line(&out));
        }
        Err(e) => log::debug!("can't run docker rm: {e}"),
        Ok(_) => {}
    }
}

/// Whether the process `pid` exists (unix: signal 0 succeeds, or it exists but is not ours).
/// Elsewhere it is never reported alive, so every container of another pid is removed.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// Spawns `cmd`. A freshly written executable can be briefly held open by another process's
/// fork, which fails the spawn with `ExecutableFileBusy`, so those failures are retried.
async fn spawn_async(cmd: &mut Command) -> std::io::Result<tokio::process::Child> {
    for _ in 1..SPAWN_TRIES {
        match cmd.spawn() {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy => {
                tokio::time::sleep(SPAWN_PAUSE).await;
            }
            other => return other,
        }
    }
    cmd.spawn()
}

/// Blocking [`spawn_async`]: retries a busy executable the same way.
pub(crate) fn spawn_sync(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    for _ in 1..SPAWN_TRIES {
        match cmd.spawn() {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(SPAWN_PAUSE);
            }
            other => return other,
        }
    }
    cmd.spawn()
}

/// Runs a std command with its output captured, waiting at most `timeout`. The child is
/// killed when the wait runs out (and the call fails with [`ErrorKind::TimedOut`]).
pub(crate) fn output_sync_timeout(
    cmd: &mut std::process::Command,
    timeout: Duration,
) -> std::io::Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_sync(cmd)?;
    let stdout = read_in_thread(child.stdout.take());
    let stderr = read_in_thread(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                format!("didn't finish within {} s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Reads a pipe to its end on its own thread, so a full pipe never stalls the child.
fn read_in_thread<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

pub(crate) fn std_command(program: &Path) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    std::process::Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SandboxSettings {
        SandboxSettings {
            enabled: true,
            ..SandboxSettings::default()
        }
    }

    #[test]
    fn run_args_default_settings() {
        let args = run_args(
            &cfg(),
            "cleanrstai-abc",
            Path::new("/home/me/work"),
            "python:3.13-slim",
        );
        let user = args.iter().position(|a| a == "--user");
        assert_eq!(user.is_some(), cfg!(target_os = "linux"));
        let mut args = args;
        if let Some(i) = user {
            let pair: Vec<String> = args.drain(i..i + 2).collect();
            assert!(pair[1].contains(':'), "{pair:?}");
        }
        let pid = pid_label();
        assert_eq!(
            args,
            strings(&[
                "run",
                "-d",
                "--init",
                "--name",
                "cleanrstai-abc",
                "--label",
                LABEL,
                "--label",
                &pid,
                "--mount",
                "type=bind,src=/home/me/work,dst=/workspace",
                "-w",
                "/workspace",
                "--network",
                "none",
                "--cpus",
                "2",
                "--memory",
                "2048m",
                "--memory-swap",
                "2048m",
                "--pids-limit",
                "512",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "-e",
                "HOME=/tmp",
                "--entrypoint",
                "",
                "python:3.13-slim",
                "sleep",
                "infinity",
            ])
        );
    }

    #[test]
    fn run_args_bridge_read_only_and_limits() {
        let settings = SandboxSettings {
            network: SandboxNetwork::Bridge,
            read_only: true,
            cpus: 0.5,
            memory_mb: 512,
            pids: 64,
            ..cfg()
        };
        let args = run_args(&settings, "n", Path::new("/w"), "img:1");
        let pairs: Vec<(&str, &str)> = args
            .windows(2)
            .map(|w| (w[0].as_str(), w[1].as_str()))
            .collect();
        assert!(pairs.contains(&("--mount", "type=bind,src=/w,dst=/workspace,readonly")));
        assert!(pairs.contains(&("--network", "bridge")));
        assert!(pairs.contains(&("--cpus", "0.5")));
        assert!(pairs.contains(&("--memory", "512m")));
        assert!(pairs.contains(&("--memory-swap", "512m")));
        assert!(pairs.contains(&("--pids-limit", "64")));
        assert_eq!(
            &args[args.len() - 3..],
            strings(&["img:1", "sleep", "infinity"])
        );
    }

    #[test]
    fn mount_quotes_a_path_with_a_comma_or_quote() {
        // The `src=` pair is CSV-quoted with inner quotes doubled; the docker CLI accepts it
        // (checked with `docker run --mount` by hand: a parse error only without the quotes).
        let comma = run_args(&cfg(), "n", Path::new("/a,b\"c"), "img");
        assert!(comma.contains(&"type=bind,\"src=/a,b\"\"c\",dst=/workspace".to_owned()));
        let quote = run_args(&cfg(), "n", Path::new("/x\"y"), "img");
        assert!(quote.contains(&"type=bind,\"src=/x\"\"y\",dst=/workspace".to_owned()));
        let plain = run_args(&cfg(), "n", Path::new("/x y"), "img");
        assert!(plain.contains(&"type=bind,src=/x y,dst=/workspace".to_owned()));
    }

    #[test]
    fn mount_quoted_path_keeps_readonly_outside_the_quotes() {
        let settings = SandboxSettings {
            read_only: true,
            ..cfg()
        };
        let args = run_args(&settings, "n", Path::new("/a,b\"c"), "img");
        assert!(args.contains(&"type=bind,\"src=/a,b\"\"c\",dst=/workspace,readonly".to_owned()));
    }

    #[test]
    fn mount_path_drops_the_windows_verbatim_prefix() {
        assert_eq!(mount_path(r"\\?\C:\work\dir"), r"C:\work\dir");
        assert_eq!(
            mount_path(r"\\?\UNC\server\share\dir"),
            r"\\server\share\dir"
        );
        assert_eq!(mount_path(r"C:\work"), r"C:\work");
        assert_eq!(mount_path("/home/me/work"), "/home/me/work");
        let args = run_args(&cfg(), "n", Path::new(r"\\?\UNC\srv\share"), "img");
        assert!(args.contains(&r"type=bind,src=\\srv\share,dst=/workspace".to_owned()));
    }

    #[test]
    fn has_separator_tells_a_name_from_a_path() {
        assert!(!has_separator("docker"));
        assert!(!has_separator("docker-desktop"));
        assert!(has_separator("/usr/bin/docker"));
        assert!(has_separator(r"C:\Program Files\docker.exe"));
        assert!(has_separator("bin/docker"));
    }

    #[cfg(unix)]
    #[test]
    fn program_resolves_a_bare_name_on_path() {
        let bare = SandboxSettings {
            docker: "sh".into(),
            ..cfg()
        };
        let found = program(&bare).unwrap();
        assert!(found.is_absolute(), "{found:?}");
        assert!(found.ends_with("sh"));
        let missing = SandboxSettings {
            docker: "cleanrstai-no-such-program".into(),
            ..cfg()
        };
        assert_eq!(
            program(&missing).unwrap_err(),
            "docker not found on PATH: cleanrstai-no-such-program"
        );
    }

    #[test]
    fn run_args_clears_the_entrypoint_just_before_the_image() {
        let args = run_args(&cfg(), "n", Path::new("/w"), "img");
        assert_eq!(
            &args[args.len() - 5..],
            strings(&["--entrypoint", "", "img", "sleep", "infinity"])
        );
    }

    #[test]
    fn exec_args_run_sh_in_the_workspace() {
        assert_eq!(
            exec_args("cleanrstai-abc", "ls -la && echo 'hi'"),
            strings(&[
                "exec",
                "-w",
                "/workspace",
                "cleanrstai-abc",
                "sh",
                "-c",
                "ls -la && echo 'hi'"
            ])
        );
    }

    #[test]
    fn image_ref_for_an_image_is_trimmed() {
        let settings = SandboxSettings {
            image: "  ghcr.io/me/tools:1.2 \n".into(),
            ..cfg()
        };
        assert_eq!(image_ref(&settings), "ghcr.io/me/tools:1.2");
    }

    #[test]
    fn image_ref_for_a_dockerfile_is_a_stable_hash_tag() {
        let a = SandboxSettings {
            source: SandboxImageSource::Dockerfile,
            dockerfile: "FROM alpine\n".into(),
            ..cfg()
        };
        let tag = image_ref(&a);
        let hex = tag.strip_prefix("cleanrstai-sandbox:").expect("prefix");
        assert_eq!(hex.len(), 12);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(image_ref(&a.clone()), tag);
        let b = SandboxSettings {
            dockerfile: "FROM alpine:3.20\n".into(),
            ..a
        };
        assert_ne!(image_ref(&b), tag);
    }

    #[test]
    fn program_uses_a_configured_path_only_if_it_exists() {
        let missing = SandboxSettings {
            docker: "/nonexistent/cleanrstai/docker".into(),
            ..cfg()
        };
        assert_eq!(
            program(&missing).unwrap_err(),
            "docker not found at /nonexistent/cleanrstai/docker"
        );
        let present = SandboxSettings {
            docker: std::env::current_exe().unwrap().display().to_string(),
            ..cfg()
        };
        assert_eq!(program(&present).unwrap(), PathBuf::from(&present.docker));
    }

    #[cfg(unix)]
    mod fake {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A `docker` stand-in in its own temp folder. It logs its arguments to `log` and
        /// answers by subcommand. Marker files in the folder change its answers: `down`
        /// (version fails), `present` (the image exists), `fail` (pull and build fail).
        /// `ps` prints the file `ps.out` when a test writes one (`id pid` lines).
        pub struct Fake {
            pub dir: PathBuf,
            pub program: PathBuf,
        }

        impl Fake {
            pub fn new() -> Self {
                let dir =
                    std::env::temp_dir().join(format!("cleanrstai-test-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir).unwrap();
                let program = dir.join("docker");
                let script = format!(
                    "#!/bin/sh\n\
                     dir='{}'\n\
                     echo \"$*\" >> \"$dir/log\"\n\
                     case \"$1\" in\n\
                     version)\n\
                       if [ -f \"$dir/down\" ]; then echo 'Cannot connect to the Docker daemon' >&2; exit 1; fi\n\
                       echo 27.3.1 ;;\n\
                     image) test -f \"$dir/present\" ;;\n\
                     pull)\n\
                       if [ -f \"$dir/fail\" ]; then echo 'pulling' >&2; echo 'denied: access denied' >&2; exit 1; fi\n\
                       touch \"$dir/present\" ;;\n\
                     build)\n\
                       cat > \"$dir/dockerfile.seen\"\n\
                       if [ -f \"$dir/fail\" ]; then echo 'Step 1 failed' >&2; echo 'no such file' >&2; exit 1; fi\n\
                       touch \"$dir/present\" ;;\n\
                     ps) cat \"$dir/ps.out\" 2>/dev/null ;;\n\
                     rm) ;;\n\
                     *) exit 2 ;;\n\
                     esac\n",
                    dir.display()
                );
                std::fs::write(&program, script).unwrap();
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
                Self { dir, program }
            }

            pub fn mark(&self, name: &str) {
                std::fs::write(self.dir.join(name), "").unwrap();
            }

            /// Every logged call, in order.
            pub fn calls(&self) -> Vec<String> {
                std::fs::read_to_string(self.dir.join("log"))
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned)
                    .collect()
            }

            pub fn count(&self, prefix: &str) -> usize {
                self.calls()
                    .iter()
                    .filter(|c| c.starts_with(prefix))
                    .count()
            }
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn rt() -> tokio::runtime::Runtime {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        }

        #[test]
        fn server_version_reads_the_daemon_version() {
            let fake = Fake::new();
            let v = rt().block_on(server_version(&fake.program)).unwrap();
            assert_eq!(v, "27.3.1");
            assert!(
                fake.calls()
                    .contains(&"version --format {{.Server.Version}}".to_owned())
            );
        }

        #[test]
        fn server_version_says_docker_is_not_running() {
            let fake = Fake::new();
            fake.mark("down");
            let err = rt().block_on(server_version(&fake.program)).unwrap_err();
            assert_eq!(err, NOT_RUNNING);
        }

        #[test]
        fn docker_helper_times_out_and_kills_the_child() {
            let err = rt()
                .block_on(docker(
                    Path::new("/bin/sleep"),
                    &strings(&["5"]),
                    None,
                    Duration::from_millis(200),
                ))
                .unwrap_err();
            assert_eq!(err, "docker 5 didn't finish within 0 s");
        }

        #[test]
        fn missing_image_with_missing_pull_policy_pulls_only_when_absent() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                pull: SandboxPull::Missing,
                ..cfg()
            };
            let rt = rt();
            let mut pulled = false;
            let img = rt
                .block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert_eq!(img, "python:3.13-slim");
            assert_eq!(fake.count("pull "), 1);
            // Now present: no second pull.
            rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert_eq!(fake.count("pull "), 1);
        }

        #[test]
        fn always_pull_policy_pulls_once_per_flag() {
            let fake = Fake::new();
            fake.mark("present");
            let settings = SandboxSettings {
                pull: SandboxPull::Always,
                ..cfg()
            };
            let rt = rt();
            let mut pulled = false;
            rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert!(pulled);
            assert_eq!(fake.count("pull "), 1);
            rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert_eq!(fake.count("pull "), 1);
            // A forced pull ignores the flag.
            rt.block_on(ensure_image(&fake.program, &settings, true, &mut pulled))
                .unwrap();
            assert_eq!(fake.count("pull "), 2);
        }

        #[test]
        fn never_policy_fails_when_the_image_is_absent() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                pull: SandboxPull::Never,
                ..cfg()
            };
            let mut pulled = false;
            let err = rt()
                .block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap_err();
            assert_eq!(
                err,
                "The image python:3.13-slim isn't on this computer: pull it, or choose \"Pull when missing\" in Settings → Docker."
            );
            assert_eq!(fake.count("pull "), 0);
            fake.mark("present");
            rt().block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert_eq!(fake.count("pull "), 0);
        }

        #[test]
        fn failed_pull_names_the_last_stderr_line() {
            let fake = Fake::new();
            fake.mark("fail");
            let settings = SandboxSettings {
                pull: SandboxPull::Always,
                ..cfg()
            };
            let mut pulled = false;
            let err = rt()
                .block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap_err();
            assert_eq!(err, "Couldn't pull python:3.13-slim: denied: access denied");
            assert!(!pulled);
        }

        #[test]
        fn dockerfile_builds_once_and_passes_the_text_on_stdin() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                source: SandboxImageSource::Dockerfile,
                dockerfile: "FROM alpine:3.20\nRUN echo hi\n".into(),
                ..cfg()
            };
            let tag = image_ref(&settings);
            let rt = rt();
            let mut pulled = false;
            assert_eq!(
                rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                    .unwrap(),
                tag
            );
            assert_eq!(
                std::fs::read_to_string(fake.dir.join("dockerfile.seen")).unwrap(),
                settings.dockerfile
            );
            assert!(fake.calls().contains(&format!("build -t {tag} -")));
            rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert_eq!(fake.count("build"), 1);
            assert_eq!(fake.count("pull "), 0);
        }

        #[test]
        fn forced_rebuild_pulls_the_base_and_skips_the_cache() {
            let fake = Fake::new();
            fake.mark("present");
            let settings = SandboxSettings {
                source: SandboxImageSource::Dockerfile,
                ..cfg()
            };
            let tag = image_ref(&settings);
            let mut pulled = false;
            rt().block_on(ensure_image(&fake.program, &settings, true, &mut pulled))
                .unwrap();
            assert!(
                fake.calls()
                    .contains(&format!("build --pull --no-cache -t {tag} -"))
            );
            assert_eq!(fake.count("build"), 1);
        }

        #[test]
        fn failed_build_names_the_last_stderr_line() {
            let fake = Fake::new();
            fake.mark("fail");
            let settings = SandboxSettings {
                source: SandboxImageSource::Dockerfile,
                ..cfg()
            };
            let mut pulled = false;
            let err = rt()
                .block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap_err();
            assert_eq!(err, "Couldn't build the sandbox image: no such file");
        }

        #[test]
        fn check_and_prepare_report_the_state() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                docker: fake.program.display().to_string(),
                ..cfg()
            };
            let rt = rt();
            assert_eq!(
                rt.block_on(check(&settings)).unwrap(),
                "Docker 27.3.1 is running · image python:3.13-slim isn't on this computer yet"
            );
            assert_eq!(
                rt.block_on(prepare(&settings)).unwrap(),
                "Pulled python:3.13-slim"
            );
            assert_eq!(
                rt.block_on(check(&settings)).unwrap(),
                "Docker 27.3.1 is running · image python:3.13-slim is present"
            );
            fake.mark("down");
            assert_eq!(rt.block_on(check(&settings)).unwrap_err(), NOT_RUNNING);
            assert_eq!(rt.block_on(prepare(&settings)).unwrap_err(), NOT_RUNNING);
        }

        #[test]
        fn check_reports_a_dockerfile_build() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                docker: fake.program.display().to_string(),
                source: SandboxImageSource::Dockerfile,
                ..cfg()
            };
            let tag = image_ref(&settings);
            let rt = rt();
            assert_eq!(
                rt.block_on(check(&settings)).unwrap(),
                format!("Docker 27.3.1 is running · image {tag} isn't built yet")
            );
            assert_eq!(
                rt.block_on(prepare(&settings)).unwrap(),
                format!("Built {tag}")
            );
            assert_eq!(
                rt.block_on(check(&settings)).unwrap(),
                format!("Docker 27.3.1 is running · image {tag} is built")
            );
        }

        #[test]
        fn reap_orphans_removes_containers_no_live_app_made() {
            let fake = Fake::new();
            let ours = std::process::id();
            std::fs::write(
                fake.dir.join("ps.out"),
                format!("mine {ours}\ndead 999999\nalive 1\nnopid\n"),
            )
            .unwrap();
            reap_orphans(&fake.program);
            assert!(
                fake.calls().contains(&format!(
                    "ps -aq --filter label={LABEL} --format {{{{.ID}}}} {{{{.Label \"net.bukn.cleanrstai.sandbox.pid\"}}}}"
                ))
            );
            // Our own containers and the one of a live process stay; the dead pid and the
            // one without a pid label go.
            assert!(
                fake.calls().contains(&"rm -f dead nopid".to_owned()),
                "{:?}",
                fake.calls()
            );
        }

        #[test]
        fn reap_orphans_does_nothing_when_none_are_found() {
            let fake = Fake::new();
            reap_orphans(&fake.program);
            assert_eq!(fake.count("rm"), 0);
            std::fs::write(
                fake.dir.join("ps.out"),
                format!("mine {}\n", std::process::id()),
            )
            .unwrap();
            reap_orphans(&fake.program);
            assert_eq!(fake.count("rm"), 0);
        }

        #[test]
        fn pid_label_is_the_format_key() {
            assert!(pid_label().starts_with(&format!("{PID_KEY}=")));
            assert!(run_args(&cfg(), "n", Path::new("/w"), "img").contains(&pid_label()));
        }

        #[test]
        fn output_sync_timeout_kills_a_slow_child() {
            let mut cmd = std::process::Command::new("/bin/sleep");
            cmd.arg("5");
            let started = Instant::now();
            let err = output_sync_timeout(&mut cmd, Duration::from_millis(200)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(4));
        }

        #[test]
        fn output_sync_timeout_returns_the_output_of_a_quick_child() {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.args(["-c", "echo out; echo err >&2; exit 3"]);
            let out = output_sync_timeout(&mut cmd, Duration::from_secs(10)).unwrap();
            assert_eq!(out.status.code(), Some(3));
            assert_eq!(out.stdout, b"out\n");
            assert_eq!(out.stderr, b"err\n");
        }

        #[test]
        fn image_inspect_and_pull_end_options_before_the_image() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                pull: SandboxPull::Missing,
                ..cfg()
            };
            let mut pulled = false;
            rt().block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert!(
                fake.calls()
                    .contains(&"image inspect -- python:3.13-slim".to_owned())
            );
            assert!(
                fake.calls()
                    .contains(&"pull -- python:3.13-slim".to_owned())
            );
        }

        #[test]
        fn an_invalid_image_is_refused_before_docker_runs() {
            let fake = Fake::new();
            let settings = SandboxSettings {
                image: "-v".into(),
                docker: fake.program.display().to_string(),
                pull: SandboxPull::Always,
                ..cfg()
            };
            let rt = rt();
            let msg = "The image \"-v\" isn't valid.";
            let mut pulled = false;
            assert_eq!(
                rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                    .unwrap_err(),
                msg
            );
            assert_eq!(rt.block_on(check(&settings)).unwrap_err(), msg);
            assert_eq!(rt.block_on(prepare(&settings)).unwrap_err(), msg);
            assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        }

        #[test]
        fn failed_always_pull_keeps_a_present_image() {
            let fake = Fake::new();
            fake.mark("present");
            fake.mark("fail");
            let settings = SandboxSettings {
                pull: SandboxPull::Always,
                docker: fake.program.display().to_string(),
                ..cfg()
            };
            let rt = rt();
            let mut pulled = false;
            assert_eq!(
                rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                    .unwrap(),
                "python:3.13-slim"
            );
            assert!(pulled);
            // Not retried for the rest of the session.
            rt.block_on(ensure_image(&fake.program, &settings, false, &mut pulled))
                .unwrap();
            assert_eq!(fake.count("pull "), 1);
            // A forced pull (Pull now) still reports the failure.
            assert_eq!(
                rt.block_on(ensure_image(&fake.program, &settings, true, &mut pulled))
                    .unwrap_err(),
                "Couldn't pull python:3.13-slim: denied: access denied"
            );
            assert_eq!(
                rt.block_on(prepare(&settings)).unwrap_err(),
                "Couldn't pull python:3.13-slim: denied: access denied"
            );
        }

        #[test]
        fn docker_capped_keeps_both_ends_and_counts_the_middle() {
            let fake = Fake::new();
            // 20000 numbered lines: 110000 bytes on each stream (10000 four-digit numbers of 5
            // bytes, then five-digit ones of 6), more than the pipe buffer, so the child only
            // finishes if both pipes are read to the end.
            let script = "awk 'BEGIN{for(i=0;i<20000;i++){printf \"%04d\\n\", i; printf \"%04d\\n\", i > \"/dev/stderr\"}}'";
            let out = rt()
                .block_on(docker_capped(
                    Path::new("/bin/sh"),
                    &strings(&["-c", script]),
                    Duration::from_secs(20),
                    1000,
                ))
                .unwrap();
            assert!(out.status.success());
            assert_eq!(out.stdout.len(), 1000);
            assert_eq!(out.stdout_cut, 109_000);
            assert!(out.stdout.starts_with(b"0000\n0001\n"));
            assert!(out.stdout.ends_with(b"19998\n19999\n"));
            assert_eq!(out.stderr.len(), 1000);
            assert_eq!(out.stderr_cut, 109_000);
            assert!(out.stderr.ends_with(b"19999\n"));
            // Nothing is cut from a short stream.
            let small = rt()
                .block_on(docker_capped(
                    Path::new("/bin/sh"),
                    &strings(&["-c", "printf hello"]),
                    Duration::from_secs(10),
                    OUTPUT_CAP,
                ))
                .unwrap();
            assert_eq!(small.stdout, b"hello");
            assert_eq!((small.stdout_cut, small.stderr_cut), (0, 0));
            drop(fake);
        }

        #[test]
        fn docker_capped_times_out_like_docker() {
            let err = rt()
                .block_on(docker_capped(
                    Path::new("/bin/sleep"),
                    &strings(&["5"]),
                    Duration::from_millis(200),
                    OUTPUT_CAP,
                ))
                .unwrap_err();
            assert_eq!(err, "docker 5 didn't finish within 0 s");
        }

        #[test]
        fn keep_holds_the_head_and_tail_across_chunks() {
            let data: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
            for chunk in [1, 7, 64, 999, 5000] {
                let mut kept = Keep::new(100);
                for part in data.chunks(chunk) {
                    kept.push(part);
                }
                let (out, cut) = kept.finish();
                assert_eq!(out.len(), 200, "chunk {chunk}");
                assert_eq!(cut, 4800, "chunk {chunk}");
                assert_eq!(&out[..100], &data[..100], "chunk {chunk}");
                assert_eq!(&out[100..], &data[4900..], "chunk {chunk}");
            }
            let mut short = Keep::new(100);
            short.push(b"abc");
            assert_eq!(short.finish(), (b"abc".to_vec(), 0));
        }
    }
}

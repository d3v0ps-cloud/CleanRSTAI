//! Docker sandbox for the workspace shell tool. Commands run in a container through the
//! `docker` command line.

mod docker;
mod network;
#[cfg(test)]
mod real_tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::config::{SandboxNetwork, SandboxSettings};

pub use docker::{check, image_ref, prepare};
pub use network::check_rule;

/// How long `docker run` may take to create a container.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `docker rm` or `docker kill` may take before it is abandoned.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long dropping the manager waits for `docker rm`.
const DROP_WAIT: Duration = Duration::from_secs(5);
/// How long `docker inspect` or `docker logs` may take.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a custom network's gateway may take to put its rules in place.
const GATEWAY_WAIT: Duration = Duration::from_secs(30);

/// Runs workspace shell commands in Docker containers: one per chat, reused between
/// commands, and one per subagent that runs commands. Cheap to clone (Arc inside). Dropping
/// the last clone removes its containers.
#[derive(Clone)]
pub struct Sandbox {
    inner: Arc<Inner>,
}

/// The outcome of one command run in a sandbox container.
pub struct SandboxRun {
    /// The command's exit code; `None` when it was killed.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Bytes dropped from the middle of `stdout` (the output was over the cap).
    pub stdout_cut: usize,
    /// Bytes dropped from the middle of `stderr`.
    pub stderr_cut: usize,
    /// The command ran past its timeout, and its container was killed.
    pub timed_out: bool,
    /// Name of the container the command ran in.
    pub container: String,
    /// The image the container runs.
    pub image: String,
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox")
            .field("image", &self.image())
            .finish()
    }
}

/// Where shell commands run: a chat's container, or a subagent's own container in that chat.
#[derive(Clone, Debug)]
pub enum Place {
    Chat(Sandbox, Uuid),
    Subagent(Arc<Subagent>),
}

impl Place {
    pub fn sandbox(&self) -> &Sandbox {
        match self {
            Self::Chat(sandbox, _) => sandbox,
            Self::Subagent(sub) => &sub.sandbox,
        }
    }

    /// Runs `command` with `sh -c` in this place's container, with `root` at /workspace.
    pub async fn exec(
        &self,
        root: &Path,
        command: &str,
        timeout: Duration,
    ) -> Result<SandboxRun, String> {
        match self {
            Self::Chat(sandbox, chat) => sandbox.exec(*chat, root, command, timeout).await,
            Self::Subagent(sub) => sub.exec(root, command, timeout).await,
        }
    }

    /// A new subagent container in the same chat. It shares the chat's network container.
    pub fn for_subagent(&self) -> Self {
        let chat = match self {
            Self::Chat(_, chat) => *chat,
            Self::Subagent(sub) => sub.chat,
        };
        Self::Subagent(Arc::new(self.sandbox().subagent(chat)))
    }
}

/// A subagent's own container in a chat. It is created by its first command and removed when
/// this handle is dropped.
#[derive(Debug)]
pub struct Subagent {
    sandbox: Sandbox,
    chat: Uuid,
    name: String,
}

impl Subagent {
    pub async fn exec(
        &self,
        root: &Path,
        command: &str,
        timeout: Duration,
    ) -> Result<SandboxRun, String> {
        self.sandbox
            .run(
                self.chat,
                &self.name,
                root,
                command,
                timeout,
                docker::OUTPUT_CAP,
            )
            .await
    }
}

impl Drop for Subagent {
    fn drop(&mut self) {
        if forget(&self.sandbox.inner, &self.name) {
            remove_in_background(&self.sandbox.inner.cfg, std::slice::from_ref(&self.name));
        }
    }
}

struct Inner {
    cfg: SandboxSettings,
    /// The running containers, by name: chats', subagents' and network gateways.
    containers: Mutex<HashMap<String, Container>>,
    /// Whether an `Always` pull already ran this session. Held while the image is made ready,
    /// so chats that start together wait for one pull or build.
    pulled: tokio::sync::Mutex<bool>,
    /// One lock per chat, so two commands can't both create the chat's containers. A chat's
    /// own commands hold it while they run; a subagent's only while its container is made.
    chat_locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Clone)]
struct Container {
    chat: Uuid,
    /// The folder mounted at /workspace; empty for a gateway.
    root: PathBuf,
    /// A custom network's gateway, shared by the chat's containers.
    gateway: bool,
    last_used: Instant,
}

impl Sandbox {
    pub fn new(cfg: SandboxSettings) -> Self {
        Self {
            inner: Arc::new(Inner {
                cfg,
                containers: Mutex::default(),
                pulled: tokio::sync::Mutex::new(false),
                chat_locks: Mutex::default(),
            }),
        }
    }

    pub fn config(&self) -> &SandboxSettings {
        &self.inner.cfg
    }

    /// The image commands run in: the typed image, or the Dockerfile's tag.
    pub fn image(&self) -> String {
        docker::image_ref(&self.inner.cfg)
    }

    /// A handle for a subagent's own container in `chat`, with a new name.
    pub fn subagent(&self, chat: Uuid) -> Subagent {
        let id = Uuid::new_v4().simple().to_string();
        Subagent {
            sandbox: self.clone(),
            chat,
            name: format!("{}-s{}", container_name(chat), &id[..8]),
        }
    }

    /// Runs `command` with `sh -c` in the chat's container, with `root` mounted at /workspace.
    /// Errors are user-facing sentences; nothing falls back to running on the host.
    pub async fn exec(
        &self,
        chat: Uuid,
        root: &Path,
        command: &str,
        timeout: Duration,
    ) -> Result<SandboxRun, String> {
        self.exec_capped(chat, root, command, timeout, docker::OUTPUT_CAP)
            .await
    }

    /// [`Sandbox::exec`], keeping at most `cap` bytes of each output stream.
    async fn exec_capped(
        &self,
        chat: Uuid,
        root: &Path,
        command: &str,
        timeout: Duration,
        cap: usize,
    ) -> Result<SandboxRun, String> {
        self.run(chat, &container_name(chat), root, command, timeout, cap)
            .await
    }

    /// Runs `command` in the container `name` of `chat`, creating it first when needed. The
    /// chat's own container keeps the chat lock while the command runs; a subagent's lets go
    /// once its container is ready, so subagents run in parallel.
    async fn run(
        &self,
        chat: Uuid,
        name: &str,
        root: &Path,
        command: &str,
        timeout: Duration,
        cap: usize,
    ) -> Result<SandboxRun, String> {
        let inner = &*self.inner;
        let program = docker::program(&inner.cfg)?;
        docker::server_version(&program).await?;

        let lock = chat_lock(inner, chat);
        let mut turn = Some(lock.lock().await);
        let image = self.image();
        // The name is fixed, so the guard is armed before anything can create the container:
        // an abandoned or failed call then removes it.
        let mut guard = Cancel {
            inner: Arc::clone(&self.inner),
            program: program.clone(),
            name: name.to_owned(),
            armed: true,
        };

        let known = locked(&inner.containers).get(name).cloned();
        let reuse = match &known {
            Some(c) if c.root.as_path() == root => {
                is_running(&program, name).await
                    && (inner.cfg.network != SandboxNetwork::Custom
                        || is_running(&program, &chat_gateway(chat)).await)
            }
            _ => false,
        };
        if !reuse {
            if known.is_some() {
                // Recorded but stopped or removed outside the app, mounted elsewhere, or its
                // network gateway stopped.
                quiet(&program, &["rm", "-f", name]).await;
                forget(inner, name);
            }
            create(inner, &program, chat, name, root).await?;
        }

        // Touched before the run too, so the container is not reaped as idle while the
        // command runs. The touch after a successful run follows below.
        touch(inner, name);
        if name != container_name(chat) {
            turn = None;
        }
        let out =
            docker::docker_capped(&program, &docker::exec_args(name, command), timeout, cap).await;
        drop(turn);
        let out = match out {
            Ok(out) => out,
            Err(e) if e.contains(docker::TIMED_OUT) => {
                guard.armed = false;
                quiet(&program, &["rm", "-f", name]).await;
                forget(inner, name);
                return Ok(SandboxRun {
                    exit_code: None,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    stdout_cut: 0,
                    stderr_cut: 0,
                    timed_out: true,
                    container: name.to_owned(),
                    image,
                });
            }
            Err(e) => return Err(e),
        };
        guard.armed = false;
        // Touched again after a successful run.
        touch(inner, name);
        Ok(SandboxRun {
            exit_code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
            stdout_cut: out.stdout_cut,
            stderr_cut: out.stderr_cut,
            timed_out: false,
            container: name.to_owned(),
            image,
        })
    }

    /// Removes the chat's containers, its subagents' and its network gateway (the chat was
    /// deleted).
    pub fn remove_chat(&self, chat: Uuid) {
        let mut removed: Vec<(String, bool)> = Vec::new();
        locked(&self.inner.containers).retain(|name, c| {
            let keep = c.chat != chat;
            if !keep {
                removed.push((name.clone(), c.gateway));
            }
            keep
        });
        prune_lock(&self.inner, chat);
        remove_in_background(&self.inner.cfg, &gateways_last(removed));
    }

    /// Removes the containers not used for longer than `idle`, and the network gateways of
    /// chats that have no containers left.
    pub fn reap_idle(&self, idle: Duration) {
        let mut stale: Vec<(String, bool)> = Vec::new();
        let mut chats = Vec::new();
        {
            let mut map = locked(&self.inner.containers);
            map.retain(|name, c| {
                let keep = c.gateway || c.last_used.elapsed() <= idle;
                if !keep {
                    stale.push((name.clone(), false));
                    chats.push(c.chat);
                }
                keep
            });
            // A gateway is touched with its chat's containers, so it is idle once they are.
            let in_use: Vec<Uuid> = map
                .values()
                .filter(|c| !c.gateway)
                .map(|c| c.chat)
                .collect();
            map.retain(|name, c| {
                let keep = !c.gateway || in_use.contains(&c.chat) || c.last_used.elapsed() <= idle;
                if !keep {
                    stale.push((name.clone(), true));
                    chats.push(c.chat);
                }
                keep
            });
        }
        for chat in chats {
            if !locked(&self.inner.containers)
                .values()
                .any(|c| c.chat == chat)
            {
                prune_lock(&self.inner, chat);
            }
        }
        remove_in_background(&self.inner.cfg, &gateways_last(stale));
    }
}

/// The names, with the gateways after the containers that use their network.
fn gateways_last(mut names: Vec<(String, bool)>) -> Vec<String> {
    names.sort_by_key(|(_, gateway)| *gateway);
    names.into_iter().map(|(name, _)| name).collect()
}

/// Removes every container labelled `net.bukn.cleanrstai.sandbox=1` (left by an earlier run).
/// Blocking; call it from a blocking task.
pub fn reap_orphans(cfg: &SandboxSettings) {
    if let Ok(program) = docker::program(cfg) {
        docker::reap_orphans(&program);
    }
}

impl Drop for Inner {
    /// Removes the recorded containers, waiting at most [`DROP_WAIT`]. Never panics.
    fn drop(&mut self) {
        let names = gateways_last(
            locked(&self.containers)
                .iter()
                .map(|(name, c)| (name.clone(), c.gateway))
                .collect(),
        );
        if names.is_empty() {
            return;
        }
        let Ok(program) = docker::program(&self.cfg) else {
            return;
        };
        let mut cmd = docker::std_command(&program);
        cmd.args(["rm", "-f"])
            .args(&names)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match docker::spawn_sync(&mut cmd) {
            Ok(mut child) => reap_within(&mut child, DROP_WAIT),
            Err(e) => log::debug!("can't run docker rm: {e}"),
        }
    }
}

/// Waits for `child` at most `limit`; a child still running then is killed and reaped.
fn reap_within(child: &mut std::process::Child, limit: Duration) {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

/// Removes a container when its command is abandoned (the `exec` future is dropped) before
/// it finishes. The chat's network gateway stays for its other containers.
struct Cancel {
    inner: Arc<Inner>,
    program: PathBuf,
    name: String,
    armed: bool,
}

impl Drop for Cancel {
    fn drop(&mut self) {
        if self.armed {
            forget(&self.inner, &self.name);
            detached(&self.program, &["rm", "-f", self.name.as_str()]);
        }
    }
}

/// Makes the image ready, creates the container `name` of `chat` (after the chat's network
/// gateway, on a custom network) and records it. The caller's removal guard must already be
/// armed: a stopped or failed container is removed here too.
async fn create(
    inner: &Inner,
    program: &Path,
    chat: Uuid,
    name: &str,
    root: &Path,
) -> Result<(), String> {
    // Bad rules are reported before anything is pulled or built.
    let script = match inner.cfg.network {
        SandboxNetwork::Custom => Some(network::gateway_script(&inner.cfg.rules)?),
        _ => None,
    };
    {
        let mut pulled = inner.pulled.lock().await;
        docker::ensure_image(program, &inner.cfg, false, &mut pulled).await?;
    }
    quiet(program, &["rm", "-f", name]).await;
    let gateway = chat_gateway(chat);
    if let Some(script) = &script {
        ensure_gateway(inner, program, chat, &gateway, script).await?;
    }
    let image = docker::image_ref(&inner.cfg);
    let args = docker::run_args(&inner.cfg, name, root, &image, &gateway);
    let out = docker::docker(program, &args, None, RUN_TIMEOUT)
        .await
        .map_err(|e| format!("Couldn't start the sandbox container: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't start the sandbox container: {}",
            docker::last_line(&out)
        ));
    }
    if !is_running(program, name).await {
        let logs = docker::docker(
            program,
            &["logs", "--tail", "5", name].map(str::to_owned),
            None,
            INSPECT_TIMEOUT,
        )
        .await;
        let last = logs.map_or_else(|_| "no log output".to_owned(), |out| last_log_line(&out));
        quiet(program, &["rm", "-f", name]).await;
        return Err(format!(
            "The sandbox container stopped right after starting: {last}"
        ));
    }
    locked(&inner.containers).insert(
        name.to_owned(),
        Container {
            chat,
            root: root.to_owned(),
            gateway: false,
            last_used: Instant::now(),
        },
    );
    Ok(())
}

/// Makes sure the chat's network gateway `name` is running: reuses a recorded one that is,
/// and otherwise starts a new one and records it. Called with the chat lock held.
async fn ensure_gateway(
    inner: &Inner,
    program: &Path,
    chat: Uuid,
    name: &str,
    script: &str,
) -> Result<(), String> {
    let known = locked(&inner.containers).contains_key(name);
    if known && is_running(program, name).await {
        touch(inner, name);
        return Ok(());
    }
    forget(inner, name);
    quiet(program, &["rm", "-f", name]).await;
    start_gateway(program, name, script).await?;
    locked(&inner.containers).insert(
        name.to_owned(),
        Container {
            chat,
            root: PathBuf::new(),
            gateway: true,
            last_used: Instant::now(),
        },
    );
    Ok(())
}

/// Starts the gateway container `name` running `script` and waits until its rules are in
/// place. A gateway that stops or doesn't get ready in time is removed.
async fn start_gateway(program: &Path, name: &str, script: &str) -> Result<(), String> {
    let fail = |e: String| format!("Couldn't set up the sandbox network: {e}");
    let out = docker::docker(
        program,
        &docker::gateway_run_args(name, script),
        None,
        RUN_TIMEOUT,
    )
    .await
    .map_err(fail)?;
    if !out.status.success() {
        return Err(fail(docker::last_line(&out)));
    }
    let started = Instant::now();
    let logs_args = ["logs", name].map(str::to_owned);
    loop {
        let logs = docker::docker(program, &logs_args, None, INSPECT_TIMEOUT).await;
        if let Ok(out) = &logs
            && String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == network::READY)
        {
            return Ok(());
        }
        let running = is_running(program, name).await;
        if !running || started.elapsed() > GATEWAY_WAIT {
            let last = logs.map_or_else(|e| e, |out| last_log_line(&out));
            quiet(program, &["rm", "-f", name]).await;
            return Err(fail(if running {
                format!("it wasn't ready within {} s", GATEWAY_WAIT.as_secs())
            } else {
                last
            }));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Whether the container `name` is running, by `docker inspect`. Any failure counts as not.
async fn is_running(program: &Path, name: &str) -> bool {
    let args = ["inspect", "-f", "{{.State.Running}}", name].map(str::to_owned);
    match docker::docker(program, &args, None, INSPECT_TIMEOUT).await {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    }
}

/// The last non-empty line of a `docker logs` output (stdout, then stderr).
fn last_log_line(out: &std::process::Output) -> String {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no log output")
        .to_owned()
}

fn container_name(chat: Uuid) -> String {
    format!("cleanrstai-{}", chat.simple())
}

/// The name of the chat's custom network gateway, shared by all its containers.
fn chat_gateway(chat: Uuid) -> String {
    network::gateway_name(&container_name(chat))
}

fn chat_lock(inner: &Inner, chat: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = locked(&inner.chat_locks);
    Arc::clone(
        locks
            .entry(chat)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
}

/// Forgets the chat's lock, unless a command holds it or waits for it.
fn prune_lock(inner: &Inner, chat: Uuid) {
    let mut locks = locked(&inner.chat_locks);
    if locks
        .get(&chat)
        .is_some_and(|lock| Arc::strong_count(lock) == 1)
    {
        locks.remove(&chat);
    }
}

/// Records that the container `name` (and its chat's gateway) was just used.
fn touch(inner: &Inner, name: &str) {
    let mut map = locked(&inner.containers);
    let Some(chat) = map.get_mut(name).map(|c| {
        c.last_used = Instant::now();
        c.chat
    }) else {
        return;
    };
    if let Some(gateway) = map.get_mut(&chat_gateway(chat)) {
        gateway.last_used = Instant::now();
    }
}

/// Forgets the container `name`. Whether it was recorded.
fn forget(inner: &Inner, name: &str) -> bool {
    locked(&inner.containers).remove(name).is_some()
}

/// Runs `docker` with `args` and waits for it, ignoring the result.
async fn quiet(program: &Path, args: &[&str]) {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let _ = docker::docker(program, &args, None, REMOVE_TIMEOUT).await;
}

/// Runs `docker` with `args` without waiting. A thread reaps the child.
fn detached(program: &Path, args: &[&str]) {
    let mut cmd = docker::std_command(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let spawned = docker::spawn_sync(&mut cmd);
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || reap_within(&mut child, REMOVE_TIMEOUT));
        }
        Err(e) => log::debug!("can't run docker {}: {e}", args.first().unwrap_or(&"")),
    }
}

/// `docker rm -f` for `names`, detached.
fn remove_in_background(cfg: &SandboxSettings, names: &[String]) {
    if names.is_empty() {
        return;
    }
    match docker::program(cfg) {
        Ok(program) => {
            let mut args = vec!["rm", "-f"];
            args.extend(names.iter().map(String::as_str));
            detached(&program, &args);
        }
        Err(e) => log::debug!("{e}"),
    }
}

/// Locks a std mutex, ignoring poisoning so Drop and cancellation never panic.
fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A `docker` stand-in for the tests (unix only), shared with the workspace tool tests.
#[cfg(all(test, unix))]
pub(crate) mod fake_docker {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const SCRIPT: &str = r#"#!/bin/sh
dir=$(dirname "$0")
echo "$*" >> "$dir/log"
case "$1" in
version)
  if [ -f "$dir/down" ]; then echo 'Cannot connect to the Docker daemon' >&2; exit 1; fi
  echo 27.3.1 ;;
image) exit 0 ;;
run) rm -f "$dir/stopped"
  if [ -f "$dir/slow" ]; then sleep 5; fi
  echo fake-container-id
  if [ -f "$dir/dies" ]; then touch "$dir/stopped"; fi ;;
inspect) if [ -f "$dir/stopped" ]; then echo false; else echo true; fi ;;
logs) echo starting; echo cleanrstai-network-ready; echo 'exec format error' >&2 ;;
rm|kill) ;;
exec)
  for last; do :; done
  cd "$dir" && sh -c "$last" ;;
*) exit 2 ;;
esac
"#;

    /// A `docker` stand-in in its own temp folder. It logs each call to `log`, and `exec`
    /// runs its command with `sh -c` in that folder. The marker files: `down` makes `version`
    /// fail; `slow` makes `run` take 5 s; `dies` makes each `run` leave a container that is
    /// not running; `stopped` marks the container as not running (`run` clears it).
    pub(crate) struct Fake {
        pub(crate) dir: PathBuf,
        program: PathBuf,
    }

    impl Fake {
        pub(crate) fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("cleanrstai-sandbox-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let program = dir.join("docker");
            std::fs::write(&program, SCRIPT).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir, program }
        }

        pub(crate) fn settings(&self) -> SandboxSettings {
            SandboxSettings {
                docker: self.program.display().to_string(),
                ..SandboxSettings::default()
            }
        }

        pub(crate) fn mark(&self, name: &str) {
            std::fs::write(self.dir.join(name), "").unwrap();
        }

        pub(crate) fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        pub(crate) fn count_prefix(&self, prefix: &str) -> usize {
            self.calls()
                .iter()
                .filter(|c| c.starts_with(prefix))
                .count()
        }

        pub(crate) fn count_eq(&self, line: &str) -> usize {
            self.calls().iter().filter(|c| c.as_str() == line).count()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::fake_docker::Fake;
    use super::*;

    const LONG: Duration = Duration::from_secs(30);

    /// Waits up to 2 s for a detached docker call to show up in the log.
    async fn within_two_seconds(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{what} didn't happen in time");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_exec_creates_the_container_and_later_ones_reuse_it() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let name = container_name(chat);

        let run = sb.exec(chat, &fake.dir, "echo hi", LONG).await.unwrap();
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(&run.stdout), "hi\n");
        assert_eq!(run.container, name);
        assert_eq!(run.image, "python:3.13-slim");
        assert_eq!(fake.count_eq(&format!("rm -f {name}")), 1);
        assert_eq!(fake.count_prefix("run -d "), 1);
        assert_eq!(fake.count_prefix("exec "), 1);

        let again = sb.exec(chat, &fake.dir, "echo again", LONG).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&again.stdout), "again\n");
        assert_eq!(fake.count_prefix("run -d "), 1);
        assert_eq!(fake.count_prefix("rm "), 1);
        assert_eq!(fake.count_prefix("exec "), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_custom_network_starts_a_gateway_first_and_removes_it_too() {
        use crate::config::{NetProtocol, SandboxNetRule};
        let fake = Fake::new();
        let sb = Sandbox::new(SandboxSettings {
            network: SandboxNetwork::Custom,
            rules: vec![SandboxNetRule {
                host: "pypi.org".into(),
                protocol: NetProtocol::Tcp,
                ports: "443".into(),
            }],
            ..fake.settings()
        });
        let chat = Uuid::new_v4();
        let name = container_name(chat);
        let gateway = network::gateway_name(&name);

        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        let runs: Vec<String> = fake
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("run -d "))
            .collect();
        assert_eq!(runs.len(), 2);
        assert!(runs[0].contains(&format!("--name {gateway} ")));
        assert!(runs[0].contains("--cap-add NET_ADMIN"));
        // The script is the gateway's last argument; the log splits it into lines.
        assert!(fake.calls().contains(
            &"iptables -A OUTPUT -m set --match-set r1 dst -p tcp --dport 443 -j ACCEPT".to_owned()
        ));
        assert!(runs[1].contains(&format!("--name {name} ")));
        assert!(runs[1].contains(&format!("--network container:{gateway} ")));
        assert!(runs[1].contains("--cap-drop ALL"));
        assert_eq!(fake.count_eq(&format!("rm -f {name}")), 1);
        assert_eq!(fake.count_eq(&format!("rm -f {gateway}")), 1);

        sb.remove_chat(chat);
        within_two_seconds("the removal", || {
            fake.count_eq(&format!("rm -f {name} {gateway}")) == 1
        })
        .await;
    }

    fn custom(fake: &Fake) -> SandboxSettings {
        use crate::config::{NetProtocol, SandboxNetRule};
        SandboxSettings {
            network: SandboxNetwork::Custom,
            rules: vec![SandboxNetRule {
                host: "pypi.org".into(),
                protocol: NetProtocol::Tcp,
                ports: "443".into(),
            }],
            ..fake.settings()
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subagents_get_their_own_containers_on_the_chats_network() {
        let fake = Fake::new();
        let sb = Sandbox::new(custom(&fake));
        let chat = Uuid::new_v4();
        let gateway = chat_gateway(chat);

        let main = sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        let (a, b) = (sb.subagent(chat), sb.subagent(chat));
        let (ra, rb) = tokio::join!(
            a.exec(&fake.dir, "echo a", LONG),
            b.exec(&fake.dir, "echo b", LONG)
        );
        let (ra, rb) = (ra.unwrap(), rb.unwrap());
        assert_eq!(String::from_utf8_lossy(&ra.stdout), "a\n");
        assert_eq!(String::from_utf8_lossy(&rb.stdout), "b\n");
        let names = [&main.container, &ra.container, &rb.container];
        assert!(names[1].starts_with(&format!("{}-s", main.container)));
        assert_ne!(names[1], names[2]);

        // One gateway, joined by all three containers.
        let runs: Vec<String> = fake
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("run -d "))
            .collect();
        assert_eq!(runs.len(), 4, "{runs:?}");
        assert_eq!(
            runs.iter()
                .filter(|r| r.contains(&format!("--name {gateway} ")))
                .count(),
            1
        );
        for name in names {
            assert!(
                runs.iter().any(|r| r.contains(&format!("--name {name} "))
                    && r.contains(&format!("--network container:{gateway} "))),
                "{name}"
            );
        }

        // A finished subagent's container goes; the chat's and the gateway stay.
        let sub = ra.container.clone();
        drop(a);
        within_two_seconds("the subagent removal", || {
            fake.count_eq(&format!("rm -f {sub}")) == 2
        })
        .await;
        assert_eq!(fake.count_eq(&format!("rm -f {gateway}")), 1);
        sb.reap_idle(Duration::from_secs(3600));
        drop(b);
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_subagent_alone_starts_the_gateway_and_idle_reaping_removes_it_last() {
        let fake = Fake::new();
        let sb = Sandbox::new(custom(&fake));
        let chat = Uuid::new_v4();
        let gateway = chat_gateway(chat);

        let sub = sb.subagent(chat);
        let run = sub.exec(&fake.dir, "true", LONG).await.unwrap();
        assert!(
            fake.calls()
                .iter()
                .any(|r| r.starts_with("run -d ") && r.contains(&format!("--name {gateway} ")))
        );
        sb.reap_idle(Duration::ZERO);
        let both = format!("rm -f {} {gateway}", run.container);
        within_two_seconds("idle reaping", || fake.count_eq(&both) == 1).await;
        assert!(locked(&sb.inner.containers).is_empty());
        // Already reaped, so dropping the handle removes nothing more.
        drop(sub);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(fake.count_prefix(&format!("rm -f {}", run.container)), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_gateway_is_replaced_with_the_container_using_it() {
        let fake = Fake::new();
        let sb = Sandbox::new(custom(&fake));
        let chat = Uuid::new_v4();
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        // The fake reports every container stopped while the marker is there.
        fake.mark("stopped");
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_bad_network_rule_fails_before_anything_starts() {
        use crate::config::SandboxNetRule;
        let fake = Fake::new();
        let sb = Sandbox::new(SandboxSettings {
            network: SandboxNetwork::Custom,
            rules: vec![SandboxNetRule {
                host: "::1".into(),
                ..SandboxNetRule::default()
            }],
            ..fake.settings()
        });
        let err = sb
            .exec(Uuid::new_v4(), &fake.dir, "true", LONG)
            .await
            .err()
            .unwrap();
        assert!(err.starts_with("Network rule 1: "), "{err}");
        assert_eq!(fake.count_prefix("run "), 0);
        assert_eq!(fake.count_prefix("image "), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_chats_get_two_containers() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let a = sb
            .exec(Uuid::new_v4(), &fake.dir, "true", LONG)
            .await
            .unwrap();
        let b = sb
            .exec(Uuid::new_v4(), &fake.dir, "true", LONG)
            .await
            .unwrap();
        assert_ne!(a.container, b.container);
        assert_eq!(fake.count_prefix("run -d "), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_changed_root_recreates_the_container() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let name = container_name(chat);
        let other = fake.dir.join("other");

        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        sb.exec(chat, &other, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 2);
        // Once when created, once for the old root, once for the new container.
        assert_eq!(fake.count_eq(&format!("rm -f {name}")), 3);
        assert!(
            fake.calls()
                .iter()
                .any(|c| c.starts_with("run -d ")
                    && c.contains(&format!("src={},", other.display())))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_timeout_removes_the_container_and_the_next_command_gets_a_fresh_one() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let name = container_name(chat);

        let run = sb
            .exec(chat, &fake.dir, "sleep 5", Duration::from_secs(1))
            .await
            .unwrap();
        assert!(run.timed_out);
        assert_eq!(run.exit_code, None);
        // Once when created, once for the timeout.
        assert_eq!(fake.count_eq(&format!("rm -f {name}")), 2);

        let next = sb.exec(chat, &fake.dir, "echo ok", LONG).await.unwrap();
        assert!(!next.timed_out);
        assert_eq!(fake.count_prefix("run -d "), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_code_and_output_come_back() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let run = sb
            .exec(
                Uuid::new_v4(),
                &fake.dir,
                "echo out; echo err >&2; exit 3",
                LONG,
            )
            .await
            .unwrap();
        assert_eq!(run.exit_code, Some(3));
        assert!(!run.timed_out);
        assert_eq!(String::from_utf8_lossy(&run.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&run.stderr), "err\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_docker_is_reported_and_nothing_runs() {
        let fake = Fake::new();
        fake.mark("down");
        let sb = Sandbox::new(fake.settings());
        let err = sb
            .exec(Uuid::new_v4(), &fake.dir, "true", LONG)
            .await
            .err()
            .unwrap();
        assert!(err.contains("Docker isn't running"), "{err}");
        assert_eq!(fake.count_prefix("run "), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_chat_removes_its_container_in_the_background() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let rm = format!("rm -f {}", container_name(chat));
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_eq(&rm), 1);

        sb.remove_chat(chat);
        within_two_seconds("remove_chat", || fake.count_eq(&rm) == 2).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reap_idle_removes_only_containers_past_the_limit() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let rm = format!("rm -f {}", container_name(chat));
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();

        sb.reap_idle(Duration::from_secs(3600));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(fake.count_eq(&rm), 1);

        sb.reap_idle(Duration::ZERO);
        within_two_seconds("reap_idle", || fake.count_eq(&rm) == 2).await;

        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_last_clone_removes_the_containers() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let a = sb
            .exec(Uuid::new_v4(), &fake.dir, "true", LONG)
            .await
            .unwrap();
        let b = sb
            .exec(Uuid::new_v4(), &fake.dir, "true", LONG)
            .await
            .unwrap();
        let clone = sb.clone();
        drop(sb);
        assert_eq!(fake.count_prefix("rm -f cleanrstai-"), 2);
        drop(clone);

        let both = fake.calls().iter().any(|c| {
            c.starts_with("rm -f ") && c.contains(&a.container) && c.contains(&b.container)
        });
        assert!(both, "{:?}", fake.calls());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_abandoned_command_removes_its_container() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let name = container_name(chat);
        let dir = fake.dir.clone();
        let task = tokio::spawn({
            let sb = sb.clone();
            async move { sb.exec(chat, &dir, "sleep 5", LONG).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        task.abort();

        let rm = format!("rm -f {name}");
        within_two_seconds("rm after cancel", || fake.count_eq(&rm) == 2).await;
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn commands_in_one_chat_share_one_container() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let (a, b) = tokio::join!(
            sb.exec(chat, &fake.dir, "echo a", LONG),
            sb.exec(chat, &fake.dir, "echo b", LONG)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 1);
        assert_eq!(fake.count_prefix("exec "), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_recorded_container_that_stopped_is_replaced_before_use() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 1);

        // Stopped outside the app (Docker restarted, or a prune removed it).
        fake.mark("stopped");
        let run = sb.exec(chat, &fake.dir, "echo back", LONG).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&run.stdout), "back\n");
        assert_eq!(fake.count_prefix("run -d "), 2);

        // A running container is reused: no further run. Inspects: after the first create,
        // before the stale reuse check, after its create, and before this reuse.
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert_eq!(fake.count_prefix("run -d "), 2);
        assert_eq!(fake.count_prefix("inspect "), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_container_that_stops_right_after_starting_is_an_error_and_removed() {
        let fake = Fake::new();
        fake.mark("dies");
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let rm = format!("rm -f {}", container_name(chat));

        let err = sb.exec(chat, &fake.dir, "true", LONG).await.err().unwrap();
        assert_eq!(
            err,
            "The sandbox container stopped right after starting: exec format error"
        );
        // Once before `docker run`, once after the container was found stopped.
        assert!(fake.count_eq(&rm) >= 2, "{:?}", fake.calls());
        assert_eq!(fake.count_prefix("exec "), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_abandoned_create_removes_the_container_it_started() {
        let fake = Fake::new();
        fake.mark("slow");
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let rm = format!("rm -f {}", container_name(chat));
        let dir = fake.dir.clone();
        let task = tokio::spawn({
            let sb = sb.clone();
            async move { sb.exec(chat, &dir, "true", LONG).await }
        });
        // `docker run` is sleeping by now.
        tokio::time::sleep(Duration::from_millis(300)).await;
        task.abort();

        // Once before `docker run`, once for the abandoned create.
        within_two_seconds("rm after an abandoned create", || fake.count_eq(&rm) == 2).await;
        assert_eq!(fake.count_prefix("run -d "), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn output_over_the_cap_keeps_both_ends_and_counts_the_rest() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        // With a cap of 1000 bytes each stream keeps 500 from each end.
        let cmd = "head -c 5000 /dev/zero | tr '\\0' x; head -c 300 /dev/zero | tr '\\0' e >&2";
        let run = sb
            .exec_capped(Uuid::new_v4(), &fake.dir, cmd, LONG, 1000)
            .await
            .unwrap();
        assert_eq!(run.stdout.len(), 1000);
        assert_eq!(run.stdout_cut, 4000);
        assert_eq!(run.stderr.len(), 300);
        assert_eq!(run.stderr_cut, 0);

        let small = sb
            .exec(Uuid::new_v4(), &fake.dir, "printf hi", LONG)
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&small.stdout), "hi");
        assert_eq!((small.stdout_cut, small.stderr_cut), (0, 0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reap_idle_spares_a_container_whose_command_is_running() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let rm = format!("rm -f {}", container_name(chat));
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        // Last used more than the idle limit ago when the next command starts.
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let dir = fake.dir.clone();
        let task = tokio::spawn({
            let sb = sb.clone();
            async move { sb.exec(chat, &dir, "sleep 3", LONG).await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        sb.reap_idle(Duration::from_secs(1));
        // The command is still running: its container must not have been removed.
        assert!(!task.is_finished());
        assert_eq!(fake.count_eq(&rm), 1, "{:?}", fake.calls());

        let run = task.await.unwrap().unwrap();
        assert!(!run.timed_out);
        assert_eq!(fake.count_eq(&rm), 1, "{:?}", fake.calls());
    }

    #[test]
    fn reap_within_kills_a_child_past_its_limit() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let started = Instant::now();
        reap_within(&mut child, Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(10));
        let status = child.wait().unwrap();
        assert_eq!(status.code(), None, "{status:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn chat_locks_are_pruned_with_their_containers_unless_held() {
        let fake = Fake::new();
        let sb = Sandbox::new(fake.settings());
        let chat = Uuid::new_v4();
        let has_lock = |sb: &Sandbox| locked(&sb.inner.chat_locks).contains_key(&chat);

        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        assert!(has_lock(&sb));
        sb.reap_idle(Duration::ZERO);
        assert!(!has_lock(&sb));

        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        sb.remove_chat(chat);
        assert!(!has_lock(&sb));

        // A lock another call still holds is kept until that call lets go.
        sb.exec(chat, &fake.dir, "true", LONG).await.unwrap();
        let held = chat_lock(&sb.inner, chat);
        sb.remove_chat(chat);
        assert!(has_lock(&sb));
        drop(held);
        sb.remove_chat(chat);
        assert!(!has_lock(&sb));
    }
}

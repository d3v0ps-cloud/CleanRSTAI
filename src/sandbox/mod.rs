//! Docker sandbox for the workspace shell tool. Commands run in a container through the
//! `docker` command line.

mod docker;
#[cfg(test)]
mod real_tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::config::SandboxSettings;

pub use docker::{check, image_ref, prepare};

/// How long `docker run` may take to create a container.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `docker rm` or `docker kill` may take before it is abandoned.
const REMOVE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long dropping the manager waits for `docker rm`.
const DROP_WAIT: Duration = Duration::from_secs(5);
/// How long `docker inspect` or `docker logs` may take.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs workspace shell commands in Docker containers, one per chat, reused between
/// commands. Cheap to clone (Arc inside). Dropping the last clone removes its containers.
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

struct Inner {
    cfg: SandboxSettings,
    containers: Mutex<HashMap<Uuid, Container>>,
    /// Whether an `Always` pull already ran this session. Held while the image is made ready,
    /// so chats that start together wait for one pull or build.
    pulled: tokio::sync::Mutex<bool>,
    /// One lock per chat, so two commands in a chat can't both create its container.
    chat_locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Clone)]
struct Container {
    name: String,
    root: PathBuf,
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
        let inner = &*self.inner;
        let program = docker::program(&inner.cfg)?;
        docker::server_version(&program).await?;

        let lock = chat_lock(inner, chat);
        let _turn = lock.lock().await;
        let image = self.image();
        // The name is fixed per chat, so the guard is armed before anything can create the
        // container: an abandoned or failed call then removes it.
        let name = container_name(chat);
        let mut guard = Cancel {
            inner: Arc::clone(&self.inner),
            program: program.clone(),
            chat,
            name: name.clone(),
            armed: true,
        };

        let known = locked(&inner.containers).get(&chat).cloned();
        let reuse = match &known {
            Some(c) if c.root.as_path() == root => is_running(&program, &name).await,
            _ => false,
        };
        if !reuse {
            if known.is_some() {
                // Recorded but stopped or removed outside the app, or mounted elsewhere.
                quiet(&program, &["rm", "-f", name.as_str()]).await;
                locked(&inner.containers).remove(&chat);
            }
            create(inner, &program, chat, root).await?;
        }

        // Touched before the run too, so the container is not reaped as idle while the
        // command runs. The touch after a successful run follows below.
        touch(inner, chat, &name);
        let out =
            docker::docker_capped(&program, &docker::exec_args(&name, command), timeout, cap).await;
        let out = match out {
            Ok(out) => out,
            Err(e) if e.contains(docker::TIMED_OUT) => {
                guard.armed = false;
                quiet(&program, &["rm", "-f", name.as_str()]).await;
                forget(inner, chat, &name);
                return Ok(SandboxRun {
                    exit_code: None,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    stdout_cut: 0,
                    stderr_cut: 0,
                    timed_out: true,
                    container: name,
                    image,
                });
            }
            Err(e) => return Err(e),
        };
        guard.armed = false;
        // Touched again after a successful run.
        touch(inner, chat, &name);
        Ok(SandboxRun {
            exit_code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
            stdout_cut: out.stdout_cut,
            stderr_cut: out.stderr_cut,
            timed_out: false,
            container: name,
            image,
        })
    }

    /// Removes the chat's container, if any (the chat was deleted).
    pub fn remove_chat(&self, chat: Uuid) {
        let removed = locked(&self.inner.containers).remove(&chat);
        prune_lock(&self.inner, chat);
        if let Some(c) = removed {
            remove_in_background(&self.inner.cfg, &[c.name]);
        }
    }

    /// Removes the containers not used for longer than `idle`.
    pub fn reap_idle(&self, idle: Duration) {
        let mut stale = Vec::new();
        let mut chats = Vec::new();
        locked(&self.inner.containers).retain(|chat, c| {
            let keep = c.last_used.elapsed() <= idle;
            if !keep {
                stale.push(c.name.clone());
                chats.push(*chat);
            }
            keep
        });
        for chat in chats {
            prune_lock(&self.inner, chat);
        }
        remove_in_background(&self.inner.cfg, &stale);
    }
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
        let names: Vec<String> = locked(&self.containers)
            .values()
            .map(|c| c.name.clone())
            .collect();
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

/// Removes a chat's container when its command is abandoned (the `exec` future is dropped)
/// before it finishes.
struct Cancel {
    inner: Arc<Inner>,
    program: PathBuf,
    chat: Uuid,
    name: String,
    armed: bool,
}

impl Drop for Cancel {
    fn drop(&mut self) {
        if self.armed {
            forget(&self.inner, self.chat, &self.name);
            detached(&self.program, &["rm", "-f", self.name.as_str()]);
        }
    }
}

/// Makes the image ready, creates the chat's container and records it. The caller's removal
/// guard must already be armed: a stopped or failed container is removed here too.
async fn create(inner: &Inner, program: &Path, chat: Uuid, root: &Path) -> Result<(), String> {
    {
        let mut pulled = inner.pulled.lock().await;
        docker::ensure_image(program, &inner.cfg, false, &mut pulled).await?;
    }
    let name = container_name(chat);
    quiet(program, &["rm", "-f", name.as_str()]).await;
    let image = docker::image_ref(&inner.cfg);
    let args = docker::run_args(&inner.cfg, &name, root, &image);
    let out = docker::docker(program, &args, None, RUN_TIMEOUT)
        .await
        .map_err(|e| format!("Couldn't start the sandbox container: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Couldn't start the sandbox container: {}",
            docker::last_line(&out)
        ));
    }
    if !is_running(program, &name).await {
        let logs = docker::docker(
            program,
            &["logs", "--tail", "5", name.as_str()].map(str::to_owned),
            None,
            INSPECT_TIMEOUT,
        )
        .await;
        let last = logs.map_or_else(|_| "no log output".to_owned(), |out| last_log_line(&out));
        quiet(program, &["rm", "-f", name.as_str()]).await;
        return Err(format!(
            "The sandbox container stopped right after starting: {last}"
        ));
    }
    locked(&inner.containers).insert(
        chat,
        Container {
            name,
            root: root.to_owned(),
            last_used: Instant::now(),
        },
    );
    Ok(())
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

/// Records that the chat's container (if it is still `name`) was just used.
fn touch(inner: &Inner, chat: Uuid, name: &str) {
    if let Some(c) = locked(&inner.containers)
        .get_mut(&chat)
        .filter(|c| c.name == name)
    {
        c.last_used = Instant::now();
    }
}

/// Forgets the chat's container if it is still `name`.
fn forget(inner: &Inner, chat: Uuid, name: &str) {
    let mut map = locked(&inner.containers);
    if map.get(&chat).is_some_and(|c| c.name == name) {
        map.remove(&chat);
    }
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
logs) echo starting; echo 'exec format error' >&2 ;;
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

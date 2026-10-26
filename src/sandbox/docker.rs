//! The `docker` command line: argument builders and small async runners. The sandbox talks to
//! Docker only through this program, never through a socket or a library.

#![allow(dead_code)] // TODO(sandbox): remove once the manager and the app use it.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::{SandboxImageSource, SandboxNetwork, SandboxPull, SandboxSettings};

/// Label set on every sandbox container, so leftovers can be found after a crash.
pub const LABEL: &str = "net.bukn.cleanrstai.sandbox=1";

const NOT_INSTALLED: &str = "Docker isn't installed: install Docker Desktop or Docker Engine, or set its location in Settings → Docker.";
const NOT_RUNNING: &str =
    "Docker isn't running: start Docker Desktop (or the Docker service) and try again.";
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const INSPECT_TIMEOUT: Duration = Duration::from_secs(10);
const BUILD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The `docker` program: the configured path if set, otherwise the first `docker` on PATH.
pub fn program(cfg: &SandboxSettings) -> Result<PathBuf, String> {
    let set = cfg.docker.trim();
    if !set.is_empty() {
        let path = PathBuf::from(set);
        return if path.exists() {
            Ok(path)
        } else {
            Err(format!("docker not found at {}", path.display()))
        };
    }
    which::which("docker").map_err(|_| NOT_INSTALLED.into())
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

/// Arguments of `docker run` that create a chat's container (without the program).
pub fn run_args(cfg: &SandboxSettings, name: &str, root: &Path, image: &str) -> Vec<String> {
    let mut mount = format!("type=bind,src={},dst=/workspace", mount_source(root));
    if cfg.read_only {
        mount.push_str(",readonly");
    }
    let network = match cfg.network {
        SandboxNetwork::None => "none",
        SandboxNetwork::Bridge => "bridge",
    };
    let memory = format!("{}m", cfg.memory_mb);
    let mut args = strings(&[
        "run", "-d", "--init", "--name", name, "--label", LABEL, "--mount",
    ]);
    args.push(mount);
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
    args.extend(strings(&["-e", "HOME=/tmp", image, "sleep", "infinity"]));
    args
}

/// Arguments of `docker exec` that run `command` with `sh -c` in the chat's container.
pub fn exec_args(name: &str, command: &str) -> Vec<String> {
    strings(&["exec", "-w", "/workspace", name, "sh", "-c", command])
}

/// The folder as `--mount` reads it. A path with a comma or quote is CSV-quoted.
fn mount_source(root: &Path) -> String {
    let s = root.display().to_string();
    if s.contains([',', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
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
    let mut child = cmd.spawn().map_err(|e| format!("Can't run docker: {e}"))?;
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
        Err(_) => Err(format!(
            "docker {} didn't finish within {} s",
            args.first().map_or("", String::as_str),
            timeout.as_secs()
        )),
    }
}

/// The last non-empty line docker wrote to stderr, for error messages.
fn last_line(out: &Output) -> String {
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
    let args = strings(&["image", "inspect", image]);
    let out = docker(program, &args, None, INSPECT_TIMEOUT).await?;
    Ok(out.status.success())
}

async fn pull(program: &Path, image: &str) -> Result<(), String> {
    let args = strings(&["pull", image]);
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
                pull(program, &image).await?;
                *pulled_this_session = true;
            }
            Ok(image)
        }
    }
}

/// For Settings → Docker "Test": the Docker version and whether the image is ready.
pub async fn check(cfg: &SandboxSettings) -> Result<String, String> {
    let program = program(cfg)?;
    let version = server_version(&program).await?;
    let image = image_ref(cfg);
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
    let program = program(cfg)?;
    server_version(&program).await?;
    let mut pulled = false;
    let image = ensure_image(&program, cfg, true, &mut pulled).await?;
    Ok(match cfg.source {
        SandboxImageSource::Image => format!("Pulled {image}"),
        SandboxImageSource::Dockerfile => format!("Built {image}"),
    })
}

/// Removes every container labelled [`LABEL`]. Blocking; errors are only logged.
pub fn reap_orphans(program: &Path) {
    let filter = format!("label={LABEL}");
    let listed = std_command(program)
        .args(["ps", "-aq", "--filter", &filter])
        .output();
    let ids: Vec<String> = match listed {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        Ok(out) => {
            log::debug!("docker ps failed: {}", last_line(&out));
            return;
        }
        Err(e) => {
            log::debug!("can't run docker ps: {e}");
            return;
        }
    };
    if ids.is_empty() {
        return;
    }
    match std_command(program).arg("rm").arg("-f").args(&ids).output() {
        Ok(out) if !out.status.success() => {
            log::debug!("docker rm failed: {}", last_line(&out));
        }
        Err(e) => log::debug!("can't run docker rm: {e}"),
        Ok(_) => {}
    }
}

fn std_command(program: &Path) -> std::process::Command {
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
    fn run_args_quotes_a_mount_path_with_a_comma() {
        let args = run_args(&cfg(), "n", Path::new("/a,b\"c"), "img");
        assert!(args.contains(&"type=bind,src=\"/a,b\"\"c\",dst=/workspace".to_owned()));
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
        /// (version fails), `present` (the image exists), `fail` (pull and build fail),
        /// `none` (ps finds nothing).
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
                     ps) [ -f \"$dir/none\" ] || printf 'c1\\nc2\\n' ;;\n\
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
        fn reap_orphans_removes_the_labelled_containers() {
            let fake = Fake::new();
            reap_orphans(&fake.program);
            assert!(
                fake.calls()
                    .contains(&format!("ps -aq --filter label={LABEL}"))
            );
            assert!(fake.calls().contains(&"rm -f c1 c2".to_owned()));
        }

        #[test]
        fn reap_orphans_does_nothing_when_none_are_found() {
            let fake = Fake::new();
            fake.mark("none");
            reap_orphans(&fake.program);
            assert_eq!(fake.count("rm"), 0);
        }
    }
}

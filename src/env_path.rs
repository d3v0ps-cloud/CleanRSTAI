//! GUI apps on macOS (and some Linux desktops) start with a minimal PATH, so commands
//! like `npx`, `uvx` or `docker` used by stdio MCP servers would not be found. Import the
//! PATH from the user's login shell, plus common tool locations.

#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    const MARK: &str = "__CLEANRSTAI_PATH__";
    let mut child = Command::new(shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!("printf '{MARK}%s{MARK}' \"$PATH\""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Some shell configs hang (e.g. waiting on a prompt); don't block startup on them.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find(MARK)? + MARK.len();
    let end = start + text[start..].find(MARK)?;
    Some(text[start..end].to_owned())
}

/// Must be called at startup before any other threads are spawned.
pub fn fix_path() {
    #[cfg(unix)]
    {
        let current = std::env::var("PATH").unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        let mut push = |p: &str| {
            if !p.is_empty() && !parts.iter().any(|x| x == p) {
                parts.push(p.to_owned());
            }
        };
        if let Some(shell_path) = login_shell_path() {
            shell_path.split(':').for_each(&mut push);
        }
        current.split(':').for_each(&mut push);
        let home = std::env::var("HOME").unwrap_or_default();
        for extra in [
            "/opt/homebrew/bin".to_owned(),
            "/usr/local/bin".to_owned(),
            format!("{home}/.local/bin"),
            format!("{home}/.cargo/bin"),
            format!("{home}/.bun/bin"),
            format!("{home}/.deno/bin"),
        ] {
            if std::path::Path::new(&extra).is_dir() {
                push(&extra);
            }
        }
        // SAFETY: called from `main` before any threads are started.
        unsafe { std::env::set_var("PATH", parts.join(":")) };
    }
}

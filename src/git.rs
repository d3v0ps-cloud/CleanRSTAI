//! Every `git` the app runs goes through [`command`] or [`safe_command`]. A repository is
//! untrusted: its own `.git/config` can name programs that git then runs (an fsmonitor hook, an
//! external diff driver, a textconv filter, a clean/smudge/process filter, a pager), so those
//! are switched off here. The diff calls also pass `--no-ext-diff --no-textconv`, and the
//! status and diff calls pass `--ignore-submodules=all` (see the call sites).

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

/// A `git` command in `dir`, with the repository's own programs switched off. The caller adds
/// the subcommand and its arguments. Nothing here reads a prompt, a pager or the index lock.
/// Anything that reads the working tree should use [`safe_command`], which also blanks the
/// filter drivers.
pub fn command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "diff.external=",
        "-c",
        "core.pager=cat",
    ])
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("GIT_PAGER", "cat")
    // Status must not write the index.
    .env("GIT_OPTIONAL_LOCKS", "0")
    .current_dir(dir)
    .stdin(Stdio::null())
    .kill_on_drop(true);
    cmd
}

/// [`command`], plus the overrides that stop a repository's filter drivers from running.
///
/// Git runs a configured clean, smudge or process filter whenever it reads a file whose stat
/// data changed, even for `git status` and `git diff`, and no flag turns that off. So every
/// configured `filter.<name>.clean|smudge|process` (from the repository, its includes, and the
/// global and system config) is set to an empty value, which git reads as "no filter". Git's
/// attributes file is also pointed at `/dev/null`. The caller adds the subcommand after this.
///
/// Reading the config runs nothing. If the list can't be read, or a name can't be passed as
/// `-c` safely, this returns an error rather than a command without the overrides.
pub async fn safe_command(dir: &Path) -> Result<Command, String> {
    let keys = filter_keys(dir).await?;
    let mut cmd = command(dir);
    for key in keys {
        cmd.arg("-c").arg(format!("{key}="));
    }
    cmd.args(["-c", "core.attributesFile=/dev/null"]);
    Ok(cmd)
}

/// The names of the configured filter commands in `dir`'s config, as `filter.<name>.<kind>`.
async fn filter_keys(dir: &Path) -> Result<Vec<String>, String> {
    let out = command(dir)
        .args([
            "config",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ])
        .output()
        .await
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    // `git config --get-regexp` exits 1 when nothing matches.
    if out.status.code() == Some(1) && out.stderr.is_empty() {
        return Ok(Vec::new());
    }
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() {
            "Couldn't read the repository's filter settings".to_owned()
        } else {
            err
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut keys = Vec::new();
    for key in text.lines().map(str::trim).filter(|k| !k.is_empty()) {
        // `-c name=value` splits at the first "=", so a name containing one can't be blanked
        // this way; refuse rather than leave that filter running.
        if key.contains('=') {
            return Err(format!(
                "The repository's filter setting {key} can't be switched off, so the git tools \
                 won't run"
            ));
        }
        keys.push(key.to_owned());
    }
    Ok(keys)
}

#[cfg(test)]
pub mod test_support {
    //! A repository whose filter drivers would run on plain `git status`, shared by the git
    //! tool and review tests. The markers are files the filter commands create when they run.

    use std::path::Path;
    #[cfg(unix)]
    use std::path::PathBuf;

    #[cfg(unix)]
    pub struct FilterRepo {
        /// Created by the `filter.x.clean` command.
        pub clean_marker: PathBuf,
        /// Created by the `filter.y.process` command.
        pub process_marker: PathBuf,
    }

    #[cfg(unix)]
    impl FilterRepo {
        pub fn clear_markers(&self) {
            let _ = std::fs::remove_file(&self.clean_marker);
            let _ = std::fs::remove_file(&self.process_marker);
        }

        pub fn any_marker(&self) -> bool {
            self.clean_marker.exists() || self.process_marker.exists()
        }
    }

    /// Runs plain git (no overrides, commit signing off); a failure panics.
    pub fn plain_git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
    }

    #[cfg(unix)]
    /// Makes `dir` a repository with `a.txt` and `b.md` committed, `*.txt` mapped to the clean
    /// filter `x` in `.gitattributes`, `*.md` mapped to the process filter `y` in
    /// `.git/info/attributes`, and a smudge filter `z` configured. Markers go in `marker_dir`.
    ///
    /// The process filter makes plain git die ("remote end hung up") after it runs, so
    /// `assert_plain_git_runs_filters` checks the two filters separately.
    pub fn setup_filter_repo(dir: &Path, marker_dir: &Path) -> FilterRepo {
        plain_git(dir, &["init", "-q"]);
        plain_git(dir, &["config", "user.name", "Test"]);
        plain_git(dir, &["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("a.txt"), "aaaaaaaa\n").unwrap();
        std::fs::write(dir.join("b.md"), "aaaaaaaa\n").unwrap();
        std::fs::write(dir.join(".gitattributes"), "*.txt filter=x\n").unwrap();
        plain_git(dir, &["add", "."]);
        // Committed before any filter is configured, so the commit runs none of them.
        plain_git(dir, &["commit", "-qm", "init"]);

        let repo = FilterRepo {
            clean_marker: marker_dir.join("clean-ran"),
            process_marker: marker_dir.join("process-ran"),
        };
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        std::fs::write(dir.join(".git/info/attributes"), "*.md filter=y\n").unwrap();
        let clean = format!("touch '{}'; cat", repo.clean_marker.display());
        let process = format!("touch '{}'; exit 1", repo.process_marker.display());
        let smudge = format!("touch '{}'; cat", marker_dir.join("smudge-ran").display());
        plain_git(dir, &["config", "filter.x.clean", &clean]);
        plain_git(dir, &["config", "filter.y.process", &process]);
        plain_git(dir, &["config", "filter.z.smudge", &smudge]);
        repo
    }

    #[cfg(unix)]
    /// Rewrites both tracked files with eight copies of `c`. The size stays the same as the
    /// committed files: git reads a changed file's content (and runs its filters) only when its
    /// size matches the index; a size change is reported without reading the file.
    pub fn dirty(dir: &Path, c: char) {
        let text = format!("{}\n", c.to_string().repeat(8));
        std::fs::write(dir.join("a.txt"), &text).unwrap();
        std::fs::write(dir.join("b.md"), &text).unwrap();
    }

    #[cfg(unix)]
    /// Checks the setup is real: plain `git status` must run each filter, or the tool checks
    /// prove nothing. The process filter makes plain git exit with an error after it runs, so
    /// the two filters are checked one at a time: first the clean filter alone (the `*.md`
    /// mapping is removed for that step), then the process filter with the full setup. Leaves
    /// the full setup in place and the markers cleared.
    pub fn assert_plain_git_runs_filters(dir: &Path, repo: &FilterRepo) {
        let info = dir.join(".git/info/attributes");
        std::fs::write(&info, "").unwrap();
        dirty(dir, 'p');
        plain_git(dir, &["status", "--short"]);
        assert!(
            repo.clean_marker.exists(),
            "plain git status did not run the clean filter, so the test would prove nothing"
        );
        repo.clear_markers();

        std::fs::write(&info, "*.md filter=y\n").unwrap();
        dirty(dir, 'q');
        let _ = std::process::Command::new("git")
            .args(["status", "--short"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            repo.process_marker.exists(),
            "plain git status did not run the process filter, so the test would prove nothing"
        );
        repo.clear_markers();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_helper_turns_off_the_repositorys_programs() {
        let cmd = command(Path::new("."));
        let std_cmd = cmd.as_std();
        let args = args_of(&cmd);
        for expected in [
            "core.fsmonitor=false",
            "core.hooksPath=/dev/null",
            "diff.external=",
            "core.pager=cat",
        ] {
            assert!(args.iter().any(|a| a == expected), "{args:?}");
        }
        let env = |key: &str| {
            std_cmd
                .get_envs()
                .find(|(k, _)| *k == key)
                .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        assert_eq!(env("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(env("GIT_PAGER").as_deref(), Some("cat"));
        assert_eq!(env("GIT_OPTIONAL_LOCKS").as_deref(), Some("0"));
    }

    #[tokio::test]
    async fn safe_command_blanks_every_configured_filter_before_the_subcommand() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("git isn't installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cleanrstai-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        test_support::plain_git(&dir, &["init", "-q"]);
        // Clean, smudge and process filters are blanked; other filter settings are not.
        test_support::plain_git(&dir, &["config", "filter.x.clean", "true"]);
        test_support::plain_git(&dir, &["config", "filter.a.b.smudge", "true"]);
        test_support::plain_git(&dir, &["config", "filter.y.required", "true"]);

        let cmd = safe_command(&dir).await;
        let _ = std::fs::remove_dir_all(&dir);
        let cmd = cmd.unwrap();
        let args = args_of(&cmd);
        let pair = |k: &str, v: &str| {
            args.windows(2)
                .any(|w| w[0] == "-c" && w[1] == format!("{k}={v}"))
        };
        assert!(pair("filter.x.clean", ""), "{args:?}");
        assert!(pair("filter.a.b.smudge", ""), "{args:?}");
        assert!(!args.iter().any(|a| a.starts_with("filter.y.")), "{args:?}");
        assert!(pair("core.attributesFile", "/dev/null"), "{args:?}");
        // The base overrides are still there.
        assert!(pair("core.fsmonitor", "false"), "{args:?}");
    }
}

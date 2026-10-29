//! Every `git` the app runs goes through [`command`]. A repository is untrusted: its own
//! `.git/config` can name programs that git then runs (an fsmonitor hook, an external diff
//! driver, a textconv filter, a pager), so those are switched off here, and the diff calls
//! also pass `--no-ext-diff --no-textconv` (see the call sites).

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

/// A `git` command in `dir`, with the repository's own programs switched off. The caller adds
/// the subcommand and its arguments. Nothing here reads a prompt, a pager or the index lock.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helper_turns_off_the_repositorys_programs() {
        let cmd = command(Path::new("."));
        let std_cmd = cmd.as_std();
        let args: Vec<_> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
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
}

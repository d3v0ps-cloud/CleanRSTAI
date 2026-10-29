//! `git_commit` and the Commit dialog: record the workspace's changes as one local commit.
//!
//! The commit stages the chosen paths (or every change in the workspace), then commits only
//! those paths with `git commit -- <paths>`, so changes staged elsewhere stay out of it. Git
//! hooks never run: `git::command` points `core.hooksPath` at /dev/null and the commit passes
//! `--no-verify`. Signing is off unless Settings allow it. Nothing is pushed.
//!
//! Every call goes through [`crate::git::safe_command`], so the repository's filter drivers
//! don't run: a file with a filter (such as Git LFS) is committed as it is on disk.

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use super::workspace::Workspace;
use crate::mcp::ToolOutput;

/// The longest commit message accepted, in characters.
pub const MAX_MESSAGE_CHARS: usize = 10_000;
/// Most files listed in the approval preview.
const MAX_PREVIEW_FILES: usize = 200;

const IDENTITY_HELP: &str = "Git doesn't know who should make the commit. Set a name and an \
    email for this repository: git config user.name \"Your Name\" and git config user.email \
    you@example.com (add --global to set them for every repository).";

/// One changed file, as `git status` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The two status letters, such as " M", "A " or "??".
    pub status: String,
    /// The file, relative to the workspace.
    pub path: String,
    /// For a rename, the file it was renamed from (relative to the workspace).
    pub from: Option<String>,
}

impl Change {
    /// The path, or "old -> new" for a rename.
    pub fn label(&self) -> String {
        match &self.from {
            Some(from) => format!("{from} -> {}", self.path),
            None => self.path.clone(),
        }
    }
}

/// The paths the checked files of a commit dialog cover: each checked file, and the file it
/// was renamed from, so the rename's removal is committed too.
pub fn selected_paths(files: &[(Change, bool)]) -> Vec<String> {
    files
        .iter()
        .filter(|(_, on)| *on)
        .flat_map(|(change, _)| std::iter::once(change.path.clone()).chain(change.from.clone()))
        .collect()
}

/// Parses `git status --porcelain=v1 -z` output. `prefix` is the workspace's place in its
/// repository (`sub/` in a subfolder, empty at the top): git reports paths from the top, so
/// entries outside `prefix` are left out and the rest are made relative to the workspace.
pub fn parse_status(out: &[u8], prefix: &str) -> Vec<Change> {
    let mut fields = out
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned());
    let mut changes = Vec::new();
    while let Some(entry) = fields.next() {
        let (Some(status), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        let status = status.to_owned();
        // A rename or copy is followed by the path it came from, as its own field.
        let from = if status.contains(['R', 'C']) {
            fields.next()
        } else {
            None
        };
        let Some(path) = path.strip_prefix(prefix) else {
            continue;
        };
        let from = match from.as_deref().map(|f| f.strip_prefix(prefix)) {
            Some(None) => continue,
            Some(Some(f)) => Some(f.to_owned()),
            None => None,
        };
        changes.push(Change {
            status,
            path: path.to_owned(),
            from,
        });
    }
    changes
}

/// The message with its surrounding space removed; an empty or oversized message is refused.
pub fn clean_message(message: &str) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("The commit message is empty".into());
    }
    if message.contains('\0') {
        return Err("The commit message can't contain a NUL character".into());
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(format!(
            "The commit message is longer than {MAX_MESSAGE_CHARS} characters"
        ));
    }
    Ok(message.to_owned())
}

/// The `paths` argument: `None` when it is left out (every change is committed), else the
/// listed paths.
pub fn paths_arg(args: &Value) -> Result<Option<Vec<String>>> {
    match args.get("paths") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            if items.is_empty() {
                bail!("\"paths\" lists no files; leave it out to commit every change");
            }
            items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .context("\"paths\" must list file paths as text")
                })
                .collect::<Result<Vec<_>>>()
                .map(Some)
        }
        Some(_) => bail!("\"paths\" must be a list of file paths"),
    }
}

/// The message a failed git call shows: git's own error, or what to do about a missing
/// identity or a folder that isn't a repository.
pub fn describe_failure(err: &str) -> String {
    let err = err.trim();
    if err.contains("not a git repository") {
        return "The workspace isn't a git repository".into();
    }
    if [
        "Author identity unknown",
        "Please tell me who you are",
        "unable to auto-detect email address",
    ]
    .iter()
    .any(|m| err.contains(m))
    {
        return IDENTITY_HELP.into();
    }
    if err.is_empty() {
        "git didn't say why it failed".into()
    } else {
        err.to_owned()
    }
}

/// What the approval prompt shows for a commit: the message, the files and the diff summary.
pub fn preview_text(message: &str, changes: &[Change], stat: &str) -> String {
    let mut out = format!(
        "Commit message:\n{message}\n\nFiles to commit ({}):\n",
        changes.len()
    );
    for c in changes.iter().take(MAX_PREVIEW_FILES) {
        out.push_str(&format!("{} {}\n", c.status, c.label()));
    }
    if changes.len() > MAX_PREVIEW_FILES {
        out.push_str(&format!(
            "… and {} more\n",
            changes.len() - MAX_PREVIEW_FILES
        ));
    }
    if !stat.trim().is_empty() {
        out.push('\n');
        out.push_str(stat.trim_end());
        out.push('\n');
    }
    out
}

/// The result of a commit: its short hash and subject, then its stat.
fn commit_summary(head: &str, stat: &str) -> String {
    let head = head.trim();
    let stat = stat.trim_end();
    if stat.is_empty() {
        format!("Committed {head}")
    } else {
        format!("Committed {head}\n\n{stat}")
    }
}

impl Workspace {
    /// Runs `git` in the workspace with the repository's own programs off. `global` goes before
    /// the subcommand in `args`. `scope`, when given, follows `--` with its paths read literally.
    async fn git_output(
        &self,
        global: &[&str],
        args: &[&str],
        scope: Option<&[String]>,
    ) -> Result<std::process::Output, String> {
        let mut cmd = crate::git::safe_command(self.path()).await?;
        cmd.args(global).args(args);
        if let Some(scope) = scope {
            cmd.arg("--").args(scope).env("GIT_LITERAL_PATHSPECS", "1");
        }
        cmd.output()
            .await
            .map_err(|e| format!("Couldn't run git: {e}"))
    }

    /// The workspace's place in its repository: `sub/` in a subfolder, empty at the top.
    async fn repo_prefix(&self) -> Result<String, String> {
        let out = self
            .git_output(&[], &["rev-parse", "--show-prefix"], None)
            .await?;
        if !out.status.success() {
            return Err(describe_failure(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned())
    }

    /// The changes under `scope` (paths relative to the workspace), as `git status` lists them.
    /// Reads only: the index isn't written.
    async fn status_of(&self, scope: &[String]) -> Result<Vec<Change>, String> {
        let prefix = self.repo_prefix().await?;
        let out = self
            .git_output(
                &[],
                &[
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                    "--ignore-submodules=all",
                ],
                Some(scope),
            )
            .await?;
        if !out.status.success() {
            return Err(describe_failure(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(parse_status(&out.stdout, &prefix))
    }

    /// Every change in the workspace, for the Commit dialog. Reads only.
    pub async fn commit_changes_list(&self) -> Result<Vec<Change>, String> {
        self.status_of(&[".".to_owned()]).await
    }

    /// The pathspecs a commit covers: the listed paths checked by the workspace rules, or the
    /// whole workspace.
    fn commit_scope(&self, paths: Option<&[String]>) -> Result<Vec<String>, String> {
        match paths {
            None => Ok(vec![".".to_owned()]),
            // An empty list would mean the whole repository to git, so it is refused here.
            Some([]) => Err("No files are chosen to commit".into()),
            Some(paths) => paths
                .iter()
                .map(|p| self.diff_path(p).map_err(|e| format!("{e:#}")))
                .collect(),
        }
    }

    /// What a `git_commit` call would commit, for the approval prompt. Reads only: the index
    /// isn't changed. `None` when the arguments are unusable (the call would fail anyway).
    pub(crate) async fn commit_preview(&self, args: &Value) -> Option<String> {
        let message = clean_message(args.get("message")?.as_str()?).ok()?;
        let paths = paths_arg(args).ok()?;
        let scope = self.commit_scope(paths.as_deref()).ok()?;
        let changes = self.status_of(&scope).await.ok()?;
        let has_head = self
            .git_output(&[], &["rev-parse", "--verify", "--quiet", "HEAD"], None)
            .await
            .is_ok_and(|o| o.status.success());
        // A repository without a commit has nothing to diff against; its files are listed.
        let stat = if has_head {
            self.git_output(
                &[],
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=all",
                    "--stat",
                    "HEAD",
                ],
                Some(&scope),
            )
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
        } else {
            String::new()
        };
        Some(preview_text(&message, &changes, &stat))
    }

    /// Commits the changes under `paths` (every change in the workspace when `None`) as one
    /// commit, and returns its hash, subject and stat. Nothing is pushed. A commit that fails
    /// leaves its paths unstaged.
    pub(crate) async fn commit_changes(
        &self,
        message: &str,
        paths: Option<&[String]>,
    ) -> Result<String, String> {
        let message = clean_message(message)?;
        let scope = self.commit_scope(paths)?;
        if self.status_of(&scope).await?.is_empty() {
            return Err("Nothing to commit: the workspace has no changes in those paths".into());
        }
        // Only paths that exist are staged. A deleted path needs no staging: `git commit -- <path>`
        // records its removal, from the index or from HEAD (a rename's old name is already gone
        // from the index when it was moved with git mv).
        let present: Vec<String> = scope
            .iter()
            .filter(|p| self.path().join(p).exists())
            .cloned()
            .collect();
        if !present.is_empty() {
            let staged = self.git_output(&[], &["add", "-A"], Some(&present)).await?;
            if !staged.status.success() {
                return Err(describe_failure(&String::from_utf8_lossy(&staged.stderr)));
            }
        }
        let signing: &[&str] = if self.signs_commits() {
            &[]
        } else {
            &["-c", "commit.gpgSign=false"]
        };
        let commit = self
            .git_output(
                signing,
                &["commit", "--no-verify", "-m", message.as_str()],
                Some(&scope),
            )
            .await?;
        if !commit.status.success() {
            let _ = self.git_output(&[], &["reset", "-q"], Some(&scope)).await;
            let stderr = String::from_utf8_lossy(&commit.stderr);
            let stdout = String::from_utf8_lossy(&commit.stdout);
            let text = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            return Err(describe_failure(&text));
        }
        let head = self
            .git_output(
                &[],
                &[
                    "show",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "-s",
                    "--format=%h %s",
                    "HEAD",
                ],
                None,
            )
            .await?;
        let stat = self
            .git_output(
                &[],
                &[
                    "show",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--stat",
                    "--format=",
                    "HEAD",
                ],
                None,
            )
            .await?;
        Ok(commit_summary(
            &String::from_utf8_lossy(&head.stdout),
            &String::from_utf8_lossy(&stat.stdout),
        ))
    }

    /// The `git_commit` tool. The approval (see `approval.rs`) comes before this runs.
    pub(crate) async fn commit_call(&self, args: &Value) -> Result<ToolOutput> {
        if self.is_subagent() {
            bail!("Subagents can't commit; the chat can ask for a commit instead");
        }
        let message = args
            .get("message")
            .and_then(Value::as_str)
            .context("missing \"message\"")?;
        let paths = paths_arg(args)?;
        let text = self
            .commit_changes(message, paths.as_deref())
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(super::text_output(text))
    }
}

#[cfg(test)]
mod tests;

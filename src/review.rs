//! "Review changes": the working folder's git diff, sent to a model with a review prompt.
//!
//! The diff is read with `git` in the background (`collect`); the app then sends it as a text
//! attachment with the prompt (`message_text`, `attachment`).

use std::path::Path;

use crate::attach::Attachment;
use crate::config::{ReviewScope, ReviewSettings, ReviewStrictness};
use crate::git;

/// The name the diff is attached under.
pub const DIFF_NAME: &str = "changes.diff";

/// A diff ready to send, and what it covers, for the message.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewDiff {
    pub text: String,
    /// E.g. "uncommitted changes in the working folder".
    pub covers: String,
}

/// Cuts `diff` to `max_kb` at a character boundary, with a note saying so.
pub fn truncate(diff: &str, max_kb: u32) -> String {
    let max = max_kb as usize * 1024;
    if diff.len() <= max {
        return diff.to_owned();
    }
    let mut end = max;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[diff cut at {max_kb} KB]", &diff[..end])
}

/// The branch the "branch" scope diffs against: the setting if set, else `origin/HEAD` (as
/// `git symbolic-ref --short` names it), else a local `main` or `master`.
pub fn pick_base(
    setting: &str,
    origin_head: Option<&str>,
    local_branches: &[String],
) -> Option<String> {
    let setting = setting.trim();
    if !setting.is_empty() {
        return Some(setting.to_owned());
    }
    if let Some(head) = origin_head.map(str::trim).filter(|h| !h.is_empty()) {
        return Some(head.to_owned());
    }
    ["main", "master"]
        .into_iter()
        .find(|name| local_branches.iter().any(|b| b == name))
        .map(str::to_owned)
}

/// The text of the message that carries the diff: the prompt, then what the attachment holds.
pub fn message_text(prompt: &str, strictness: ReviewStrictness, diff: &ReviewDiff) -> String {
    let extra = strictness.instruction();
    let prompt = if extra.is_empty() {
        prompt.trim().to_owned()
    } else {
        format!("{} {extra}", prompt.trim())
    };
    format!(
        "{prompt}\n\nThe diff covers {} and is attached as {DIFF_NAME}.",
        diff.covers
    )
}

/// The diff as a text attachment.
pub fn attachment(diff: &ReviewDiff) -> Attachment {
    Attachment {
        name: DIFF_NAME.into(),
        mime: "text/x-diff".into(),
        text: Some(diff.text.clone()),
        data: None,
    }
}

/// Runs `git` in `dir` (through [`git::safe_command`]) and returns its standard output, or its
/// error output.
async fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = git::safe_command(dir).await?;
    let out = cmd
        .args(args)
        .output()
        .await
        .map_err(|e| format!("Couldn't run git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() {
            format!("git {} failed", args.first().unwrap_or(&""))
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Reads the diff that `settings.scope` asks for, from the git repository at `dir`.
pub async fn collect(dir: &Path, settings: &ReviewSettings) -> Result<ReviewDiff, String> {
    let inside = run_git(dir, &["rev-parse", "--is-inside-work-tree"]).await;
    if inside.ok().as_deref().map(str::trim) != Some("true") {
        return Err("The working folder isn't a git repository".into());
    }
    let (mut text, untracked, covers) = match settings.scope {
        ReviewScope::Uncommitted => {
            let text = run_git(
                dir,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=all",
                    "HEAD",
                ],
            )
            .await?;
            let listed = run_git(dir, &["ls-files", "--others", "--exclude-standard"]).await?;
            let names: Vec<String> = listed
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            (
                text,
                names,
                "uncommitted changes in the working folder".to_owned(),
            )
        }
        ReviewScope::Branch => {
            let origin_head = run_git(
                dir,
                &[
                    "symbolic-ref",
                    "--quiet",
                    "--short",
                    "refs/remotes/origin/HEAD",
                ],
            )
            .await
            .ok();
            let branches = run_git(
                dir,
                &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            )
            .await?;
            let names: Vec<String> = branches.lines().map(str::to_owned).collect();
            let base = pick_base(&settings.base_branch, origin_head.as_deref(), &names)
                .ok_or("Couldn't find a base branch; set one under Settings → Context & files")?;
            if base.starts_with('-') {
                return Err(format!("The base branch {base} isn't a valid branch name"));
            }
            let range = format!("{base}...HEAD");
            let text = run_git(
                dir,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--ignore-submodules=all",
                    &range,
                ],
            )
            .await
            .map_err(|e| format!("Couldn't diff against {base}: {e}"))?;
            (
                text,
                Vec::new(),
                format!("changes on this branch against {base}"),
            )
        }
    };
    if text.trim().is_empty() && untracked.is_empty() {
        return Err("No changes to review".into());
    }
    if !untracked.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("\nNew files not in the diff above (contents not included):\n");
        for name in &untracked {
            text.push_str("  new file: ");
            text.push_str(name);
            text.push('\n');
        }
    }
    Ok(ReviewDiff {
        text: truncate(&text, settings.max_diff_kb),
        covers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_diffs_and_cuts_long_ones_at_a_char_boundary() {
        assert_eq!(truncate("short", 16), "short");
        let long = "é".repeat(10_000); // 2 bytes each
        let cut = truncate(&long, 16);
        assert!(cut.ends_with("\n[diff cut at 16 KB]"));
        let kept = cut.trim_end_matches("\n[diff cut at 16 KB]");
        assert!(kept.len() <= 16 * 1024);
        assert!(kept.chars().all(|c| c == 'é'));
    }

    #[test]
    fn base_branch_prefers_the_setting_then_origin_head_then_main_or_master() {
        let names = vec!["feature".to_owned(), "master".to_owned()];
        assert_eq!(
            pick_base(" dev ", Some("origin/main"), &names).as_deref(),
            Some("dev")
        );
        assert_eq!(
            pick_base("", Some("origin/trunk"), &names).as_deref(),
            Some("origin/trunk")
        );
        assert_eq!(pick_base("", None, &names).as_deref(), Some("master"));
        let both = vec!["master".to_owned(), "main".to_owned()];
        assert_eq!(pick_base("", None, &both).as_deref(), Some("main"));
        assert_eq!(pick_base("", None, &["dev".to_owned()]), None);
    }

    #[test]
    fn message_names_the_attachment_and_keeps_the_prompt() {
        let diff = ReviewDiff {
            text: "+x\n".into(),
            covers: "uncommitted changes in the working folder".into(),
        };
        let text = message_text("  Review it.  ", ReviewStrictness::Normal, &diff);
        assert_eq!(
            text,
            "Review it.\n\nThe diff covers uncommitted changes in the working folder and is \
             attached as changes.diff."
        );
        let a = attachment(&diff);
        assert_eq!(a.name, DIFF_NAME);
        assert_eq!(a.text.as_deref(), Some("+x\n"));
    }

    #[test]
    fn strictness_adds_its_instruction_to_the_prompt() {
        let diff = ReviewDiff {
            text: String::new(),
            covers: "x".into(),
        };
        let lenient = message_text("Review it.", ReviewStrictness::Lenient, &diff);
        assert!(lenient.starts_with(
            "Review it. Only report problems that would cause bugs, data loss or security \
             issues; skip style and minor suggestions.\n\n"
        ));
        let strict = message_text("Review it.", ReviewStrictness::Strict, &diff);
        assert!(strict.starts_with(
            "Review it. Be thorough: also report style, naming, readability, missing tests and \
             documentation, and any risky assumption.\n\n"
        ));
        assert!(
            message_text("Review it.", ReviewStrictness::Normal, &diff)
                .starts_with("Review it.\n\n")
        );
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn sync_git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn uncommitted_scope_reads_a_real_repository() {
        if !git_available() {
            eprintln!("git isn't installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cleanrstai-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = ReviewSettings::default();

        // A plain folder is not a repository.
        let err = collect(&dir, &settings).await.unwrap_err();
        assert_eq!(err, "The working folder isn't a git repository");

        sync_git(&dir, &["init", "-q"]);
        sync_git(&dir, &["config", "user.name", "Test"]);
        sync_git(&dir, &["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("lib.txt"), "one\n").unwrap();
        sync_git(&dir, &["add", "."]);
        sync_git(&dir, &["commit", "-qm", "init"]);

        std::fs::write(dir.join("lib.txt"), "one\ntwo changed\n").unwrap();
        std::fs::write(dir.join("new_file.txt"), "hello\n").unwrap();
        let diff = collect(&dir, &settings).await;
        std::fs::remove_dir_all(&dir).ok();
        let diff = diff.unwrap();
        assert!(diff.text.contains("+two changed"), "{}", diff.text);
        assert!(diff.text.contains("new_file.txt"), "{}", diff.text);
        assert_eq!(diff.covers, "uncommitted changes in the working folder");
    }

    /// The review's diff must not run the repository's filter drivers (see
    /// `git::test_support`). Plain git must run them for the same state first.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn uncommitted_scope_does_not_run_the_repositorys_filter_drivers() {
        use crate::git::test_support::{assert_plain_git_runs_filters, dirty, setup_filter_repo};
        if !git_available() {
            eprintln!("git isn't installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cleanrstai-review-{}", uuid::Uuid::new_v4()));
        let markers =
            std::env::temp_dir().join(format!("cleanrstai-markers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&markers).unwrap();
        let settings = ReviewSettings {
            scope: ReviewScope::Uncommitted,
            ..ReviewSettings::default()
        };
        let repo = setup_filter_repo(&dir, &markers);

        assert_plain_git_runs_filters(&dir, &repo);
        dirty(&dir, 'c');
        let diff = collect(&dir, &settings).await;
        let ran = repo.any_marker();
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&markers).ok();
        let diff = diff.unwrap();
        assert!(diff.text.contains("+cccccccc"), "{}", diff.text);
        assert!(
            !ran,
            "the review's diff ran a filter driver of the repository"
        );
    }
}

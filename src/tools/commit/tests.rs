use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::tools::workspace::GIT_COMMIT;

/// A temporary folder removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("cleanrstai-commit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Runs git in `dir` with commit signing off; returns its stdout, and fails the test on error.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with a.txt and b.txt committed, and a workspace opened on it. `None` when git
/// isn't installed.
fn repo() -> Option<(Dir, Workspace)> {
    if !git_available() {
        eprintln!("git isn't installed; skipping");
        return None;
    }
    let dir = Dir::new();
    git(&dir.0, &["init", "-q"]);
    git(&dir.0, &["config", "user.name", "Test"]);
    git(&dir.0, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.0.join("a.txt"), "a\n").unwrap();
    std::fs::write(dir.0.join("b.txt"), "b\n").unwrap();
    git(&dir.0, &["add", "."]);
    git(&dir.0, &["commit", "-qm", "init"]);
    let ws = Workspace::open(&dir.0).unwrap();
    Some((dir, ws))
}

async fn commit(ws: &Workspace, args: serde_json::Value) -> ToolOutput {
    ws.call(GIT_COMMIT, &args, Duration::from_secs(30)).await
}

fn subjects(dir: &Path) -> Vec<String> {
    git(dir, &["log", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn files_in_head(dir: &Path) -> Vec<String> {
    // --no-renames lists a rename's old path as a removal, which is what the tests check.
    git(
        dir,
        &["show", "--no-renames", "--name-only", "--format=", "HEAD"],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn status(dir: &Path) -> String {
    git(dir, &["status", "--porcelain"])
}

#[test]
fn porcelain_entries_become_changes_relative_to_the_workspace() {
    let out = b" M a.txt\0?? sub/new.txt\0R  sub/new.md\0sub/old.md\0 M other/x.txt\0";
    // At the top of the repository every entry is kept.
    let all = parse_status(out, "");
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].path, "a.txt");
    assert_eq!(all[2].label(), "sub/old.md -> sub/new.md");
    // In sub/, the entries outside it are left out and the rest lose the prefix.
    let inside = parse_status(out, "sub/");
    assert_eq!(
        inside,
        vec![
            Change {
                status: "??".into(),
                path: "new.txt".into(),
                from: None
            },
            Change {
                status: "R ".into(),
                path: "new.md".into(),
                from: Some("old.md".into())
            },
        ]
    );
}

#[test]
fn a_rename_takes_both_paths_into_the_selection() {
    let renamed = Change {
        status: "R ".into(),
        path: "new.md".into(),
        from: Some("old.md".into()),
    };
    let other = Change {
        status: " M".into(),
        path: "a.txt".into(),
        from: None,
    };
    let files = vec![(renamed, true), (other, false)];
    assert_eq!(selected_paths(&files), vec!["new.md", "old.md"]);
}

#[test]
fn messages_are_trimmed_and_checked() {
    assert_eq!(clean_message("  Fix it \n").unwrap(), "Fix it");
    assert!(clean_message("   ").is_err());
    assert!(clean_message("a\0b").is_err());
    let long = "x".repeat(MAX_MESSAGE_CHARS + 1);
    assert!(clean_message(&long).unwrap_err().contains("longer"));
}

#[test]
fn the_paths_argument_is_optional_and_must_list_files() {
    assert_eq!(paths_arg(&json!({"message": "m"})).unwrap(), None);
    assert_eq!(paths_arg(&json!({"paths": null})).unwrap(), None);
    assert_eq!(
        paths_arg(&json!({"paths": [" a.txt ", "src/b.rs"]})).unwrap(),
        Some(vec!["a.txt".into(), "src/b.rs".into()])
    );
    assert!(paths_arg(&json!({"paths": []})).is_err());
    assert!(paths_arg(&json!({"paths": [""]})).is_err());
    assert!(paths_arg(&json!({"paths": "a.txt"})).is_err());
}

#[test]
fn failures_say_what_to_do() {
    let identity = "Author identity unknown\n\n*** Please tell me who you are.";
    assert!(describe_failure(identity).contains("git config user.name"));
    assert_eq!(describe_failure(""), "git didn't say why it failed");
    assert_eq!(
        describe_failure("fatal: not a git repository"),
        "The workspace isn't a git repository"
    );
}

#[test]
fn an_empty_selection_never_means_the_whole_repository() {
    let ws = Workspace::open(&std::env::temp_dir()).unwrap();
    assert!(ws.commit_scope(Some(&[])).is_err());
    assert_eq!(ws.commit_scope(None).unwrap(), vec!["."]);
}

#[test]
fn the_preview_lists_the_message_files_and_stat() {
    let changes = vec![Change {
        status: " M".into(),
        path: "a.txt".into(),
        from: None,
    }];
    let text = preview_text("Fix a", &changes, " a.txt | 2 +-\n 1 file changed\n");
    assert!(
        text.starts_with("Commit message:\nFix a\n\nFiles to commit (1):\n M a.txt\n"),
        "{text}"
    );
    assert!(text.ends_with("1 file changed\n"), "{text}");
}

#[tokio::test]
async fn commits_every_change_including_new_files() {
    let Some((dir, ws)) = repo() else { return };
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    std::fs::write(dir.0.join("new.txt"), "new\n").unwrap();
    let out = commit(&ws, json!({"message": "Update a and add new"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.starts_with("Committed "), "{}", out.text);
    assert!(out.text.contains("Update a and add new"), "{}", out.text);
    assert!(out.text.contains("new.txt"), "{}", out.text);
    assert_eq!(subjects(&dir.0), vec!["Update a and add new", "init"]);
    assert_eq!(files_in_head(&dir.0), vec!["a.txt", "new.txt"]);
    assert_eq!(status(&dir.0), "");
}

#[tokio::test]
async fn commits_only_the_given_paths_and_keeps_other_staged_changes_out() {
    let Some((dir, ws)) = repo() else { return };
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    std::fs::write(dir.0.join("b.txt"), "changed\n").unwrap();
    git(&dir.0, &["add", "b.txt"]);
    let out = commit(&ws, json!({"message": "Only a", "paths": ["a.txt"]})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(files_in_head(&dir.0), vec!["a.txt"]);
    // b.txt was staged by the user, so it stays staged for a later commit.
    assert_eq!(status(&dir.0), "M  b.txt\n");
}

#[tokio::test]
async fn refuses_paths_outside_the_workspace_and_commits_nothing() {
    let Some((dir, ws)) = repo() else { return };
    let outside = Dir::new();
    std::fs::write(outside.0.join("secret.txt"), "x\n").unwrap();
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    let absolute = outside.0.join("secret.txt").display().to_string();
    for path in ["../escape.txt".to_owned(), absolute] {
        let out = commit(&ws, json!({"message": "Nope", "paths": [path.clone()]})).await;
        assert!(out.is_error, "{path} was accepted: {}", out.text);
        assert!(
            out.text.contains("outside the workspace") || out.text.contains("\"..\""),
            "{}",
            out.text
        );
    }
    assert_eq!(subjects(&dir.0), vec!["init"]);
    assert_eq!(status(&dir.0), " M a.txt\n");
}

#[tokio::test]
async fn nothing_to_commit_is_an_error() {
    let Some((dir, ws)) = repo() else { return };
    let out = commit(&ws, json!({"message": "Empty"})).await;
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("Nothing to commit"), "{}", out.text);
    assert_eq!(subjects(&dir.0), vec!["init"]);
}

#[tokio::test]
async fn a_commit_in_a_subfolder_covers_only_that_folder() {
    let Some((dir, _)) = repo() else { return };
    std::fs::create_dir_all(dir.0.join("sub")).unwrap();
    std::fs::write(dir.0.join("sub/x.txt"), "x\n").unwrap();
    git(&dir.0, &["add", "."]);
    git(&dir.0, &["commit", "-qm", "add sub"]);
    std::fs::write(dir.0.join("sub/x.txt"), "changed\n").unwrap();
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    let ws = Workspace::open(&dir.0.join("sub")).unwrap();
    let listed = ws.commit_changes_list().await.unwrap();
    let paths: Vec<&str> = listed.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, vec!["x.txt"]);
    let out = commit(&ws, json!({"message": "Sub only"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(files_in_head(&dir.0), vec!["sub/x.txt"]);
    assert_eq!(status(&dir.0), " M a.txt\n");
}

#[tokio::test]
async fn the_dialog_path_commits_checked_files_and_a_rename_with_both_paths() {
    let Some((dir, ws)) = repo() else { return };
    git(&dir.0, &["mv", "b.txt", "renamed.txt"]);
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    let listed = ws.commit_changes_list().await.unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
    let files: Vec<(Change, bool)> = listed
        .into_iter()
        .map(|c| {
            let on = c.path == "renamed.txt";
            (c, on)
        })
        .collect();
    let paths = selected_paths(&files);
    assert_eq!(paths, vec!["renamed.txt", "b.txt"]);
    let summary = ws.commit_changes(" Rename b ", Some(&paths)).await.unwrap();
    assert!(summary.contains("Rename b"), "{summary}");
    let mut head = files_in_head(&dir.0);
    head.sort();
    assert_eq!(head, vec!["b.txt", "renamed.txt"]);
    assert_eq!(status(&dir.0), " M a.txt\n");
}

#[tokio::test]
async fn a_first_commit_in_a_new_repository_works_and_previews() {
    if !git_available() {
        return;
    }
    let dir = Dir::new();
    git(&dir.0, &["init", "-q"]);
    git(&dir.0, &["config", "user.name", "Test"]);
    git(&dir.0, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.0.join("first.txt"), "one\n").unwrap();
    let ws = Workspace::open(&dir.0).unwrap();
    let preview = ws
        .commit_preview(&json!({"message": "First"}))
        .await
        .unwrap();
    assert!(preview.contains("?? first.txt"), "{preview}");
    let out = commit(&ws, json!({"message": "First"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(files_in_head(&dir.0), vec!["first.txt"]);
}

#[tokio::test]
async fn the_preview_shows_the_commit_and_changes_nothing_staged() {
    let Some((dir, ws)) = repo() else { return };
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    std::fs::write(dir.0.join("new.txt"), "new\n").unwrap();
    let preview = ws
        .commit_preview(&json!({"message": "Update", "paths": ["a.txt", "new.txt"]}))
        .await
        .unwrap();
    assert!(
        preview.starts_with("Commit message:\nUpdate\n"),
        "{preview}"
    );
    assert!(preview.contains(" M a.txt"), "{preview}");
    assert!(preview.contains("?? new.txt"), "{preview}");
    assert!(preview.contains("a.txt | 2 +-"), "{preview}");
    // The preview staged nothing.
    assert_eq!(status(&dir.0), " M a.txt\n?? new.txt\n");
    assert_eq!(subjects(&dir.0), vec!["init"]);
}

#[tokio::test]
async fn a_blank_message_is_a_tool_error() {
    let Some((dir, ws)) = repo() else { return };
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    let out = commit(&ws, json!({"message": "  "})).await;
    assert!(out.is_error && out.text.contains("empty"), "{}", out.text);
    assert_eq!(status(&dir.0), " M a.txt\n");
}

/// A pre-commit hook that records it ran and refuses the commit. The git commands never run
/// hooks (`core.hooksPath` is /dev/null), so the commit must succeed without running it.
#[cfg(unix)]
#[tokio::test]
async fn git_hooks_do_not_run_for_a_commit() {
    use std::os::unix::fs::PermissionsExt;
    let Some((dir, ws)) = repo() else { return };
    let marker = dir.0.join("hook-ran");
    let hook = dir.0.join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Proof the hook really runs under plain git, so the check below can't pass vacuously.
    let plain = std::process::Command::new("git")
        .args(["commit", "-q", "--allow-empty", "-m", "plain"])
        .current_dir(&dir.0)
        .output()
        .unwrap();
    assert!(
        !plain.status.success() && marker.exists(),
        "the hook doesn't run under plain git"
    );
    std::fs::remove_file(&marker).unwrap();

    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    let out = commit(&ws, json!({"message": "Hooks off"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert!(!marker.exists(), "the commit ran the pre-commit hook");
    assert_eq!(subjects(&dir.0), vec!["Hooks off", "init"]);
}

/// The repository asks for signed commits with a gpg program that always fails. Signing is
/// off unless Settings allow it, so the commit still works; with it allowed, the failed
/// signature fails the commit, and nothing is left staged.
#[cfg(unix)]
#[tokio::test]
async fn commits_are_signed_only_when_allowed() {
    let Some((dir, ws)) = repo() else { return };
    // The local config wins over a global gpg.format=ssh, so gpg.program is the signer used.
    git(&dir.0, &["config", "commit.gpgsign", "true"]);
    git(&dir.0, &["config", "gpg.format", "openpgp"]);
    git(&dir.0, &["config", "gpg.program", "/bin/false"]);
    std::fs::write(dir.0.join("a.txt"), "changed\n").unwrap();
    let out = commit(&ws, json!({"message": "Unsigned"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(subjects(&dir.0), vec!["Unsigned", "init"]);

    std::fs::write(dir.0.join("a.txt"), "again\n").unwrap();
    let signing = ws.clone().with_commit_signing(true);
    let out = commit(&signing, json!({"message": "Signed"})).await;
    assert!(out.is_error, "{}", out.text);
    assert_eq!(subjects(&dir.0), vec!["Unsigned", "init"]);
    assert_eq!(status(&dir.0), " M a.txt\n");
}

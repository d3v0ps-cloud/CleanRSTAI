//! Undoing a turn's file edits: each `write_file` and `edit_file` result keeps the file's
//! previous content (a checkpoint, see `tools::workspace`), and undo writes it back, newest
//! first. A file that changed again since the edit is left alone.

use std::ops::Range;
use std::path::Path;

use crate::types::{ChatMessage, Role};

/// The turn holding message `idx`: from its user message up to the next one.
pub fn turn_of(messages: &[ChatMessage], idx: usize) -> Range<usize> {
    let start = messages[..=idx.min(messages.len().saturating_sub(1))]
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(0);
    let end = messages[start + 1..]
        .iter()
        .position(|m| m.role == Role::User)
        .map_or(messages.len(), |p| start + 1 + p);
    start..end
}

/// Edits in `range` that can still be undone.
pub fn undoable(messages: &[ChatMessage], range: Range<usize>) -> usize {
    messages[range]
        .iter()
        .filter_map(|m| m.edit.as_ref())
        .filter(|e| e.checkpoint && !e.undone)
        .count()
}

/// What an undo did.
#[derive(Debug, Default, PartialEq)]
pub struct UndoReport {
    pub restored: Vec<String>,
    /// Files left alone, and why.
    pub skipped: Vec<(String, String)>,
}

impl UndoReport {
    pub fn summary(&self) -> String {
        let mut out = match self.restored.len() {
            0 => "No file changes were undone.".to_owned(),
            1 => format!("Undid the change to {}.", self.restored[0]),
            n => format!("Undid the changes to {n} files."),
        };
        for (file, why) in &self.skipped {
            out.push_str(&format!(" Left {file} alone: {why}."));
        }
        out
    }
}

/// Undo the edits in `messages[range]`, newest first, inside the workspace `root`.
pub fn undo(messages: &mut [ChatMessage], range: Range<usize>, root: &Path) -> UndoReport {
    let mut report = UndoReport::default();
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for m in messages[range].iter_mut().rev() {
        let Some(edit) = m.edit.as_mut().filter(|e| e.checkpoint && !e.undone) else {
            continue;
        };
        // The path was checked when the edit ran; check again, the workspace may have moved.
        if !edit.abs.starts_with(&root) {
            report.skipped.push((
                edit.path.clone(),
                "it's outside the chat's workspace".into(),
            ));
            continue;
        }
        let current = std::fs::read(&edit.abs).ok();
        let unchanged = current
            .as_deref()
            .is_some_and(|c| crate::tools::workspace::sha256_hex(c) == edit.after_sha256);
        if !unchanged {
            let why = if current.is_none() {
                "it no longer exists"
            } else {
                "it changed after this edit"
            };
            report.skipped.push((edit.path.clone(), why.into()));
            continue;
        }
        let result = match (&edit.before, edit.created) {
            (_, true) => std::fs::remove_file(&edit.abs),
            (Some(before), false) => std::fs::write(&edit.abs, before),
            (None, false) => continue,
        };
        match result {
            Ok(()) => {
                edit.undone = true;
                edit.before = None;
                report.restored.push(edit.path.clone());
            }
            Err(e) => report.skipped.push((edit.path.clone(), e.to_string())),
        }
    }
    report.restored.dedup();
    report
}

/// Drop the checkpoints of all but the latest `keep_turns` turns (0 keeps them all).
pub fn prune(messages: &mut [ChatMessage], keep_turns: u32) {
    if keep_turns == 0 {
        return;
    }
    let Some(cut) = messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m.role == Role::User)
        .nth(keep_turns as usize - 1)
        .map(|(i, _)| i)
    else {
        return;
    };
    for m in &mut messages[..cut] {
        if let Some(e) = m.edit.as_mut().filter(|e| e.checkpoint) {
            e.checkpoint = false;
            e.before = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::workspace::{EDIT_FILE, EditOptions, WRITE_FILE, Workspace};
    use serde_json::json;
    use std::time::Duration;

    fn ws() -> (std::path::PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("cleanrstai-cp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::open(&dir)
            .unwrap()
            .with_edit_options(EditOptions {
                diff: true,
                checkpoint: true,
                max_checkpoint_bytes: 1 << 20,
            });
        (dir, ws)
    }

    async fn run(ws: &Workspace, f: &str, args: serde_json::Value) -> ChatMessage {
        let out = ws.call(f, &args, Duration::from_secs(5)).await;
        assert!(!out.is_error, "{}", out.text);
        let mut m = ChatMessage::new(Role::Tool, out.text);
        m.edit = out.edit;
        m
    }

    #[tokio::test]
    async fn undoes_a_turn_newest_first_and_skips_files_changed_since() {
        let (dir, ws) = ws();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let mut messages = vec![ChatMessage::new(Role::User, "go")];
        let edit = run(
            &ws,
            EDIT_FILE,
            json!({"path": "a.txt", "old_string": "two", "new_string": "2"}),
        )
        .await;
        let e = edit.edit.as_ref().unwrap();
        assert!(e.diff.contains("-two\n+2\n"), "{}", e.diff);
        assert_eq!(e.before.as_deref(), Some("one\ntwo\n"));
        messages.push(edit);
        messages.push(
            run(
                &ws,
                EDIT_FILE,
                json!({"path": "a.txt", "old_string": "one", "new_string": "1"}),
            )
            .await,
        );
        messages.push(
            run(
                &ws,
                WRITE_FILE,
                json!({"path": "new/b.txt", "content": "b"}),
            )
            .await,
        );
        messages.push(run(&ws, WRITE_FILE, json!({"path": "c.txt", "content": "c"})).await);
        // c.txt changes again after the edit, so undo leaves it alone.
        std::fs::write(dir.join("c.txt"), "changed by hand").unwrap();
        messages.push(ChatMessage::new(Role::Assistant, "done"));

        let turn = turn_of(&messages, 5);
        assert_eq!(turn, 0..6);
        assert_eq!(undoable(&messages, turn.clone()), 4);
        let report = undo(&mut messages, turn.clone(), &dir);
        assert_eq!(report.restored, ["new/b.txt", "a.txt"]);
        assert_eq!(report.skipped.len(), 1);
        assert!(
            report.summary().contains("Left c.txt alone"),
            "{}",
            report.summary()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert!(!dir.join("new/b.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("c.txt")).unwrap(),
            "changed by hand"
        );
        assert_eq!(undoable(&messages, turn), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn without_checkpoints_nothing_is_kept() {
        let (dir, ws) = ws();
        let ws = ws.with_edit_options(EditOptions {
            diff: false,
            checkpoint: false,
            max_checkpoint_bytes: 0,
        });
        let out = ws
            .call(
                WRITE_FILE,
                &json!({"path": "x.txt", "content": "x"}),
                Duration::from_secs(5),
            )
            .await;
        assert!(out.edit.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_older_turns() {
        let edit = |checkpoint| {
            let mut m = ChatMessage::new(Role::Tool, "");
            m.edit = Some(crate::types::FileEdit {
                path: "a".into(),
                abs: "/a".into(),
                diff: String::new(),
                created: false,
                before: Some("old".into()),
                checkpoint,
                after_sha256: String::new(),
                undone: false,
            });
            m
        };
        let mut messages = vec![
            ChatMessage::new(Role::User, "1"),
            edit(true),
            ChatMessage::new(Role::User, "2"),
            edit(true),
        ];
        prune(&mut messages, 1);
        assert!(!messages[1].edit.as_ref().unwrap().checkpoint);
        assert!(messages[1].edit.as_ref().unwrap().before.is_none());
        assert!(messages[3].edit.as_ref().unwrap().checkpoint);
        prune(&mut messages, 0);
        assert!(messages[3].edit.as_ref().unwrap().checkpoint);
    }
}

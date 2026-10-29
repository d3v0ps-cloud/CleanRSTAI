//! Conversation branches: the other versions of a tail of the chat.
//!
//! A tail is the messages from one position to the end of the chat. Regenerating a reply
//! replaces the tail from the reply on, and editing a prompt replaces the tail from the prompt
//! on. The replaced tail is kept as a version of that position: the first message of the
//! current tail stores the others in `ChatMessage::alternatives` (each a complete tail, which
//! may hold alternatives of its own further down), and `alt_index` says where the current one
//! sits among them in creation order.
//!
//! The current tail is what the chat shows, sends to the model, exports, forks and compacts.
//! The versions stay out of `messages` until they're switched to.
//!
//! A regeneration has no new first message until the reply streams in, so its replaced tail
//! waits in a [`Hold`] (see `app.rs`): it is attached when that message arrives, and put back
//! when the reply is stopped or produces nothing ([`settle`], [`restore`]).

use serde::{Deserialize, Serialize};

use crate::types::{ChatMessage, Role};

/// Branch settings (Settings → General → Branches). A chat can turn them off with its
/// `feature:branches` switch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BranchSettings {
    /// Keep the replaced version when a reply is regenerated or a prompt is edited. Off, the
    /// replaced version is dropped, as before; versions already kept stay switchable.
    pub enabled: bool,
    /// Older versions kept per message (1–50). The oldest is dropped when there are more.
    pub max_per_message: u32,
}

impl Default for BranchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_per_message: 10,
        }
    }
}

impl BranchSettings {
    /// Keeps the limit in its range.
    pub fn normalized(&mut self) {
        self.max_per_message = self.max_per_message.clamp(1, 50);
    }
}

/// A replaced tail, set aside while its replacement is produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    /// The position the tail starts at.
    pub at: usize,
    /// Every version at `at` in creation order, the replaced one included.
    versions: Vec<Vec<ChatMessage>>,
    /// Where the replaced version is in `versions`.
    old: usize,
    /// The replaced version stays as an alternative (otherwise it is dropped).
    keep_old: bool,
}

/// Takes the tail at `at` out of `messages` (leaving `messages[..at]`), and returns it with
/// the versions it carried, in creation order. `at` must be in range.
fn gather(messages: &mut Vec<ChatMessage>, at: usize) -> (Vec<Vec<ChatMessage>>, usize) {
    let mut tail = messages.split_off(at);
    let head = &mut tail[0];
    let mut versions = std::mem::take(&mut head.alternatives);
    let old = std::mem::take(&mut head.alt_index).min(versions.len());
    versions.insert(old, tail);
    (versions, old)
}

/// Drops the oldest versions other than `old` until `versions` has at most `cap`, keeping
/// `old` pointing at the same version.
fn trim(versions: &mut Vec<Vec<ChatMessage>>, old: &mut usize, cap: usize) {
    while versions.len() > cap {
        let Some(i) = (0..versions.len()).find(|&i| i != *old) else {
            break;
        };
        versions.remove(i);
        if i < *old {
            *old -= 1;
        }
    }
}

/// Moves the tail `messages[at..]` aside as the version a new tail replaces, keeping it as an
/// alternative. `messages` is left as `messages[..at]`; attach the new tail's first message
/// with [`attach`]. At most `limit` older versions are kept.
pub fn keep_branch(messages: &mut Vec<ChatMessage>, at: usize, limit: usize) -> Hold {
    let (mut versions, mut old) = gather(messages, at);
    trim(&mut versions, &mut old, limit.max(1));
    Hold {
        at,
        versions,
        old,
        keep_old: true,
    }
}

/// Like [`keep_branch`], but the replaced tail is dropped (branching off). Versions kept
/// before stay switchable.
pub fn replace_branch(messages: &mut Vec<ChatMessage>, at: usize, limit: usize) -> Hold {
    let (mut versions, mut old) = gather(messages, at);
    trim(&mut versions, &mut old, limit.max(1) + 1);
    Hold {
        at,
        versions,
        old,
        keep_old: false,
    }
}

/// Gives the message at `hold.at` (the first message of the new tail, already pushed) the
/// versions the hold carries, and the new one's place among them.
pub fn attach(messages: &mut [ChatMessage], hold: &Hold) {
    let Some(head) = messages.get_mut(hold.at) else {
        return;
    };
    let mut versions = hold.versions.clone();
    let (alternatives, alt_index) = if hold.keep_old {
        let n = versions.len();
        (versions, n)
    } else {
        versions.remove(hold.old);
        (versions, hold.old)
    };
    head.alternatives = alternatives;
    head.alt_index = alt_index;
}

/// The first message of a reply was just pushed: when it sits at `hold.at`, it takes the held
/// versions. Later messages of the reply (further rounds) leave it alone.
pub fn reply_started(messages: &mut [ChatMessage], hold: &Hold) {
    if hold.at + 1 == messages.len() {
        attach(messages, hold);
    }
}

/// A compaction marker was inserted at `at`: a held tail at or after it moves up with the
/// messages that follow the marker.
pub fn marker_inserted(hold: &mut Hold, at: usize) {
    if at <= hold.at {
        hold.at += 1;
    }
}

/// Puts the replaced tail back in place of whatever is at `hold.at` now, with the other
/// versions still attached to it.
pub fn restore(messages: &mut Vec<ChatMessage>, hold: Hold) {
    messages.truncate(hold.at);
    let mut versions = hold.versions;
    let mut tail = versions.remove(hold.old);
    if let Some(head) = tail.first_mut() {
        head.alternatives = versions;
        head.alt_index = hold.old;
    }
    messages.extend(tail);
}

/// After a reply ends, the held tail comes back when nothing was streamed into its place, or
/// the reply came out empty (a failed request). A reply with content stays.
pub fn settle(messages: &mut Vec<ChatMessage>, hold: Hold) {
    let empty = messages.get(hold.at).is_none_or(|m| {
        m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty()
    });
    if empty {
        restore(messages, hold);
    }
}

/// Makes the version `to` (in creation order) the current tail at `at`. The tail that was
/// current becomes an alternative. Returns false when there is no such version.
pub fn switch(messages: &mut Vec<ChatMessage>, at: usize, to: usize) -> bool {
    let Some(head) = messages.get(at) else {
        return false;
    };
    if to > head.alternatives.len() || to == head.alt_index.min(head.alternatives.len()) {
        return false;
    }
    let (mut versions, _) = gather(messages, at);
    let mut next = versions.remove(to);
    if let Some(head) = next.first_mut() {
        head.alternatives = versions;
        head.alt_index = to;
    }
    messages.extend(next);
    true
}

/// The current version at `at` and how many there are (both from 1), for the `k/N` arrows.
/// `(1, 1)` when there are no alternatives.
pub fn count(messages: &[ChatMessage], at: usize) -> (usize, usize) {
    messages.get(at).map_or((1, 1), position)
}

/// [`count`] for one message.
pub fn position(msg: &ChatMessage) -> (usize, usize) {
    if msg.alternatives.is_empty() {
        return (1, 1);
    }
    let total = msg.alternatives.len() + 1;
    (msg.alt_index.min(total - 1) + 1, total)
}

/// Sets up a regeneration of the reply after the last user message: the reply's tail is taken
/// aside (kept as a version when branching is on, dropped when it is off), and the chat is left
/// ending at the user message. Returns the hold to attach, or `None` when there is no tail.
pub fn prepare_regenerate(
    messages: &mut Vec<ChatMessage>,
    settings: &BranchSettings,
) -> Result<Option<Hold>, String> {
    let last_user = messages
        .iter()
        .rposition(|m| m.role == Role::User)
        .ok_or("There is nothing to regenerate")?;
    messages[last_user].cancelled = false;
    let at = last_user + 1;
    if at >= messages.len() {
        return Ok(None);
    }
    Ok(Some(divide(messages, at, settings)))
}

/// Takes the tail at `at` aside for an edit of the message there (see [`divide`]). The caller
/// pushes the edited message and attaches the hold to it.
pub fn divide(messages: &mut Vec<ChatMessage>, at: usize, settings: &BranchSettings) -> Hold {
    if settings.enabled {
        keep_branch(messages, at, settings.max_per_message as usize)
    } else {
        replace_branch(messages, at, settings.max_per_message as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::workspace::{EDIT_FILE, EditOptions, Workspace};
    use crate::types::{FileEdit, ToolCall};
    use serde_json::json;
    use std::time::Duration;

    fn msg(role: Role, text: &str) -> ChatMessage {
        ChatMessage::new(role, text)
    }

    fn texts(messages: &[ChatMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.content.as_str()).collect()
    }

    /// Regenerates the reply at `at`: keeps the tail, pushes `reply` as the new head, attaches.
    fn regenerate_with(messages: &mut Vec<ChatMessage>, at: usize, reply: &str, limit: usize) {
        let hold = keep_branch(messages, at, limit);
        messages.push(msg(Role::Assistant, reply));
        attach(messages, &hold);
    }

    #[test]
    fn keep_and_switch_round_trip() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        regenerate_with(&mut m, 1, "new", 10);
        assert_eq!(texts(&m), ["q", "new"]);
        assert_eq!(count(&m, 1), (2, 2));

        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "old"]);
        assert_eq!(count(&m, 1), (1, 2));

        assert!(switch(&mut m, 1, 1));
        assert_eq!(texts(&m), ["q", "new"]);
        assert_eq!(count(&m, 1), (2, 2));
        // Switching to the current version or past the end does nothing.
        assert!(!switch(&mut m, 1, 1));
        assert!(!switch(&mut m, 1, 2));
        assert_eq!(texts(&m), ["q", "new"]);
    }

    #[test]
    fn creation_order_is_kept_across_several_regenerations() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "a")];
        regenerate_with(&mut m, 1, "b", 10);
        regenerate_with(&mut m, 1, "c", 10);
        assert_eq!(texts(&m), ["q", "c"]);
        assert_eq!(count(&m, 1), (3, 3));
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "a"]);
        assert_eq!(count(&m, 1), (1, 3));
        assert!(switch(&mut m, 1, 1));
        assert_eq!(texts(&m), ["q", "b"]);
        assert!(switch(&mut m, 1, 2));
        assert_eq!(texts(&m), ["q", "c"]);
    }

    #[test]
    fn nested_alternatives_survive_switching_outside_them() {
        // q, reply (a tool call, then a result and an answer), and a second regeneration of
        // the answer inside the reply's tail.
        let mut call = msg(Role::Assistant, "");
        call.tool_calls.push(ToolCall {
            id: "c1".into(),
            name: "time".into(),
            arguments: "{}".into(),
        });
        let mut m = vec![
            msg(Role::User, "q"),
            call.clone(),
            msg(Role::Tool, "12:00"),
            msg(Role::Assistant, "noon"),
        ];
        // Regenerate the answer at 3 (inside the outer tail once it is the current one).
        regenerate_with(&mut m, 3, "midnight", 10);
        assert_eq!(texts(&m), ["q", "", "12:00", "midnight"]);

        // Regenerate the whole reply at 1: the tail with its own alternative goes aside.
        regenerate_with(&mut m, 1, "fresh", 10);
        assert_eq!(texts(&m), ["q", "fresh"]);
        assert_eq!(count(&m, 1), (2, 2));

        // Switch back to the first reply: its inner alternative is still there.
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "", "12:00", "midnight"]);
        assert_eq!(count(&m, 3), (2, 2));
        assert!(switch(&mut m, 3, 0));
        assert_eq!(texts(&m), ["q", "", "12:00", "noon"]);
        assert_eq!(count(&m, 3), (1, 2));
        // And the outer alternative is still reachable.
        assert!(switch(&mut m, 1, 1));
        assert_eq!(texts(&m), ["q", "fresh"]);
    }

    #[test]
    fn the_limit_drops_the_oldest_alternative() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "v0")];
        for i in 1..=4 {
            regenerate_with(&mut m, 1, &format!("v{i}"), 2);
        }
        // Two older versions are kept, plus the current one.
        assert_eq!(count(&m, 1), (3, 3));
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "v2"]);
        assert!(switch(&mut m, 1, 1));
        assert_eq!(texts(&m), ["q", "v3"]);
        assert!(switch(&mut m, 1, 2));
        assert_eq!(texts(&m), ["q", "v4"]);
    }

    #[test]
    fn the_limit_keeps_the_replaced_version_when_it_is_the_oldest() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "v0")];
        regenerate_with(&mut m, 1, "v1", 1);
        regenerate_with(&mut m, 1, "v2", 1);
        // Only one alternative is kept: the one replaced last (v1), not the first.
        assert_eq!(count(&m, 1), (2, 2));
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "v1"]);
    }

    #[test]
    fn replacing_drops_the_old_version_and_keeps_the_others() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "a")];
        regenerate_with(&mut m, 1, "b", 10);
        assert_eq!(count(&m, 1), (2, 2));
        // Branching off: the current reply (b) is replaced by c; a stays, b is gone.
        let hold = replace_branch(&mut m, 1, 10);
        m.push(msg(Role::Assistant, "c"));
        attach(&mut m, &hold);
        assert_eq!(texts(&m), ["q", "c"]);
        assert_eq!(count(&m, 1), (2, 2));
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "a"]);
        assert!(switch(&mut m, 1, 1));
        assert_eq!(texts(&m), ["q", "c"]);
    }

    #[test]
    fn only_the_first_message_of_a_reply_takes_the_versions() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        let hold = keep_branch(&mut m, 1, 10);
        // Round one: a tool call. Its result follows; round two's message is later still.
        let mut call = msg(Role::Assistant, "");
        call.tool_calls.push(ToolCall {
            id: "c1".into(),
            name: "time".into(),
            arguments: "{}".into(),
        });
        m.push(call);
        reply_started(&mut m, &hold);
        assert_eq!(count(&m, 1), (2, 2));
        m.push(msg(Role::Tool, "12:00"));
        m.push(msg(Role::Assistant, "noon"));
        reply_started(&mut m, &hold);
        // The later message is not given the versions, and the first keeps them.
        assert_eq!(count(&m, 1), (2, 2));
        assert!(m[3].alternatives.is_empty());
    }

    #[test]
    fn a_marker_before_the_held_tail_moves_the_hold() {
        let mut m = vec![
            msg(Role::User, "a"),
            msg(Role::Assistant, "b"),
            msg(Role::User, "c"),
        ];
        let mut hold = keep_branch(&mut m, 2, 10);
        assert_eq!(hold.at, 2);
        assert_eq!(m.len(), 2);
        // A marker goes in before the held tail: the hold moves up with it.
        m.insert(0, msg(Role::System, "summary"));
        marker_inserted(&mut hold, 0);
        assert_eq!(hold.at, 3);
        // A marker after the held position leaves it where it is.
        marker_inserted(&mut hold, 4);
        assert_eq!(hold.at, 3);
        m.push(msg(Role::Assistant, "d"));
        reply_started(&mut m, &hold);
        assert_eq!((m[3].alt_index, m[3].alternatives.len()), (1, 1));
    }

    #[test]
    fn a_held_tail_is_put_back_on_restore() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        regenerate_with(&mut m, 1, "first", 10);
        // A second regeneration is stopped after its first message arrived.
        let hold = keep_branch(&mut m, 1, 10);
        m.push(msg(Role::Assistant, "partial"));
        attach(&mut m, &hold);
        restore(&mut m, hold);
        assert_eq!(texts(&m), ["q", "first"]);
        assert_eq!(count(&m, 1), (2, 2));
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "old"]);
    }

    #[test]
    fn a_held_tail_is_put_back_when_nothing_streamed() {
        let mut m = vec![
            msg(Role::User, "q"),
            msg(Role::Assistant, "old"),
            msg(Role::Tool, "t"),
        ];
        let hold = keep_branch(&mut m, 1, 10);
        assert_eq!(texts(&m), ["q"]);
        restore(&mut m, hold);
        assert_eq!(texts(&m), ["q", "old", "t"]);
        assert_eq!(count(&m, 1), (1, 1));
    }

    #[test]
    fn settle_keeps_a_reply_with_content_and_restores_an_empty_one() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        let hold = keep_branch(&mut m, 1, 10);
        m.push(msg(Role::Assistant, "new"));
        attach(&mut m, &hold);
        settle(&mut m, hold);
        assert_eq!(texts(&m), ["q", "new"]);
        assert_eq!(count(&m, 1), (2, 2));

        // A request that failed leaves an empty reply: the old one comes back.
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        let hold = keep_branch(&mut m, 1, 10);
        m.push(msg(Role::Assistant, ""));
        attach(&mut m, &hold);
        settle(&mut m, hold);
        assert_eq!(texts(&m), ["q", "old"]);
        assert_eq!(count(&m, 1), (1, 1));

        // Nothing arrived at all.
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        let hold = keep_branch(&mut m, 1, 10);
        settle(&mut m, hold);
        assert_eq!(texts(&m), ["q", "old"]);
    }

    #[test]
    fn regenerate_keeps_the_old_reply_and_off_replaces_it() {
        let on = BranchSettings::default();
        let mut m = vec![
            msg(Role::User, "q"),
            msg(Role::Assistant, "old"),
            msg(Role::Tool, "t"),
        ];
        let hold = prepare_regenerate(&mut m, &on).unwrap().expect("a tail");
        assert_eq!(texts(&m), ["q"]);
        assert_eq!(hold.at, 1);
        m.push(msg(Role::Assistant, "new"));
        attach(&mut m, &hold);
        assert_eq!(count(&m, 1), (2, 2));

        let off = BranchSettings {
            enabled: false,
            ..BranchSettings::default()
        };
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        let hold = prepare_regenerate(&mut m, &off).unwrap().expect("a tail");
        m.push(msg(Role::Assistant, "new"));
        attach(&mut m, &hold);
        assert_eq!(texts(&m), ["q", "new"]);
        assert_eq!(count(&m, 1), (1, 1));
    }

    #[test]
    fn off_keeps_versions_that_already_exist() {
        let on = BranchSettings::default();
        let off = BranchSettings {
            enabled: false,
            ..on.clone()
        };
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "a")];
        let hold = prepare_regenerate(&mut m, &on).unwrap().unwrap();
        m.push(msg(Role::Assistant, "b"));
        attach(&mut m, &hold);
        let hold = prepare_regenerate(&mut m, &off).unwrap().unwrap();
        m.push(msg(Role::Assistant, "c"));
        attach(&mut m, &hold);
        assert_eq!(texts(&m), ["q", "c"]);
        assert_eq!(count(&m, 1), (2, 2));
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m), ["q", "a"]);
    }

    #[test]
    fn regenerate_without_a_tail_changes_nothing_but_the_cancel_flag() {
        let on = BranchSettings::default();
        let mut prompt = msg(Role::User, "q");
        prompt.cancelled = true;
        let mut m = vec![prompt];
        assert_eq!(prepare_regenerate(&mut m, &on).unwrap(), None);
        assert!(!m[0].cancelled);
        assert!(prepare_regenerate(&mut Vec::new(), &on).is_err());
    }

    #[test]
    fn editing_keeps_the_old_turn_as_a_version() {
        let on = BranchSettings::default();
        let mut m = vec![
            msg(Role::User, "one"),
            msg(Role::Assistant, "two"),
            msg(Role::User, "three"),
            msg(Role::Assistant, "four"),
        ];
        let hold = divide(&mut m, 2, &on);
        assert_eq!(texts(&m), ["one", "two"]);
        m.push(msg(Role::User, "three, edited"));
        attach(&mut m, &hold);
        assert_eq!(texts(&m), ["one", "two", "three, edited"]);
        assert_eq!(count(&m, 2), (2, 2));
        assert!(switch(&mut m, 2, 0));
        assert_eq!(texts(&m), ["one", "two", "three", "four"]);
        assert!(switch(&mut m, 2, 1));
        assert_eq!(texts(&m), ["one", "two", "three, edited"]);
    }

    #[test]
    fn editing_with_branching_off_replaces_the_turn() {
        let off = BranchSettings {
            enabled: false,
            ..BranchSettings::default()
        };
        let mut m = vec![msg(Role::User, "one"), msg(Role::Assistant, "two")];
        let hold = divide(&mut m, 0, &off);
        m.push(msg(Role::User, "uno"));
        attach(&mut m, &hold);
        assert_eq!(texts(&m), ["uno"]);
        assert_eq!(count(&m, 0), (1, 1));
    }

    #[test]
    fn a_chat_without_branches_serializes_as_before() {
        let m = msg(Role::Assistant, "hi");
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("alternatives") && !json.contains("alt_index"));
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(count(&[back], 0), (1, 1));
    }

    #[test]
    fn a_chat_with_branches_round_trips_through_json() {
        let mut m = vec![msg(Role::User, "q"), msg(Role::Assistant, "old")];
        regenerate_with(&mut m, 1, "new", 10);
        let json = serde_json::to_string(&m).unwrap();
        let back: Vec<ChatMessage> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(count(&back, 1), (2, 2));
    }

    #[test]
    fn settings_load_from_older_files_and_clamp() {
        let s: BranchSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, BranchSettings::default());
        let mut s = BranchSettings {
            enabled: false,
            max_per_message: 0,
        };
        s.normalized();
        assert_eq!(s.max_per_message, 1);
        s.max_per_message = 500;
        s.normalized();
        assert_eq!(s.max_per_message, 50);
    }

    fn workspace() -> (std::path::PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!("cleanrstai-branch-{}", uuid::Uuid::new_v4()));
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

    async fn edit(ws: &Workspace, old: &str, new: &str) -> ChatMessage {
        let args = json!({"path": "a.txt", "old_string": old, "new_string": new});
        let out = ws.call(EDIT_FILE, &args, Duration::from_secs(5)).await;
        assert!(!out.is_error, "{}", out.text);
        let mut m = ChatMessage::new(Role::Tool, out.text);
        m.edit = out.edit;
        m
    }

    #[tokio::test]
    async fn undo_after_a_switch_works_on_the_current_tail() {
        let (dir, ws) = workspace();
        std::fs::write(dir.join("a.txt"), "orig\n").unwrap();
        // The first version edits orig -> A.
        let mut m = vec![msg(Role::User, "go"), msg(Role::Assistant, "first")];
        m.push(edit(&ws, "orig", "A").await);
        // Regenerate: the second version edits A -> B.
        let hold = keep_branch(&mut m, 1, 10);
        m.push(msg(Role::Assistant, "second"));
        attach(&mut m, &hold);
        m.push(edit(&ws, "A", "B").await);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "B\n");

        // Undo the current (second) turn: the file goes back to A.
        let turn = crate::checkpoints::turn_of(&m, 2);
        crate::checkpoints::undo(&mut m, turn, &dir);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "A\n");

        // Switch to the first version, whose edit wrote A: its undo now applies.
        assert!(switch(&mut m, 1, 0));
        assert_eq!(texts(&m)[..2], ["go", "first"]);
        let turn = crate::checkpoints::turn_of(&m, 2);
        crate::checkpoints::undo(&mut m, turn, &dir);
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "orig\n"
        );

        // The second version kept its own undo state while it was away.
        assert!(switch(&mut m, 1, 1));
        assert_eq!(texts(&m)[..2], ["go", "second"]);
        let fe: &FileEdit = m[2].edit.as_ref().unwrap();
        assert!(fe.undone);
    }
}

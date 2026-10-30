//! Replies that run in the background: the queue for replies past the parallel limit, the
//! time limit, and desktop notifications (Settings → Background). The decisions are pure so
//! they can be tested; `ChatApp` applies them.

use crate::config::BackgroundSettings;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// A reply waiting for a free slot. It is built when it starts, so it reads the chat as it is
/// then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub conv: Uuid,
    /// A "Compact now" run rather than a reply.
    pub compact_only: bool,
    /// The reply comes from the review provider.
    pub review: bool,
    /// The provider's id for this reply (a scheduled task's); `None` for the active provider.
    pub provider: Option<String>,
}

/// Replies waiting for a slot, first come first served. A chat is in it at most once.
#[derive(Debug, Default)]
pub struct RunQueue {
    items: VecDeque<Queued>,
}

impl RunQueue {
    /// Adds a reply to the back of the queue. False when its chat is already queued.
    pub fn push(&mut self, item: Queued) -> bool {
        if self.contains(item.conv) {
            return false;
        }
        self.items.push_back(item);
        true
    }

    /// Takes a chat out of the queue, returning what it was queued to run.
    pub fn remove(&mut self, conv: Uuid) -> Option<Queued> {
        let at = self.items.iter().position(|q| q.conv == conv)?;
        self.items.remove(at)
    }

    pub fn contains(&self, conv: Uuid) -> bool {
        self.items.iter().any(|q| q.conv == conv)
    }

    /// How many replies are queued ahead of this chat's, when it is queued.
    pub fn position(&self, conv: Uuid) -> Option<usize> {
        self.items.iter().position(|q| q.conv == conv)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The reply at the front of the queue, once fewer than `limit` replies are running. It is
    /// taken out of the queue, so calling this in a loop starts each one once.
    pub fn pop_ready(&mut self, running: usize, limit: usize) -> Option<Queued> {
        if running >= limit.max(1) {
            return None;
        }
        self.items.pop_front()
    }
}

/// How a reply ended, for the notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Failed,
    /// Waiting for approval to run this tool.
    NeedsApproval(String),
}

/// The time limit on a reply, or `None` for no limit.
pub fn time_limit(settings: &BackgroundSettings) -> Option<Duration> {
    (settings.time_limit_mins > 0)
        .then(|| Duration::from_secs(u64::from(settings.time_limit_mins) * 60))
}

/// Time left before a reply that started at `started` reaches `limit`.
pub fn time_left(started: Instant, limit: Duration, now: Instant) -> Duration {
    limit.saturating_sub(now.saturating_duration_since(started))
}

/// The error recorded when the time limit stops a reply.
pub fn time_limit_message(mins: u32) -> String {
    let unit = if mins == 1 { "minute" } else { "minutes" };
    format!("Stopped after {mins} {unit} (Settings → Background)")
}

/// Whether a notification is wanted for a reply event. `chat_selected` is whether the chat is
/// the open one; `window_focused` whether the app's window has focus. With
/// `notify_only_unfocused` only an unfocused window notifies; otherwise a notification is sent
/// unless the open chat is the one in view.
pub fn should_notify(
    settings: &BackgroundSettings,
    chat_selected: bool,
    window_focused: bool,
) -> bool {
    if !settings.notify {
        return false;
    }
    if settings.notify_only_unfocused {
        return !window_focused;
    }
    !(chat_selected && window_focused)
}

/// The notification's body: the chat's title and what happened.
pub fn message(chat_title: &str, outcome: &Outcome) -> String {
    let what = match outcome {
        Outcome::Finished => "reply finished".to_owned(),
        Outcome::Failed => "failed".to_owned(),
        Outcome::NeedsApproval(tool) => format!("needs approval for {tool}"),
    };
    format!("{chat_title}: {what}")
}

/// Shows a desktop notification titled CleanRSTAI on a blocking thread, so the UI never waits
/// for the OS. Errors are logged and otherwise ignored. Does nothing in tests.
pub fn notify(rt: &tokio::runtime::Handle, body: String) {
    if cfg!(test) {
        return;
    }
    rt.spawn_blocking(move || {
        let result = notify_rust::Notification::new()
            .appname("CleanRSTAI")
            .summary("CleanRSTAI")
            .body(&body)
            .show();
        if let Err(e) = result {
            log::warn!("desktop notification failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(conv: Uuid) -> Queued {
        Queued {
            conv,
            compact_only: false,
            review: false,
            provider: None,
        }
    }

    #[test]
    fn queue_is_first_in_first_out_and_respects_the_limit() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut q = RunQueue::default();
        assert!(q.push(item(a)));
        assert!(q.push(item(b)));
        assert!(q.push(item(c)));
        assert_eq!(q.position(b), Some(1));
        // Two running with a limit of two: nothing starts.
        assert_eq!(q.pop_ready(2, 2), None);
        assert!(!q.is_empty());
        // One slot frees: the oldest starts.
        assert_eq!(q.pop_ready(1, 2).map(|i| i.conv), Some(a));
        assert_eq!(q.position(b), Some(0));
        assert_eq!(q.pop_ready(1, 2).map(|i| i.conv), Some(b));
        assert_eq!(q.pop_ready(2, 2), None);
        assert_eq!(q.pop_ready(0, 2).map(|i| i.conv), Some(c));
        assert!(q.is_empty());
        assert_eq!(q.pop_ready(0, 2), None);
    }

    #[test]
    fn a_chat_is_queued_once_and_can_be_removed_from_anywhere() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut q = RunQueue::default();
        assert!(q.push(item(a)));
        assert!(
            !q.push(item(a)),
            "a second push for the same chat is refused"
        );
        q.push(item(b));
        q.push(item(c));
        assert_eq!(q.remove(b).map(|i| i.conv), Some(b));
        assert!(!q.contains(b));
        assert_eq!(q.remove(b), None);
        assert_eq!(q.pop_ready(0, 1).map(|i| i.conv), Some(a));
        assert_eq!(q.pop_ready(0, 1).map(|i| i.conv), Some(c));
    }

    #[test]
    fn a_zero_limit_still_lets_one_reply_run() {
        let a = Uuid::new_v4();
        let mut q = RunQueue::default();
        q.push(item(a));
        assert_eq!(q.pop_ready(0, 0).map(|i| i.conv), Some(a));
    }

    #[test]
    fn time_limit_is_none_at_zero_and_counts_down() {
        let mut bg = BackgroundSettings::default();
        assert_eq!(time_limit(&bg), None);
        bg.time_limit_mins = 2;
        let limit = time_limit(&bg).unwrap();
        assert_eq!(limit, Duration::from_secs(120));
        let t0 = Instant::now();
        assert_eq!(time_left(t0, limit, t0), Duration::from_secs(120));
        assert_eq!(
            time_left(t0, limit, t0 + Duration::from_secs(30)),
            Duration::from_secs(90)
        );
        assert_eq!(
            time_left(t0, limit, t0 + Duration::from_secs(500)),
            Duration::ZERO
        );
        assert_eq!(
            time_limit_message(1),
            "Stopped after 1 minute (Settings → Background)"
        );
        assert_eq!(
            time_limit_message(15),
            "Stopped after 15 minutes (Settings → Background)"
        );
    }

    #[test]
    fn notification_decision_follows_the_settings() {
        let mut bg = BackgroundSettings::default();
        // Default: only when the window is unfocused.
        assert!(should_notify(&bg, true, false));
        assert!(should_notify(&bg, false, false));
        assert!(!should_notify(&bg, false, true));
        assert!(!should_notify(&bg, true, true));

        // Unfocused-only off: any chat that isn't the one in view, or any time the window is away.
        bg.notify_only_unfocused = false;
        assert!(should_notify(&bg, false, true));
        assert!(should_notify(&bg, true, false));
        assert!(!should_notify(&bg, true, true));

        // Notifications off: never.
        bg.notify = false;
        assert!(!should_notify(&bg, false, false));
        assert!(!should_notify(&bg, false, true));
    }

    #[test]
    fn notification_bodies_name_the_chat_and_outcome() {
        assert_eq!(
            message("Trip plan", &Outcome::Finished),
            "Trip plan: reply finished"
        );
        assert_eq!(message("Trip plan", &Outcome::Failed), "Trip plan: failed");
        assert_eq!(
            message("Trip plan", &Outcome::NeedsApproval("write_file".into())),
            "Trip plan: needs approval for write_file"
        );
    }
}

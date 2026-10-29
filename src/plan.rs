//! Plan mode: the chat investigates with read-only tools and replies with a numbered plan. The
//! user approves the plan with a button under the reply, which switches the chat to its
//! after-approval mode and asks the model to carry the plan out.

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalMode;
use crate::types::{Conversation, Role};

/// The default system prompt section for a Plan chat (Settings → General → Plan mode).
pub const PLAN_PROMPT: &str = "Plan mode is on, so only read-only tools are available: you can \
read, search and inspect the working folder and use the other tools this chat allows, but you \
cannot write files or run commands. Investigate what the task needs first. Then reply with a \
numbered plan of the changes you would make: the files involved, what changes in each, and any \
open questions or risks. Do not attempt the changes, and do not ask for approval in the reply: \
the user approves the plan with a button under your reply.";

/// The message sent when the user approves a plan, unless Settings say otherwise.
pub const APPROVED_MESSAGE: &str = "The plan is approved. Carry it out.";

/// Plan mode settings (Settings → General → Plan mode).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PlanSettings {
    /// Offer Plan in the mode pickers. Off, a chat in Plan mode is read only.
    pub enabled: bool,
    /// The mode a chat switches to when its plan is approved (never Plan or Read only).
    pub after_approval: ApprovalMode,
    /// The user message that asks the model to carry the approved plan out.
    pub approve_message: String,
    /// The system prompt section for a Plan chat.
    pub prompt: String,
}

impl Default for PlanSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            after_approval: ApprovalMode::Default,
            approve_message: APPROVED_MESSAGE.into(),
            prompt: PLAN_PROMPT.into(),
        }
    }
}

impl PlanSettings {
    /// Keeps the settings usable: the after-approval mode is one a plan can run in, and blank
    /// texts fall back to the defaults.
    pub fn normalized(&mut self) {
        self.after_approval = self.after_mode();
        self.approve_message = self.approval_text();
        if self.prompt.trim().is_empty() {
            self.prompt = PLAN_PROMPT.into();
        }
    }

    /// The mode a chat switches to on approval: Plan and Read only become Default.
    pub fn after_mode(&self) -> ApprovalMode {
        match self.after_approval {
            ApprovalMode::Plan | ApprovalMode::ReadOnly => ApprovalMode::Default,
            m => m,
        }
    }

    /// The approval message, or the default one when it is blank.
    pub fn approval_text(&self) -> String {
        let text = self.approve_message.trim();
        if text.is_empty() {
            APPROVED_MESSAGE.into()
        } else {
            text.to_owned()
        }
    }

    /// The system prompt section for a Plan chat, or the default when the setting is blank.
    pub fn prompt_text(&self) -> &str {
        if self.prompt.trim().is_empty() {
            PLAN_PROMPT
        } else {
            &self.prompt
        }
    }
}

/// Approves the plan in `conv`: the chat switches to its after-approval mode, and the returned
/// text is the user message that asks the model to carry the plan out.
pub fn approve(conv: &mut Conversation, settings: &PlanSettings) -> String {
    conv.permissions.mode = settings.after_mode();
    settings.approval_text()
}

/// Whether the chat is in Plan mode and its latest reply has finished with text and no tool
/// calls, so the plan can be approved. Replies still running or queued are not checked here.
pub fn awaiting_approval(conv: &Conversation, settings: &PlanSettings) -> bool {
    conv.permissions.mode.effective(settings.enabled) == ApprovalMode::Plan
        && conv.messages.last().is_some_and(|m| {
            m.role == Role::Assistant
                && !m.cancelled
                && m.tool_calls.is_empty()
                && !m.content.trim().is_empty()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ToolCall};

    fn chat(mode: ApprovalMode, last: Option<ChatMessage>) -> Conversation {
        let mut conv = Conversation::with_permission_mode(mode);
        conv.messages
            .push(ChatMessage::new(Role::User, "fix the parser"));
        conv.messages.extend(last);
        conv
    }

    #[test]
    fn approve_switches_mode_and_returns_the_message() {
        let mut conv = chat(ApprovalMode::Plan, None);
        let settings = PlanSettings {
            after_approval: ApprovalMode::AutoWorkspace,
            ..Default::default()
        };
        assert_eq!(approve(&mut conv, &settings), APPROVED_MESSAGE);
        assert_eq!(conv.permissions.mode, ApprovalMode::AutoWorkspace);

        // Settings that name a planning or read-only mode fall back to Default.
        let mut conv = chat(ApprovalMode::Plan, None);
        let settings = PlanSettings {
            after_approval: ApprovalMode::ReadOnly,
            approve_message: "  Go.  ".into(),
            ..Default::default()
        };
        assert_eq!(approve(&mut conv, &settings), "Go.");
        assert_eq!(conv.permissions.mode, ApprovalMode::Default);
    }

    #[test]
    fn only_a_finished_plan_reply_in_plan_mode_awaits_approval() {
        let on = PlanSettings::default();
        let reply = ChatMessage::new(Role::Assistant, "1. Edit the lexer.");
        assert!(awaiting_approval(
            &chat(ApprovalMode::Plan, Some(reply.clone())),
            &on
        ));
        assert!(!awaiting_approval(
            &chat(ApprovalMode::Default, Some(reply.clone())),
            &on
        ));
        assert!(!awaiting_approval(&chat(ApprovalMode::Plan, None), &on));

        let mut cancelled = reply.clone();
        cancelled.cancelled = true;
        assert!(!awaiting_approval(
            &chat(ApprovalMode::Plan, Some(cancelled)),
            &on
        ));

        let mut calling = reply.clone();
        calling.tool_calls = vec![ToolCall {
            id: "c".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        }];
        assert!(!awaiting_approval(
            &chat(ApprovalMode::Plan, Some(calling)),
            &on
        ));

        // With plan mode off, a Plan chat is read only and has nothing to approve.
        let off = PlanSettings {
            enabled: false,
            ..Default::default()
        };
        assert!(!awaiting_approval(
            &chat(ApprovalMode::Plan, Some(reply)),
            &off
        ));
    }

    #[test]
    fn settings_normalize_and_load_older_files() {
        let mut s: PlanSettings =
            serde_json::from_str(r#"{"after_approval":"plan","approve_message":"  ","prompt":""}"#)
                .unwrap();
        s.normalized();
        assert_eq!(s.after_approval, ApprovalMode::Default);
        assert_eq!(s.approve_message, APPROVED_MESSAGE);
        assert_eq!(s.prompt, PLAN_PROMPT);
        assert!(s.enabled);

        // A settings file from before plan mode loads with the defaults.
        let old: PlanSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old, PlanSettings::default());
        assert_eq!(old.prompt_text(), PLAN_PROMPT);
    }
}

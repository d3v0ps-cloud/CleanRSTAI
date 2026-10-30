//! Tool-call approvals: calls waiting for the user, and the tools and MCP servers they chose
//! to always allow in a chat. The allow list lives in memory only, so it lasts until the app
//! quits.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::types::ToolCall;

/// The user's answer on an approval prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Deny,
    Once,
    /// Run this call and every later call of the same tool in this chat.
    AlwaysTool,
    /// Run this call and every later call to any tool of the same MCP server in this chat.
    AlwaysServer,
}

impl Approval {
    pub fn runs(self) -> bool {
        self != Approval::Deny
    }
}

/// How a chat's tool calls are approved, picked in the chat's tools menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask for the calls that need approval, unless Settings or "Always allow" say otherwise.
    #[default]
    Default,
    /// File writes and edits, and commands in the Docker sandbox, run without asking.
    AutoWorkspace,
    /// Every tool call runs without asking.
    AllowAll,
    /// No workspace writes or commands; other tools ask as by default.
    ReadOnly,
    /// Read only, and the model proposes a plan that runs once the user approves it (see
    /// `plan`). With plan mode off in Settings, a chat in this mode is read only.
    Plan,
}

impl ApprovalMode {
    pub const ALL: [ApprovalMode; 5] = [
        Self::Default,
        Self::AutoWorkspace,
        Self::AllowAll,
        Self::ReadOnly,
        Self::Plan,
    ];

    /// The modes the pickers list: Plan only while plan mode is on in Settings.
    pub fn offered(plan_enabled: bool) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|m| plan_enabled || *m != Self::Plan)
            .collect()
    }

    /// The mode a chat runs in: a Plan chat is read only while plan mode is off.
    pub fn effective(self, plan_enabled: bool) -> Self {
        match self {
            Self::Plan if !plan_enabled => Self::ReadOnly,
            m => m,
        }
    }

    /// Modes that switch the workspace's writes and commands off (see `ToolSelection::with_mode`).
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::ReadOnly | Self::Plan)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default permissions",
            Self::AutoWorkspace => "Auto in workspace",
            Self::AllowAll => "Allow all",
            Self::ReadOnly => "Read only",
            Self::Plan => "Plan",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Default => "Asks when approval settings don't apply",
            Self::AutoWorkspace => "Edits files and runs sandboxed commands without asking",
            Self::AllowAll => "Runs tool calls without asking",
            Self::ReadOnly => "No file changes or commands; other tools ask",
            Self::Plan => "Reads and proposes a plan; changes nothing until you approve it",
        }
    }

    /// Whether a call that would ask for approval runs without asking. `sandboxed`: shell
    /// commands run in the Docker sandbox.
    pub fn runs_unasked(self, function: &str, sandboxed: bool) -> bool {
        use crate::tools::workspace::{SHELL, changes_things};
        if always_asks(function) {
            return self == Self::AllowAll;
        }
        match self {
            Self::AllowAll => true,
            Self::AutoWorkspace => changes_things(function) && (function != SHELL || sandboxed),
            Self::Default | Self::ReadOnly | Self::Plan => false,
        }
    }
}

/// Whether `function` asks even where the mode or "Run all tools without asking" would let it
/// run: a commit always asks, except in Allow all. It is never remembered by "Always allow".
pub fn always_asks(function: &str) -> bool {
    function == crate::tools::workspace::GIT_COMMIT
}

/// A chat's tool permissions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatPermissions {
    #[serde(skip_serializing_if = "is_default_mode")]
    pub mode: ApprovalMode,
    /// Run this chat's shell commands on this computer even when the Docker sandbox is on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_sandbox: bool,
}

fn is_default_mode(m: &ApprovalMode) -> bool {
    *m == ApprovalMode::Default
}

impl ChatPermissions {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// A tool call waiting for the user's approval.
pub struct PendingApproval {
    pub call: ToolCall,
    /// The MCP server id and tool name behind the call; `None` for built-ins.
    pub origin: Option<(String, String)>,
    /// The server's display name, for the "Always allow" button.
    pub server_name: Option<String>,
    /// For a workspace file write or edit: the change it would make, as a diff.
    pub preview: Option<String>,
    respond: oneshot::Sender<bool>,
}

impl PendingApproval {
    pub fn new(
        call: ToolCall,
        origin: Option<(String, String)>,
        server_name: Option<String>,
        respond: oneshot::Sender<bool>,
    ) -> Self {
        Self {
            call,
            origin,
            server_name,
            preview: None,
            respond,
        }
    }

    /// The diff shown on the prompt.
    pub fn with_preview(mut self, preview: Option<String>) -> Self {
        self.preview = preview;
        self
    }

    pub fn answer(self, run: bool) {
        let _ = self.respond.send(run);
    }

    /// Identifies the tool across rounds: the server and MCP tool name rather than the routed
    /// function name, which can change when servers reconnect.
    fn tool_key(&self) -> String {
        match &self.origin {
            Some((server, tool)) => format!("mcp:{server}:{tool}"),
            None => self.call.name.clone(),
        }
    }
}

/// Tools and MCP servers the user chose to always allow in one chat.
#[derive(Default, Debug)]
pub struct AlwaysAllow {
    tools: HashSet<String>,
    servers: HashSet<String>,
}

impl AlwaysAllow {
    pub fn allows(&self, p: &PendingApproval) -> bool {
        if always_asks(&p.call.name) {
            return false;
        }
        self.tools.contains(&p.tool_key())
            || p.origin
                .as_ref()
                .is_some_and(|(server, _)| self.servers.contains(server))
    }

    /// Remember an "Always allow" answer to `p`. Other answers change nothing.
    pub fn remember(&mut self, p: &PendingApproval, answer: Approval) {
        if always_asks(&p.call.name) {
            return;
        }
        match answer {
            Approval::AlwaysTool => {
                self.tools.insert(p.tool_key());
            }
            Approval::AlwaysServer => {
                if let Some((server, _)) = &p.origin {
                    self.servers.insert(server.clone());
                }
            }
            Approval::Deny | Approval::Once => {}
        }
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.servers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(
        name: &str,
        origin: Option<(&str, &str)>,
    ) -> (PendingApproval, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let call = ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments: "{}".into(),
        };
        let origin = origin.map(|(s, t)| (s.to_owned(), t.to_owned()));
        (PendingApproval::new(call, origin, None, tx), rx)
    }

    #[test]
    fn always_allow_tool_and_server() {
        let mut allow = AlwaysAllow::default();
        let (read, _) = pending("files__read", Some(("srv1", "read")));
        let (write, _) = pending("files__write", Some(("srv1", "write")));
        let (other, _) = pending("web__fetch", Some(("srv2", "fetch")));
        assert!(!allow.allows(&read));
        allow.remember(&read, Approval::Once);
        assert!(allow.is_empty());

        allow.remember(&read, Approval::AlwaysTool);
        assert!(allow.allows(&read));
        assert!(!allow.allows(&write));
        // Same server and tool under a new routed name (e.g. after a reconnect).
        let (renamed, _) = pending("files__read_2", Some(("srv1", "read")));
        assert!(allow.allows(&renamed));

        allow.remember(&write, Approval::AlwaysServer);
        assert!(allow.allows(&write));
        assert!(!allow.allows(&other));
    }

    #[test]
    fn builtins_are_keyed_by_name_and_have_no_server() {
        let mut allow = AlwaysAllow::default();
        let (delegate, _) = pending(crate::tools::DELEGATE, None);
        allow.remember(&delegate, Approval::AlwaysServer);
        assert!(allow.is_empty());
        allow.remember(&delegate, Approval::AlwaysTool);
        assert!(allow.allows(&delegate));
    }

    #[test]
    fn answer_reaches_the_agent() {
        let (p, mut rx) = pending("x__y", Some(("s", "y")));
        p.answer(true);
        assert_eq!(rx.try_recv(), Ok(true));
        assert!(Approval::AlwaysServer.runs());
        assert!(!Approval::Deny.runs());
    }

    #[test]
    fn a_commit_is_never_always_allowed() {
        let mut allow = AlwaysAllow::default();
        let (commit, _) = pending(crate::tools::workspace::GIT_COMMIT, None);
        allow.remember(&commit, Approval::AlwaysTool);
        assert!(allow.is_empty());
        assert!(!allow.allows(&commit));
    }

    #[test]
    fn approval_deserializes_snake_case() {
        assert_eq!(
            serde_json::from_str::<Approval>("\"always_server\"").unwrap(),
            Approval::AlwaysServer
        );
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;

    #[test]
    fn modes_decide_which_calls_ask() {
        use ApprovalMode::*;
        assert!(!Default.runs_unasked("write_file", true));
        assert!(AllowAll.runs_unasked("srv__anything", false));
        assert!(AutoWorkspace.runs_unasked("edit_file", false));
        assert!(AutoWorkspace.runs_unasked("exec_shell_command", true));
        assert!(!AutoWorkspace.runs_unasked("exec_shell_command", false));
        assert!(!AutoWorkspace.runs_unasked("srv__tool", true));
        assert!(!ReadOnly.runs_unasked("builtin__fetch", true));
        // A commit asks in every mode but Allow all, Auto in workspace included.
        assert!(!Default.runs_unasked("git_commit", false));
        assert!(!AutoWorkspace.runs_unasked("git_commit", false));
        assert!(!ReadOnly.runs_unasked("git_commit", false));
        assert!(AllowAll.runs_unasked("git_commit", false));
        assert!(always_asks("git_commit"));
        assert!(!always_asks("edit_file"));
        let p: ChatPermissions = serde_json::from_str("{}").unwrap();
        assert!(p.is_default());
        assert_eq!(serde_json::to_string(&p).unwrap(), "{}");
    }

    #[test]
    fn plan_asks_like_read_only_and_is_offered_only_when_enabled() {
        use ApprovalMode::*;
        assert!(!Plan.runs_unasked("builtin__fetch", true));
        assert!(!Plan.runs_unasked("write_file", true));
        assert!(!Plan.runs_unasked("exec_shell_command", true));
        assert!(!ApprovalMode::offered(false).contains(&Plan));
        assert!(ApprovalMode::offered(true).contains(&Plan));
        assert_eq!(ApprovalMode::offered(false).len(), 4);
        assert_eq!(Plan.effective(true), Plan);
        assert_eq!(Plan.effective(false), ReadOnly);
        assert_eq!(Default.effective(false), Default);
        assert!(Plan.is_read_only() && ReadOnly.is_read_only());
        assert!(!Default.is_read_only());
        assert_eq!(serde_json::to_string(&Plan).unwrap(), "\"plan\"");
        assert_eq!(
            serde_json::from_str::<ApprovalMode>("\"plan\"").unwrap(),
            Plan
        );
        assert_eq!(Plan.label(), "Plan");
    }
}

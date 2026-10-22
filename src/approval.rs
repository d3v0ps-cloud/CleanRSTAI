//! Tool-call approvals: calls waiting for the user, and the tools and MCP servers they chose
//! to always allow in a chat. The allow list lives in memory only, so it lasts until the app
//! quits.

use std::collections::HashSet;

use tokio::sync::oneshot;

use crate::types::ToolCall;

/// The user's answer on an approval prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// A tool call waiting for the user's approval.
pub struct PendingApproval {
    pub call: ToolCall,
    /// The MCP server id and tool name behind the call; `None` for built-ins.
    pub origin: Option<(String, String)>,
    /// The server's display name, for the "Always allow" button.
    pub server_name: Option<String>,
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
            respond,
        }
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
        self.tools.contains(&p.tool_key())
            || p.origin
                .as_ref()
                .is_some_and(|(server, _)| self.servers.contains(server))
    }

    /// Remember an "Always allow" answer to `p`. Other answers change nothing.
    pub fn remember(&mut self, p: &PendingApproval, answer: Approval) {
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
}

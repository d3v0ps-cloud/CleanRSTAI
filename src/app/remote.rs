//! Remote access from the desktop side: starting the server and the Cloudflare Tunnel,
//! answering browser requests with the desktop's own functions, and telling browsers what
//! changed (see `remote/` for the server itself). A child module of `app`, so it uses
//! `ChatApp`'s private fields.

use super::*;

impl ChatApp {
    /// Starts, restarts or stops the remote-access server and its Cloudflare Tunnel to match
    /// the settings. A running server or tunnel with the same settings is left alone; a
    /// change to the tunnel alone doesn't restart the server.
    pub(super) fn sync_remote(&mut self) {
        // Access settings don't restart the server (they are not in `server_settings`), so
        // the server's checks are updated here.
        self.access.configure(&self.settings.general.remote.tunnel);
        self.sync_server();
        self.sync_tunnel();
    }

    pub(super) fn sync_server(&mut self) {
        let cfg = &self.settings.general.remote;
        let server = crate::remote::server_settings(cfg);
        if self
            .remote
            .as_ref()
            .is_some_and(|h| crate::remote::server_settings(h.config()) == server)
        {
            return;
        }
        // Stop the old server first, so its port is free for the new one.
        self.remote = None;
        self.remote_seen.clear();
        self.remote_list_seen = 0;
        if !cfg.enabled {
            self.remote_status = RemoteStatus::Off;
            return;
        }
        let ctx = self.repaint_ctx.clone();
        let repaint = Arc::new(move || ctx.request_repaint());
        let hosts = self.tunnel_hosts.clone();
        let access = self.access.clone();
        match crate::remote::start(&self.rt, cfg, self.tx.clone(), repaint, hosts, access) {
            Ok(handle) => {
                self.remote_status = RemoteStatus::Running {
                    urls: handle.urls().to_vec(),
                    plain_http: cfg.tls == RemoteTls::Off,
                };
                self.remote = Some(handle);
            }
            Err(e) => {
                log::warn!("remote access did not start: {e}");
                self.remote_status = RemoteStatus::Failed(e);
            }
        }
    }

    pub(super) fn sync_tunnel(&mut self) {
        let cfg = &self.settings.general.remote;
        let wanted = cfg.enabled && cfg.tunnel.enabled;
        let password_ok =
            cfg.password.trim().chars().count() >= crate::remote::MIN_TUNNEL_PASSWORD_CHARS;
        let service = crate::remote::tunnel_service_url(cfg);
        if wanted
            && password_ok
            && self.remote.is_some()
            && self
                .tunnel
                .as_ref()
                .is_some_and(|t| *t.config() == cfg.tunnel && t.service() == service)
        {
            return;
        }
        self.tunnel = None;
        self.tunnel_error = None;
        if !wanted {
            return;
        }
        if self.remote.is_none() {
            self.tunnel_error = Some("The tunnel starts once the server is running".into());
            return;
        }
        let ctx = self.repaint_ctx.clone();
        let repaint = Arc::new(move || ctx.request_repaint());
        match crate::remote::start_tunnel(&self.rt, cfg, self.tunnel_hosts.clone(), repaint) {
            Ok(handle) => self.tunnel = Some(handle),
            Err(e) => {
                log::warn!("the Cloudflare Tunnel did not start: {e}");
                self.tunnel_error = Some(e);
            }
        }
    }

    /// The tunnel's state for the settings page.
    pub(super) fn tunnel_status(&self) -> crate::remote::TunnelStatus {
        match (&self.tunnel, &self.tunnel_error) {
            (Some(t), _) => t.status(),
            (None, Some(e)) => crate::remote::TunnelStatus::Failed {
                error: e.clone(),
                retrying: false,
            },
            (None, None) => crate::remote::TunnelStatus::Off,
        }
    }

    /// Answers a request from a browser, through the same code the desktop UI uses.
    pub(super) fn handle_remote(&mut self, cmd: RemoteCommand) {
        let ctx = self.repaint_ctx.clone();
        match cmd {
            RemoteCommand::State { reply } => {
                let _ = reply.send(self.remote_state());
            }
            RemoteCommand::Chat { id, reply } => {
                let _ = reply.send(self.remote_chat(id));
            }
            RemoteCommand::NewChat { reply } => {
                let _ = reply.send(self.create_chat());
            }
            RemoteCommand::Send { chat, text, reply } => {
                let _ = reply.send(self.remote_send(chat, text, &ctx));
            }
            RemoteCommand::Stop { chat, reply } => {
                self.stop(chat);
                let _ = reply.send(());
            }
            RemoteCommand::Regenerate { chat, reply } => {
                let _ = reply.send(self.regenerate(chat, &ctx));
            }
            RemoteCommand::Approve {
                chat,
                call_id,
                answer,
                reply,
            } => {
                let result = if self.approve(chat, &call_id, answer) {
                    Ok(())
                } else {
                    Err("No such approval is waiting".into())
                };
                let _ = reply.send(result);
            }
            RemoteCommand::Rename { chat, title, reply } => {
                let _ = reply.send(self.rename_chat(chat, &title));
            }
            RemoteCommand::Pin {
                chat,
                pinned,
                reply,
            } => {
                let _ = reply.send(self.set_pinned(chat, pinned));
            }
            RemoteCommand::Delete { chat, reply } => {
                self.delete_chat(chat);
                let _ = reply.send(());
            }
            RemoteCommand::SetModel {
                provider,
                model,
                reply,
            } => {
                let _ = reply.send(self.set_remote_model(provider, model));
            }
        }
    }

    /// Sends a browser's message. Without a chat, it goes to a new one (or the empty one).
    pub(super) fn remote_send(
        &mut self,
        chat: Option<Uuid>,
        text: String,
        ctx: &egui::Context,
    ) -> Result<Uuid, String> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Err("The message is empty".into());
        }
        let id = chat.unwrap_or_else(|| self.create_chat());
        self.send_message(id, text, Vec::new(), ctx)?;
        Ok(id)
    }

    /// Picks a provider and its model, as the model picker does.
    pub(super) fn set_remote_model(&mut self, index: usize, model: String) -> Result<(), String> {
        let model = remote_model_name(&model)?;
        let provider = self
            .settings
            .providers
            .get_mut(index)
            .ok_or("No such provider")?;
        provider.model = model.to_owned();
        self.settings.active_provider = index;
        self.save_settings();
        Ok(())
    }

    pub(super) fn remote_state(&self) -> StateView {
        let mut chats: Vec<&Conversation> = self
            .conversations
            .iter()
            .filter(|c| listed(c, &self.runs))
            .collect();
        // Pinned first, then newest first, as in the sidebar.
        chats.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        StateView {
            chats: chats
                .into_iter()
                .map(|c| chat_summary(c, &self.runs, &self.projects))
                .collect(),
            providers: self
                .settings
                .providers
                .iter()
                .map(|p| ProviderView {
                    name: p.name.clone(),
                    model: p.model.clone(),
                    models: p.models.clone(),
                })
                .collect(),
            active_provider: self
                .settings
                .active_provider
                .min(self.settings.providers.len().saturating_sub(1)),
            send_with_enter: self.settings.general.send_with_enter,
            theme: theme_name(&self.settings.general.theme).to_owned(),
            user_name: self.user_name.clone(),
        }
    }

    pub(super) fn remote_chat(&self, id: Uuid) -> Option<ChatView> {
        let conv = self.conversations.iter().find(|c| c.id == id)?;
        let run = self.runs.get(&id);
        Some(ChatView {
            id,
            title: conv.title.clone(),
            pinned: conv.pinned,
            messages: conv.messages.iter().map(message_view).collect(),
            running: run.is_some(),
            running_tool: run.and_then(|r| r.running_tool.clone()),
            approvals: run
                .map(|r| r.approvals.iter().map(approval_view).collect())
                .unwrap_or_default(),
            error: self.errors.get(&id).cloned(),
        })
    }

    /// Tells the connected browsers about the chats and the list that changed since the last
    /// frame. Does nothing while the server is off. A chat that changes faster than
    /// `REMOTE_CHAT_INTERVAL` is told about later, and a repaint is requested for then.
    pub(super) fn publish_remote(&mut self) {
        let Some(remote) = &self.remote else {
            return;
        };
        let current: Vec<(Uuid, u64)> = self
            .conversations
            .iter()
            .map(|conv| {
                let fp = chat_fingerprint(conv, self.runs.get(&conv.id), self.errors.get(&conv.id));
                (conv.id, fp)
            })
            .collect();
        let (chats, wait) = chat_notices(
            &current,
            &mut self.remote_seen,
            &mut self.remote_sent,
            Instant::now(),
        );
        let mut events: Vec<RemoteEvent> = chats
            .into_iter()
            .map(|id| RemoteEvent::Chat { id })
            .collect();
        let list = list_fingerprint(
            &self.conversations,
            &self.runs,
            &self.settings,
            &self.projects,
        );
        if list != self.remote_list_seen {
            self.remote_list_seen = list;
            events.push(RemoteEvent::List);
        }
        for event in events {
            remote.notify(event);
        }
        if let Some(wait) = wait {
            self.repaint_ctx.request_repaint_after(wait);
        }
    }
}

/// The least time between two `RemoteEvent::Chat` notices for the same chat.
const REMOTE_CHAT_INTERVAL: Duration = Duration::from_millis(200);

/// Decides which chats to tell the browsers about this frame. `current` lists each chat with
/// its fingerprint now, `seen` the fingerprints last sent, `sent` when each chat was last
/// notified. Returns the chats to notify, and how long until a changed chat that was held back
/// may go out (`None` when nothing is held back). A held-back chat keeps its old fingerprint, so
/// it still differs on a later frame and is notified then. Deleted chats are notified at once.
pub(super) fn chat_notices(
    current: &[(Uuid, u64)],
    seen: &mut HashMap<Uuid, u64>,
    sent: &mut HashMap<Uuid, Instant>,
    now: Instant,
) -> (Vec<Uuid>, Option<Duration>) {
    let mut notify = Vec::new();
    let mut wait: Option<Duration> = None;
    let mut next_seen = HashMap::with_capacity(current.len());
    for &(id, fp) in current {
        let previous = seen.get(&id).copied();
        if previous == Some(fp) {
            next_seen.insert(id, fp);
            continue;
        }
        match throttle(sent, id, now) {
            None => {
                notify.push(id);
                next_seen.insert(id, fp);
            }
            Some(left) => {
                wait = Some(wait.map_or(left, |w| w.min(left)));
                if let Some(old) = previous {
                    next_seen.insert(id, old);
                }
            }
        }
    }
    let live: HashSet<Uuid> = current.iter().map(|(id, _)| *id).collect();
    // Chats deleted since the last frame: a final notice, never held back.
    notify.extend(seen.keys().filter(|id| !live.contains(id)));
    sent.retain(|id, _| live.contains(id));
    *seen = next_seen;
    (notify, wait)
}

/// Whether chat `id` may be notified at `now`: `None` and the notice is recorded, or the time
/// left until it may be.
pub(super) fn throttle(
    sent: &mut HashMap<Uuid, Instant>,
    id: Uuid,
    now: Instant,
) -> Option<Duration> {
    if let Some(&at) = sent.get(&id) {
        let elapsed = now.saturating_duration_since(at);
        if elapsed < REMOTE_CHAT_INTERVAL {
            return Some(REMOTE_CHAT_INTERVAL - elapsed);
        }
    }
    sent.insert(id, now);
    None
}

/// The model name a browser sent, trimmed. Names are limited to 200 characters.
pub(super) fn remote_model_name(model: &str) -> Result<&str, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Choose a model".into());
    }
    if model.chars().count() > REMOTE_MODEL_MAX_CHARS {
        return Err("Model name is too long".into());
    }
    Ok(model)
}

/// The longest model name a browser may set.
const REMOTE_MODEL_MAX_CHARS: usize = 200;

pub(super) fn chat_summary(
    conv: &Conversation,
    runs: &HashMap<Uuid, Run>,
    projects: &[Project],
) -> ChatSummary {
    ChatSummary {
        id: conv.id,
        title: conv.title.clone(),
        pinned: conv.pinned,
        updated_at: conv.updated_at,
        running: runs.contains_key(&conv.id),
        approvals: runs.get(&conv.id).map_or(0, |r| r.approvals.len()),
        project: conv
            .project
            .and_then(|pid| projects.iter().find(|p| p.id == pid))
            .map(|p| p.name.clone()),
    }
}

pub(super) fn message_view(m: &ChatMessage) -> MessageView {
    MessageView {
        role: m.role,
        content: m.content.clone(),
        reasoning: m.reasoning.clone(),
        tool_calls: m
            .tool_calls
            .iter()
            .map(|call| ToolCallView {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                label: tools::display_name(&call.name),
            })
            .collect(),
        tool_call_id: m.tool_call_id.clone(),
        tool_name: m.tool_name.clone(),
        is_error: m.is_error,
        cancelled: m.cancelled,
        // The text and data of a file stay in the app.
        attachments: m
            .attachments
            .iter()
            .map(|a| AttachmentView {
                name: a.name.clone(),
                mime: a.mime.clone(),
            })
            .collect(),
        stats: m.stats.as_ref().map(crate::ui::stats_view::summary),
        sandbox: m.sandbox.as_ref().map(|s| s.image.clone()),
    }
}

pub(super) fn approval_view(p: &PendingApproval) -> ApprovalView {
    ApprovalView {
        call_id: p.call.id.clone(),
        name: p.call.name.clone(),
        label: tools::display_name(&p.call.name),
        arguments: p.call.arguments.clone(),
        server_name: p.server_name.clone(),
        can_always_tool: !crate::approval::always_asks(&p.call.name),
        can_always_server: p.origin.is_some(),
    }
}

/// A change marker for one chat as the browsers see it. Streaming text changes the last
/// message's length, so that is hashed rather than the whole text.
pub(super) fn chat_fingerprint(
    conv: &Conversation,
    run: Option<&Run>,
    error: Option<&String>,
) -> u64 {
    let mut h = DefaultHasher::new();
    conv.messages.len().hash(&mut h);
    if let Some(last) = conv.messages.last() {
        last.content.len().hash(&mut h);
        last.reasoning.as_ref().map(String::len).hash(&mut h);
    }
    conv.title.hash(&mut h);
    conv.pinned.hash(&mut h);
    conv.updated_at.hash(&mut h);
    conv.workspace.hash(&mut h);
    run.is_some().hash(&mut h);
    if let Some(run) = run {
        run.approvals.len().hash(&mut h);
        run.running_tool.hash(&mut h);
    }
    error.hash(&mut h);
    h.finish()
}

/// A change marker for the chat list and the models, as the browsers see them.
pub(super) fn list_fingerprint(
    conversations: &[Conversation],
    runs: &HashMap<Uuid, Run>,
    settings: &Settings,
    projects: &[Project],
) -> u64 {
    let mut h = DefaultHasher::new();
    for conv in conversations.iter().filter(|c| listed(c, runs)) {
        conv.id.hash(&mut h);
        conv.title.hash(&mut h);
        conv.pinned.hash(&mut h);
        conv.updated_at.hash(&mut h);
        conv.project.hash(&mut h);
        let run = runs.get(&conv.id);
        run.is_some().hash(&mut h);
        run.map_or(0, |r| r.approvals.len()).hash(&mut h);
    }
    for project in projects {
        project.id.hash(&mut h);
        project.name.hash(&mut h);
    }
    settings.active_provider.hash(&mut h);
    settings.general.send_with_enter.hash(&mut h);
    theme_name(&settings.general.theme).hash(&mut h);
    for p in &settings.providers {
        p.name.hash(&mut h);
        p.model.hash(&mut h);
        p.models.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod remote_tests {
    use super::*;
    use crate::attach::Attachment;
    use crate::types::ToolCall;

    fn conv_with_reply(text: &str) -> Conversation {
        let mut conv = Conversation::new();
        conv.messages.push(ChatMessage::new(Role::Assistant, text));
        conv
    }

    /// A run that has nothing going on but one approval waiting for `call_id`.
    fn run_awaiting(call_id: &str) -> Run {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let task = rt.spawn(std::future::pending::<()>());
        let call = ToolCall {
            id: call_id.into(),
            name: "fetch".into(),
            arguments: "{}".into(),
        };
        let (respond, _answer) = tokio::sync::oneshot::channel();
        Run {
            task,
            approvals: vec![PendingApproval::new(call, None, None, respond)],
            running_tool: None,
            live_stats: None,
            retrying: None,
            subagents: HashMap::new(),
            compacting: false,
            compact_only: false,
            started: Instant::now(),
        }
    }

    #[test]
    fn chat_fingerprint_changes_when_a_reply_streams() {
        let mut conv = conv_with_reply("Hel");
        let before = chat_fingerprint(&conv, None, None);
        assert_eq!(before, chat_fingerprint(&conv, None, None));
        conv.messages.last_mut().unwrap().content.push_str("lo");
        assert_ne!(before, chat_fingerprint(&conv, None, None));
    }

    #[test]
    fn chat_fingerprint_changes_when_an_approval_is_added() {
        let conv = conv_with_reply("Working on it");
        let mut run = run_awaiting("call_1");
        let waiting = chat_fingerprint(&conv, Some(&run), None);
        run.approvals.clear();
        assert_ne!(waiting, chat_fingerprint(&conv, Some(&run), None));
        assert_ne!(
            chat_fingerprint(&conv, None, None),
            chat_fingerprint(&conv, None, Some(&"boom".to_owned()))
        );
    }

    #[test]
    fn list_fingerprint_changes_when_a_chat_is_pinned() {
        let settings = Settings::default();
        let mut conv = conv_with_reply("hi");
        let runs = HashMap::new();
        let before = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[]);
        conv.pinned = true;
        let after = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[]);
        assert_ne!(before, after);
    }

    #[test]
    fn list_fingerprint_changes_when_a_chat_moves_to_a_project() {
        let settings = Settings::default();
        let mut conv = conv_with_reply("hi");
        let runs = HashMap::new();
        let project = Project::new("Site", &settings.general.projects);
        let before = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[]);
        conv.project = Some(project.id);
        let after = list_fingerprint(std::slice::from_ref(&conv), &runs, &settings, &[project]);
        assert_ne!(before, after);
    }

    #[test]
    fn chat_notices_hold_back_a_quick_change_and_deliver_it_after_the_interval() {
        let (a, t0) = (Uuid::new_v4(), Instant::now());
        let (mut seen, mut sent) = (HashMap::new(), HashMap::new());
        // A new chat is notified at once.
        let (notify, wait) = chat_notices(&[(a, 1)], &mut seen, &mut sent, t0);
        assert_eq!((notify, wait), (vec![a], None));
        // A change 50 ms later is held back for the rest of the interval, keeping the old
        // fingerprint so it still counts as changed.
        let at = t0 + Duration::from_millis(50);
        let (notify, wait) = chat_notices(&[(a, 2)], &mut seen, &mut sent, at);
        assert!(notify.is_empty());
        assert_eq!(wait, Some(Duration::from_millis(150)));
        assert_eq!(seen.get(&a), Some(&1));
        // A frame before the interval ends still holds it back.
        let (notify, _) = chat_notices(
            &[(a, 2)],
            &mut seen,
            &mut sent,
            t0 + Duration::from_millis(199),
        );
        assert!(notify.is_empty());
        // Once the interval has passed, the held-back change is delivered.
        let (notify, wait) =
            chat_notices(&[(a, 2)], &mut seen, &mut sent, t0 + REMOTE_CHAT_INTERVAL);
        assert_eq!((notify, wait), (vec![a], None));
        assert_eq!(seen.get(&a), Some(&2));
        // Unchanged chats are not notified.
        let (notify, wait) =
            chat_notices(&[(a, 2)], &mut seen, &mut sent, t0 + Duration::from_secs(5));
        assert_eq!((notify, wait), (vec![], None));
    }

    #[test]
    fn chat_notices_send_a_deleted_chat_at_once_and_forget_it() {
        let (a, b, t0) = (Uuid::new_v4(), Uuid::new_v4(), Instant::now());
        let (mut seen, mut sent) = (HashMap::new(), HashMap::new());
        chat_notices(&[(a, 1), (b, 1)], &mut seen, &mut sent, t0);
        // `b` changes and is held back, then is deleted in the same frame: it still goes out.
        let quick = t0 + Duration::from_millis(10);
        let (notify, _) = chat_notices(&[(a, 1), (b, 2)], &mut seen, &mut sent, quick);
        assert!(notify.is_empty());
        let (notify, _) = chat_notices(&[(a, 1)], &mut seen, &mut sent, quick);
        assert_eq!(notify, vec![b]);
        assert!(!seen.contains_key(&b) && !sent.contains_key(&b));
    }

    #[test]
    fn compaction_marker_goes_before_the_kept_messages() {
        let mut cancelled = ChatMessage::new(Role::User, "stopped");
        cancelled.cancelled = true;
        let mut marker = ChatMessage::new(Role::System, "old summary");
        marker.compaction = Some(crate::types::CompactionInfo {
            messages: 2,
            tokens_before: 0,
            manual: false,
        });
        let messages = vec![
            ChatMessage::new(Role::User, "a"),
            marker,
            ChatMessage::new(Role::Assistant, "b"),
            cancelled,
            ChatMessage::new(Role::User, "c"),
            ChatMessage::new(Role::Assistant, "d"),
        ];
        // The cancelled prompt and the old marker don't count.
        assert_eq!(compaction_marker_at(&messages, 2), 4);
        assert_eq!(compaction_marker_at(&messages, 3), 2);
        assert_eq!(compaction_marker_at(&messages, 0), 6);
        assert_eq!(compaction_marker_at(&messages, 4), 0);
    }

    #[test]
    fn regenerate_keeps_the_chat_when_there_is_no_user_message() {
        let mut messages = vec![ChatMessage::new(Role::Assistant, "hi")];
        let off = BranchSettings {
            enabled: false,
            ..BranchSettings::default()
        };
        assert!(branches::prepare_regenerate(&mut messages, &off).is_err());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn regenerate_cuts_back_to_the_last_user_message() {
        let mut messages = vec![
            ChatMessage::new(Role::User, "first"),
            ChatMessage::new(Role::Assistant, "answer"),
            ChatMessage::new(Role::User, "second"),
            ChatMessage::new(Role::Assistant, "old reply"),
            ChatMessage::new(Role::Tool, "result"),
        ];
        messages[2].cancelled = true;
        let off = BranchSettings {
            enabled: false,
            ..BranchSettings::default()
        };
        branches::prepare_regenerate(&mut messages, &off).unwrap();
        let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["first", "answer", "second"]);
        assert!(!messages[2].cancelled);
    }

    #[test]
    fn remote_model_names_are_trimmed_and_limited_to_200_characters() {
        assert_eq!(remote_model_name("  gpt-x "), Ok("gpt-x"));
        assert_eq!(remote_model_name("  "), Err("Choose a model".to_owned()));
        let max = "m".repeat(200);
        assert_eq!(remote_model_name(&max), Ok(max.as_str()));
        let long = "m".repeat(201);
        assert_eq!(
            remote_model_name(&long),
            Err("Model name is too long".to_owned())
        );
        // Counted in characters, not bytes.
        let accents = "é".repeat(200);
        assert_eq!(remote_model_name(&accents), Ok(accents.as_str()));
    }

    #[test]
    fn tool_calls_and_approvals_carry_the_desktop_label() {
        let mut msg = ChatMessage::new(Role::Assistant, "");
        msg.tool_calls.push(ToolCall {
            id: "call_1".into(),
            name: crate::tools::FETCH.into(),
            arguments: "{}".into(),
        });
        let view = message_view(&msg);
        assert_eq!(view.tool_calls[0].label, "Web fetch");
        assert_eq!(view.tool_calls[0].name, crate::tools::FETCH);
        let run = run_awaiting("call_2");
        let approval = approval_view(&run.approvals[0]);
        assert_eq!(approval.name, "fetch");
        assert_eq!(approval.label, "fetch");
    }

    #[test]
    fn message_view_never_carries_attachment_text_or_data() {
        let mut msg = ChatMessage::new(Role::User, "see the file");
        msg.attachments.push(Attachment {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            text: Some("TOP SECRET TEXT".into()),
            data: Some("U0VDUkVUIERBVEE=".into()),
            ..Default::default()
        });
        let view = message_view(&msg);
        assert_eq!(view.attachments[0].name, "notes.txt");
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("notes.txt"));
        assert!(!json.contains("TOP SECRET TEXT"));
        assert!(!json.contains("U0VDUkVUIERBVEE="));
    }
}

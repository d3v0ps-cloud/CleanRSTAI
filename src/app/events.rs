//! Events from background work (`AppEvent`, drained each frame) and from a reply's agent
//! loop (`ChatEvent`). A child module of `app`, so it uses `ChatApp`'s private fields.

use super::*;

impl ChatApp {
    pub(super) fn handle_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::VoiceCaptured(result) => {
                    self.voice.recording = None;
                    match result {
                        Ok(wav) => {
                            self.voice.transcribing = true;
                            let ctx = self.egui_ctx.clone();
                            crate::voice::spawn_transcription(
                                &self.rt,
                                &self.http,
                                &self.settings,
                                wav,
                                &self.tx,
                                move || ctx.request_repaint(),
                            );
                        }
                        Err(e) => self.notice = Some(e),
                    }
                }
                AppEvent::VoiceTranscribed(result) => {
                    self.voice.transcribing = false;
                    match result {
                        Ok(text) => {
                            let id = egui::Id::unique(INPUT_ID);
                            let cursor = egui::TextEdit::load_state(&self.egui_ctx, id)
                                .and_then(|s| s.cursor.char_range())
                                .map(|r| r.primary.index.0);
                            crate::voice::insert_text(&mut self.input, cursor, &text);
                            self.egui_ctx.memory_mut(|m| m.request_focus(id));
                            // Voice mode sends what was said at once.
                            if self.voice_mode && !text.trim().is_empty() {
                                let ctx = self.egui_ctx.clone();
                                self.send(&ctx);
                            } else if self.voice_mode {
                                self.voice_listen_again();
                            }
                        }
                        Err(e) => {
                            self.voice_mode = false;
                            self.notice = Some(format!("Voice input failed: {e}"));
                        }
                    }
                }
                AppEvent::SpeechEnded { key, result } => {
                    if self.voice.speaking_key() == Some(key) {
                        self.voice.speaking = None;
                    }
                    match result {
                        Err(e) => {
                            self.voice_mode = false;
                            self.notice = Some(format!("Reading aloud failed: {e}"));
                        }
                        Ok(()) => self.voice_listen_again(),
                    }
                }
                AppEvent::UpdateChecked { result, explicit } => {
                    use crate::update::UpdateCheck;
                    if let Some(ui) = self.settings_ui.as_mut().filter(|_| explicit) {
                        ui.checking_update = false;
                        ui.update_status = Some(match &result {
                            Ok(UpdateCheck::UpToDate) => Ok(format!(
                                "You're on the latest version (v{}).",
                                env!("CARGO_PKG_VERSION")
                            )),
                            Ok(UpdateCheck::Available(r)) => {
                                Ok(format!("Version {} is available.", r.version))
                            }
                            Err(e) => Err(format!("Update check failed: {e}")),
                        });
                        ui.update_available = matches!(
                            &result,
                            Ok(UpdateCheck::Available(r)) if crate::update::can_install(r)
                        );
                    }
                    match result {
                        Ok(UpdateCheck::Available(r)) => self.update = Some(r),
                        Ok(UpdateCheck::UpToDate) => self.update = None,
                        Err(e) => log::info!("update check failed: {e}"),
                    }
                }
                AppEvent::ReleasesListed(result) => {
                    if let Some(ui) = &mut self.settings_ui {
                        ui.listing_releases = false;
                        ui.releases = Some(result);
                    }
                }
                AppEvent::SandboxChecked(result) => {
                    if let Some(ui) = &mut self.settings_ui {
                        ui.sandbox_busy = false;
                        ui.sandbox_status = Some(result);
                    }
                }
                AppEvent::UpdateProgress { done, total } => {
                    if let Some(UpdateInstall::Downloading { .. }) = self.update_install {
                        self.update_install = Some(UpdateInstall::Downloading { done, total });
                    }
                }
                AppEvent::UpdateInstalled(result) => {
                    self.update_install = Some(match result {
                        Ok(target) => UpdateInstall::Ready(target),
                        Err(e) => {
                            self.notice = Some(format!("Couldn't install the update: {e}"));
                            UpdateInstall::Failed
                        }
                    });
                }
                AppEvent::ModelsFetched {
                    provider_id,
                    result,
                } => {
                    if let Some(ui) = &mut self.settings_ui {
                        ui.fetching_models = false;
                        match result {
                            Ok(models) => {
                                if let Some(p) =
                                    ui.draft.providers.iter_mut().find(|p| p.id == provider_id)
                                {
                                    if p.model.is_empty() {
                                        p.model = models.first().cloned().unwrap_or_default();
                                    }
                                    p.models = models;
                                }
                            }
                            Err(e) => ui.models_error = Some(e),
                        }
                    }
                }
                AppEvent::ReviewReady { conv_id, result } => {
                    if let Err(e) = self.finish_review(conv_id, result) {
                        self.notice = Some(e);
                    }
                }
                AppEvent::CommitListed { conv_id, result } => {
                    if let Some(d) = self.commit_dialog.as_mut().filter(|d| d.conv == conv_id) {
                        d.set_listing(result);
                    }
                }
                AppEvent::Committed { conv_id, result } => {
                    if self
                        .commit_dialog
                        .as_ref()
                        .is_some_and(|d| d.conv == conv_id)
                    {
                        match result {
                            Ok(text) => {
                                self.commit_dialog = None;
                                self.notice = text.lines().next().map(str::to_owned);
                            }
                            Err(e) => {
                                if let Some(d) = self.commit_dialog.as_mut() {
                                    d.busy = false;
                                    d.error = Some(e);
                                }
                            }
                        }
                    }
                }
                AppEvent::OcrStarted => self.reading_ocr += 1,
                AppEvent::FileAttached { result, ocr } => {
                    self.reading_files = self.reading_files.saturating_sub(1);
                    if ocr {
                        self.reading_ocr = self.reading_ocr.saturating_sub(1);
                    }
                    match result {
                        Ok(a) => self.pending_attachments.push(a),
                        // Several files can fail in one drop; list them together.
                        Err(e) => match &mut self.notice {
                            Some(n) if n.starts_with("Couldn't attach: ") => {
                                n.push_str("; ");
                                n.push_str(&e);
                            }
                            n => *n = Some(format!("Couldn't attach: {e}")),
                        },
                    }
                }
                AppEvent::McpResourceRead { name, result } => match result {
                    Ok(list) => self.pending_attachments.extend(list),
                    Err(e) => self.notice = Some(format!("Couldn't read {name}: {e}")),
                },
                AppEvent::McpPromptReady { name, result } => match result {
                    Ok(messages) => self.insert_prompt(messages),
                    Err(e) => self.notice = Some(format!("Prompt {name} failed: {e}")),
                },
                AppEvent::ServerInfo(report) => {
                    let crate::agent::ServerReport {
                        key,
                        info,
                        router,
                        litellm,
                        litellm_key,
                        details,
                    } = *report;
                    if let Some(state) = self.server_info.as_mut().filter(|s| s.key == key) {
                        state.info = info;
                        state.router = router;
                        state.litellm = litellm;
                        state.litellm_key = litellm_key;
                        state.details = details;
                        state.pending = false;
                        state.fetched = Some(std::time::Instant::now());
                    }
                }
                AppEvent::ModelLoadChanged(result) => {
                    if let Err(e) = result {
                        self.notice = Some(format!("Couldn't change the model's load state: {e}"));
                    }
                    // Show the new state straight away.
                    if let Some(state) = &mut self.server_info {
                        state.fetched = None;
                    }
                }
                AppEvent::Remote(cmd) => self.handle_remote(cmd),
                AppEvent::KindDetected {
                    provider_id,
                    kind,
                    explicit,
                } => {
                    if let Some(ui) = &mut self.settings_ui {
                        if explicit {
                            ui.detecting_kind = false;
                        }
                        if let Some(p) = ui.draft.providers.iter_mut().find(|p| p.id == provider_id)
                        {
                            // A background probe only upgrades from Generic; it never overrides a choice.
                            if explicit || p.kind == crate::config::ProviderKind::Generic {
                                p.kind = kind;
                            }
                            if explicit {
                                ui.detect_result = Some(format!("Detected: {}", kind.label()));
                            }
                        }
                    }
                }
                AppEvent::Chat { conv_id, event } => self.handle_chat_event(conv_id, event),
                AppEvent::SavedAs(result) => {
                    self.notice = Some(match result {
                        Ok(msg) | Err(msg) => msg,
                    });
                }
            }
        }
    }

    pub(super) fn handle_chat_event(&mut self, id: Uuid, event: ChatEvent) {
        // Late events from a stopped run are ignored (except approvals, which get dropped → denied).
        if !self.runs.contains_key(&id) {
            return;
        }
        match event {
            ChatEvent::ToolApproval {
                call,
                origin,
                preview,
                respond,
            } => {
                let server_name = origin.as_ref().and_then(|(server, _)| {
                    self.settings
                        .mcp_servers
                        .iter()
                        .find(|s| &s.id == server)
                        .map(|s| s.name.clone())
                });
                let tool = call.name.clone();
                let pending =
                    PendingApproval::new(call, origin, server_name, respond).with_preview(preview);
                if self
                    .always_allow
                    .get(&id)
                    .is_some_and(|a| a.allows(&pending))
                {
                    pending.answer(true);
                } else {
                    self.runs.get_mut(&id).unwrap().approvals.push(pending);
                    self.notify_chat(id, Outcome::NeedsApproval(tool));
                }
                return;
            }
            ChatEvent::ToolRunning(call_id) => {
                self.runs.get_mut(&id).unwrap().running_tool = Some(call_id);
                return;
            }
            ChatEvent::StatsLive(s) => {
                let run = self.runs.get_mut(&id).unwrap();
                run.live_stats = Some(*s);
                run.retrying = None;
                return;
            }
            ChatEvent::Retrying(text) => {
                self.runs.get_mut(&id).unwrap().retrying = Some(text);
                return;
            }
            ChatEvent::SubagentProgress { call_id, reports } => {
                self.runs
                    .get_mut(&id)
                    .unwrap()
                    .subagents
                    .insert(call_id, reports);
                return;
            }
            ChatEvent::Error(e) => {
                self.errors.insert(id, e);
                return;
            }
            ChatEvent::Compacting => {
                self.runs.get_mut(&id).unwrap().compacting = true;
                return;
            }
            ChatEvent::Instructions(files) => {
                self.instructions.insert(id, files);
                return;
            }
            ChatEvent::HookNotice(text) => {
                self.notice = Some(text);
                return;
            }
            _ => {}
        }
        let done = matches!(event, ChatEvent::Done);
        let is_tool_result = matches!(event, ChatEvent::ToolResult(_));
        if matches!(event, ChatEvent::Compacted { .. })
            && let Some(run) = self.runs.get_mut(&id)
        {
            run.compacting = false;
        }
        let is_final = matches!(event, ChatEvent::AssistantFinal(_));
        let keep_turns = self.settings.general.file_edits.keep_turns;
        // Disjoint fields, so the held tails can change while the chat is borrowed.
        let holds = &mut self.holds;
        let Some(conv) = self.conversations.iter_mut().find(|c| c.id == id) else {
            return;
        };
        match event {
            ChatEvent::AssistantStart => {
                conv.messages.push(ChatMessage::new(Role::Assistant, ""));
                // The first reply of a regeneration takes the versions it is replacing.
                if let Some(hold) = holds.get(&id) {
                    branches::reply_started(&mut conv.messages, hold);
                }
            }
            ChatEvent::ContentDelta(s) => {
                if let Some(m) = conv
                    .messages
                    .last_mut()
                    .filter(|m| m.role == Role::Assistant)
                {
                    m.content.push_str(&s);
                }
            }
            ChatEvent::ReasoningDelta(s) => {
                if let Some(m) = conv
                    .messages
                    .last_mut()
                    .filter(|m| m.role == Role::Assistant)
                {
                    m.reasoning.get_or_insert_default().push_str(&s);
                }
            }
            ChatEvent::AssistantFinal(msg) => {
                if let Some(m) = conv
                    .messages
                    .last_mut()
                    .filter(|m| m.role == Role::Assistant)
                {
                    // The message's versions stay with it.
                    let (alternatives, alt_index) =
                        (std::mem::take(&mut m.alternatives), m.alt_index);
                    *m = *msg;
                    m.alternatives = alternatives;
                    m.alt_index = alt_index;
                }
            }
            ChatEvent::ToolResult(msg) => conv.messages.push(*msg),
            ChatEvent::Compacted {
                kept,
                summary,
                info,
                stats,
            } => {
                let at = compaction_marker_at(&conv.messages, kept);
                let mut marker = ChatMessage::new(Role::System, summary);
                marker.compaction = Some(info);
                // The summary request counts in the chat's totals, but not as its context.
                marker.stats = Some(*stats);
                conv.messages.insert(at, marker);
                // A held tail moves up with the messages after the marker.
                if let Some(hold) = holds.get_mut(&id) {
                    branches::marker_inserted(hold, at);
                }
                conv.save();
            }
            ChatEvent::Done => {
                if let Some(hold) = holds.remove(&id) {
                    branches::settle(&mut conv.messages, hold);
                }
                if conv.messages.last().is_some_and(|m| {
                    m.role == Role::Assistant && m.content.is_empty() && m.tool_calls.is_empty()
                }) {
                    conv.messages.pop();
                }
                crate::checkpoints::prune(&mut conv.messages, keep_turns);
                conv.updated_at = chrono::Utc::now();
                conv.save();
            }
            _ => {}
        }
        if done {
            let compact_only = self.runs.get(&id).is_some_and(|r| r.compact_only);
            let failed = self.errors.contains_key(&id);
            self.open_new_artifact(id);
            self.runs.remove(&id);
            if !compact_only {
                let outcome = if failed {
                    Outcome::Failed
                } else {
                    Outcome::Finished
                };
                self.reply_ended(id, outcome);
            }
            self.conversations
                .sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        } else {
            // The final stats are attached to the message now, so drop the live readout.
            if is_final && let Some(run) = self.runs.get_mut(&id) {
                run.live_stats = None;
            }
            if is_tool_result && let Some(run) = self.runs.get_mut(&id) {
                run.running_tool = None;
            }
        }
    }

    /// A reply ended: marks its chat unread when another chat is open, and notifies.
    pub(super) fn reply_ended(&mut self, id: Uuid, outcome: Outcome) {
        if self.current != Some(id) {
            self.unread.insert(id);
        }
        // Voice mode waits while the reply waits for an approval.
        let (finished, failed) = (
            matches!(outcome, Outcome::Finished),
            matches!(outcome, Outcome::Failed),
        );
        self.notify_chat(id, outcome);
        if finished {
            self.voice_reply_done(id);
        } else if failed {
            self.voice_mode = false;
        }
    }

    /// Sends a desktop notification about a chat, when Settings → Background and the chat's
    /// `notify` switch ask for one at this moment.
    pub(super) fn notify_chat(&self, id: Uuid, outcome: Outcome) {
        let Some(conv) = self.conversations.iter().find(|c| c.id == id) else {
            return;
        };
        let mut bg = self.settings.general.background.clone();
        bg.notify =
            ToolSelection::new(&self.settings, &conv.tool_overrides).feature_on(tools::NOTIFY);
        if background::should_notify(&bg, self.current == Some(id), self.window_focused) {
            background::notify(&self.rt, background::message(&conv.title, &outcome));
        }
    }
}

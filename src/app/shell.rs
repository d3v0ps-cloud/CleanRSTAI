//! The window around the chat: the title row (sidebar toggle, back and forward, the Chat/Code
//! switch), the sidebar (New, the pages, Recents, the account footer), the home screen
//! (greeting, composer, recent artifacts), the Projects and Artifacts pages, the strip of
//! pickers under the composer, voice mode, and the Settings modal's frame.

use super::*;
use crate::config::AppMode;
use crate::ui::chrome;

/// Room for the macOS traffic lights, which sit over the window's top-left corner.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: f32 = 72.0;
#[cfg(not(target_os = "macos"))]
const TRAFFIC_LIGHTS: f32 = 0.0;

/// The title row's height, shared by the sidebar and the canvas.
pub(super) const TITLE_HEIGHT: f32 = 44.0;

/// The sidebar's width.
pub(super) const SIDEBAR_WIDTH: f32 = 254.0;

/// Artifact cards: three in a row under the home composer.
const CARD_SIZE: egui::Vec2 = egui::vec2(205.0, 140.0);
const CARD_GAP: f32 = 12.0;

/// Chats looked through for artifacts, newest first.
const ARTIFACT_CHATS: usize = 200;

/// What the main canvas shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Page {
    /// The open chat, or the home screen when it is empty.
    #[default]
    Chat,
    Projects,
    Artifacts,
    /// Scheduled agents: prompts that run on a schedule.
    Scheduled,
}

/// One stop in the back/forward history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Place {
    pub page: Page,
    pub chat: Option<Uuid>,
}

/// Where the window has been, for the title row's back and forward chevrons.
#[derive(Default)]
pub(super) struct History {
    back: Vec<Place>,
    forward: Vec<Place>,
    last: Option<Place>,
}

impl History {
    const LIMIT: usize = 50;

    /// Notes where the window is now: a new place puts the previous one on the back stack and
    /// clears the forward one.
    pub fn observe(&mut self, now: Place) {
        if let Some(last) = self.last
            && last != now
        {
            self.back.push(last);
            if self.back.len() > Self::LIMIT {
                self.back.remove(0);
            }
            self.forward.clear();
        }
        self.last = Some(now);
    }

    /// The place before this one that `valid` still accepts (deleted chats are skipped).
    pub fn back(&mut self, valid: impl Fn(&Place) -> bool) -> Option<Place> {
        while let Some(place) = self.back.pop() {
            if valid(&place) && Some(place) != self.last {
                if let Some(last) = self.last {
                    self.forward.push(last);
                }
                self.last = Some(place);
                return Some(place);
            }
        }
        None
    }

    /// The place after this one, after going back.
    pub fn forward(&mut self, valid: impl Fn(&Place) -> bool) -> Option<Place> {
        while let Some(place) = self.forward.pop() {
            if valid(&place) && Some(place) != self.last {
                if let Some(last) = self.last {
                    self.back.push(last);
                }
                self.last = Some(place);
                return Some(place);
            }
        }
        None
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

/// One artifact found in a chat, for the home screen's cards and the Artifacts page.
pub(super) struct Found {
    pub conv: Uuid,
    pub chat: String,
    pub when: chrono::DateTime<chrono::Utc>,
    pub artifact: crate::artifacts::Artifact,
}

/// Artifacts across the recent chats, found again when a chat gains or loses messages.
#[derive(Default)]
pub(super) struct ArtifactIndex {
    key: Option<u64>,
    pub items: Vec<Found>,
    /// Chats with at least one artifact, for the sidebar's document mark.
    pub chats: HashSet<Uuid>,
}

impl ArtifactIndex {
    pub fn refresh(&mut self, conversations: &[Conversation]) {
        let mut recent: Vec<&Conversation> = conversations
            .iter()
            .filter(|c| !c.messages.is_empty())
            .collect();
        recent.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        recent.truncate(ARTIFACT_CHATS);
        let mut h = DefaultHasher::new();
        for c in &recent {
            (c.id, c.messages.len(), c.updated_at, &c.title).hash(&mut h);
        }
        let key = h.finish();
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.items.clear();
        self.chats.clear();
        for c in recent {
            let found = crate::artifacts::in_conversation(&c.messages);
            if !found.is_empty() {
                self.chats.insert(c.id);
            }
            // Newest first within the chat too.
            for artifact in found.into_iter().rev() {
                self.items.push(Found {
                    conv: c.id,
                    chat: c.title.clone(),
                    when: c.updated_at,
                    artifact,
                });
            }
        }
    }
}

/// The text of a permission mode in the strip under the composer.
fn mode_short(mode: crate::approval::ApprovalMode) -> &'static str {
    use crate::approval::ApprovalMode;
    match mode {
        ApprovalMode::Default => "Ask",
        ApprovalMode::AutoWorkspace => "Auto",
        ApprovalMode::AllowAll => "Allow all",
        ApprovalMode::ReadOnly => "Read only",
        ApprovalMode::Plan => "Plan",
    }
}

/// A model's name for the strip under the composer: without an `org/` prefix, and at most
/// 32 characters.
pub(super) fn short_model(model: &str) -> String {
    let name = model.rsplit('/').next().unwrap_or(model);
    if name.chars().count() > 32 {
        let cut: String = name.chars().take(31).collect();
        format!("{cut}…")
    } else {
        name.to_owned()
    }
}

/// The last part of a folder's path, for labels.
pub(super) fn folder_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl ChatApp {
    // ------------------------------------------------------------------ navigation

    /// Where the window is now.
    fn place(&self) -> Place {
        Place {
            page: self.page,
            chat: self.current,
        }
    }

    /// Records the current place for back/forward, and follows a chat into its mode: opening
    /// a chat with a working folder switches to Code, one without to Chat.
    pub(super) fn track_place(&mut self) {
        // Opening a chat from anywhere (the palette, a notification, a fork) shows it.
        if self.page != Page::Chat
            && self
                .history
                .last
                .is_some_and(|last| last.chat != self.current && last.page == self.page)
        {
            self.page = Page::Chat;
        }
        let now = self.place();
        if self.history.last != Some(now) && self.page == Page::Chat {
            let folder = self
                .current
                .and_then(|id| self.conversations.iter().find(|c| c.id == id))
                .filter(|c| !c.messages.is_empty())
                .map(|c| c.workspace.is_some());
            if let Some(has_folder) = folder {
                let mode = if has_folder {
                    AppMode::Code
                } else {
                    AppMode::Chat
                };
                if self.settings.general.mode != mode {
                    self.settings.general.mode = mode;
                    self.save_settings();
                }
            }
        }
        self.history.observe(now);
    }

    fn go_to(&mut self, place: Place) {
        self.page = place.page;
        self.current = place.chat;
        self.searching = false;
    }

    pub(super) fn go_back(&mut self) {
        let ids: HashSet<Uuid> = self.conversations.iter().map(|c| c.id).collect();
        if let Some(place) = self
            .history
            .back(|p| p.chat.is_none_or(|id| ids.contains(&id)))
        {
            self.go_to(place);
        }
    }

    pub(super) fn go_forward(&mut self) {
        let ids: HashSet<Uuid> = self.conversations.iter().map(|c| c.id).collect();
        if let Some(place) = self
            .history
            .forward(|p| p.chat.is_none_or(|id| ids.contains(&id)))
        {
            self.go_to(place);
        }
    }

    /// The folder of the most recently used chat that has one.
    pub(super) fn last_folder(&self) -> Option<std::path::PathBuf> {
        self.conversations
            .iter()
            .filter(|c| !c.messages.is_empty() && c.workspace.is_some())
            .max_by_key(|c| c.updated_at)
            .and_then(|c| c.workspace.clone())
    }

    /// The working folders of recent chats, newest first, without repeats.
    fn recent_folders(&self, limit: usize) -> Vec<std::path::PathBuf> {
        let mut chats: Vec<&Conversation> = self
            .conversations
            .iter()
            .filter(|c| c.workspace.is_some())
            .collect();
        chats.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        let mut out: Vec<std::path::PathBuf> = Vec::new();
        for c in chats {
            if let Some(w) = &c.workspace
                && !out.contains(w)
            {
                out.push(w.clone());
                if out.len() == limit {
                    break;
                }
            }
        }
        out
    }

    /// A new chat follows the mode: in Code it starts in the last folder used, in Chat it has
    /// none (an empty chat reused from Code mode loses its folder).
    pub(super) fn fit_new_chat_to_mode(&mut self, id: Uuid) {
        let mode = self.settings.general.mode;
        let last = self.last_folder();
        if let Some(conv) = self.conv_mut(id)
            && conv.messages.is_empty()
            && conv.project.is_none()
        {
            match mode {
                AppMode::Chat => conv.workspace = None,
                AppMode::Code if conv.workspace.is_none() => conv.workspace = last,
                AppMode::Code => {}
            }
        }
    }

    pub(super) fn set_mode(&mut self, mode: AppMode) {
        if self.settings.general.mode == mode {
            return;
        }
        self.settings.general.mode = mode;
        self.save_settings();
        self.page = Page::Chat;
        // Open the newest chat of the mode, or a new one.
        let code = mode == AppMode::Code;
        let newest = self
            .conversations
            .iter()
            .filter(|c| listed(c, &self.runs) && c.workspace.is_some() == code)
            .max_by_key(|c| c.updated_at)
            .map(|c| c.id);
        let open_is_fresh = self.is_fresh_chat();
        match newest {
            Some(id) if !open_is_fresh => self.current = Some(id),
            _ => self.new_chat(),
        }
    }

    // ------------------------------------------------------------------ title row

    /// A strip of the title row that moves the window when dragged (and maximizes it on a
    /// double click), behind whatever is drawn on it afterwards.
    fn drag_area(ui: &mut egui::Ui, rect: egui::Rect, salt: &str) {
        let resp = ui.interact(rect, egui::Id::unique(salt), egui::Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }

    /// The sidebar toggle, back and forward, and the Chat/Code switch. `leading` leaves room
    /// for the macOS traffic lights.
    fn title_controls(&mut self, ui: &mut egui::Ui, leading: bool) {
        ui.add_space(if leading { TRAFFIC_LIGHTS } else { 0.0 });
        // Compact buttons, so the row fits beside the macOS traffic lights in the sidebar's
        // 238px: 72 + 3 × 28 + the 62px switch, with small gaps.
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding = egui::vec2(4.0, 4.0);
        let tip = if self.sidebar_open {
            "Hide sidebar (⌘B)"
        } else {
            "Show sidebar (⌘B)"
        };
        if chrome::ghost_icon(ui, icon::SIDEBAR, tip, false).clicked() {
            self.sidebar_open = !self.sidebar_open;
        }
        let back = ui
            .add_enabled_ui(self.history.can_go_back(), |ui| {
                chrome::ghost_icon(ui, icon::CARET_LEFT, "Back (⌘[)", false)
            })
            .inner;
        if back.clicked() {
            self.go_back();
        }
        let forward = ui
            .add_enabled_ui(self.history.can_go_forward(), |ui| {
                chrome::ghost_icon(ui, icon::CARET_RIGHT, "Forward (⌘])", false)
            })
            .inner;
        if forward.clicked() {
            self.go_forward();
        }
        ui.add_space(6.0);
        let mut mode = self.settings.general.mode;
        if chrome::segmented(
            ui,
            &mut mode,
            &[
                (AppMode::Chat, icon::CHAT_CIRCLE, "Chat"),
                (
                    AppMode::Code,
                    icon::CODE,
                    "Code: chats with a working folder",
                ),
            ],
        ) {
            self.set_mode(mode);
        }
    }

    /// The sidebar's part of the title row.
    fn sidebar_title(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), TITLE_HEIGHT),
            egui::Sense::hover(),
        );
        Self::drag_area(ui, rect, "sidebar-drag");
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        self.title_controls(&mut row, true);
    }

    /// The canvas's title row: with the sidebar hidden, its controls on the left; on the right,
    /// update and MCP status, the artifact panel's toggle and the incognito ghost.
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), TITLE_HEIGHT),
            egui::Sense::hover(),
        );
        Self::drag_area(ui, rect, "canvas-drag");
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if !self.sidebar_open {
            self.title_controls(&mut row, true);
        }
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let memory_on = self.settings.general.memory.enabled;
            let incognito = memory_on && self.incognito_now();
            let tip = if !memory_on {
                "Memory is off, so no chat uses or saves memories. Click for its settings."
            } else if incognito {
                "Incognito: this chat uses no memories and saves none. Click for a normal chat."
            } else {
                "New incognito chat: no memories are used or saved"
            };
            if chrome::ghost_icon(ui, icon::GHOST, tip, incognito).clicked() {
                if !memory_on {
                    self.open_settings(Tab::Context);
                } else if incognito && self.is_fresh_chat() {
                    self.new_chat();
                    if let Some(conv) = self.current.and_then(|id| self.conv_mut(id)) {
                        conv.tool_overrides.set(
                            ToolOverrides::feature_key(tools::MEMORY),
                            true,
                            true,
                        );
                    }
                } else {
                    self.new_incognito_chat();
                }
                self.page = Page::Chat;
            }
            self.status_items(ui);
        });
    }

    // ------------------------------------------------------------------ sidebar

    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        // The 1px divider on the right edge, over the panel's margin.
        let edge = ui.max_rect().right() + 8.0 - 0.5;
        ui.painter().with_clip_rect(ui.ctx().content_rect()).vline(
            edge,
            ui.ctx().content_rect().y_range(),
            egui::Stroke::new(1.0, p.hairline),
        );
        // For the document marks on chats with artifacts (cheap unless chats changed).
        self.artifact_index.refresh(&self.conversations);
        self.sidebar_title(ui);
        egui::Panel::bottom("sidebar-footer")
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 6,
                bottom: 10,
            }))
            .show(ui, |ui| self.sidebar_footer(ui));

        ui.spacing_mut().item_spacing.y = chrome::ROW_GAP;
        ui.add_space(2.0);
        if chrome::nav_row(ui, icon::PLUS, "New", false, true)
            .on_hover_text("New chat (⌘N)")
            .clicked()
        {
            self.new_chat();
            self.page = Page::Chat;
            self.settings_ui = None;
        }
        ui.add_space(6.0);
        let items = [
            (icon::SEARCH, "Search"),
            (icon::FOLDERS, "Projects"),
            (icon::SHAPES, "Artifacts"),
            (icon::CALENDAR, "Scheduled"),
            (icon::SLIDERS_H, "Customize"),
        ];
        for (glyph, label) in items {
            let active = match label {
                "Search" => self.searching,
                "Projects" => self.page == Page::Projects,
                "Artifacts" => self.page == Page::Artifacts,
                "Scheduled" => self.page == Page::Scheduled,
                _ => false,
            };
            let hover = match label {
                "Search" => "Search chats by title and text",
                "Projects" => "Groups of chats with shared instructions, files and settings",
                "Artifacts" => "Web pages, drawings and code from your chats",
                "Scheduled" => "Agents that run a prompt on a schedule",
                _ => "Skills, hooks and slash commands (Settings → Skills & hooks)",
            };
            if chrome::nav_row(ui, glyph, label, active, false)
                .on_hover_text(hover)
                .clicked()
            {
                match label {
                    "Search" => {
                        self.searching = !self.searching;
                        if !self.searching {
                            self.search.clear();
                        }
                    }
                    "Projects" => self.page = Page::Projects,
                    "Artifacts" => self.page = Page::Artifacts,
                    "Scheduled" => self.page = Page::Scheduled,
                    _ => self.open_settings(Tab::Extensions),
                }
            }
        }
        if self.searching {
            ui.add_space(6.0);
            let id = egui::Id::unique("sidebar-search");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id(id)
                    .hint_text(RichText::new("Search chats").color(p.pencil))
                    .desired_width(f32::INFINITY),
            );
            if self.search_focus {
                resp.request_focus();
                self.search_focus = false;
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.searching = false;
                self.search.clear();
            }
        } else {
            self.search_focus = true;
        }
        ui.add_space(12.0);

        let needle = self.search.trim().to_lowercase();
        let searching = self.searching && !needle.is_empty();
        let header = if searching { "Results" } else { "Recents" };
        if chrome::disclosure(ui, header, self.recents_open || searching).clicked() && !searching {
            self.recents_open = !self.recents_open;
        }
        if !(self.recents_open || searching) {
            return;
        }
        let code = self.settings.general.mode == AppMode::Code;
        let mut rows: Vec<usize> = (0..self.conversations.len())
            .filter(|&i| {
                let c = &self.conversations[i];
                // An empty new chat isn't listed until it has a message; a search looks in
                // both modes.
                listed(c, &self.runs)
                    && if searching {
                        c.title.to_lowercase().contains(&needle)
                            || c.messages
                                .iter()
                                .any(|m| m.content.to_lowercase().contains(&needle))
                    } else {
                        c.workspace.is_some() == code
                    }
            })
            .collect();
        // Pinned chats first, then everything else, newest first.
        rows.sort_by(|&a, &b| {
            let (a, b) = (&self.conversations[a], &self.conversations[b]);
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        let mut acts = SidebarActs::default();
        egui::ScrollArea::vertical()
            .id_salt("recents")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = chrome::ROW_GAP;
                if rows.is_empty() {
                    let text = if searching {
                        "No matching chats"
                    } else if code {
                        "Chats with a working folder show here"
                    } else {
                        "Your chats show here"
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.label(RichText::new(text).size(13.0).color(p.pencil));
                    });
                }
                for idx in rows {
                    self.chat_row(ui, idx, &mut acts, p.shelf);
                }
            });
        self.apply_sidebar_acts(acts);
    }

    /// The account footer: avatar, name and provider with a menu, and the remote-access
    /// (this device) button.
    fn sidebar_footer(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        let name = self.display_name().unwrap_or_else(|| "You".to_owned());
        let plan = self.settings.provider().name.clone();
        ui.horizontal(|ui| {
            let device_w = 34.0;
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width() - device_w, 40.0),
                egui::Sense::click(),
            );
            let label = format!("{name} · {plan}");
            resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            let t = chrome::hover_amount(ui, &resp);
            let painter = ui.painter();
            painter.rect_filled(rect, 8, p.hover.gamma_multiply(t));
            let avatar = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 8.0 + 12.0, rect.center().y),
                egui::Vec2::splat(24.0),
            );
            painter.circle_filled(avatar.center(), 12.0, p.ink2);
            let initial: String = name
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_else(|| "?".into());
            painter.text(
                avatar.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(12.0),
                p.shelf,
            );
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &name,
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(14.0), p.ink),
            );
            job.append(
                &format!(" · {plan}"),
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(13.0), p.pencil),
            );
            let text_left = avatar.right() + 10.0;
            job.wrap = egui::text::TextWrapping::truncate_at_width(
                (rect.right() - 22.0 - text_left).max(20.0),
            );
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
            let y = rect.center().y - galley.size().y / 2.0;
            painter.galley(egui::pos2(text_left, y), galley, p.ink);
            painter.text(
                egui::pos2(rect.right() - 10.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                icon::CARET_DOWN,
                egui::FontId::proportional(11.0),
                p.pencil,
            );
            egui::Popup::menu(&resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(220.0);
                    if ui
                        .button(theme::labelled(icon::GEAR, "Settings  ⌘,"))
                        .clicked()
                    {
                        self.open_settings(Tab::General);
                    }
                    if ui
                        .button(theme::labelled(icon::CLOUD, "Providers and models"))
                        .clicked()
                    {
                        self.open_settings(Tab::Providers);
                    }
                    if ui
                        .button(theme::labelled(icon::PALETTE, "Appearance"))
                        .clicked()
                    {
                        self.open_settings(Tab::Appearance);
                    }
                    if ui
                        .button(theme::labelled(icon::COMMAND, "Command palette  ⌘K"))
                        .clicked()
                    {
                        self.palette.toggle();
                    }
                    ui.separator();
                    if ui
                        .button(theme::labelled(icon::KEYBOARD, "Keyboard shortcuts"))
                        .clicked()
                    {
                        let _ = open::that_detached(
                            "https://d3v0ps-cloud.github.io/CleanRSTAI/reference/keyboard-shortcuts.html",
                        );
                    }
                    if ui
                        .button(theme::labelled(icon::BOOKS, "Documentation"))
                        .clicked()
                    {
                        let _ = open::that_detached("https://d3v0ps-cloud.github.io/CleanRSTAI/");
                    }
                    if ui
                        .button(theme::labelled(icon::REGENERATE, "Check for updates"))
                        .clicked()
                    {
                        let url = self.settings.general.update_url.clone();
                        self.open_settings(Tab::General);
                        if let Some(s) = &mut self.settings_ui {
                            s.checking_update = true;
                            s.update_status = None;
                        }
                        self.spawn_update_check(true, url);
                    }
                    ui.separator();
                    ui.label(
                        RichText::new(concat!("CleanRSTAI v", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(p.pencil),
                    );
                });

            let (tint, tip) = match &self.remote_status {
                crate::remote::RemoteStatus::Running { urls, .. } => (
                    p.ok,
                    format!(
                        "Remote access is on: {}\nClick for its settings",
                        urls.first().map(String::as_str).unwrap_or("running")
                    ),
                ),
                crate::remote::RemoteStatus::Failed(e) => {
                    (p.warn, format!("Remote access didn't start: {e}"))
                }
                crate::remote::RemoteStatus::Off => (
                    p.ink2,
                    "This computer: remote access from a browser is off. Click to set it up."
                        .to_owned(),
                ),
            };
            let device = ui
                .add(
                    egui::Button::new(RichText::new(icon::LAPTOP).size(17.0).color(tint))
                        .frame_when_inactive(false)
                        .min_size(egui::vec2(30.0, 30.0)),
                )
                .on_hover_text(tip);
            if device.clicked() {
                self.open_settings(Tab::General);
            }
        });
    }

    /// The name for the greeting and the footer: Settings → Appearance, or the account's.
    pub(super) fn display_name(&self) -> Option<String> {
        let own = self.settings.general.display_name.trim();
        if own.is_empty() {
            self.user_name.clone()
        } else {
            Some(own.to_owned())
        }
    }

    // ------------------------------------------------------------------ home

    /// The empty chat: the greeting, the composer with its strip, and recent artifacts, the
    /// composer a little above the middle.
    pub(super) fn home(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        self.artifact_index.refresh(&self.conversations);
        let cards = self.artifact_index.items.len().min(3);
        let avail = ui.available_height();
        // The greeting and composer, and the whole page, as tall as they were last frame.
        let measured = egui::Id::unique("home-heights");
        let (above, total) = ui
            .ctx()
            .data(|d| d.get_temp::<(f32, f32)>(measured))
            .unwrap_or((200.0, if cards > 0 { 420.0 } else { 240.0 }));
        // The composer's middle at about 55% of the height, if everything fits.
        let ideal = avail * 0.55 - above + 50.0;
        let top = ideal.min(avail - total - 16.0).max(24.0);
        egui::ScrollArea::vertical()
            .id_salt("home")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(top);
                let start = ui.cursor().top();
                let code = self.settings.general.mode == AppMode::Code;
                chat_view::greeting(ui, self.display_name().as_deref(), code);
                ui.add_space(28.0);
                // Down to the composer's box, which follows the notice and slash menu.
                let above = ui.cursor().top() - start + 100.0;
                self.composer(ui);
                let provider = self.settings.provider();
                let local = ["localhost", "127.0.0.1", "0.0.0.0"]
                    .iter()
                    .any(|h| provider.base_url.contains(h));
                if provider.api_key.is_empty() && !local {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(
                                "No API key for this provider: add one in Settings (⌘,).",
                            )
                            .small()
                            .color(p.warn),
                        );
                    });
                }
                if cards > 0 {
                    ui.add_space(28.0);
                    chat_view::centered_at(ui, chat_view::HOME_WIDTH, |ui| {
                        self.recent_artifacts(ui);
                    });
                }
                let total = ui.cursor().top() - start;
                ui.ctx()
                    .data_mut(|d| d.insert_temp(measured, (above, total)));
                ui.add_space(24.0);
            });
    }

    /// "Recent artifacts" with "View all", over the three newest artifacts' cards.
    fn recent_artifacts(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Recent artifacts").size(13.0).color(p.pencil));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("View all").size(13.0).color(p.pencil))
                            .frame_when_inactive(false),
                    )
                    .clicked()
                {
                    self.page = Page::Artifacts;
                }
            });
        });
        ui.add_space(8.0);
        let width = ui.available_width();
        let card_w = ((width - 2.0 * CARD_GAP) / 3.0).max(120.0);
        let size = egui::vec2(card_w, CARD_SIZE.y);
        let mut open = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for (i, found) in self.artifact_index.items.iter().take(3).enumerate() {
                if artifact_card(ui, found, size, i).clicked() {
                    open = Some(i);
                }
            }
        });
        if let Some(i) = open {
            self.open_found(i);
        }
    }

    /// Opens artifact `i` of the index in its chat, in the side panel.
    fn open_found(&mut self, i: usize) {
        let Some(found) = self.artifact_index.items.get(i) else {
            return;
        };
        let conv = found.conv;
        self.artifact = Some(ArtifactView::new(conv, &found.artifact, false));
        self.current = Some(conv);
        self.page = Page::Chat;
    }

    // ------------------------------------------------------------------ pages

    /// A page's serif heading, with buttons on the right.
    pub(super) fn page_heading(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
        let p = theme::palette(ui.visuals().dark_mode);
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).font(theme::serif(30.0)).color(p.ink));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
        });
        ui.add_space(16.0);
    }

    /// An empty page's serif line and a muted explanation.
    pub(super) fn empty_state(ui: &mut egui::Ui, headline: &str, text: &str) {
        let p = theme::palette(ui.visuals().dark_mode);
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(headline)
                    .font(theme::serif(24.0))
                    .color(p.ink),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(text).color(p.pencil));
        });
    }

    pub(super) fn projects_page(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let p = theme::palette(dark);
        let mut acts = SidebarActs::default();
        let mut enable = false;
        egui::ScrollArea::vertical()
            .id_salt("projects-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(28.0);
                centered(ui, |ui| {
                    let enabled = self.settings.general.projects.enabled;
                    Self::page_heading(ui, "Projects", |ui| {
                        if enabled {
                            if ui
                                .add(theme::primary_button(
                                    theme::labelled(icon::PLUS, "New project"),
                                    dark,
                                ))
                                .clicked()
                            {
                                acts.new_project = true;
                            }
                            if ui
                                .button(theme::labelled(icon::DOWNLOAD, "Import…"))
                                .clicked()
                            {
                                acts.import_project = true;
                            }
                        }
                    });
                    if !enabled {
                        Self::empty_state(
                            ui,
                            "Projects are off",
                            "A project gives its chats shared instructions, files, a folder, a \
                             model and tool switches.",
                        );
                        ui.vertical_centered(|ui| {
                            ui.add_space(10.0);
                            if ui.button("Turn projects on").clicked() {
                                enable = true;
                            }
                        });
                        return;
                    }
                    if self.projects.is_empty() {
                        Self::empty_state(
                            ui,
                            "No projects yet",
                            "Create one to give a group of chats shared instructions and files.",
                        );
                        return;
                    }
                    let projects: Vec<(Uuid, String, Option<std::path::PathBuf>, String)> = self
                        .projects
                        .iter()
                        .map(|p| {
                            let about = p
                                .instructions
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or_default()
                                .trim()
                                .to_owned();
                            (p.id, p.name.clone(), p.workspace.clone(), about)
                        })
                        .collect();
                    for (pid, name, folder, about) in projects {
                        let mut chats: Vec<usize> = (0..self.conversations.len())
                            .filter(|&i| {
                                self.conversations[i].project == Some(pid)
                                    && listed(&self.conversations[i], &self.runs)
                            })
                            .collect();
                        chats.sort_by_key(|&i| std::cmp::Reverse(self.conversations[i].updated_at));
                        let expanded = !self.collapsed_projects.contains(&pid);
                        egui::Frame::new()
                            .fill(p.raised)
                            .stroke(egui::Stroke::new(1.0, p.hairline))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(icon::FOLDERS).size(18.0).color(p.ink2));
                                    ui.label(RichText::new(&name).size(16.0).color(p.ink));
                                    ui.label(
                                        RichText::new(format!(
                                            "{} chat{}",
                                            chats.len(),
                                            if chats.len() == 1 { "" } else { "s" }
                                        ))
                                        .size(13.0)
                                        .color(p.pencil),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.menu_button(
                                                RichText::new(icon::DOTS).size(16.0),
                                                |ui| {
                                                    if ui
                                                        .button(theme::labelled(
                                                            icon::EDIT,
                                                            "Edit…",
                                                        ))
                                                        .clicked()
                                                    {
                                                        acts.edit_project = Some(pid);
                                                    }
                                                    if ui
                                                        .button(theme::labelled(
                                                            icon::EXPORT,
                                                            "Export…",
                                                        ))
                                                        .clicked()
                                                    {
                                                        acts.export_project = Some(pid);
                                                    }
                                                    ui.separator();
                                                    if ui
                                                        .button(theme::labelled(
                                                            icon::TRASH,
                                                            "Delete",
                                                        ))
                                                        .clicked()
                                                    {
                                                        acts.delete_project = Some(pid);
                                                    }
                                                },
                                            );
                                            if ui
                                                .button(theme::labelled(icon::PLUS, "New chat"))
                                                .clicked()
                                            {
                                                acts.new_in_project = Some(pid);
                                            }
                                        },
                                    );
                                });
                                let mut meta = Vec::new();
                                if let Some(f) = &folder {
                                    meta.push(format!("{}  {}", icon::FOLDER, folder_name(f)));
                                }
                                if !about.is_empty() {
                                    meta.push(about.clone());
                                }
                                if !meta.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(meta.join("   ·   "))
                                                .size(13.0)
                                                .color(p.pencil),
                                        )
                                        .truncate(),
                                    );
                                }
                                if !chats.is_empty() {
                                    ui.add_space(6.0);
                                    let shown = if expanded { chats.len() } else { 3 };
                                    ui.spacing_mut().item_spacing.y = chrome::ROW_GAP;
                                    for &idx in chats.iter().take(shown) {
                                        self.chat_row(ui, idx, &mut acts, p.raised);
                                    }
                                    if chats.len() > 3
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(if expanded {
                                                        "Show fewer".to_owned()
                                                    } else {
                                                        format!("Show all {}", chats.len())
                                                    })
                                                    .size(13.0)
                                                    .color(p.pencil),
                                                )
                                                .frame_when_inactive(false),
                                            )
                                            .clicked()
                                    {
                                        acts.toggle_project = Some(pid);
                                    }
                                }
                            });
                        ui.add_space(10.0);
                    }
                });
                ui.add_space(24.0);
            });
        if acts.select.is_some() || acts.new_in_project.is_some() {
            self.page = Page::Chat;
        }
        self.apply_sidebar_acts(acts);
        if enable {
            self.settings.general.projects.enabled = true;
            self.save_settings();
        }
    }

    pub(super) fn artifacts_page(&mut self, ui: &mut egui::Ui) {
        self.artifact_index.refresh(&self.conversations);
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("artifacts-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(28.0);
                centered(ui, |ui| {
                    Self::page_heading(ui, "Artifacts", |_| {});
                    if self.artifact_index.items.is_empty() {
                        Self::empty_state(
                            ui,
                            "No artifacts yet",
                            "Web pages, drawings, documents and longer code from replies \
                             collect here.",
                        );
                        return;
                    }
                    let width = ui.available_width();
                    let cols =
                        (((width + CARD_GAP) / (CARD_SIZE.x + CARD_GAP)).floor() as usize).max(1);
                    let card_w = (width - CARD_GAP * (cols as f32 - 1.0)) / cols as f32;
                    let size = egui::vec2(card_w, CARD_SIZE.y);
                    let items = &self.artifact_index.items;
                    for (row, chunk) in items.chunks(cols).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = CARD_GAP;
                            for (j, found) in chunk.iter().enumerate() {
                                let i = row * cols + j;
                                if artifact_card(ui, found, size, i).clicked() {
                                    open = Some(i);
                                }
                            }
                        });
                        ui.add_space(CARD_GAP);
                    }
                });
                ui.add_space(24.0);
            });
        if let Some(i) = open {
            self.open_found(i);
        }
    }

    // ------------------------------------------------------------------ composer strip

    /// Under the composer: the project or folder on the left; the model, its effort and the
    /// permission mode on the right. Text buttons that brighten on hover.
    pub(super) fn control_strip(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // No fills or outlines until hovered.
            let w = &mut ui.visuals_mut().widgets.inactive;
            w.bg_stroke = egui::Stroke::NONE;
            w.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;
            ui.spacing_mut().item_spacing.x = 2.0;
            self.context_picker(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.mode_picker(ui);
                self.model_options_menu(ui);
                self.model_picker(ui);
                self.server_status(ui);
            });
        });
    }

    /// "Project or folder ⌄": the chat's working folder (recent ones, choose, clear) and its
    /// project.
    fn context_picker(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        let conv = self
            .current
            .and_then(|id| self.conversations.iter().find(|c| c.id == id));
        let folder = conv.and_then(|c| c.workspace.clone());
        let project = conv
            .and_then(|c| c.project)
            .and_then(|pid| self.projects.iter().find(|p| p.id == pid))
            .map(|p| (p.id, p.name.clone()));
        let label = match (&project, &folder) {
            (Some((_, name)), Some(f)) => format!("{name} / {}", folder_name(f)),
            (Some((_, name)), None) => name.clone(),
            (None, Some(f)) => folder_name(f),
            (None, None) => "Project or folder".to_owned(),
        };
        let glyph = if project.is_some() {
            icon::FOLDERS
        } else {
            icon::FOLDER
        };
        let color = if project.is_some() || folder.is_some() {
            p.ink2
        } else {
            p.pencil
        };
        let text = RichText::new(format!("{glyph}  {label}  {}", icon::CARET_DOWN))
            .size(13.0)
            .color(color);
        let recent = self.recent_folders(5);
        let projects: Vec<(Uuid, String)> = if self.settings.general.projects.enabled {
            self.projects
                .iter()
                .map(|p| (p.id, p.name.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let fresh = self.is_fresh_chat();
        let mut set_folder: Option<Option<std::path::PathBuf>> = None;
        let mut choose = false;
        let mut to_project: Option<Option<Uuid>> = None;
        let hover = match &folder {
            Some(f) => format!(
                "{}\nThe model can read, search, edit and run commands in this folder.",
                f.display()
            ),
            None => "Pick a working folder (the model gets tools to read, search, edit and \
                     run commands in it) or a project for this chat"
                .to_owned(),
        };
        ui.menu_button(text, |ui| {
            ui.set_min_width(260.0);
            ui.label(RichText::new("Working folder").small().color(p.pencil));
            for f in &recent {
                let on = folder.as_ref() == Some(f);
                if ui
                    .selectable_label(on, theme::labelled(icon::FOLDER, &folder_name(f)))
                    .on_hover_text(f.display().to_string())
                    .clicked()
                {
                    set_folder = Some(Some(f.clone()));
                }
            }
            if ui
                .button(theme::labelled(icon::PLUS, "Choose a folder…"))
                .clicked()
            {
                choose = true;
            }
            if folder.is_some()
                && ui
                    .button(theme::labelled(icon::CLOSE, "No folder"))
                    .clicked()
            {
                set_folder = Some(None);
            }
            if !projects.is_empty() {
                ui.separator();
                ui.label(RichText::new("Project").small().color(p.pencil));
                for (pid, name) in &projects {
                    let on = project.as_ref().is_some_and(|(id, _)| id == pid);
                    if ui
                        .selectable_label(on, theme::labelled(icon::FOLDERS, name))
                        .clicked()
                    {
                        to_project = Some(Some(*pid));
                    }
                }
                if project.is_some() && ui.button("No project").clicked() {
                    to_project = Some(None);
                }
            }
        })
        .response
        .on_hover_text(hover);
        if choose {
            let mut dialog = rfd::FileDialog::new().set_title("Choose a working folder");
            if let Some(path) = &folder {
                dialog = dialog.set_directory(path);
            }
            if let Some(f) = dialog.pick_folder() {
                set_folder = Some(Some(f));
            }
        }
        if let Some(f) = set_folder {
            self.set_workspace(f);
        }
        match to_project {
            // A new chat starts in the project, with its settings.
            Some(Some(pid)) if fresh => self.new_project_chat(pid),
            Some(target) => {
                if let Some(id) = self.current {
                    self.move_chat_to(id, target);
                }
            }
            None => {}
        }
    }

    /// The chat's permission mode, as a short word ("Auto"), with the full picker as its menu.
    fn mode_picker(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        let mut permissions = self.permissions();
        let before = permissions.clone();
        let plan_on = self.settings.general.plan.enabled;
        let sandbox_on = self.settings.sandbox.enabled;
        let current = permissions.mode.effective(plan_on);
        let text = RichText::new(format!("{}  {}", mode_short(current), icon::CARET_DOWN))
            .size(13.0)
            .color(p.pencil);
        ui.menu_button(text, |ui| {
            crate::ui::widgets::permissions_list(ui, &mut permissions, sandbox_on, plan_on);
        })
        .response
        .on_hover_text(format!("{}: {}", current.label(), current.description()));
        if permissions != before {
            self.set_permissions(permissions);
        }
    }

    // ------------------------------------------------------------------ voice

    /// The microphone, its device chevron, and the voice-mode waveform (or send / stop),
    /// right-to-left at the end of the composer's toolbar.
    pub(super) fn mic_device_menu(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui.visuals().dark_mode);
        let devices = self
            .mic_devices
            .get_or_insert_with(crate::voice::record::input_devices)
            .clone();
        let mut device = self.settings.general.voice.input.device.clone();
        let mut refresh = false;
        ui.menu_button(
            RichText::new(icon::CARET_DOWN).size(10.0).color(p.pencil),
            |ui| {
                ui.set_min_width(240.0);
                ui.label(RichText::new("Microphone").small().color(p.pencil));
                if ui
                    .selectable_label(device.is_empty(), "System default")
                    .clicked()
                {
                    device.clear();
                }
                for d in &devices {
                    if ui.selectable_label(device == *d, d).clicked() {
                        device = d.clone();
                    }
                }
                ui.separator();
                if ui
                    .button(theme::labelled(
                        icon::REGENERATE,
                        "Look for microphones again",
                    ))
                    .clicked()
                {
                    refresh = true;
                }
                if ui
                    .button(theme::labelled(icon::GEAR, "Voice settings…"))
                    .clicked()
                {
                    self.open_settings(Tab::Voice);
                }
            },
        )
        .response
        .on_hover_text("Input device");
        if refresh {
            self.mic_devices = None;
        }
        if device != self.settings.general.voice.input.device {
            self.settings.general.voice.input.device = device;
            self.save_settings();
        }
    }

    /// Voice mode on or off: speak, the transcript is sent, the reply is read aloud, and it
    /// listens again, until it is switched off.
    pub(super) fn toggle_voice_mode(&mut self) {
        if self.voice_mode {
            self.voice_mode = false;
            self.voice.stop_recording();
            self.voice.stop_speaking();
            return;
        }
        if !self.settings.general.voice.input.enabled {
            self.notice =
                Some("Voice mode needs voice input: turn it on in Settings → Voice".into());
            return;
        }
        self.voice_mode = true;
        if self.voice.recording.is_none() && !self.voice.transcribing {
            self.toggle_recording();
        }
    }

    /// In voice mode, a reply in the open chat ended: read it aloud, or listen again when
    /// reading aloud is off.
    pub(super) fn voice_reply_done(&mut self, id: Uuid) {
        if !self.voice_mode || self.current != Some(id) {
            return;
        }
        let latest = self
            .conversations
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| crate::voice::latest_reply(&c.messages));
        match latest {
            Some(idx) if self.settings.general.voice.output.enabled => {
                self.toggle_speaking(id, idx)
            }
            _ => self.voice_listen_again(),
        }
    }

    /// In voice mode, start the next recording when nothing else is going on.
    pub(super) fn voice_listen_again(&mut self) {
        let busy = self.current.is_some_and(|id| self.busy(id));
        if self.voice_mode
            && !busy
            && self.voice.recording.is_none()
            && !self.voice.transcribing
            && self.voice.speaking.is_none()
        {
            self.toggle_recording();
        }
    }

    // ------------------------------------------------------------------ settings modal

    /// Settings as a modal over the dimmed window: about 1020×800, fading and zooming in from
    /// 98% unless motion is reduced. Returns the actions its contents produced.
    pub(super) fn settings_modal(
        &mut self,
        ctx: &egui::Context,
        sui: &mut SettingsUi,
    ) -> Vec<SettingsAction> {
        let dark = ctx.global_style().visuals.dark_mode;
        let p = theme::palette(dark);
        let views = self.mcp.snapshot();
        let tunnel = self.tunnel_status();
        let access_error = self.access.last_error();
        let chats: Vec<(Uuid, String)> = self
            .conversations
            .iter()
            .map(|c| (c.id, c.title.clone()))
            .collect();
        let memories = self.memory.all();
        let projects: Vec<(Uuid, String)> = self
            .projects
            .iter()
            .map(|p| (p.id, p.name.clone()))
            .collect();
        let opened = *self.settings_opened.get_or_insert_with(Instant::now);
        let t = if self.settings.general.reduce_motion {
            1.0
        } else {
            (opened.elapsed().as_secs_f32() / 0.16).min(1.0)
        };
        if t < 1.0 {
            ctx.request_repaint();
        }
        let ease = egui::emath::easing::cubic_out(t);
        let screen = ctx.content_rect();
        let size = egui::vec2(
            1020.0_f32.min(screen.width() - 32.0),
            800.0_f32.min(screen.height() - 32.0),
        );
        let id = egui::Id::unique("settings-modal");
        let modal = egui::Modal::new(id)
            .backdrop_color(egui::Color32::from_black_alpha((190.0 * ease) as u8))
            .frame(
                egui::Frame::new()
                    .fill(p.shelf)
                    .stroke(egui::Stroke::new(1.0, p.outline))
                    .corner_radius(14),
            );
        let layer = egui::LayerId::new(egui::Order::Foreground, id);
        let scale = 0.98 + 0.02 * ease;
        let centre = screen.center().to_vec2();
        ctx.set_transform_layer(
            layer,
            egui::emath::TSTransform::from_translation(centre)
                * egui::emath::TSTransform::from_scaling(scale)
                * egui::emath::TSTransform::from_translation(-centre),
        );
        let resp = modal.show(ctx, |ui| {
            ui.set_opacity(ease);
            ui.set_min_size(size);
            ui.set_max_size(size);
            sui.show(
                ui,
                &self.settings,
                &views,
                &self.remote_status,
                &tunnel,
                access_error.as_deref(),
                &chats,
                &memories,
                &projects,
            )
        });
        let mut actions = resp.inner;
        // A click outside closes it when nothing is unsaved.
        if resp.backdrop_response.clicked() && sui.draft == self.settings {
            actions.push(SettingsAction::Close);
        }
        actions
    }
}

/// One artifact's card: a thumbnail of its contents on top, the title and the date under it.
fn artifact_card(ui: &mut egui::Ui, found: &Found, size: egui::Vec2, n: usize) -> egui::Response {
    use crate::artifacts::Kind;
    let p = theme::palette(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let a = &found.artifact;
    let label = format!("{} — {}", a.title, found.chat);
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let t = chrome::hover_amount(ui, &resp);
    chrome::card_frame(ui, rect, t);
    let thumb = egui::Rect::from_min_max(
        rect.min + egui::vec2(6.0, 6.0),
        egui::pos2(rect.right() - 6.0, rect.bottom() - 50.0),
    );
    let painter = ui.painter();
    painter.rect_filled(thumb, 8, p.paper);
    let inner = thumb.shrink(8.0);
    match a.kind {
        Kind::Svg => {
            let mut h = DefaultHasher::new();
            a.source.hash(&mut h);
            let uri = format!("bytes://artifact-thumb-{:x}.svg", h.finish());
            egui::Image::from_bytes(uri, a.source.as_bytes().to_vec())
                .fit_to_exact_size(inner.size())
                .paint_at(ui, inner);
        }
        _ => {
            // The first lines of the source, small and muted, as a sketch of the contents.
            let clip = painter.with_clip_rect(inner);
            let text: String = a
                .source
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(8)
                .map(|l| l.chars().take(80).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            clip.text(
                inner.left_top(),
                egui::Align2::LEFT_TOP,
                text,
                egui::FontId::monospace(8.5),
                p.pencil,
            );
        }
    }
    let glyph = match a.kind {
        Kind::Html => icon::GLOBE,
        Kind::Svg | Kind::Mermaid => icon::FLOW,
        Kind::Markdown => icon::FILE,
        _ => icon::CODE,
    };
    let badge = egui::Rect::from_min_size(
        egui::pos2(thumb.right() - 26.0, thumb.top() + 6.0),
        egui::vec2(20.0, 20.0),
    );
    painter.rect_filled(badge, 6, p.raised);
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(12.0),
        p.ink2,
    );
    let text_left = rect.left() + 12.0;
    let text_w = rect.width() - 24.0;
    let mut job = egui::text::LayoutJob::single_section(
        a.title.clone(),
        egui::TextFormat::simple(egui::FontId::proportional(14.0), p.ink),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    ui.painter()
        .galley(egui::pos2(text_left, thumb.bottom() + 8.0), galley, p.ink);
    let date = chat_view::short_date(
        found.when.with_timezone(&chrono::Local),
        chrono::Local::now(),
    );
    let mut job = egui::text::LayoutJob::single_section(
        format!("{date} · {}", found.chat),
        egui::TextFormat::simple(egui::FontId::proportional(12.0), p.pencil),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(
        egui::pos2(text_left, thumb.bottom() + 28.0),
        galley,
        p.pencil,
    );
    let _ = n;
    resp.on_hover_text(format!("{} · {}", a.title, found.chat))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(page: Page, chat: Option<u128>) -> Place {
        Place {
            page,
            chat: chat.map(Uuid::from_u128),
        }
    }

    #[test]
    fn back_and_forward_walk_the_places_seen() {
        let mut h = History::default();
        let (a, b, c) = (
            at(Page::Chat, Some(1)),
            at(Page::Projects, None),
            at(Page::Chat, Some(2)),
        );
        for place in [a, a, b, c] {
            h.observe(place);
        }
        assert!(h.can_go_back() && !h.can_go_forward());
        assert_eq!(h.back(|_| true), Some(b));
        h.observe(b);
        assert_eq!(h.back(|_| true), Some(a));
        assert_eq!(h.back(|_| true), None);
        assert_eq!(h.forward(|_| true), Some(b));
        assert_eq!(h.forward(|_| true), Some(c));
        // A new place after going back drops the forward ones.
        h.back(|_| true);
        h.observe(at(Page::Artifacts, None));
        assert!(!h.can_go_forward());
    }

    #[test]
    fn going_back_skips_deleted_chats() {
        let mut h = History::default();
        for place in [
            at(Page::Chat, Some(1)),
            at(Page::Chat, Some(2)),
            at(Page::Chat, Some(3)),
        ] {
            h.observe(place);
        }
        let alive = |p: &Place| p.chat != Some(Uuid::from_u128(2));
        assert_eq!(h.back(alive), Some(at(Page::Chat, Some(1))));
    }

    #[test]
    fn model_names_are_shortened_for_the_strip() {
        assert_eq!(
            short_model("anthropic/claude-sonnet-5.5"),
            "claude-sonnet-5.5"
        );
        assert_eq!(short_model("llama3"), "llama3");
        assert_eq!(short_model(&"x".repeat(40)).chars().count(), 32);
    }

    #[test]
    fn permission_modes_have_short_names() {
        use crate::approval::ApprovalMode;
        assert_eq!(mode_short(ApprovalMode::AutoWorkspace), "Auto");
        for m in ApprovalMode::ALL {
            assert!(mode_short(m).len() <= 9);
        }
    }
}

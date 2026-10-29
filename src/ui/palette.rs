//! ⌘K command palette: fuzzy-search every app command.

use eframe::egui;
use uuid::Uuid;

use super::settings::Tab;
use super::theme::icon;
use crate::config::Theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    NewChat,
    OpenChat(Uuid),
    ToggleSidebar,
    OpenSettings(Tab),
    UseProvider(usize),
    UseModel(String),
    SetTheme(Theme),
    Attach,
    ExportChatMarkdown,
    ExportChatHtml,
    CompactChat,
    UndoLastEdits,
    ForkChat,
    ExportSettings,
    ExportConversations,
    ImportSettings,
    ImportConversations,
    CheckForUpdates,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub icon: &'static str,
    pub label: String,
    pub category: &'static str,
    pub command: Command,
}

impl Entry {
    pub fn new(
        icon: &'static str,
        label: impl Into<String>,
        category: &'static str,
        command: Command,
    ) -> Self {
        Self {
            icon,
            label: label.into(),
            category,
            command,
        }
    }
}

/// Case-insensitive subsequence match. Higher is better; `None` means no match.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut qi = 0;
    let mut prev_match: Option<usize> = None;
    for (ti, &c) in text.iter().enumerate() {
        if qi < query.len() && c == query[qi] {
            score += 1;
            if prev_match == Some(ti.wrapping_sub(1)) {
                score += 4; // consecutive characters
            }
            if ti == 0 || !text[ti - 1].is_alphanumeric() {
                score += 6; // start of a word
            }
            prev_match = Some(ti);
            qi += 1;
        }
    }
    (qi == query.len()).then(|| score - text.len() as i32 / 8)
}

/// Entries matching `query`, best first.
pub fn filter<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let mut scored: Vec<(i32, usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            score(query, &format!("{} {}", e.category, e.label)).map(|s| (s, i, e))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, e)| e).collect()
}

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    query: String,
    selected: usize,
}

impl Palette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.query.clear();
        self.selected = 0;
    }

    /// Show the palette if open. Returns the command the user picked.
    pub fn show(&mut self, ctx: &egui::Context, entries: &[Entry]) -> Option<Command> {
        if !self.open {
            return None;
        }
        let matches = filter(entries, &self.query);
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if down && self.selected + 1 < matches.len() {
            self.selected += 1;
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        // Follow the selection only when the keyboard moved it, so scrolling the list with
        // the mouse or trackpad isn't pulled back to the selected entry every frame.
        let follow_selection = up || down;
        self.selected = self.selected.min(matches.len().saturating_sub(1));
        let mut picked = enter
            .then(|| matches.get(self.selected).map(|e| e.command.clone()))
            .flatten();

        let modal = egui::Modal::new(egui::Id::unique("command-palette")).show(ctx, |ui| {
            ui.set_width(520.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(format!("{}  Type a command or search chats…", icon::SEARCH))
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Body),
            );
            edit.request_focus();
            if edit.changed() {
                self.selected = 0;
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    if matches.is_empty() {
                        ui.weak("No matching commands");
                    }
                    for (i, e) in matches.iter().enumerate().take(50) {
                        let selected = i == self.selected;
                        let resp = ui.add(
                            egui::Button::selectable(
                                selected,
                                egui::RichText::new(format!("{}   {}", e.icon, e.label)),
                            )
                            .truncate()
                            .right_text(egui::RichText::new(e.category).small().weak())
                            .min_size(egui::vec2(ui.available_width(), 30.0)),
                        );
                        if selected && follow_selection {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() {
                            picked = Some(e.command.clone());
                        }
                    }
                });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("↑↓ to move · Enter to run · Esc to close")
                    .small()
                    .weak(),
            );
        });
        if picked.is_some() || escape || modal.should_close() {
            self.open = false;
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_scoring() {
        assert!(score("nc", "New chat").is_some());
        assert!(score("xyz", "New chat").is_none());
        assert!(
            score("set", "Open settings").unwrap()
                > score("set", "Use preset theme").unwrap_or(-99) - 10
        );
        // Word starts and consecutive runs beat scattered matches.
        assert!(
            score("new", "New chat").unwrap() > score("new", "Rename window").unwrap_or(i32::MIN)
        );
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn filters_and_orders() {
        let entries = vec![
            Entry::new(icon::PLUS, "New chat", "Chat", Command::NewChat),
            Entry::new(
                icon::GEAR,
                "Providers",
                "Settings",
                Command::OpenSettings(Tab::Providers),
            ),
            Entry::new(icon::SUN, "Light", "Theme", Command::SetTheme(Theme::Light)),
        ];
        let got = filter(&entries, "theme light");
        assert_eq!(got[0].command, Command::SetTheme(Theme::Light));
        assert_eq!(filter(&entries, "").len(), 3);
        assert_eq!(filter(&entries, "prov")[0].label, "Providers");
    }
}

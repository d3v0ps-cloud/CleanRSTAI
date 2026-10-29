//! The Commit dialog: the chat's workspace changes with checkboxes, a message and a Commit
//! button. The app runs the git calls (`ChatApp::commit_dialog_window`); this file draws the
//! window and reports what the user chose.

use std::path::PathBuf;

use eframe::egui;
use egui::RichText;
use uuid::Uuid;

use crate::tools::commit::{Change, selected_paths};
use crate::ui::theme;

/// A commit dialog open on a chat.
pub struct CommitDialog {
    pub conv: Uuid,
    /// The chat's working folder, which the commit is made in.
    pub root: PathBuf,
    pub message: String,
    /// The changes and whether each is checked, once listed; the listing's error otherwise.
    pub files: Option<Result<Vec<(Change, bool)>, String>>,
    /// Listing or committing is running.
    pub busy: bool,
    /// Why the last commit failed.
    pub error: Option<String>,
}

impl CommitDialog {
    /// A dialog that is waiting for the working folder's changes to be listed.
    pub fn new(conv: Uuid, root: PathBuf) -> Self {
        Self {
            conv,
            root,
            message: String::new(),
            files: None,
            busy: true,
            error: None,
        }
    }

    /// Applies a listing from the app: every change starts checked.
    pub fn set_listing(&mut self, result: Result<Vec<Change>, String>) {
        self.busy = false;
        self.files = Some(result.map(|changes| changes.into_iter().map(|c| (c, true)).collect()));
    }
}

/// What the user did in the dialog this frame.
pub enum CommitAction {
    /// Nothing to do: the dialog stays open.
    Keep,
    /// Close the dialog.
    Close,
    /// Commit these paths (see `selected_paths`) with this message.
    Commit { message: String, paths: Vec<String> },
}

/// Draws the dialog. `idle` is false while a reply runs in the chat, which blocks a commit.
pub fn show(ctx: &egui::Context, dialog: &mut CommitDialog, idle: bool) -> CommitAction {
    let mut open = true;
    let mut close = false;
    let mut commit = false;
    let checked = matches!(&dialog.files, Some(Ok(files)) if files.iter().any(|(_, on)| *on));
    let can_commit = idle && !dialog.busy && checked && !dialog.message.trim().is_empty();
    egui::Window::new("Commit changes")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(format!(
                    "Records the checked files in {} as one local commit. Nothing is pushed, \
                     and git hooks don't run.",
                    dialog.root.display()
                ))
                .weak(),
            );
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::multiline(&mut dialog.message)
                    .hint_text("Commit message")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            match &mut dialog.files {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the changes…");
                    });
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                }
                Some(Ok(files)) if files.is_empty() => {
                    ui.label(RichText::new("No changes to commit.").weak());
                }
                Some(Ok(files)) => {
                    ui.label(format!("{} changed files", files.len()));
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for (change, on) in files.iter_mut() {
                                ui.checkbox(on, format!("{}  {}", change.status, change.label()));
                            }
                        });
                }
            }
            if let Some(e) = &dialog.error {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, e.as_str());
            }
            if dialog.busy && dialog.files.is_some() {
                ui.spinner();
            }
            if !idle {
                ui.label(
                    RichText::new("A reply is running in this chat; commit when it ends.").weak(),
                );
            }
            ui.add_space(8.0);
            let dark = ui.visuals().dark_mode;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_commit, theme::primary_button("Commit", dark))
                    .clicked()
                {
                    commit = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if commit {
        let paths = match &dialog.files {
            Some(Ok(files)) => selected_paths(files),
            _ => Vec::new(),
        };
        return CommitAction::Commit {
            message: dialog.message.trim().to_owned(),
            paths,
        };
    }
    if close || !open {
        CommitAction::Close
    } else {
        CommitAction::Keep
    }
}

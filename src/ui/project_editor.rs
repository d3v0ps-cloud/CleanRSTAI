//! The project editor window (name, instructions, files, folder, provider, model and
//! permission mode) and the delete confirmation. `ChatApp` keeps the draft and saves it.

use eframe::egui;

use crate::approval::ApprovalMode;
use crate::projects::Project;
use crate::ui::settings::FIELD_WIDTH;

/// A project being created or edited, until it is saved or cancelled.
pub struct ProjectEditor {
    /// The project is new: it isn't in the list yet.
    pub is_new: bool,
    pub draft: Project,
}

/// What the user did in the editor this frame.
pub enum EditorOutcome {
    Open,
    Save(Box<Project>),
    Cancel,
}

/// Shows the editor. `providers` are (id, name) pairs for the provider picker.
pub fn show(
    ctx: &egui::Context,
    editor: &mut ProjectEditor,
    providers: &[(String, String)],
) -> EditorOutcome {
    let mut open = true;
    let mut outcome = EditorOutcome::Open;
    let title = if editor.is_new {
        "New project"
    } else {
        "Edit project"
    };
    let draft = &mut editor.draft;
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .default_width(520.0)
        .resizable(true)
        .show(ctx, |ui| {
            egui::Grid::new("project-editor-grid")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.name)
                            .hint_text("Project name")
                            .desired_width(FIELD_WIDTH),
                    );
                    ui.end_row();

                    ui.label("Instructions");
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.instructions)
                            .hint_text("Added to the system prompt of every chat in the project")
                            .desired_rows(6)
                            .desired_width(FIELD_WIDTH),
                    );
                    ui.end_row();

                    ui.label("Files");
                    ui.vertical(|ui| {
                        let mut remove = None;
                        for (i, path) in draft.files.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.display().to_string());
                                ui.label(name).on_hover_text(path.display().to_string());
                                if ui.small_button("Remove").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove {
                            draft.files.remove(i);
                        }
                        if ui.button("Add files…").clicked()
                            && let Some(paths) = rfd::FileDialog::new()
                                .set_title("Add project files")
                                .pick_files()
                        {
                            for p in paths {
                                if !draft.files.contains(&p) {
                                    draft.files.push(p);
                                }
                            }
                        }
                        ui.weak(
                            "Their text is sent with every request in the project. Images are \
                             left out.",
                        );
                    });
                    ui.end_row();

                    ui.label("Folder");
                    ui.horizontal(|ui| {
                        let shown = draft
                            .workspace
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "None".into());
                        ui.label(shown);
                        if ui.small_button("Choose…").clicked()
                            && let Some(dir) = rfd::FileDialog::new()
                                .set_title("Choose a working folder")
                                .pick_folder()
                        {
                            draft.workspace = Some(dir);
                        }
                        if draft.workspace.is_some() && ui.small_button("Clear").clicked() {
                            draft.workspace = None;
                        }
                    });
                    ui.end_row();

                    ui.label("Provider");
                    let selected = match &draft.provider {
                        None => "Settings' active provider".to_owned(),
                        Some(id) => providers
                            .iter()
                            .find(|(pid, _)| pid == id)
                            .map_or("A provider that no longer exists".into(), |(_, n)| {
                                n.clone()
                            }),
                    };
                    egui::ComboBox::from_id_salt("project-provider")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut draft.provider,
                                None,
                                "Settings' active provider",
                            );
                            for (id, name) in providers {
                                ui.selectable_value(&mut draft.provider, Some(id.clone()), name);
                            }
                        });
                    ui.end_row();

                    ui.label("Model");
                    let mut model = draft.model.clone().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut model)
                                .hint_text("The provider's model")
                                .desired_width(FIELD_WIDTH),
                        )
                        .changed()
                    {
                        draft.model = Some(model).filter(|m| !m.trim().is_empty());
                    }
                    ui.end_row();

                    ui.label("Permissions");
                    let selected = draft
                        .approval_mode
                        .map_or("Settings' default", ApprovalMode::label);
                    egui::ComboBox::from_id_salt("project-mode")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut draft.approval_mode,
                                None,
                                "Settings' default",
                            );
                            for mode in ApprovalMode::ALL {
                                ui.selectable_value(
                                    &mut draft.approval_mode,
                                    Some(mode),
                                    mode.label(),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Tool switches");
                    ui.horizontal(|ui| {
                        let n = draft.tool_overrides.0.len();
                        ui.label(match n {
                            0 => "Settings' defaults".to_owned(),
                            1 => "1 switch changed".to_owned(),
                            n => format!("{n} switches changed"),
                        });
                        if n > 0 && ui.small_button("Reset").clicked() {
                            draft.tool_overrides = Default::default();
                        }
                    });
                    ui.end_row();
                });
            ui.weak(
                "New chats in the project start with these settings. Each chat can change its \
                 own afterwards.",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_save = !draft.name.trim().is_empty();
                if ui
                    .add_enabled(can_save, egui::Button::new("Save"))
                    .on_disabled_hover_text("A project needs a name")
                    .clicked()
                {
                    let mut saved = draft.clone();
                    saved.name = saved.name.trim().to_owned();
                    outcome = EditorOutcome::Save(Box::new(saved));
                }
                if ui.button("Cancel").clicked() {
                    outcome = EditorOutcome::Cancel;
                }
            });
        });
    if !open && matches!(outcome, EditorOutcome::Open) {
        outcome = EditorOutcome::Cancel;
    }
    outcome
}

/// The delete confirmation for the project called `name`: `Some(true)` to delete, `Some(false)`
/// to keep it, `None` while it is still open.
pub fn confirm_delete(ctx: &egui::Context, name: &str) -> Option<bool> {
    let mut answer = None;
    let mut open = true;
    egui::Window::new("Delete project?")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!(
                "Delete \"{name}\"? Its chats are kept and leave the project. Its instructions \
                 and file list are gone."
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Delete project").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
    if !open && answer.is_none() {
        answer = Some(false);
    }
    answer
}

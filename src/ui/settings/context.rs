//! The Context & files page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn context_tab(
        &mut self,
        ui: &mut egui::Ui,
        memories: &[Memory],
        projects: &[(Uuid, String)],
        actions: &mut Vec<SettingsAction>,
    ) {
        let providers: Vec<(String, String, Vec<String>)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.models.clone()))
            .collect();
        let g = &mut self.draft.general;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    "These are the defaults for new chats; switch each one for one chat under \
                     Chat features in the tools menu next to the message box.",
                );
                ui.add_space(10.0);

                // ---- compaction
                let c = &mut g.compaction;
                ui.checkbox(
                    &mut c.enabled,
                    egui::RichText::new(format!(
                        "{}  Compact long chats automatically",
                        icon::STACK
                    ))
                    .strong(),
                );
                ui.indent("compaction-help", |ui| {
                    ui.weak(
                        "When the next request would fill most of the model's context, the older \
                         messages are summarised by a model and only the summary and the latest \
                         messages are sent from then on. The chat keeps every message, with a \
                         marker where the summary starts. \"Compact now\" in the tools menu and \
                         the command palette works even when this is off.",
                    );
                });
                ui.add_space(6.0);
                egui::Grid::new("compaction-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Compact at");
                        ui.add(
                            egui::Slider::new(&mut c.threshold_percent, 50..=95)
                                .suffix("% of the context"),
                        );
                        ui.end_row();

                        ui.label("Keep word for word");
                        ui.add(
                            egui::DragValue::new(&mut c.keep_recent)
                                .range(2..=50)
                                .suffix(" latest messages"),
                        )
                        .on_hover_text(
                            "Tool results stay with the call that asked for them, so a few more \
                             may be kept",
                        );
                        ui.end_row();

                        ui.label("Context size");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut c.context_tokens)
                                    .range(0..=10_000_000)
                                    .speed(256.0)
                                    .suffix(" tokens"),
                            );
                            ui.weak(if c.context_tokens == 0 {
                                "0: only when the server reports its size"
                            } else {
                                "used when the server doesn't report one"
                            });
                        })
                        .response
                        .on_hover_text(
                            "llama.cpp, LiteLLM, Ollama, LM Studio, vLLM and OpenRouter report \
                             the context size; for other servers set the model's size here",
                        );
                        ui.end_row();

                        ui.label("Summary by");
                        let current = providers
                            .iter()
                            .find(|(id, ..)| *id == c.provider_id)
                            .map_or("The chat's provider".to_owned(), |(_, n, _)| n.clone());
                        egui::ComboBox::from_id_salt("compaction-provider")
                            .selected_text(current)
                            .width(FIELD_WIDTH)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut c.provider_id,
                                    String::new(),
                                    "The chat's provider",
                                );
                                for (id, name, _) in &providers {
                                    ui.selectable_value(&mut c.provider_id, id.clone(), name);
                                }
                            });
                        ui.end_row();

                        ui.label("Summary model");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut c.model)
                                    .hint_text("the provider's selected model")
                                    .desired_width(FIELD_WIDTH - 40.0),
                            );
                            let models = providers
                                .iter()
                                .find(|(id, ..)| *id == c.provider_id)
                                .map(|(.., m)| m.clone())
                                .unwrap_or_default();
                            if !models.is_empty() {
                                ui.menu_button(icon::CARET_DOWN, |ui| {
                                    ui.set_max_height(320.0);
                                    egui::ScrollArea::vertical().show(ui, |ui| {
                                        for m in models {
                                            if ui.button(&m).clicked() {
                                                c.model = m;
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                            }
                        });
                        ui.end_row();

                        ui.label("Older tool results");
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut c.trim_tool_results,
                                "Shorten long results of older tool calls in what is sent",
                            )
                            .on_hover_text(
                                "A cheaper step before a summary: results older than the kept \
                                 messages are cut to the length below. The chat keeps them whole.",
                            );
                            ui.add_enabled(
                                c.trim_tool_results,
                                egui::DragValue::new(&mut c.trim_chars)
                                    .range(200..=100_000)
                                    .speed(50.0)
                                    .suffix(" characters"),
                            );
                        });
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("Summary instructions");
                    if c.prompt != crate::config::DEFAULT_COMPACTION_PROMPT
                        && ui.small_button("Reset").clicked()
                    {
                        c.prompt = crate::config::DEFAULT_COMPACTION_PROMPT.into();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut c.prompt)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);

                // ---- project instructions
                let ins = &mut g.instructions;
                ui.checkbox(
                    &mut ins.enabled,
                    egui::RichText::new(format!(
                        "{}  Read project instructions from the workspace",
                        icon::FILE
                    ))
                    .strong(),
                );
                ui.indent("instructions-help", |ui| {
                    ui.weak(
                        "When a chat has a working folder, files such as AGENTS.md (used by Codex \
                         and others) or CLAUDE.md in it are added to the system prompt, so the \
                         model follows the project's own conventions. The tools menu shows which \
                         files the latest request read.",
                    );
                });
                ui.add_space(6.0);
                egui::Grid::new("instructions-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("File names");
                        let mut names = ins.file_names.join(", ");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut names)
                                    .hint_text("AGENTS.md, CLAUDE.md")
                                    .desired_width(FIELD_WIDTH),
                            )
                            .on_hover_text("Looked for in this order, separated by commas")
                            .changed()
                        {
                            // Empty names stay while typing (so a comma can be typed);
                            // saving drops them.
                            ins.file_names =
                                names.split(',').map(|n| n.trim().to_owned()).collect();
                        }
                        ui.end_row();

                        ui.label("Which files");
                        ui.vertical(|ui| {
                            ui.radio_value(&mut ins.all_files, false, "The first one found");
                            ui.radio_value(&mut ins.all_files, true, "Every one found, in order");
                        });
                        ui.end_row();

                        ui.label("Parent folders");
                        ui.checkbox(
                            &mut ins.parent_folders,
                            "Also read them from the folders above the workspace",
                        )
                        .on_hover_text(
                            "Outermost first, so the workspace's own file reads as the most \
                             specific",
                        );
                        ui.end_row();

                        ui.label("Size limit");
                        ui.add(
                            egui::DragValue::new(&mut ins.max_kb)
                                .range(1..=1024)
                                .suffix(" KB"),
                        )
                        .on_hover_text("Text beyond this, in all files together, is left out");
                        ui.end_row();
                    });
                ui.add_space(16.0);

                // ---- projects
                let pr = &mut g.projects;
                ui.checkbox(
                    &mut pr.enabled,
                    egui::RichText::new(format!("{}  Projects", icon::FOLDER)).strong(),
                );
                ui.indent("projects-help", |ui| {
                    ui.weak(
                        "Group chats into projects in the sidebar. A project's instructions and \
                         files are added to the system prompt of each of its chats, and its \
                         folder, provider, model, tool switches and permission mode are what a \
                         new chat in it starts with. Off hides projects and stops adding their \
                         context. Each chat can switch the project context off too.",
                    );
                });
                ui.add_space(6.0);
                egui::Grid::new("projects-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Files size limit");
                        ui.add(
                            egui::DragValue::new(&mut pr.max_files_kb)
                                .range(
                                    crate::projects::MIN_FILES_KB..=crate::projects::MAX_FILES_KB,
                                )
                                .suffix(" KB"),
                        )
                        .on_hover_text(
                            "The text of a project's files sent with each request, in all. \
                             Longer files are cut.",
                        );
                        ui.end_row();

                        ui.label("New projects: folder");
                        ui.horizontal(|ui| {
                            let shown = pr
                                .default_workspace
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "None".into());
                            ui.label(shown);
                            if ui.small_button("Choose…").clicked()
                                && let Some(dir) = rfd::FileDialog::new()
                                    .set_title("Choose a default working folder")
                                    .pick_folder()
                            {
                                pr.default_workspace = Some(dir);
                            }
                            if pr.default_workspace.is_some() && ui.small_button("Clear").clicked()
                            {
                                pr.default_workspace = None;
                            }
                        });
                        ui.end_row();

                        ui.label("New projects: provider");
                        let selected = match &pr.default_provider {
                            None => "Settings' active provider".to_owned(),
                            Some(id) => providers.iter().find(|(pid, ..)| pid == id).map_or(
                                "A provider that no longer exists".into(),
                                |(_, name, _)| name.clone(),
                            ),
                        };
                        egui::ComboBox::from_id_salt("projects-default-provider")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut pr.default_provider,
                                    None,
                                    "Settings' active provider",
                                );
                                for (id, name, _) in &providers {
                                    ui.selectable_value(
                                        &mut pr.default_provider,
                                        Some(id.clone()),
                                        name,
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("New projects: instructions");
                    if !pr.default_instructions.is_empty() && ui.small_button("Clear").clicked() {
                        pr.default_instructions.clear();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut pr.default_instructions)
                        .hint_text("Nothing: a new project starts empty")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);

                // ---- memory
                memory_section(
                    ui,
                    &mut g.memory,
                    &mut self.memory_ui,
                    memories,
                    projects,
                    actions,
                );
                ui.add_space(16.0);

                // ---- file edits
                let fe = &mut g.file_edits;
                ui.label(
                    egui::RichText::new(format!("{}  Workspace file edits", icon::UNDO)).strong(),
                );
                ui.add_space(4.0);
                ui.checkbox(
                    &mut fe.diff_preview,
                    "Show the change as a diff on the approval prompt and in the tool card",
                );
                ui.checkbox(
                    &mut fe.checkpoints,
                    "Keep checkpoints so a turn's write_file and edit_file changes can be undone",
                )
                .on_hover_text(
                    "Each edit keeps the file's previous content in the chat. \"Undo file edits\" \
                     under the turn's reply puts the files back, newest first; a file that \
                     changed again since is left alone. Shell commands aren't undone.",
                );
                ui.add_enabled_ui(fe.checkpoints, |ui| {
                    egui::Grid::new("file-edits-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Largest file");
                            ui.add(
                                egui::DragValue::new(&mut fe.max_file_kb)
                                    .range(16..=65_536)
                                    .speed(16.0)
                                    .suffix(" KB"),
                            )
                            .on_hover_text("Bigger files are changed without a checkpoint");
                            ui.end_row();

                            ui.label("Keep for");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::DragValue::new(&mut fe.keep_turns)
                                        .range(0..=1000)
                                        .suffix(" turns"),
                                );
                                ui.weak("0 keeps them all");
                            });
                            ui.end_row();
                        });
                });
                ui.add_space(16.0);

                // ---- code review
                let r = &mut g.review;
                ui.checkbox(
                    &mut r.enabled,
                    egui::RichText::new(format!("{}  Code review", icon::CODE)).strong(),
                )
                .on_hover_text("Adds Review changes to the tools menu and the command palette");
                ui.indent("review-help", |ui| {
                    ui.weak(
                        "Review changes sends the working folder's git diff to a model with the \
                         instructions below, as a new message in the chat. The diff leaves your \
                         computer for the provider chosen here.",
                    );
                });
                ui.add_enabled_ui(r.enabled, |ui| {
                    ui.add_space(6.0);
                    ui.label("Changes to review");
                    ui.radio_value(
                        &mut r.scope,
                        ReviewScope::Uncommitted,
                        "Uncommitted changes (staged, unstaged and new files)",
                    );
                    ui.radio_value(
                        &mut r.scope,
                        ReviewScope::Branch,
                        "This branch against its base",
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("How strict");
                        ui.radio_value(&mut r.strictness, ReviewStrictness::Lenient, "Lenient")
                            .on_hover_text("Only bugs, data loss and security issues");
                        ui.radio_value(&mut r.strictness, ReviewStrictness::Normal, "Normal")
                            .on_hover_text("The review instructions below, as written");
                        ui.radio_value(&mut r.strictness, ReviewStrictness::Strict, "Strict")
                            .on_hover_text(
                                "Also style, naming, readability, missing tests and \
                                 documentation, and risky assumptions",
                            );
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("review-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Base branch");
                            ui.add(
                                egui::TextEdit::singleline(&mut r.base_branch)
                                    .hint_text("auto: origin/HEAD, main or master")
                                    .desired_width(FIELD_WIDTH),
                            )
                            .on_hover_text("Used by \"This branch against its base\"");
                            ui.end_row();

                            ui.label("Review by");
                            let current = providers
                                .iter()
                                .find(|(id, ..)| *id == r.provider_id)
                                .map_or("The chat's provider".to_owned(), |(_, n, _)| n.clone());
                            egui::ComboBox::from_id_salt("review-provider")
                                .selected_text(current)
                                .width(FIELD_WIDTH)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut r.provider_id,
                                        String::new(),
                                        "The chat's provider",
                                    );
                                    for (id, name, _) in &providers {
                                        ui.selectable_value(&mut r.provider_id, id.clone(), name);
                                    }
                                });
                            ui.end_row();

                            ui.label("Review model");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut r.model)
                                        .hint_text("the provider's selected model")
                                        .desired_width(FIELD_WIDTH - 40.0),
                                );
                                let models = providers
                                    .iter()
                                    .find(|(id, ..)| *id == r.provider_id)
                                    .map(|(.., m)| m.clone())
                                    .unwrap_or_default();
                                if !models.is_empty() {
                                    ui.menu_button(icon::CARET_DOWN, |ui| {
                                        ui.set_max_height(320.0);
                                        egui::ScrollArea::vertical().show(ui, |ui| {
                                            for m in models {
                                                if ui.button(&m).clicked() {
                                                    r.model = m;
                                                    ui.close();
                                                }
                                            }
                                        });
                                    });
                                }
                            });
                            ui.end_row();

                            ui.label("Largest diff");
                            ui.add(
                                egui::DragValue::new(&mut r.max_diff_kb)
                                    .range(16..=4096)
                                    .suffix(" KB"),
                            )
                            .on_hover_text(
                                "A longer diff is cut to this size, and the model is told so",
                            );
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.strong("Review instructions");
                        if r.prompt != crate::config::DEFAULT_REVIEW_PROMPT
                            && ui.small_button("Reset").clicked()
                        {
                            r.prompt = crate::config::DEFAULT_REVIEW_PROMPT.into();
                        }
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut r.prompt)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    );
                });

                ui.add_space(16.0);

                // ---- OCR for scanned PDFs
                let o = &mut g.ocr;
                ui.checkbox(
                    &mut o.enabled,
                    egui::RichText::new(format!("{}  Read scanned PDFs (OCR)", icon::FILE))
                        .strong(),
                );
                ui.indent("ocr-help", |ui| {
                    ui.weak(
                        "A PDF with no text layer (a scan or a photo of pages) has its page \
                         images read. A vision model reads each page, and the page image is sent \
                         to the provider chosen here; Tesseract runs on this computer. Pages past \
                         the limit are not read. Without OCR, such a PDF is attached with a note.",
                    );
                });
                ui.add_enabled_ui(o.enabled, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Read with");
                        ui.radio_value(&mut o.engine, OcrEngine::VisionModel, "A vision model");
                        ui.radio_value(&mut o.engine, OcrEngine::Tesseract, "Tesseract");
                    });
                    ui.add_space(6.0);
                    egui::Grid::new("ocr-grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .show(ui, |ui| {
                            if o.engine == OcrEngine::VisionModel {
                                ui.label("Vision model by");
                                let current = o
                                    .provider
                                    .as_ref()
                                    .and_then(|id| providers.iter().find(|(pid, ..)| pid == id))
                                    .map_or("The chat's provider".to_owned(), |(_, n, _)| {
                                        n.clone()
                                    });
                                egui::ComboBox::from_id_salt("ocr-provider")
                                    .selected_text(current)
                                    .width(FIELD_WIDTH)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut o.provider,
                                            None,
                                            "The chat's provider",
                                        );
                                        for (id, name, _) in &providers {
                                            ui.selectable_value(
                                                &mut o.provider,
                                                Some(id.clone()),
                                                name,
                                            );
                                        }
                                    });
                                ui.end_row();
                            } else {
                                ui.label("Tesseract program");
                                ui.add(
                                    egui::TextEdit::singleline(&mut o.tesseract_path)
                                        .hint_text("tesseract")
                                        .desired_width(FIELD_WIDTH),
                                )
                                .on_hover_text("A name on PATH, or the full path of the program");
                                ui.end_row();

                                ui.label("Languages");
                                ui.add(
                                    egui::TextEdit::singleline(&mut o.languages)
                                        .hint_text("eng")
                                        .desired_width(FIELD_WIDTH),
                                )
                                .on_hover_text(
                                    "Tesseract language codes joined with +, e.g. eng+deu; \
                                     each must be installed with Tesseract",
                                );
                                ui.end_row();
                            }

                            ui.label("Pages per PDF");
                            ui.add(
                                egui::DragValue::new(&mut o.max_pages)
                                    .range(1..=200)
                                    .suffix(" pages"),
                            )
                            .on_hover_text(
                                "Pages past this are not read, and the model is told so",
                            );
                            ui.end_row();
                        });
                    if o.engine == OcrEngine::VisionModel {
                        ui.add_space(4.0);
                        weak_note(
                            ui,
                            "The model must accept images. Pages are sent one at a time, so a \
                             long scan takes a while.",
                        );
                    }
                });
            });
    }
}

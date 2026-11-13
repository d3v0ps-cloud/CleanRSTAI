//! The Context & memory, Workspace and Artifacts & files pages of Settings.

use super::*;

/// Each provider's id, name and models, for the "… by" and model rows.
type Providers = Vec<(String, String, Vec<String>)>;

impl SettingsUi {
    fn provider_list(&self) -> Providers {
        self.draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.models.clone()))
            .collect()
    }

    /// Compaction, memory, projects and branches: what a chat carries along.
    pub(super) fn context_tab(
        &mut self,
        ui: &mut egui::Ui,
        memories: &[Memory],
        projects: &[(Uuid, String)],
        actions: &mut Vec<SettingsAction>,
    ) {
        let providers = self.provider_list();
        let g = &mut self.draft.general;
        page(ui, |ui| {
            note(
                ui,
                "These are the defaults for new chats; switch each one for one chat under Chat \
                 options in the menu next to the message box.",
            );

            section(ui, "Compaction");
            let c = &mut g.compaction;
            switch(
                ui,
                "Compact long chats automatically",
                "When the next request would fill most of the model's context, the older \
                 messages are summarised and only the summary and the latest messages are sent \
                 from then on. The chat keeps every message. \"Compact now\" works even when \
                 this is off",
                &mut c.enabled,
            );
            setting_row(ui, "Compact at", "How full the context is", |ui| {
                ui.add(egui::Slider::new(&mut c.threshold_percent, 50..=95).suffix("%"));
            });
            setting_row(
                ui,
                "Keep word for word",
                "Tool results stay with the call that asked for them, so a few more may be kept",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut c.keep_recent)
                            .range(2..=50)
                            .suffix(" latest messages"),
                    );
                },
            );
            setting_row(
                ui,
                "Context size",
                "Used when the server doesn't report one (llama.cpp, LiteLLM, Ollama, LM Studio, \
                 vLLM and OpenRouter do). 0: compact only when the server reports its size",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut c.context_tokens)
                            .range(0..=10_000_000)
                            .speed(256.0)
                            .suffix(" tokens"),
                    );
                },
            );
            provider_row(
                ui,
                "compaction-provider",
                "Summary by",
                "",
                &mut c.provider_id,
                &providers,
            );
            let models = models_of(&providers, &c.provider_id);
            model_row(ui, "Summary model", &mut c.model, &models);
            switch(
                ui,
                "Shorten older tool results",
                "A cheaper step before a summary: results older than the kept messages are cut \
                 to the length below in what is sent. The chat keeps them whole",
                &mut c.trim_tool_results,
            );
            ui.add_enabled_ui(c.trim_tool_results, |ui| {
                setting_row(ui, "Shorten to", "", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut c.trim_chars)
                            .range(200..=100_000)
                            .speed(50.0)
                            .suffix(" characters"),
                    );
                });
            });
            long_text(
                ui,
                "Summary instructions",
                "",
                &mut c.prompt,
                4,
                Some(crate::config::DEFAULT_COMPACTION_PROMPT),
            );

            section(ui, "Memory");
            ui.add_space(4.0);
            memory_section(
                ui,
                &mut g.memory,
                &mut self.memory_ui,
                memories,
                projects,
                actions,
            );

            section(ui, "Projects");
            let pr = &mut g.projects;
            switch(
                ui,
                "Projects",
                "Group chats into projects in the sidebar. A project's instructions and files \
                 are added to the system prompt of each of its chats, and its folder, provider, \
                 model, tool switches and permission mode are what a new chat in it starts \
                 with. Off hides projects and stops adding their context",
                &mut pr.enabled,
            );
            setting_row(
                ui,
                "Files size limit",
                "The text of a project's files sent with each request, in all. Longer files \
                 are cut",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut pr.max_files_kb)
                            .range(crate::projects::MIN_FILES_KB..=crate::projects::MAX_FILES_KB)
                            .suffix(" KB"),
                    );
                },
            );
            let shown = pr
                .default_workspace
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "None".into());
            setting_row(ui, "New projects: folder", &shown, |ui| {
                if pr.default_workspace.is_some() && ui.button("Clear").clicked() {
                    pr.default_workspace = None;
                }
                if ui.button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title("Choose a default working folder")
                        .pick_folder()
                {
                    pr.default_workspace = Some(dir);
                }
            });
            setting_row(ui, "New projects: provider", "", |ui| {
                let selected = match &pr.default_provider {
                    None => "Settings' active provider".to_owned(),
                    Some(id) => providers
                        .iter()
                        .find(|(pid, ..)| pid == id)
                        .map_or("A provider that no longer exists".into(), |(_, name, _)| {
                            name.clone()
                        }),
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
                            ui.selectable_value(&mut pr.default_provider, Some(id.clone()), name);
                        }
                    });
            });
            long_text(
                ui,
                "New projects: instructions",
                "What a new project's instructions start as. Empty: a new project starts empty.",
                &mut pr.default_instructions,
                3,
                None,
            );

            section(ui, "Branches");
            switch(
                ui,
                "Keep the version a reply replaces",
                "Regenerating a reply, or editing a message, keeps the version it replaces as \
                 a branch, with ‹ k/N › arrows to switch between them. Off, the replaced version \
                 is dropped; versions already kept stay switchable",
                &mut g.branching.enabled,
            );
            ui.add_enabled_ui(g.branching.enabled, |ui| {
                setting_row(
                    ui,
                    "Versions kept per message",
                    "Past this many, the oldest version of a message is dropped. Files changed \
                     by a version are only undone while it is the one shown",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut g.branching.max_per_message).range(1..=50),
                        );
                    },
                );
            });
        });
    }

    /// What the model may do in a chat's working folder, and how its changes are handled.
    pub(super) fn workspace_tab(&mut self, ui: &mut egui::Ui) {
        let providers = self.provider_list();
        let g = &mut self.draft.general;
        page(ui, |ui| {
            let b = &mut g.builtin_tools;
            switch(
                ui,
                "Workspace tools",
                "When a chat has a working folder (set below the message box): read_file, \
                 file_glob_search, grep_search, get_info, git_status and git_diff run freely \
                 inside it; write_file, edit_file and exec_shell_command ask for approval. \
                 Paths can't leave the folder",
                &mut b.workspace,
            );
            ui.add_enabled_ui(b.workspace, |ui| {
                switch(
                    ui,
                    "Commit (git_commit)",
                    "Lets the model commit the workspace's changes as one local commit. Every \
                     commit asks for approval, even in Auto in workspace mode. Nothing is \
                     pushed, and git hooks don't run",
                    &mut b.git_commit,
                );
                ui.add_enabled_ui(b.git_commit, |ui| {
                    switch(
                        ui,
                        "Allow signed commits",
                        "Off: commits are never signed. On: a repository set to sign its \
                         commits signs them, and gpg may ask for a passphrase",
                        &mut b.allow_signing,
                    );
                });
            });

            section(ui, "Project instructions");
            let ins = &mut g.instructions;
            switch(
                ui,
                "Read project instructions from the workspace",
                "Files such as AGENTS.md or CLAUDE.md in the working folder are added to the \
                 system prompt, so the model follows the project's own conventions. The chat's \
                 menu shows which files the latest request read",
                &mut ins.enabled,
            );
            setting_row(
                ui,
                "File names",
                "Looked for in this order, separated by commas",
                |ui| {
                    let mut names = ins.file_names.join(", ");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut names)
                                .hint_text("AGENTS.md, CLAUDE.md")
                                .desired_width(250.0),
                        )
                        .changed()
                    {
                        // Empty names stay while typing (so a comma can be typed); saving
                        // drops them.
                        ins.file_names = names.split(',').map(|n| n.trim().to_owned()).collect();
                    }
                },
            );
            choice(
                ui,
                "instructions-which",
                "Which files",
                "",
                &mut ins.all_files,
                &[
                    (false, "The first one found"),
                    (true, "Every one found, in order"),
                ],
            );
            switch(
                ui,
                "Also read parent folders",
                "From the folders above the workspace, outermost first, so the workspace's own \
                 file reads as the most specific",
                &mut ins.parent_folders,
            );
            setting_row(
                ui,
                "Size limit",
                "Text beyond this, in all files together, is left out",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut ins.max_kb)
                            .range(1..=1024)
                            .suffix(" KB"),
                    );
                },
            );

            section(ui, "File edits");
            let fe = &mut g.file_edits;
            switch(
                ui,
                "Show changes as a diff",
                "On the approval prompt and in the tool card",
                &mut fe.diff_preview,
            );
            switch(
                ui,
                "Keep checkpoints for undo",
                "Each write_file and edit_file keeps the file's previous content in the chat. \
                 \"Undo file edits\" under the turn's reply puts the files back, newest first; \
                 a file that changed again since is left alone. Shell commands aren't undone",
                &mut fe.checkpoints,
            );
            ui.add_enabled_ui(fe.checkpoints, |ui| {
                setting_row(
                    ui,
                    "Largest file",
                    "Bigger files are changed without a checkpoint",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut fe.max_file_kb)
                                .range(16..=65_536)
                                .speed(16.0)
                                .suffix(" KB"),
                        );
                    },
                );
                setting_row(ui, "Keep for", "0 keeps them all", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut fe.keep_turns)
                            .range(0..=1000)
                            .suffix(" turns"),
                    );
                });
            });

            section(ui, "Code review");
            let r = &mut g.review;
            switch(
                ui,
                "Code review",
                "Adds Review changes to the chat's menu and the command palette. It sends the \
                 working folder's git diff to a model with the instructions below, as a new \
                 message in the chat. The diff leaves your computer for the provider chosen here",
                &mut r.enabled,
            );
            ui.add_enabled_ui(r.enabled, |ui| {
                choice(
                    ui,
                    "review-scope",
                    "Changes to review",
                    "",
                    &mut r.scope,
                    &[
                        (
                            ReviewScope::Uncommitted,
                            "Uncommitted changes (staged, unstaged, new)",
                        ),
                        (ReviewScope::Branch, "This branch against its base"),
                    ],
                );
                choice(
                    ui,
                    "review-strictness",
                    "How strict",
                    "Lenient: only bugs, data loss and security issues. Normal: the \
                     instructions as written. Strict: also style, naming, missing tests and \
                     documentation, and risky assumptions",
                    &mut r.strictness,
                    &[
                        (ReviewStrictness::Lenient, "Lenient"),
                        (ReviewStrictness::Normal, "Normal"),
                        (ReviewStrictness::Strict, "Strict"),
                    ],
                );
                text_row(
                    ui,
                    "Base branch",
                    "Used by \"This branch against its base\"",
                    &mut r.base_branch,
                    "auto: origin/HEAD, main or master",
                );
                provider_row(
                    ui,
                    "review-provider",
                    "Review by",
                    "",
                    &mut r.provider_id,
                    &providers,
                );
                let models = models_of(&providers, &r.provider_id);
                model_row(ui, "Review model", &mut r.model, &models);
                setting_row(
                    ui,
                    "Largest diff",
                    "A longer diff is cut to this size, and the model is told so",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut r.max_diff_kb)
                                .range(16..=4096)
                                .suffix(" KB"),
                        );
                    },
                );
                long_text(
                    ui,
                    "Review instructions",
                    "",
                    &mut r.prompt,
                    4,
                    Some(crate::config::DEFAULT_REVIEW_PROMPT),
                );
            });
        });
    }

    /// The artifact panel, live web pages, the Save as formats and OCR for scanned PDFs.
    pub(super) fn artifacts_tab(&mut self, ui: &mut egui::Ui) {
        let providers = self.provider_list();
        let g = &mut self.draft.general;
        page(ui, |ui| {
            switch(
                ui,
                "Artifacts in a side panel",
                "Open web pages, drawings and long code beside the chat as they arrive",
                &mut g.open_artifacts,
            );

            section(ui, "Web page preview");
            let hp = &mut g.html_preview;
            ui.add_enabled_ui(
                crate::ui::webview::SUPPORTED || crate::ui::webview::OWN_WINDOW,
                |ui| {
                    switch(
                        ui,
                        "Show web pages live",
                        "Off: the panel shows the code and \"Open in browser\" opens the page",
                        &mut hp.enabled,
                    );
                    ui.add_enabled_ui(hp.enabled, |ui| {
                        switch(ui, "Let pages run their scripts", "", &mut hp.scripts);
                        switch(
                            ui,
                            "Let pages load from the network",
                            "Off: scripts, styles, images and requests from the web are \
                             blocked, so a page can't send chat content anywhere. Pages that use \
                             a CDN library need this on",
                            &mut hp.network,
                        );
                    });
                },
            );
            #[cfg(target_os = "linux")]
            note(ui, &crate::ui::linux_preview::status());
            #[cfg(not(target_os = "linux"))]
            if !crate::ui::webview::SUPPORTED {
                note(
                    ui,
                    "Needs the macOS or Windows app; on Linux web pages open in your browser.",
                );
            }

            section(ui, "Save as");
            ui.add_space(4.0);
            note(
                ui,
                "The Save as menu on a reply or artifact offers the formats that are on.",
            );
            switch(ui, "Word document (.docx)", "", &mut g.save_as.docx);
            switch(ui, "PDF (.pdf)", "", &mut g.save_as.pdf);
            switch(
                ui,
                "Excel workbook (.xlsx)",
                "Offered for replies with a table and for CSV or TSV artifacts",
                &mut g.save_as.xlsx,
            );

            section(ui, "Scanned PDFs");
            let o = &mut g.ocr;
            switch(
                ui,
                "Read scanned PDFs (OCR)",
                "A PDF with no text layer (a scan or a photo of pages) has its page images \
                 read. A vision model gets each page image, sent to the provider chosen here; \
                 Tesseract runs on this computer. Without OCR, such a PDF is attached with a note",
                &mut o.enabled,
            );
            ui.add_enabled_ui(o.enabled, |ui| {
                choice(
                    ui,
                    "ocr-engine",
                    "Read with",
                    "",
                    &mut o.engine,
                    &[
                        (OcrEngine::VisionModel, "A vision model"),
                        (OcrEngine::Tesseract, "Tesseract"),
                    ],
                );
                if o.engine == OcrEngine::VisionModel {
                    setting_row(
                        ui,
                        "Vision model by",
                        "The model must accept images. Pages are sent one at a time, so a long \
                         scan takes a while",
                        |ui| {
                            let current = o
                                .provider
                                .as_ref()
                                .and_then(|id| providers.iter().find(|(pid, ..)| pid == id))
                                .map_or("The active provider".to_owned(), |(_, n, _)| n.clone());
                            egui::ComboBox::from_id_salt("ocr-provider")
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut o.provider,
                                        None,
                                        "The active provider",
                                    );
                                    for (id, name, _) in &providers {
                                        ui.selectable_value(
                                            &mut o.provider,
                                            Some(id.clone()),
                                            name,
                                        );
                                    }
                                });
                        },
                    );
                } else {
                    text_row(
                        ui,
                        "Tesseract program",
                        "A name on PATH, or the full path of the program",
                        &mut o.tesseract_path,
                        "tesseract",
                    );
                    text_row(
                        ui,
                        "Languages",
                        "Tesseract language codes joined with +, e.g. eng+deu; each must be \
                         installed with Tesseract",
                        &mut o.languages,
                        "eng",
                    );
                }
                setting_row(
                    ui,
                    "Pages per PDF",
                    "Pages past this are not read, and the model is told so",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut o.max_pages)
                                .range(1..=200)
                                .suffix(" pages"),
                        );
                    },
                );
            });
        });
    }
}

/// The models of the provider with this id (none for "the chat's provider").
fn models_of(providers: &Providers, id: &str) -> Vec<String> {
    providers
        .iter()
        .find(|(pid, ..)| pid == id)
        .map(|(.., m)| m.clone())
        .unwrap_or_default()
}

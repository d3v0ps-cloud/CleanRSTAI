//! The Tools and Extensions (skills) page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn tools_tab(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.draft.general.builtin_tools;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(
                    "Built-in tools that run on this computer don't ask for approval; web fetch \
                     reaches the internet, so it asks first, like MCP tools. These are the \
                     defaults for new chats; switch them for one chat from the tools menu next \
                     to the message box.",
                );
                ui.add_space(10.0);
                for (on, ic, name, desc) in [
                    (
                        &mut b.time,
                        icon::CLOCK,
                        "Time & date",
                        "The current date, time, weekday and time zone, and the time at other UTC offsets.",
                    ),
                    (
                        &mut b.calculator,
                        icon::CALCULATOR,
                        "Calculator",
                        "Exact arithmetic and functions (powers, roots, logarithms, trigonometry, rounding, min/max/avg) instead of mental maths.",
                    ),
                    (
                        &mut b.tasks,
                        icon::LIST_CHECKS,
                        "Task list",
                        "For multi-step requests the model breaks the work into actionable tasks, shows them as a checklist in the chat, and ticks them off as it goes.",
                    ),
                    (
                        &mut b.fetch,
                        icon::GLOBE,
                        "Web fetch",
                        "Reads a web URL: pages as text with their links, plain text and JSON as they are, images as images. Asks for approval before each fetch.",
                    ),
                    (
                        &mut b.web_search,
                        icon::SEARCH,
                        "Web search",
                        "Searches the web with the service below (or the provider's own search, when Search with says so) and lists the results (title, address, snippet), so the model can read the useful pages with web fetch.",
                    ),
                    (
                        &mut b.workspace,
                        icon::FOLDER,
                        "Workspace tools",
                        "When a chat has a working folder (set below the message box): read_file, file_glob_search, grep_search, get_info, git_status and git_diff run freely inside it; write_file, edit_file and exec_shell_command ask for approval. Paths can't leave the folder.",
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.checkbox(on, egui::RichText::new(format!("{ic}  {name}")).strong());
                    });
                    ui.indent(name, |ui| {
                        ui.weak(desc);
                    });
                    ui.add_space(8.0);
                }
                ui.add_enabled_ui(b.workspace, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut b.git_commit,
                            egui::RichText::new(format!("{}  Commit (git_commit)", icon::CHECK_SQUARE))
                                .strong(),
                        );
                    });
                    ui.indent("git-commit", |ui| {
                        ui.weak(
                            "Lets the model commit the workspace's changes to its git repository as \
                             one local commit. Every commit asks for approval, even in Auto in \
                             workspace mode. Nothing is pushed, and git hooks don't run. Off by \
                             default.",
                        );
                        ui.add_enabled_ui(b.git_commit, |ui| {
                            ui.checkbox(&mut b.allow_signing, "Allow signed commits");
                            ui.weak(
                                "Off: commits are never signed. On: a repository set to sign its \
                                 commits signs them, and gpg may ask for a passphrase.",
                            );
                        });
                    });
                    ui.add_space(8.0);
                });
                ui.add_enabled_ui(b.fetch, |ui| {
                    ui.checkbox(
                        &mut b.fetch_private_networks,
                        "Let web fetch reach this computer and private networks",
                    );
                    ui.indent("fetch-private", |ui| {
                        ui.weak(
                            "Off: localhost, private (10.x, 192.168.x, …) and link-local addresses, \
                             such as cloud metadata endpoints, are refused, also after a redirect. \
                             Subagents and auto-approved calls fetch without asking, so leave \
                             this off unless you need it.",
                        );
                    });
                });
                ui.add_space(12.0);
                ui.add_enabled_ui(b.web_search, |ui| {
                    web_search_rows(ui, &mut b.search, &mut self.show_search_key);
                });
                ui.add_space(8.0);
                ui.weak(format!(
                    "{}  The subagent tool has its own page: Subagents. Context compaction, \
                     project instructions and undo for file edits are on Context & files. \
                     Slash commands and skills are on Skills & hooks.",
                    icon::ROBOT
                ));
            });
    }

    pub(super) fn extensions_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                slash_command_rows(ui, &mut self.draft.general.slash_commands);
                ui.add_space(16.0);
                self.skill_rows(ui);
                ui.add_space(16.0);
                hook_rows(ui, &mut self.draft.general.hooks);
            });
    }

    /// Skills: the on/off switch, the workspace switch, the size limit, the skills folder and
    /// the skills it holds, each with its own switch.
    pub(super) fn skill_rows(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.draft.general.skills;
        ui.label(egui::RichText::new(format!("{}  Skills", icon::BOOKS)).strong());
        ui.indent("skills-help", |ui| {
            ui.weak(
                "A skill is a folder with a SKILL.md file: instructions the model reads with \
                 its load_skill tool when a task matches the skill's description. Only the \
                 names and descriptions are sent with each message; the text is sent when the \
                 model loads it.",
            );
        });
        ui.add_space(4.0);
        ui.checkbox(&mut s.enabled, "Offer skills to the model");
        ui.add_enabled_ui(s.enabled, |ui| {
            ui.checkbox(
                &mut s.workspace_skills,
                "Load skills from the workspace's .cleanrstai/skills folder",
            )
            .on_hover_text(
                "Each .cleanrstai/skills/<name>/SKILL.md in a chat's working folder is a skill. \
                 It replaces a skill from your skills folder with the same name.",
            );
            ui.horizontal(|ui| {
                ui.label("Size limit per load");
                ui.add(
                    egui::DragValue::new(&mut s.max_kb)
                        .range(1..=1024)
                        .suffix(" KB"),
                )
                .on_hover_text("Text beyond this is left out when the model loads a skill");
            });
        });
        ui.add_space(8.0);

        let folder = crate::skills::user_root();
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Open skills folder", icon::FOLDER))
                .on_hover_text("Create the folder if it's missing and show it")
                .clicked()
            {
                self.skills_folder_error = match std::fs::create_dir_all(&folder)
                    .map_err(|e| e.to_string())
                    .and_then(|()| open::that_detached(&folder).map_err(|e| e.to_string()))
                {
                    Ok(()) => None,
                    Err(e) => Some(format!("Couldn't open {}: {e}", folder.display())),
                };
                self.user_skills = None;
            }
            if ui.button("Refresh").clicked() {
                self.user_skills = None;
            }
        });
        ui.weak(format!("Your skills folder: {}", folder.display()));
        if let Some(e) = &self.skills_folder_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.add_space(6.0);

        let found = self
            .user_skills
            .get_or_insert_with(|| {
                // Listed without the disabled names, so switched-off skills can be switched on.
                let all = crate::config::SkillSettings {
                    disabled: Vec::new(),
                    ..self.draft.general.skills.clone()
                };
                crate::skills::discover(&[(folder.clone(), crate::skills::SkillSource::User)], &all)
            })
            .clone();
        if found.is_empty() {
            ui.weak(
                "No skills yet. Add a folder with a SKILL.md file to the skills folder above, \
                 then press Refresh.",
            );
        }
        let disabled = &mut self.draft.general.skills.disabled;
        for skill in &found {
            ui.push_id(("skill", &skill.dir), |ui| {
                let mut on = !disabled.contains(&skill.name);
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut on, egui::RichText::new(&skill.name).strong())
                        .changed()
                    {
                        if on {
                            disabled.retain(|n| *n != skill.name);
                        } else if !disabled.contains(&skill.name) {
                            disabled.push(skill.name.clone());
                        }
                    }
                    ui.weak(skill.source.label());
                });
                ui.indent("skill-desc", |ui| {
                    if skill.description.is_empty() {
                        ui.weak("No description");
                    } else {
                        ui.weak(skill.description.as_str());
                    }
                });
            });
            ui.add_space(4.0);
        }
    }
}

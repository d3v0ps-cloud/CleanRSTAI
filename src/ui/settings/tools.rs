//! The Tools and Extensions (skills) page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn tools_tab(&mut self, ui: &mut egui::Ui) {
        let b = &mut self.draft.general.builtin_tools;
        page(ui, |ui| {
            note(
                ui,
                "Built-in tools that run on this computer don't ask for approval; web fetch and \
                 web search reach the internet, so they ask first, like MCP tools. These are the \
                 defaults for new chats; switch them for one chat from the Tools menu next to \
                 the message box.",
            );
            ui.add_space(6.0);
            switch(
                ui,
                "Time & date",
                "The current date, time, weekday and time zone, and the time at other UTC offsets",
                &mut b.time,
            );
            switch(
                ui,
                "Calculator",
                "Exact arithmetic and functions (powers, roots, logarithms, trigonometry, \
                 rounding, min/max/avg) instead of mental maths",
                &mut b.calculator,
            );
            switch(
                ui,
                "Task list",
                "For multi-step requests the model breaks the work into tasks, shows them as a \
                 checklist in the chat, and ticks them off as it goes",
                &mut b.tasks,
            );
            switch(
                ui,
                "Web fetch",
                "Reads a web URL: pages as text with their links, plain text and JSON as they \
                 are, images as images. Asks for approval before each fetch",
                &mut b.fetch,
            );
            ui.add_enabled_ui(b.fetch, |ui| {
                switch(
                    ui,
                    "Let web fetch reach this computer and private networks",
                    "Off: localhost, private (10.x, 192.168.x, …) and link-local addresses, \
                     such as cloud metadata endpoints, are refused, also after a redirect. \
                     Subagents and auto-approved calls fetch without asking, so leave this off \
                     unless you need it",
                    &mut b.fetch_private_networks,
                );
            });
            switch(
                ui,
                "Web search",
                "Searches the web with the service below (or the provider's own search) and \
                 lists the results, so the model can read the useful pages with web fetch",
                &mut b.web_search,
            );
            ui.add_enabled_ui(b.web_search, |ui| {
                web_search_rows(ui, &mut b.search, &mut self.show_search_key);
            });
            ui.add_space(10.0);
            note(
                ui,
                "The workspace tools (files, search, shell, commit) are on Workspace, the \
                 subagent tool on Subagents, and slash commands and skills on Skills & hooks.",
            );
        });
    }

    pub(super) fn extensions_tab(&mut self, ui: &mut egui::Ui) {
        page(ui, |ui| {
            section(ui, "Slash commands");
            slash_command_rows(ui, &mut self.draft.general.slash_commands);
            section(ui, "Skills");
            self.skill_rows(ui);
            section(ui, "Hooks");
            hook_rows(ui, &mut self.draft.general.hooks);
        });
    }

    /// Skills: the on/off switch, the workspace switch, the size limit, the skills folder and
    /// the skills it holds, each with its own switch.
    pub(super) fn skill_rows(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.draft.general.skills;
        switch(
            ui,
            "Offer skills to the model",
            "A skill is a folder with a SKILL.md file: instructions the model reads with its \
             load_skill tool when a task matches the skill's description. Only the names and \
             descriptions are sent with each message; the text is sent when the model loads it",
            &mut s.enabled,
        );
        ui.add_enabled_ui(s.enabled, |ui| {
            switch(
                ui,
                "Load skills from the workspace",
                "Each .cleanrstai/skills/<name>/SKILL.md in a chat's working folder is a skill. \
                 It replaces a skill from your skills folder with the same name",
                &mut s.workspace_skills,
            );
            setting_row(
                ui,
                "Size limit per load",
                "Text beyond this is left out when the model loads a skill",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.max_kb)
                            .range(1..=1024)
                            .suffix(" KB"),
                    );
                },
            );
        });

        let folder = crate::skills::user_root();
        let folder_text = folder.display().to_string();
        setting_row(ui, "Your skills folder", &folder_text, |ui| {
            if ui.button("Refresh").clicked() {
                self.user_skills = None;
            }
            if ui
                .button("Open folder")
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
        });
        if let Some(e) = &self.skills_folder_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }

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
            ui.add_space(6.0);
            note(
                ui,
                "No skills yet. Add a folder with a SKILL.md file to the skills folder above, \
                 then press Refresh.",
            );
        }
        let disabled = &mut self.draft.general.skills.disabled;
        for skill in &found {
            let mut on = !disabled.contains(&skill.name);
            let before = on;
            let about = if skill.description.is_empty() {
                format!("{} · no description", skill.source.label())
            } else {
                format!("{} · {}", skill.source.label(), skill.description)
            };
            switch(ui, &skill.name, &about, &mut on);
            if on != before {
                if on {
                    disabled.retain(|n| *n != skill.name);
                } else if !disabled.contains(&skill.name) {
                    disabled.push(skill.name.clone());
                }
            }
        }
    }
}

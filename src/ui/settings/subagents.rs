//! The Subagents page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn subagents_tab(&mut self, ui: &mut egui::Ui) {
        let providers: Vec<(String, String, Vec<String>)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.models.clone()))
            .collect();
        let servers: Vec<(String, String)> = self
            .draft
            .mcp_servers
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let sa = &mut self.draft.subagents;
        page(ui, |ui| {
            switch(
                ui,
                "Let the model delegate tasks to subagents",
                "Adds a \"delegate\" tool. Each subagent works on one task on its own, in \
                 parallel with the others, and its result goes back to the main chat. \
                 Delegating asks for approval unless tool calls are auto-approved \
                 (Permissions). It can also be switched per chat from the Tools menu",
                &mut sa.enabled,
            );
            ui.add_enabled_ui(sa.enabled, |ui| {
                section(ui, "Model");
                provider_row_or(
                    ui,
                    "subagent-provider",
                    "Provider",
                    "",
                    &mut sa.provider_id,
                    &providers,
                    "Same as the chat",
                );
                let models = providers
                    .iter()
                    .find(|(id, ..)| *id == sa.provider_id)
                    .map(|(.., m)| m.clone())
                    .unwrap_or_default();
                model_row(ui, "Model", &mut sa.model, &models);
                switch(
                    ui,
                    "Let the model choose per task",
                    "The delegate tool lists your provider profiles, so the model can, for \
                     example, send simple tasks to a fast local model",
                    &mut sa.allow_provider_choice,
                );

                section(ui, "Tools");
                switch(
                    ui,
                    "Time, calculator and web fetch",
                    "When they are on in the chat",
                    &mut sa.builtin_tools,
                );
                switch(ui, "The chat's MCP tools", "", &mut sa.mcp_tools);
                if sa.mcp_tools {
                    for (id, name) in &servers {
                        let mut on = !sa.excluded_servers.contains(id);
                        let before = on;
                        switch(ui, &format!("    {name}"), "", &mut on);
                        if on != before {
                            if on {
                                sa.excluded_servers.retain(|x| x != id);
                            } else {
                                sa.excluded_servers.push(id.clone());
                            }
                        }
                    }
                    warning(ui, "Subagents call MCP tools without asking for approval.");
                }
                switch(
                    ui,
                    "The workspace tools",
                    "Only while the Docker sandbox is on. Each subagent gets its own container \
                     with the chat's folder at /workspace, sharing the chat's network rules. \
                     It is removed when the subagent finishes",
                    &mut sa.workspace,
                );
                if sa.workspace {
                    warning(
                        ui,
                        "Subagents read, write and run commands without asking for approval; \
                         approving the delegate call allows it.",
                    );
                }

                section(ui, "Limits");
                setting_row(
                    ui,
                    "Run at once",
                    "How many subagents work at the same time",
                    |ui| {
                        ui.add(egui::DragValue::new(&mut sa.max_parallel).range(1..=16));
                    },
                );
                setting_row(
                    ui,
                    "Tasks per call",
                    "Most tasks one delegate call may hand out",
                    |ui| {
                        ui.add(egui::DragValue::new(&mut sa.max_tasks).range(1..=32));
                    },
                );
                setting_row(
                    ui,
                    "Tool rounds",
                    "Tool-call rounds each subagent may make",
                    |ui| {
                        ui.add(egui::DragValue::new(&mut sa.max_rounds).range(1..=100));
                    },
                );
                setting_row(ui, "Time limit", "Per subagent", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut sa.timeout_secs)
                            .range(10..=3600)
                            .suffix(" s"),
                    );
                });
                long_text(
                    ui,
                    "Subagent system prompt",
                    "",
                    &mut sa.system_prompt,
                    4,
                    None,
                );
            });
        });
    }
}

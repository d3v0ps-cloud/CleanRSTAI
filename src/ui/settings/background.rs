//! The Background and Remote access pages of Settings.

use super::*;

impl SettingsUi {
    /// The tray, notifications, the reply queue and the command line.
    pub(super) fn background_tab(&mut self, ui: &mut egui::Ui) {
        let plan_on = self.draft.general.plan.enabled;
        let bg = &mut self.draft.general.background;
        let ex = &mut self.draft.general.exec;
        page(ui, |ui| {
            ui.add_enabled_ui(crate::tray::SUPPORTED, |ui| {
                switch(
                    ui,
                    crate::tray::setting_label(),
                    if crate::tray::SUPPORTED {
                        "Closing the window hides it, and replies and MCP servers keep running. \
                         Show the window or quit from the icon's menu. On macOS, Command-Q quits \
                         as usual"
                    } else {
                        "Not available on Linux yet"
                    },
                    &mut bg.keep_running,
                );
            });

            section(ui, "Notifications");
            switch(
                ui,
                "Notify when a reply finishes, fails or needs approval",
                "Sent through the system's notifications. Each chat can switch its own off \
                 under Chat options",
                &mut bg.notify,
            );
            ui.add_enabled_ui(bg.notify, |ui| {
                switch(
                    ui,
                    "Only while the window is in the background",
                    "",
                    &mut bg.notify_only_unfocused,
                );
            });

            section(ui, "Replies");
            setting_row(
                ui,
                "Replies at once",
                "Replies beyond this wait in a queue, marked Queued, and start in the order \
                 they were sent as others finish. Stop on a queued chat takes it out of the queue",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut bg.max_parallel)
                            .range(1..=crate::config::MAX_PARALLEL_RUNS),
                    );
                },
            );
            setting_row(
                ui,
                "Time limit",
                "0 means no limit. A reply that runs longer is stopped and says so. The time \
                 counts from when the reply starts, including any wait for approval",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut bg.time_limit_mins)
                            .range(0..=crate::config::MAX_TIME_LIMIT_MINS)
                            .suffix(" min"),
                    );
                },
            );

            section(ui, "Command line");
            switch(
                ui,
                "Allow `cleanrstai exec`",
                "Runs requests from a terminal or script. Replies go to stdout and tool calls \
                 to stderr; `cleanrstai exec --help` lists every option",
                &mut ex.enabled,
            );
            ui.add_enabled_ui(ex.enabled, |ui| {
                setting_row(
                    ui,
                    "Approvals",
                    "The mode of a command-line request; --mode overrides it. Calls it doesn't \
                     allow are denied, not asked, and the model is told",
                    |ui| {
                        egui::ComboBox::from_id_salt("exec-approval-mode")
                            .selected_text(ex.approval_mode.label())
                            .show_ui(ui, |ui| {
                                for m in ApprovalMode::offered(plan_on) {
                                    ui.selectable_value(&mut ex.approval_mode, m, m.label())
                                        .on_hover_text(m.description());
                                }
                            });
                    },
                );
                switch(
                    ui,
                    "Start MCP servers for each request",
                    "--no-mcp skips them",
                    &mut ex.mcp,
                );
                setting_row(
                    ui,
                    "Wait for MCP servers",
                    "How long a request waits for servers to connect before it starts",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut ex.mcp_wait_secs)
                                .range(0..=crate::exec::MAX_MCP_WAIT_SECS)
                                .suffix(" s"),
                        );
                    },
                );
                switch(ui, "Save each run's chat", "", &mut ex.save_chats);
            });
            ui.add_space(8.0);
            note(
                ui,
                "Example: git diff | cleanrstai exec --mode read-only \"review this\" -",
            );
            note(
                ui,
                "Scheduled agents have their own page: Scheduled, in the sidebar.",
            );
        });
    }

    /// The browser client's server, the Cloudflare Tunnel and Cloudflare Access.
    pub(super) fn remote_tab(
        &mut self,
        ui: &mut egui::Ui,
        live: &Settings,
        remote: &RemoteStatus,
        (tunnel, access_error): (&TunnelStatus, Option<&str>),
    ) {
        let (draft_remote, live_remote) = (&self.draft.general.remote, &live.general.remote);
        let status = RemoteUiStatus {
            server: remote,
            tunnel,
            server_dirty: crate::remote::server_settings(draft_remote)
                != crate::remote::server_settings(live_remote),
            tunnel_dirty: draft_remote.tunnel != live_remote.tunnel,
            access_error,
        };
        page(ui, |ui| {
            remote_rows(
                ui,
                &mut self.draft.general.remote,
                &mut self.show_remote_password,
                &status,
            );
        });
    }
}

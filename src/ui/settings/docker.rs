//! The Docker sandbox page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn docker_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let busy = self.sandbox_busy;
        let sb = &mut self.draft.sandbox;
        page(ui, |ui| {
            switch(
                ui,
                "Run shell commands in a Docker container",
                "The sandbox applies to new commands; settings changes remove the running \
                 containers",
                &mut sb.enabled,
            );

            section(ui, "Image");
            choice(
                ui,
                "sandbox-source",
                "Use",
                "",
                &mut sb.source,
                &[
                    (SandboxImageSource::Image, "A Docker image"),
                    (SandboxImageSource::Dockerfile, "A custom Dockerfile"),
                ],
            );
            match sb.source {
                SandboxImageSource::Image => {
                    text_row(ui, "Image name", "", &mut sb.image, "python:3.13-slim");
                    choice(
                        ui,
                        "sandbox-pull",
                        "Get the image",
                        "",
                        &mut sb.pull,
                        &[
                            (SandboxPull::Missing, "Pull when missing"),
                            (SandboxPull::Always, "Pull every time the app starts"),
                            (
                                SandboxPull::Never,
                                "Only use an image already on this computer",
                            ),
                        ],
                    );
                }
                SandboxImageSource::Dockerfile => {
                    let built_as = format!(
                        "Built by the app as {} when it changes. There is no build context, so \
                         COPY and ADD of local files don't work.",
                        crate::sandbox::image_ref(sb)
                    );
                    setting_row(ui, "Dockerfile", &built_as, |ui| {
                        if ui
                            .button("Full example")
                            .on_hover_text(
                                "Ubuntu with Python, Node.js, build tools and network tools \
                                 (ping, nmap, dig, traceroute); Python data, document, PDF, \
                                 image, chart and NLP packages; about 3 GB",
                            )
                            .clicked()
                        {
                            sb.dockerfile = FULL_SANDBOX_DOCKERFILE.into();
                        }
                        if ui
                            .button("Small example")
                            .on_hover_text("python:3.13-slim with git and curl")
                            .clicked()
                        {
                            sb.dockerfile = DEFAULT_SANDBOX_DOCKERFILE.into();
                        }
                    });
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::multiline(&mut sb.dockerfile)
                            .code_editor()
                            .desired_rows(10)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);
                }
            }
            setting_row(
                ui,
                "Check",
                "Test finds Docker and the image; the other button fetches or builds it now",
                |ui| {
                    let prepare = match sb.source {
                        SandboxImageSource::Image => "Pull now",
                        SandboxImageSource::Dockerfile => "Build now",
                    };
                    if ui.add_enabled(!busy, egui::Button::new(prepare)).clicked() {
                        self.sandbox_status = None;
                        actions.push(SettingsAction::PrepareSandbox(sb.normalized()));
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Test")).clicked() {
                        self.sandbox_status = None;
                        actions.push(SettingsAction::CheckSandbox(sb.normalized()));
                    }
                    if busy {
                        ui.spinner();
                    }
                },
            );
            match &self.sandbox_status {
                Some(Ok(m)) => note(ui, m),
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                None => {}
            }

            section(ui, "Network");
            choice(
                ui,
                "sandbox-network",
                "Network",
                "Normal reaches this computer, your local network and the internet",
                &mut sb.network,
                &[
                    (SandboxNetwork::None, "None"),
                    (SandboxNetwork::Bridge, "Normal network"),
                    (SandboxNetwork::Custom, "Only the hosts and ports below"),
                ],
            );
            if sb.network == SandboxNetwork::Custom {
                ui.add_space(8.0);
                net_rules_editor(ui, &mut sb.rules);
                ui.add_space(4.0);
            }

            section(ui, "Limits");
            setting_row(ui, "CPUs", "", |ui| {
                ui.add(
                    egui::DragValue::new(&mut sb.cpus)
                        .range(0.25..=64.0)
                        .speed(0.25),
                );
            });
            setting_row(ui, "Memory", "", |ui| {
                ui.add(
                    egui::DragValue::new(&mut sb.memory_mb)
                        .range(256..=262_144)
                        .speed(64)
                        .suffix(" MB"),
                );
            });
            setting_row(ui, "Processes", "", |ui| {
                ui.add(
                    egui::DragValue::new(&mut sb.pids)
                        .range(32..=65_536)
                        .speed(16),
                );
            });
            switch(
                ui,
                "Mount the workspace read-only",
                "Commands can't change files",
                &mut sb.read_only,
            );
            file_row(
                ui,
                "docker program",
                "Empty: found on PATH",
                &mut sb.docker,
                "found on PATH",
                "Choose the docker program",
                &[],
            );
        });
    }
}

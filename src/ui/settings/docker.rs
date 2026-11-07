//! The Docker sandbox page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn docker_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        let busy = self.sandbox_busy;
        let sb = &mut self.draft.sandbox;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("docker-grid")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Sandbox");
                        ui.checkbox(&mut sb.enabled, "Run shell commands in a Docker container");
                        ui.end_row();

                        ui.label("Image");
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut sb.source,
                                SandboxImageSource::Image,
                                "Docker image",
                            );
                            ui.radio_value(
                                &mut sb.source,
                                SandboxImageSource::Dockerfile,
                                "Custom Dockerfile",
                            );
                        });
                        ui.end_row();

                        match sb.source {
                            SandboxImageSource::Image => {
                                ui.label("Image name");
                                ui.add(
                                    egui::TextEdit::singleline(&mut sb.image)
                                        .hint_text("python:3.13-slim")
                                        .desired_width(FIELD_WIDTH),
                                );
                                ui.end_row();

                                ui.label("Get the image");
                                ui.vertical(|ui| {
                                    ui.radio_value(
                                        &mut sb.pull,
                                        SandboxPull::Missing,
                                        "Pull when missing",
                                    );
                                    ui.radio_value(
                                        &mut sb.pull,
                                        SandboxPull::Always,
                                        "Pull every time the app starts",
                                    );
                                    ui.radio_value(
                                        &mut sb.pull,
                                        SandboxPull::Never,
                                        "Only use an image already on this computer",
                                    );
                                });
                                ui.end_row();
                            }
                            SandboxImageSource::Dockerfile => {
                                ui.label("Dockerfile");
                                ui.vertical(|ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut sb.dockerfile)
                                            .code_editor()
                                            .desired_rows(10)
                                            .desired_width(FIELD_WIDTH),
                                    );
                                    ui.weak(format!(
                                        "Built by the app as {} when it changes. There is no \
                                         build context, so COPY and ADD of local files don't work.",
                                        crate::sandbox::image_ref(sb)
                                    ));
                                    ui.horizontal(|ui| {
                                        if ui
                                            .button("Small example")
                                            .on_hover_text("python:3.13-slim with git and curl")
                                            .clicked()
                                        {
                                            sb.dockerfile = DEFAULT_SANDBOX_DOCKERFILE.into();
                                        }
                                        if ui
                                            .button("Full example")
                                            .on_hover_text(
                                                "Ubuntu with Python, Node.js, build tools and \
                                                 network tools (ping, nmap, dig, traceroute); \
                                                 Python data, document, PDF, image, chart \
                                                 and NLP packages; about 3 GB",
                                            )
                                            .clicked()
                                        {
                                            sb.dockerfile = FULL_SANDBOX_DOCKERFILE.into();
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        }

                        ui.label("Network");
                        ui.vertical(|ui| {
                            ui.radio_value(
                                &mut sb.network,
                                SandboxNetwork::None,
                                "None (no network)",
                            );
                            ui.radio_value(
                                &mut sb.network,
                                SandboxNetwork::Bridge,
                                "Normal network (can reach this computer, your local network and the internet)",
                            );
                            ui.radio_value(
                                &mut sb.network,
                                SandboxNetwork::Custom,
                                "Only the hosts and ports below",
                            );
                        });
                        ui.end_row();

                        if sb.network == SandboxNetwork::Custom {
                            ui.label("Allowed outbound");
                            ui.vertical(|ui| net_rules_editor(ui, &mut sb.rules));
                            ui.end_row();
                        }

                        ui.label("CPUs");
                        ui.add(
                            egui::DragValue::new(&mut sb.cpus)
                                .range(0.25..=64.0)
                                .speed(0.25),
                        );
                        ui.end_row();

                        ui.label("Memory");
                        ui.add(
                            egui::DragValue::new(&mut sb.memory_mb)
                                .range(256..=262_144)
                                .speed(64)
                                .suffix(" MB"),
                        );
                        ui.end_row();

                        ui.label("Processes");
                        ui.add(
                            egui::DragValue::new(&mut sb.pids)
                                .range(32..=65_536)
                                .speed(16),
                        );
                        ui.end_row();

                        ui.label("Workspace folder");
                        ui.checkbox(
                            &mut sb.read_only,
                            "Mount read-only (commands can't change files)",
                        );
                        ui.end_row();

                        ui.label("docker");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut sb.docker)
                                    .hint_text("found on PATH")
                                    .desired_width(FIELD_WIDTH - 100.0),
                            );
                            if ui.button("Browse…").clicked()
                                && let Some(file) = rfd::FileDialog::new()
                                    .set_title("Choose the docker program")
                                    .pick_file()
                            {
                                sb.docker = file.display().to_string();
                            }
                        });
                        ui.end_row();

                        ui.label("Status");
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!busy, egui::Button::new("Test")).clicked() {
                                    self.sandbox_status = None;
                                    actions.push(SettingsAction::CheckSandbox(sb.normalized()));
                                }
                                let prepare = match sb.source {
                                    SandboxImageSource::Image => "Pull now",
                                    SandboxImageSource::Dockerfile => "Build now",
                                };
                                if ui.add_enabled(!busy, egui::Button::new(prepare)).clicked() {
                                    self.sandbox_status = None;
                                    actions.push(SettingsAction::PrepareSandbox(sb.normalized()));
                                }
                                if busy {
                                    ui.spinner();
                                }
                            });
                            match &self.sandbox_status {
                                Some(Ok(m)) => {
                                    ui.weak(m);
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(ui.visuals().error_fg_color, e);
                                }
                                None => {}
                            }
                            ui.weak(
                                "The sandbox applies to new commands; settings changes remove the \
                                 running containers.",
                            );
                        });
                        ui.end_row();
                    });
            });
    }
}

//! The Voice page of Settings.

use super::*;

impl SettingsUi {
    pub(super) fn voice_tab(&mut self, ui: &mut egui::Ui) {
        let providers: Vec<(String, String)> = self
            .draft
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let devices = self
            .mic_devices
            .get_or_insert_with(crate::voice::record::input_devices)
            .clone();
        let voice = &mut self.draft.general.voice;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.checkbox(
                    &mut voice.input.enabled,
                    egui::RichText::new("Voice input").strong(),
                );
                ui.indent("voice-in", |ui| {
                    ui.weak(
                        "A microphone button under the message box records a clip, and the \
                         transcript goes into the message box to read and edit before you \
                         send it. Ctrl/⌘+Shift+M starts and stops a recording. The clip goes to \
                         the transcription endpoint below: a cloud service, or a local server \
                         such as whisper.cpp, faster-whisper-server, speaches or LocalAI.",
                    );
                    ui.add_enabled_ui(voice.input.enabled, |ui| {
                        egui::Grid::new("voice-in-grid")
                            .num_columns(2)
                            .spacing([12.0, 10.0])
                            .show(ui, |ui| {
                                ui.label("Provider");
                                provider_choice(
                                    ui,
                                    "voice-in-provider",
                                    &mut voice.input.provider,
                                    &providers,
                                );
                                ui.end_row();
                                ui.label("Model");
                                ui.add(
                                    egui::TextEdit::singleline(&mut voice.input.model)
                                        .desired_width(220.0),
                                );
                                ui.end_row();
                                ui.label("Language");
                                ui.add(
                                    egui::TextEdit::singleline(&mut voice.input.language)
                                        .hint_text("Detect")
                                        .desired_width(220.0),
                                );
                                ui.end_row();
                                ui.label("Longest recording (seconds)");
                                ui.add(
                                    egui::DragValue::new(&mut voice.input.max_secs)
                                        .range(5..=crate::voice::MAX_RECORD_SECS),
                                );
                                ui.end_row();
                                ui.label("Microphone");
                                mic_picker(ui, &mut voice.input.device, &devices);
                                ui.end_row();
                            });
                    });
                });
                ui.add_space(12.0);
                ui.checkbox(
                    &mut voice.output.enabled,
                    egui::RichText::new("Read replies aloud").strong(),
                );
                ui.indent("voice-out", |ui| {
                    ui.weak(
                        "A speaker button under each reply reads it aloud, and Ctrl/⌘+Shift+S \
                         reads the latest reply or stops the reading. Code blocks and tables are \
                         announced rather than read, and links are read as their text.",
                    );
                    ui.add_enabled_ui(voice.output.enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut voice.output.engine,
                                crate::voice::SpeechEngine::OsVoice,
                                crate::voice::SpeechEngine::OsVoice.label(),
                            );
                            ui.radio_value(
                                &mut voice.output.engine,
                                crate::voice::SpeechEngine::Endpoint,
                                crate::voice::SpeechEngine::Endpoint.label(),
                            );
                        });
                        let endpoint = voice.output.engine == crate::voice::SpeechEngine::Endpoint;
                        ui.weak(if endpoint {
                            "Sent to the speech endpoint below as WAV audio, then played on this computer."
                        } else {
                            "Spoken by this computer: say on macOS, System.Speech on Windows, spd-say or espeak-ng on Linux. Nothing leaves the computer."
                        });
                        egui::Grid::new("voice-out-grid")
                            .num_columns(2)
                            .spacing([12.0, 10.0])
                            .show(ui, |ui| {
                                if endpoint {
                                    ui.label("Provider");
                                    provider_choice(
                                        ui,
                                        "voice-out-provider",
                                        &mut voice.output.provider,
                                        &providers,
                                    );
                                    ui.end_row();
                                    ui.label("Model");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut voice.output.model)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                    ui.label("Voice");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut voice.output.voice)
                                            .desired_width(220.0),
                                    );
                                    ui.end_row();
                                }
                                ui.label("Speed");
                                ui.add(
                                    egui::Slider::new(&mut voice.output.speed, 0.5..=2.0)
                                        .step_by(0.05),
                                );
                                ui.end_row();
                                ui.label("Longest reply read (characters)");
                                ui.add(
                                    egui::DragValue::new(&mut voice.output.max_chars).range(
                                        200..=crate::voice::MAX_SPEAK_CHARS,
                                    ),
                                );
                                ui.end_row();
                            });
                    });
                });
            });
    }
}

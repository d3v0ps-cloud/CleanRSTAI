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
        page(ui, |ui| {
            section(ui, "Voice input");
            let input = &mut voice.input;
            switch(
                ui,
                "Voice input",
                "A microphone button under the message box records a clip, and the transcript \
                 goes into the message box to read and edit before you send it. Ctrl/⌘+Shift+M \
                 starts and stops a recording. The clip goes to the transcription endpoint \
                 below: a cloud service, or a local server such as whisper.cpp, \
                 faster-whisper-server, speaches or LocalAI",
                &mut input.enabled,
            );
            ui.add_enabled_ui(input.enabled, |ui| {
                setting_row(ui, "Provider", "", |ui| {
                    provider_choice(ui, "voice-in-provider", &mut input.provider, &providers);
                });
                text_row(ui, "Model", "The transcription model", &mut input.model, "");
                text_row(
                    ui,
                    "Language",
                    "Empty: the service detects it",
                    &mut input.language,
                    "Detect",
                );
                setting_row(ui, "Longest recording", "", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut input.max_secs)
                            .range(5..=crate::voice::MAX_RECORD_SECS)
                            .suffix(" s"),
                    );
                });
                setting_row(ui, "Microphone", "", |ui| {
                    mic_picker(ui, &mut input.device, &devices);
                });
            });

            section(ui, "Read aloud");
            let out = &mut voice.output;
            switch(
                ui,
                "Read replies aloud",
                "A speaker button under each reply reads it aloud, and Ctrl/⌘+Shift+S reads the \
                 latest reply or stops the reading. Code blocks and tables are announced rather \
                 than read, and links are read as their text",
                &mut out.enabled,
            );
            ui.add_enabled_ui(out.enabled, |ui| {
                use crate::voice::SpeechEngine;
                let endpoint = out.engine == SpeechEngine::Endpoint;
                choice(
                    ui,
                    "speech-engine",
                    "Spoken by",
                    if endpoint {
                        "Sent to the speech endpoint below as WAV audio, then played on this \
                         computer"
                    } else {
                        "This computer: say on macOS, System.Speech on Windows, spd-say or \
                         espeak-ng on Linux. Nothing leaves the computer"
                    },
                    &mut out.engine,
                    &[
                        (SpeechEngine::OsVoice, SpeechEngine::OsVoice.label()),
                        (SpeechEngine::Endpoint, SpeechEngine::Endpoint.label()),
                    ],
                );
                if endpoint {
                    setting_row(ui, "Provider", "", |ui| {
                        provider_choice(ui, "voice-out-provider", &mut out.provider, &providers);
                    });
                    text_row(ui, "Model", "The speech model", &mut out.model, "");
                    text_row(ui, "Voice", "", &mut out.voice, "");
                }
                setting_row(ui, "Speed", "", |ui| {
                    ui.add(egui::Slider::new(&mut out.speed, 0.5..=2.0).step_by(0.05));
                });
                setting_row(
                    ui,
                    "Longest reply read",
                    "A longer reply is read up to here",
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut out.max_chars)
                                .range(200..=crate::voice::MAX_SPEAK_CHARS)
                                .suffix(" characters"),
                        );
                    },
                );
            });
        });
    }
}

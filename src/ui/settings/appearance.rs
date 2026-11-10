//! The Appearance page of Settings.

use super::*;

impl SettingsUi {
    /// Theme, UI scale, motion and how replies look; each as
    /// one row with the control on the right.
    pub(super) fn appearance_tab(&mut self, ui: &mut egui::Ui) {
        use crate::ui::chrome::{segmented, setting_row};
        let g = &mut self.draft.general;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                setting_row(ui, "Theme", "", |ui| {
                    segmented(
                        ui,
                        &mut g.theme,
                        &[
                            (Theme::Dark, icon::MOON, "Dark"),
                            (Theme::Light, icon::SUN, "Light"),
                            (Theme::System, icon::MONITOR, "Match the system"),
                        ],
                    );
                });
                setting_row(
                    ui,
                    "UI scale",
                    "Everything in the window, text included",
                    |ui| {
                        egui::ComboBox::from_id_salt("ui-scale")
                            .selected_text(format!("{:.0}%", g.zoom * 100.0))
                            .show_ui(ui, |ui| {
                                for z in ZOOM_STEPS {
                                    ui.selectable_value(
                                        &mut g.zoom,
                                        z,
                                        format!("{:.0}%", z * 100.0),
                                    );
                                }
                            });
                    },
                );
                setting_row(
                    ui,
                    "Reduce motion",
                    "No fades, sliding sidebar or zooming dialogs",
                    |ui| {
                        crate::ui::chrome::toggle(ui, &mut g.reduce_motion);
                    },
                );
                setting_row(
                    ui,
                    "Reasoning",
                    "How the model's \"Thinking\" section starts out",
                    |ui| {
                        egui::ComboBox::from_id_salt("reasoning-display")
                            .selected_text(g.reasoning_display.label())
                            .show_ui(ui, |ui| {
                                for mode in crate::config::ReasoningDisplay::ALL {
                                    ui.selectable_value(
                                        &mut g.reasoning_display,
                                        mode,
                                        mode.label(),
                                    );
                                }
                            });
                    },
                );
                setting_row(
                    ui,
                    "Speed stats",
                    "Prefill/decode speed and token counts under responses",
                    |ui| {
                        crate::ui::chrome::toggle(ui, &mut g.show_stats);
                    },
                );
            });
    }
}

//! Small reusable editors for the settings screens.

use eframe::egui;

use crate::config::KeyValue;

/// Editable list of key/value rows (env vars, HTTP headers).
pub fn kv_editor(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    list: &mut Vec<KeyValue>,
    key_hint: &str,
    value_hint: &str,
    mask_values: bool,
) {
    ui.push_id(id_salt, |ui| {
        let mut remove = None;
        for (i, kv) in list.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut kv.key)
                        .hint_text(key_hint)
                        .desired_width(160.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut kv.value)
                        .hint_text(value_hint)
                        .password(mask_values)
                        .desired_width(240.0),
                );
                if icon_button(ui, super::theme::icon::CLOSE, "Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            list.remove(i);
        }
        if ui
            .small_button(format!("{}  Add", super::theme::icon::PLUS))
            .clicked()
        {
            list.push(KeyValue::default());
        }
    });
}

/// Editable ordered list of strings (command-line arguments).
pub fn list_editor(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    list: &mut Vec<String>,
    hint: &str,
) {
    ui.push_id(id_salt, |ui| {
        let mut remove = None;
        let mut swap = None;
        let len = list.len();
        for (i, item) in list.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(item)
                        .hint_text(hint)
                        .desired_width(super::settings::FIELD_WIDTH - 84.0),
                );
                if ui
                    .add_enabled(
                        i > 0,
                        egui::Button::new(super::theme::icon::CARET_UP).small(),
                    )
                    .clicked()
                {
                    swap = Some((i - 1, i));
                }
                if ui
                    .add_enabled(
                        i + 1 < len,
                        egui::Button::new(super::theme::icon::CARET_DOWN).small(),
                    )
                    .clicked()
                {
                    swap = Some((i, i + 1));
                }
                if icon_button(ui, super::theme::icon::CLOSE, "Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some((a, b)) = swap {
            list.swap(a, b);
        }
        if let Some(i) = remove {
            list.remove(i);
        }
        if ui
            .small_button(format!("{}  Add argument", super::theme::icon::PLUS))
            .clicked()
        {
            list.push(String::new());
        }
    });
}

/// A read-only, selectable monospace block.
pub fn code_block(ui: &mut egui::Ui, text: &str) {
    let mut text = text;
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .code_editor()
            .desired_rows(1)
            .desired_width(f32::INFINITY),
    );
}

/// Optional numeric value with an enable checkbox.
pub fn optional_value<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Option<T>,
    default: T,
    range: std::ops::RangeInclusive<T>,
    speed: f64,
) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, "").changed() {
            *value = enabled.then_some(value.unwrap_or(default));
        }
        match value {
            Some(v) => {
                ui.add(egui::DragValue::new(v).range(range).speed(speed));
            }
            None => {
                ui.weak("server default");
            }
        }
    });
}

/// A frameless icon button with a tooltip and an accessible name.
pub fn icon_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let resp = ui
        .add(egui::Button::new(egui::RichText::new(icon).size(16.0)).frame(false))
        .on_hover_text(label);
    let label = label.to_owned();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// Quiet text actions under a message or drawing ("copy", "edit"); returns which was clicked.
pub fn text_actions(ui: &mut egui::Ui, labels: &[&str]) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
        // Line the first label's text up with the column, not its hover frame.
        ui.add_space(-4.0);
        for (i, label) in labels.iter().enumerate() {
            let text = egui::RichText::new(*label)
                .small()
                .color(ui.visuals().weak_text_color());
            if ui
                .add(egui::Button::new(text).frame_when_inactive(false))
                .clicked()
            {
                clicked = Some(i);
            }
        }
    });
    clicked
}

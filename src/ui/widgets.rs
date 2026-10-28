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

/// A unified diff, with added lines in green and removed lines in red; a long one scrolls.
pub fn diff_block(ui: &mut egui::Ui, diff: &str) {
    let p = super::theme::palette(ui.visuals().dark_mode);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    let lines: Vec<&str> = diff.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let color = if line.starts_with("+++") || line.starts_with("---") {
            p.pencil
        } else if line.starts_with('+') {
            p.ok
        } else if line.starts_with('-') {
            p.error
        } else if line.starts_with("@@") {
            p.signal
        } else {
            p.ink2
        };
        let text = if i + 1 < lines.len() {
            format!("{line}\n")
        } else {
            (*line).to_owned()
        };
        job.append(&text, 0.0, egui::TextFormat::simple(font.clone(), color));
    }
    job.wrap.max_width = f32::INFINITY;
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(8)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if lines.len() <= 24 {
                ui.add(egui::Label::new(job).selectable(true).wrap());
                return;
            }
            // A long diff scrolls; its own id keeps it apart from the chat's scroll area.
            let mut h = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(diff, &mut h);
            egui::ScrollArea::vertical()
                .id_salt(("diff", std::hash::Hasher::finish(&h)))
                .min_scrolled_height(360.0)
                .max_height(360.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(egui::Label::new(job).selectable(true).wrap());
                });
        });
}

/// "+3 −1" for a unified diff.
pub fn diff_stat(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(add, del), l| {
        if l.starts_with("+++") || l.starts_with("---") {
            (add, del)
        } else if l.starts_with('+') {
            (add + 1, del)
        } else if l.starts_with('-') {
            (add, del + 1)
        } else {
            (add, del)
        }
    })
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

/// Muted icon buttons under a message or drawing (copy, edit, retry), each with a tooltip
/// and an accessible name; returns which was clicked.
pub fn quiet_icons(ui: &mut egui::Ui, items: &[(&str, &str)]) -> Option<usize> {
    let pencil = super::theme::palette(ui.visuals().dark_mode).pencil;
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (glyph, label)) in items.iter().enumerate() {
            let resp = ui
                .add(
                    egui::Button::new(egui::RichText::new(*glyph).size(15.0).color(pencil))
                        .frame_when_inactive(false),
                )
                .on_hover_text(*label);
            let label = (*label).to_owned();
            resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// A number that is either set or left to `inherited` (the provider's or model's), or to
/// the server when nothing sets it.
fn inherited_value<T: egui::emath::Numeric + std::fmt::Display>(
    ui: &mut egui::Ui,
    value: &mut Option<T>,
    inherited: Option<T>,
    default: T,
    range: std::ops::RangeInclusive<T>,
    speed: f64,
) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, "").changed() {
            *value = enabled.then_some(value.or(inherited).unwrap_or(default));
        }
        match value {
            Some(v) => {
                ui.add(egui::DragValue::new(v).range(range).speed(speed));
            }
            None => {
                ui.weak(match inherited {
                    Some(v) => format!("default ({v})"),
                    None => "server default".into(),
                });
            }
        }
    });
}

/// Rows for reasoning and sampling options. `inherited` is what applies when an option is
/// left unset here (the provider's options under a model's, or both under a chat's).
pub fn model_options_grid(
    ui: &mut egui::Ui,
    id_salt: &str,
    o: &mut crate::model_options::ModelOptions,
    inherited: &crate::model_options::ModelOptions,
    api: crate::config::ProviderApi,
) {
    use crate::config::ProviderApi;
    use crate::model_options::ReasoningEffort;
    egui::Grid::new(id_salt)
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Reasoning effort").on_hover_text(
                "How hard a reasoning model thinks. Off turns thinking off where the server \
                 can. Models that don't reason may reject it.",
            );
            let default_label = match inherited.reasoning_effort {
                Some(e) => format!("Default ({})", e.label()),
                None => "Server default".into(),
            };
            egui::ComboBox::from_id_salt((id_salt, "effort"))
                .selected_text(
                    o.reasoning_effort
                        .map_or(default_label.clone(), |e| e.label().to_owned()),
                )
                .width(150.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut o.reasoning_effort, None, default_label);
                    for e in ReasoningEffort::ALL {
                        ui.selectable_value(&mut o.reasoning_effort, Some(e), e.label());
                    }
                });
            ui.end_row();

            ui.label("Thinking budget").on_hover_text(
                "Tokens the model may think for: Anthropic, OpenRouter, LiteLLM and servers \
                 that take `thinking`. Without one, Anthropic derives it from the effort. \
                 The Responses API, llama.cpp and vLLM use the effort only.",
            );
            ui.add_enabled_ui(api != ProviderApi::Responses, |ui| {
                inherited_value(
                    ui,
                    &mut o.thinking_budget,
                    inherited.thinking_budget,
                    4096,
                    1024..=128_000,
                    64.0,
                );
            });
            ui.end_row();

            ui.label("Temperature");
            inherited_value(
                ui,
                &mut o.temperature,
                inherited.temperature,
                0.7,
                0.0..=2.0,
                0.01,
            );
            ui.end_row();

            ui.label("Top-p");
            inherited_value(ui, &mut o.top_p, inherited.top_p, 1.0, 0.0..=1.0, 0.01);
            ui.end_row();

            ui.label("Max tokens");
            inherited_value(
                ui,
                &mut o.max_tokens,
                inherited.max_tokens,
                4096,
                1..=1_000_000,
                16.0,
            );
            ui.end_row();

            ui.label("Stop sequences").on_hover_text(
                "Generation stops at any of these, one per line. Not supported by the \
                 Responses API.",
            );
            ui.add_enabled_ui(api != ProviderApi::Responses, |ui| {
                let mut text = o.stop.join("\n");
                let hint = if inherited.stops().is_empty() {
                    "One per line".to_owned()
                } else {
                    format!("Default: {}", inherited.stops().join(", "))
                };
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut text)
                            .desired_rows(2)
                            .desired_width(220.0)
                            .hint_text(hint),
                    )
                    .changed()
                {
                    o.stop = if text.is_empty() {
                        Vec::new()
                    } else {
                        text.split('\n').map(str::to_owned).collect()
                    };
                }
            });
            ui.end_row();
        });
}

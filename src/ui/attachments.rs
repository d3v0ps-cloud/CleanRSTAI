//! Rendering of image and text-file attachments.

use std::collections::HashMap;

use eframe::egui;

use super::theme::icon;
use super::widgets::code_block;
use crate::attach::Attachment;

/// Decoded image bytes by URI, so base64 is decoded once rather than every frame.
pub type ImageCache = HashMap<String, egui::load::Bytes>;

/// egui caches decoded images by URI, so the URI must change when the content does
/// (e.g. a message edited and resent with a different image at the same position).
fn uri(key: &str, a: &Attachment) -> String {
    let ext = a.mime.rsplit('/').next().unwrap_or("png");
    let data = a.data.as_deref().unwrap_or("");
    let edge = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    let head = edge(&data[..data.len().min(16)]);
    let tail = edge(&data[data.len().saturating_sub(16)..]);
    format!("bytes://{key}-{}-{head}{tail}.{ext}", data.len())
}

fn image_source(cache: &mut ImageCache, key: &str, a: &Attachment) -> (String, egui::load::Bytes) {
    let uri = uri(key, a);
    let bytes = cache
        .entry(uri.clone())
        .or_insert_with(|| a.bytes().unwrap_or_default().into())
        .clone();
    (uri, bytes)
}

/// Show a message's attachments. Returns the URI of an image the user clicked, if any.
pub fn show(
    ui: &mut egui::Ui,
    cache: &mut ImageCache,
    key: &str,
    list: &[Attachment],
) -> Option<(String, egui::load::Bytes, String)> {
    let mut clicked = None;
    // One per row: inside a horizontal layout an image is squeezed to the row height.
    for (n, a) in list.iter().enumerate().filter(|(_, a)| a.is_image()) {
        let (uri, bytes) = image_source(cache, &format!("{key}-{n}"), a);
        let resp = ui
            .add(
                egui::Image::from_bytes(uri.clone(), bytes.clone())
                    .fit_to_original_size(1.0)
                    .max_height(220.0)
                    .max_width(320.0_f32.min(ui.available_width()))
                    .corner_radius(6)
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(format!(
                "{} · {} · click to enlarge",
                a.name,
                a.size_label()
            ));
        if resp.clicked() {
            clicked = Some((uri, bytes, a.name.clone()));
        }
    }
    for (n, a) in list.iter().enumerate() {
        if let Some(text) = &a.text {
            egui::CollapsingHeader::new(format!("{}  {} · {}", icon::FILE, a.name, a.size_label()))
                .id_salt((key, "file", n))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| code_block(ui, text));
                });
        }
    }
    clicked
}

/// Chips for attachments waiting to be sent. Returns the index the user removed.
pub fn pending_chips(
    ui: &mut egui::Ui,
    cache: &mut ImageCache,
    list: &[Attachment],
) -> Option<usize> {
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (n, a) in list.iter().enumerate() {
            egui::Frame::group(ui.style())
                .inner_margin(4)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if a.is_image() {
                            let (uri, bytes) =
                                image_source(cache, &format!("pending-{n}-{}", a.name), a);
                            ui.add(
                                egui::Image::from_bytes(uri, bytes)
                                    .max_height(40.0)
                                    .max_width(60.0),
                            );
                        } else {
                            ui.label(icon::FILE);
                        }
                        ui.label(egui::RichText::new(&a.name).small())
                            .on_hover_text(a.size_label());
                        if super::widgets::icon_button(ui, icon::CLOSE, "Remove attachment")
                            .clicked()
                        {
                            remove = Some(n);
                        }
                    });
                });
        }
    });
    remove
}

/// Full-size image viewer. Returns false when closed.
pub fn viewer(ctx: &egui::Context, image: &(String, egui::load::Bytes, String)) -> bool {
    let mut open = true;
    egui::Window::new(image.2.as_str())
        .id(egui::Id::unique("image-viewer"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([720.0, 540.0])
        .show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add(egui::Image::from_bytes(image.0.clone(), image.1.clone()).shrink_to_fit());
            });
        });
    open && !ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

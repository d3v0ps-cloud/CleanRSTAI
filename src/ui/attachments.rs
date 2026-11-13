//! Rendering of image and text-file attachments.

use std::collections::HashMap;

use eframe::egui;

use super::theme::icon;
use super::widgets::code_block;
use crate::attach::Attachment;

/// Images kept before the least recently used are dropped, by count and by decoded bytes.
const MAX_CACHED_IMAGES: usize = 256;
const MAX_CACHED_BYTES: usize = 256 * 1024 * 1024;

/// Decoded image bytes by URI, so base64 is decoded once rather than every frame. When it grows
/// too large the least recently used entries go, together with egui's own copies.
#[derive(Default)]
pub struct ImageCache {
    /// Bytes and the frame each was last used in.
    images: HashMap<String, (egui::load::Bytes, u64)>,
    total: usize,
}

/// Which entries (uri, last-used frame, size) to drop so one more of `incoming` bytes fits:
/// oldest first, never one used in `frame`. If only current ones remain, the budget is exceeded
/// rather than thrashed.
fn evict(
    entries: &[(&str, u64, usize)],
    frame: u64,
    incoming: usize,
    max_count: usize,
    max_bytes: usize,
) -> Vec<String> {
    let mut count = entries.len();
    let mut total: usize = entries.iter().map(|e| e.2).sum();
    let mut old: Vec<_> = entries.iter().filter(|e| e.1 != frame).collect();
    old.sort_by_key(|e| e.1);
    let mut out = Vec::new();
    for (uri, _, size) in old {
        if count < max_count && total + incoming <= max_bytes {
            break;
        }
        out.push((*uri).to_owned());
        count -= 1;
        total -= size;
    }
    out
}

impl ImageCache {
    fn get(&mut self, ctx: &egui::Context, uri: &str, a: &Attachment) -> egui::load::Bytes {
        let frame = ctx.cumulative_frame_nr();
        if let Some((b, used)) = self.images.get_mut(uri) {
            *used = frame;
            return b.clone();
        }
        let bytes: egui::load::Bytes = a.bytes().unwrap_or_default().into();
        let entries: Vec<_> = self
            .images
            .iter()
            .map(|(k, (b, used))| (k.as_str(), *used, b.len()))
            .collect();
        for gone in evict(
            &entries,
            frame,
            bytes.len(),
            MAX_CACHED_IMAGES,
            MAX_CACHED_BYTES,
        ) {
            ctx.forget_image(&gone);
            if let Some((b, _)) = self.images.remove(&gone) {
                self.total -= b.len();
            }
        }
        self.total += bytes.len();
        self.images.insert(uri.to_owned(), (bytes.clone(), frame));
        bytes
    }
}

/// egui caches decoded images by URI, so the URI must change when the content does
/// (e.g. a message edited and resent with a different image at the same position).
fn uri(key: &str, a: &Attachment) -> String {
    let ext = a.mime.rsplit('/').next().unwrap_or("png");
    format!("bytes://{key}-{}.{ext}", a.content_tag())
}

fn image_source(
    ui: &egui::Ui,
    cache: &mut ImageCache,
    key: &str,
    a: &Attachment,
) -> (String, egui::load::Bytes) {
    let uri = uri(key, a);
    let bytes = cache.get(ui.ctx(), &uri, a);
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
        let (uri, bytes) = image_source(ui, cache, &format!("{key}-{n}"), a);
        let resp = ui
            .add(
                egui::Image::from_bytes(uri.clone(), bytes.clone())
                    .fit_to_original_size(1.0)
                    .max_height(220.0)
                    .max_width(320.0_f32.min(ui.available_width()))
                    .corner_radius(super::theme::radius::MD)
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
            let id = ui.make_persistent_id((key, "file", n));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    // Truncated, so a long file name can't push the message past the window.
                    ui.add(egui::Label::new(file_label(a)).truncate())
                        .on_hover_text(file_label(a));
                })
                .body(|ui| {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| code_block(ui, text));
                });
        }
    }
    clicked
}

/// "📄  report.pdf · PDF · 7 lines", the header of a text or document attachment.
pub fn file_label(a: &Attachment) -> String {
    format!("{}  {} · {}", icon::FILE, a.name, a.size_label())
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
                                image_source(ui, cache, &format!("pending-{n}-{}", a.name), a);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_drops_the_least_recently_used_first() {
        let e = [("a", 5, 10), ("b", 1, 10), ("c", 3, 10)];
        // Room for a fourth by count: nothing goes.
        assert!(evict(&e, 9, 10, 4, 1000).is_empty());
        // Full by count: the oldest goes.
        assert_eq!(evict(&e, 9, 10, 3, 1000), ["b"]);
        // Full by bytes: enough go, oldest first.
        assert_eq!(evict(&e, 9, 25, 10, 40), ["b", "c"]);
    }

    #[test]
    fn eviction_spares_entries_used_this_frame() {
        let e = [("a", 7, 10), ("b", 7, 10), ("c", 2, 10)];
        assert_eq!(evict(&e, 7, 10, 3, 1000), ["c"]);
        // Everything is current: go over the budget instead of thrashing.
        let all = [("a", 7, 10), ("b", 7, 10)];
        assert!(evict(&all, 7, 10, 2, 15).is_empty());
    }
}

//! The window's low-chrome building blocks: sidebar and settings navigation rows, the recents
//! rows that fade out on the right, segmented toggles, text-style strip buttons, settings rows
//! (label on the left, control on the right, hairline between), cards and the brand mark.
//! Each draws a faint fill on hover that fades in over the style's animation time (none with
//! "Reduce motion" on), and the app's focus ring shows keyboard focus.

use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

use super::theme;

/// The vertical pitch of sidebar rows: 32px tall plus 2px between.
pub const ROW_HEIGHT: f32 = 32.0;
pub const ROW_GAP: f32 = 2.0;

/// How hovered (or keyboard-focused) a widget is, from 0 to 1, eased over the style's
/// animation time.
pub fn hover_amount(ui: &egui::Ui, resp: &egui::Response) -> f32 {
    ui.ctx()
        .animate_bool(resp.id.with("hover"), resp.hovered() || resp.has_focus())
}

/// `top` (which may be translucent) composited over the opaque `bottom`.
pub fn over(top: Color32, bottom: Color32) -> Color32 {
    let a = f32::from(top.a()) / 255.0;
    let mix = |t: u8, b: u8| (f32::from(t) + f32::from(b) * (1.0 - a)).round().min(255.0) as u8;
    Color32::from_rgb(
        mix(top.r(), bottom.r()),
        mix(top.g(), bottom.g()),
        mix(top.b(), bottom.b()),
    )
}

/// A navigation row: an icon and a label, `ink2` until hovered or active. `filled` gives it a
/// persistent fill (the primary "New" row).
pub fn nav_row(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    active: bool,
    filled: bool,
) -> egui::Response {
    let p = theme::palette(ui.visuals().dark_mode);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let name = label.to_owned();
    resp.widget_info(move || egui::WidgetInfo::selected(egui::Role::Button, true, active, &name));
    if ui.is_rect_visible(rect) {
        let t = hover_amount(ui, &resp);
        let painter = ui.painter();
        let base = if active {
            p.active
        } else if filled {
            // About 8% white (or black in the light theme).
            p.active.gamma_multiply(0.8)
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, 8, base);
        if t > 0.0 && !active {
            painter.rect_filled(rect, 8, p.hover.gamma_multiply(t));
        }
        let color = if active || filled {
            p.ink
        } else {
            p.ink2.lerp_to_gamma(p.ink, t)
        };
        painter.text(
            pos2(rect.left() + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            egui::FontId::proportional(17.0),
            color,
        );
        painter.text(
            pos2(rect.left() + 36.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            color,
        );
    }
    resp
}

/// A recents row: plain text that truncates with an ellipsis and fades out on the right, and
/// an optional status mark at the right edge. `surface` is the colour underneath, for the fade.
pub fn recent_row(
    ui: &mut egui::Ui,
    title: &str,
    active: bool,
    mark: Option<(&str, Color32)>,
    surface: Color32,
) -> egui::Response {
    let p = theme::palette(ui.visuals().dark_mode);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let name = title.to_owned();
    resp.widget_info(move || egui::WidgetInfo::selected(egui::Role::Button, true, active, &name));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let t = hover_amount(ui, &resp);
    let painter = ui.painter();
    let fill = if active {
        p.active
    } else {
        p.hover.gamma_multiply(t)
    };
    painter.rect_filled(rect, 8, fill);
    let mark_w = if mark.is_some() { 22.0 } else { 0.0 };
    let text_left = rect.left() + 10.0;
    let text_right = rect.right() - 8.0 - mark_w;
    let color = if active {
        p.ink
    } else {
        p.ink2.lerp_to_gamma(p.ink, t)
    };
    let mut job = egui::text::LayoutJob::single_section(
        title.to_owned(),
        egui::TextFormat::simple(egui::FontId::proportional(14.0), color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - text_left).max(10.0));
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let elided = galley.elided;
    let text_pos = pos2(text_left, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley, color);
    if elided {
        // Fade the last few pixels of a cut-off title into the row's background.
        let bg = over(fill, surface);
        let fade = Rect::from_min_max(
            pos2(text_right - 24.0, rect.top()),
            pos2(text_right, rect.bottom()),
        );
        fade_right(painter, fade, bg);
    }
    if let Some((glyph, tint)) = mark {
        painter.text(
            pos2(rect.right() - 10.0 - 7.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(13.0),
            tint,
        );
    }
    resp
}

/// A horizontal gradient from transparent to `bg` over `rect`.
fn fade_right(painter: &egui::Painter, rect: Rect, bg: Color32) {
    let mut mesh = egui::Mesh::default();
    let clear = bg.gamma_multiply(0.0);
    mesh.colored_vertex(rect.left_top(), clear);
    mesh.colored_vertex(rect.right_top(), bg);
    mesh.colored_vertex(rect.right_bottom(), bg);
    mesh.colored_vertex(rect.left_bottom(), clear);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// A small muted section label, e.g. "Recents" or "Preferences".
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    let p = theme::palette(ui.visuals().dark_mode);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(text).size(12.0).color(p.pencil));
    });
    ui.add_space(2.0);
}

/// A collapsible section header: the label and a tiny chevron. Returns true when clicked.
pub fn disclosure(ui: &mut egui::Ui, text: &str, open: bool) -> egui::Response {
    let p = theme::palette(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let name = text.to_owned();
    resp.widget_info(move || egui::WidgetInfo::selected(egui::Role::Button, true, open, &name));
    let t = hover_amount(ui, &resp);
    let color = p.pencil.lerp_to_gamma(p.ink2, t);
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), color);
    let pos = pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0);
    let width = galley.size().x;
    painter.galley(pos, galley, color);
    let chevron = if open {
        theme::icon::CARET_DOWN
    } else {
        theme::icon::CARET_RIGHT
    };
    painter.text(
        pos2(rect.left() + 10.0 + width + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        chevron,
        egui::FontId::proportional(10.0),
        color,
    );
    resp
}

/// An icon button with no frame until hovered, in the secondary text colour.
pub fn ghost_icon(ui: &mut egui::Ui, icon: &str, hover: &str, on: bool) -> egui::Response {
    let p = theme::palette(ui.visuals().dark_mode);
    let color = if on { p.signal } else { p.ink2 };
    let resp = ui
        .add(
            egui::Button::new(RichText::new(icon).size(17.0).color(color))
                .frame_when_inactive(on)
                .fill(if on { p.active } else { Color32::TRANSPARENT })
                .min_size(vec2(30.0, 30.0)),
        )
        .on_hover_text(hover);
    let label = hover.to_owned();
    resp.widget_info(move || egui::WidgetInfo::selected(egui::Role::Button, true, on, &label));
    resp
}

/// A segmented toggle: icon segments in a hairline frame, the selected one on a filled pill.
/// Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str, &str)],
) -> bool {
    let p = theme::palette(ui.visuals().dark_mode);
    let mut changed = false;
    egui::Frame::new()
        .stroke(Stroke::new(1.0, p.hairline))
        .corner_radius(8)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, glyph, tip) in options {
                    let on = *value == *option;
                    let resp = ui
                        .add(
                            egui::Button::new(RichText::new(*glyph).size(15.0).color(if on {
                                p.ink
                            } else {
                                p.pencil
                            }))
                            .frame_when_inactive(on)
                            .fill(if on { p.active } else { Color32::TRANSPARENT })
                            .stroke(Stroke::NONE)
                            .corner_radius(6)
                            .min_size(vec2(30.0, 24.0)),
                        )
                        .on_hover_text(*tip);
                    let label = (*tip).to_owned();
                    resp.widget_info(move || {
                        egui::WidgetInfo::selected(egui::Role::Button, true, on, &label)
                    });
                    if resp.clicked() && !on {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// One settings row, about 53px: the label (and a muted description under it) on the left,
/// the control on the right, a hairline underneath.
pub fn setting_row(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    let p = theme::palette(ui.visuals().dark_mode);
    let width = ui.available_width();
    let height = if description.is_empty() { 53.0 } else { 60.0 };
    ui.allocate_ui_with_layout(
        vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_height(height);
            ui.vertical(|ui| {
                ui.set_max_width(width * 0.58);
                ui.spacing_mut().item_spacing.y = 2.0;
                // Centre the label (and description) on the row.
                let text = if description.is_empty() { 18.0 } else { 38.0 };
                ui.add_space(((height - text) / 2.0).max(0.0));
                ui.label(RichText::new(label).size(14.0).color(p.ink));
                if !description.is_empty() {
                    ui.label(RichText::new(description).size(12.5).color(p.pencil));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
        },
    );
    let y = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.hairline));
    ui.add_space(1.0);
}

/// A card's frame: slightly raised, hairline border that firms up on hover, 12px corners.
pub fn card_frame(ui: &egui::Ui, rect: Rect, hover: f32) {
    let p = theme::palette(ui.visuals().dark_mode);
    let border = p.hairline.lerp_to_gamma(p.outline, hover);
    let fill = over(p.hover.gamma_multiply(hover), p.raised);
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        fill,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
}

/// Paints the brand mark into `rect`.
pub fn paint_brand_mark(painter: &egui::Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let body = Rect::from_center_size(c, Vec2::splat(s * 0.62));
    let stroke = Stroke::new((s * 0.085).max(1.5), color);
    painter.rect_stroke(
        body,
        CornerRadius::same((s * 0.14) as u8),
        stroke,
        egui::StrokeKind::Middle,
    );
    // The core.
    painter.rect_filled(
        Rect::from_center_size(c, Vec2::splat(s * 0.26)),
        CornerRadius::same((s * 0.05) as u8),
        color,
    );
    // Three pins on each side.
    let pin = s * 0.13;
    for i in [-1.0, 0.0, 1.0] {
        let off = i * s * 0.17;
        for (from, dir) in [
            (pos2(c.x + off, body.top()), vec2(0.0, -pin)),
            (pos2(c.x + off, body.bottom()), vec2(0.0, pin)),
            (pos2(body.left(), c.y + off), vec2(-pin, 0.0)),
            (pos2(body.right(), c.y + off), vec2(pin, 0.0)),
        ] {
            painter.line_segment([from, from + dir], stroke);
        }
    }
}

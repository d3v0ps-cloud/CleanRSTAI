//! Visual design: icon font, colours, spacing and type scale for light and dark mode.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle};

/// Phosphor icon codepoints used in the app (font in `assets/fonts`, MIT licensed).
pub mod icon {
    pub const PLUS: &str = "\u{E3D4}";
    pub const GEAR: &str = "\u{E272}";
    pub const CHAT: &str = "\u{E16E}";
    pub const SEARCH: &str = "\u{E30C}";
    pub const PAPERCLIP: &str = "\u{E39A}";
    pub const SEND: &str = "\u{E396}";
    pub const STOP: &str = "\u{E46C}";
    pub const COPY: &str = "\u{E1CA}";
    pub const EDIT: &str = "\u{E3B4}";
    pub const REGENERATE: &str = "\u{E036}";
    pub const TRASH: &str = "\u{E4A6}";
    pub const PIN: &str = "\u{E3E2}";
    pub const UNPIN: &str = "\u{E3E4}";
    pub const DOWNLOAD: &str = "\u{E20C}";
    pub const SIDEBAR: &str = "\u{EC24}";
    pub const TOOL: &str = "\u{E5D4}";
    pub const OK: &str = "\u{E184}";
    pub const FAILED: &str = "\u{E4F8}";
    pub const CLOSE: &str = "\u{E4F6}";
    pub const CARET_UP: &str = "\u{E13C}";
    pub const CARET_DOWN: &str = "\u{E136}";
    pub const BOOKS: &str = "\u{E758}";
    pub const LIGHTNING: &str = "\u{E2DE}";
    pub const BRAIN: &str = "\u{E74E}";
    pub const COMMAND: &str = "\u{E1C4}";
    pub const PLUGS: &str = "\u{EB56}";
    pub const SIGN_IN: &str = "\u{E428}";
    pub const FILE: &str = "\u{E23A}";
    pub const SUN: &str = "\u{E472}";
    pub const MOON: &str = "\u{E330}";
    pub const MONITOR: &str = "\u{E32E}";
    pub const KEY: &str = "\u{E2D6}";
    pub const EXPORT: &str = "\u{EAF0}";
    pub const HOURGLASS: &str = "\u{E2B2}";
    pub const WARNING: &str = "\u{E4E0}";
    pub const SERVER: &str = "\u{E47E}";
    pub const SPARKLE: &str = "\u{E6A2}";
    pub const LIST_CHECKS: &str = "\u{EADC}";
    pub const CHECK_SQUARE: &str = "\u{E186}";
    pub const SQUARE: &str = "\u{E45E}";
    pub const ARROW_RIGHT: &str = "\u{E06C}";
    pub const ROBOT: &str = "\u{E762}";
    pub const TOOLBOX: &str = "\u{ECA0}";
    pub const SLIDERS: &str = "\u{E434}";
    pub const CLOCK: &str = "\u{E19A}";
    pub const CALCULATOR: &str = "\u{E538}";
}

/// Accent colours: indigo, tuned for contrast in each mode.
pub fn accent(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(129, 140, 248)
    } else {
        Color32::from_rgb(79, 70, 229)
    }
}

pub fn success(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(74, 222, 128)
    } else {
        Color32::from_rgb(22, 163, 74)
    }
}

/// Background of the user's message bubbles.
pub fn user_bubble(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(39, 41, 56)
    } else {
        Color32::from_rgb(234, 236, 251)
    }
}

/// Background of cards (tool calls, code, settings groups).
pub fn card(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(28, 29, 35)
    } else {
        Color32::from_rgb(246, 247, 250)
    }
}

pub fn border(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(48, 50, 60)
    } else {
        Color32::from_rgb(222, 224, 232)
    }
}

/// Load the icon font as a fallback for every family, so icons mix with text.
pub fn install_fonts(ctx: &egui::Context) {
    ctx.add_font(egui::epaint::text::FontInsert::new(
        "phosphor",
        egui::FontData::from_static(include_bytes!("../../assets/fonts/Phosphor.ttf")),
        vec![
            egui::epaint::text::InsertFontFamily {
                family: FontFamily::Proportional,
                priority: egui::epaint::text::FontPriority::Lowest,
            },
            egui::epaint::text::InsertFontFamily {
                family: FontFamily::Monospace,
                priority: egui::epaint::text::FontPriority::Lowest,
            },
        ],
    ));
}

/// Apply spacing, type scale and palette to both the light and dark styles.
pub fn apply(ctx: &egui::Context) {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            let dark = theme == egui::Theme::Dark;
            style.text_styles = [
                (TextStyle::Heading, FontId::proportional(20.0)),
                (TextStyle::Body, FontId::proportional(14.5)),
                (TextStyle::Button, FontId::proportional(14.0)),
                (TextStyle::Small, FontId::proportional(11.5)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();

            let s = &mut style.spacing;
            s.item_spacing = egui::vec2(8.0, 7.0);
            s.button_padding = egui::vec2(10.0, 5.0);
            s.interact_size.y = 26.0;
            s.window_margin = Margin::same(14);
            s.menu_margin = Margin::same(6);
            s.indent = 16.0;

            let v = &mut style.visuals;
            let (bg, panel, extreme, faint, text) = if dark {
                (
                    Color32::from_rgb(22, 23, 28),
                    Color32::from_rgb(18, 19, 23),
                    Color32::from_rgb(13, 14, 17),
                    Color32::from_rgb(30, 31, 38),
                    Color32::from_rgb(229, 231, 238),
                )
            } else {
                (
                    Color32::from_rgb(255, 255, 255),
                    Color32::from_rgb(248, 248, 251),
                    Color32::from_rgb(255, 255, 255),
                    Color32::from_rgb(241, 242, 246),
                    Color32::from_rgb(28, 29, 36),
                )
            };
            let accent = accent(dark);
            v.panel_fill = bg;
            v.window_fill = bg;
            v.extreme_bg_color = extreme;
            v.faint_bg_color = faint;
            v.code_bg_color = card(dark);
            v.window_stroke = Stroke::new(1.0, border(dark));
            v.window_corner_radius = CornerRadius::same(12);
            v.menu_corner_radius = CornerRadius::same(8);
            v.hyperlink_color = accent;
            v.selection.bg_fill = accent.gamma_multiply(if dark { 0.35 } else { 0.25 });
            v.selection.stroke = Stroke::new(1.0, accent);
            v.override_text_color = None;
            v.collapsing_header_frame = false;
            v.indent_has_left_vline = false;
            let _ = panel;

            let radius = CornerRadius::same(7);
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = radius;
            }
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border(dark));
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
            v.widgets.inactive.weak_bg_fill = faint;
            // Checkboxes/radios use bg_fill; buttons use weak_bg_fill. Keep boxes visible.
            v.widgets.inactive.bg_fill = border(dark);
            v.widgets.inactive.bg_stroke = Stroke::NONE;
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, text);
            v.widgets.hovered.weak_bg_fill = faint.lerp_to_gamma(accent, 0.12);
            v.widgets.hovered.bg_fill = faint.lerp_to_gamma(accent, 0.12);
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.6));
            v.widgets.hovered.fg_stroke = Stroke::new(1.5, text);
            v.widgets.hovered.expansion = 0.0;
            v.widgets.active.weak_bg_fill = faint.lerp_to_gamma(accent, 0.25);
            v.widgets.active.bg_fill = faint.lerp_to_gamma(accent, 0.25);
            v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
            v.widgets.active.expansion = 0.0;
            v.widgets.open.weak_bg_fill = faint.lerp_to_gamma(accent, 0.18);
        });
    }
}

/// A primary (accent-filled) button.
pub fn primary_button<'a>(text: impl Into<egui::WidgetText>, dark: bool) -> egui::Button<'a> {
    egui::Button::new(text.into().color(Color32::WHITE))
        .fill(accent(dark).gamma_multiply(if dark { 0.85 } else { 1.0 }))
}

/// Text with a leading icon, e.g. `labelled(icon::PLUS, "New chat")`.
pub fn labelled(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

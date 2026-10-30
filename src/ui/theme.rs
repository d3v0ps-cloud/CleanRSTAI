//! Visual design: dark first, flat and quiet. Near-black neutral greys (or near-white ones in
//! the light theme) carry the hierarchy, with one warm orange accent kept mostly for the brand
//! glyph. Instrument Sans for the interface and the model's replies, Newsreader (a serif) for
//! the greeting and empty-state headlines, IBM Plex Mono for code. 8px corners on rows and
//! buttons, 12px on cards, 14px on the composer and modals; hairline borders, no shadows.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
};

/// Phosphor icon codepoints used in the app (font in `assets/fonts`, MIT licensed).
pub mod icon {
    pub const PLUS: &str = "\u{E3D4}";
    pub const GEAR: &str = "\u{E272}";
    pub const CHAT: &str = "\u{E16E}";
    pub const SEARCH: &str = "\u{E30C}";
    pub const PAPERCLIP: &str = "\u{E39A}";
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
    pub const COMMAND: &str = "\u{E1C4}";
    pub const PLUGS: &str = "\u{EB56}";
    pub const SIGN_IN: &str = "\u{E428}";
    pub const FILE: &str = "\u{E23A}";
    pub const SUN: &str = "\u{E472}";
    pub const MOON: &str = "\u{E330}";
    pub const MONITOR: &str = "\u{E32E}";
    pub const EXPORT: &str = "\u{EAF0}";
    pub const HOURGLASS: &str = "\u{E2B2}";
    pub const WARNING: &str = "\u{E4E0}";
    pub const SERVER: &str = "\u{E47E}";
    pub const INFO: &str = "\u{E2CE}";
    pub const SPARKLE: &str = "\u{E6A2}";
    pub const COPY: &str = "\u{E1CA}";
    pub const STOP: &str = "\u{E46C}";
    pub const SLIDERS: &str = "\u{E434}";
    pub const ARROW_UP: &str = "\u{E08E}";
    pub const CHECK_SQUARE: &str = "\u{E186}";
    pub const SQUARE: &str = "\u{E45E}";
    pub const ARROW_RIGHT: &str = "\u{E06C}";
    pub const LIST_CHECKS: &str = "\u{EADC}";
    pub const ROBOT: &str = "\u{E762}";
    pub const TOOLBOX: &str = "\u{ECA0}";
    pub const CLOCK: &str = "\u{E19A}";
    pub const CALCULATOR: &str = "\u{E538}";
    pub const GLOBE: &str = "\u{E288}";
    pub const FOLDER: &str = "\u{E24A}";
    pub const TERMINAL: &str = "\u{EAE8}";
    pub const CODE: &str = "\u{E1BC}";
    pub const ARROW_SQUARE_OUT: &str = "\u{E5DE}";
    pub const CARET_LEFT: &str = "\u{E138}";
    pub const CARET_RIGHT: &str = "\u{E13A}";
    pub const IMAGE: &str = "\u{E2CA}";
    pub const FLOW: &str = "\u{E67C}";
    pub const UNDO: &str = "\u{E038}";
    pub const STACK: &str = "\u{E466}";
    pub const SHIELD: &str = "\u{E40A}";
    pub const SHIELD_CHECK: &str = "\u{E40C}";
    pub const LIGHTNING: &str = "\u{E2DE}";
    pub const EYE: &str = "\u{E220}";
    pub const MICROPHONE: &str = "\u{E326}";
    pub const SPEAKER: &str = "\u{E44A}";
    pub const GHOST: &str = "\u{E62A}";
    pub const CLOUD: &str = "\u{E1AA}";
    pub const WAVEFORM: &str = "\u{E802}";
    pub const CALENDAR: &str = "\u{E7B4}";
    pub const FOLDERS: &str = "\u{E260}";
    pub const SHAPES: &str = "\u{EC5E}";
    pub const PALETTE: &str = "\u{E6C8}";
    pub const SLIDERS_H: &str = "\u{E434}";
    pub const CHAT_CIRCLE: &str = "\u{E168}";
    pub const LAPTOP: &str = "\u{E586}";
    pub const KEYBOARD: &str = "\u{E2D8}";
    pub const DOTS: &str = "\u{E1FE}";
}

/// The serif display face (Newsreader), for the greeting and empty-state headlines only.
pub fn serif(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("serif".into()))
}

/// The colours of one mode. Every text colour (ink to error) reads at WCAG AA contrast,
/// 4.5:1, on `paper`, `shelf` and `raised`; the tests check it.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Main background.
    pub paper: Color32,
    /// The sidebar.
    pub shelf: Color32,
    /// Raised surfaces: the composer, your messages, code.
    pub raised: Color32,
    /// The selected chat, the "New chat" row.
    pub selected: Color32,
    /// Primary text.
    pub ink: Color32,
    /// Secondary text: chat titles in the sidebar.
    pub ink2: Color32,
    /// Muted text: labels, hints, metadata.
    pub pencil: Color32,
    /// Dividers.
    pub rule: Color32,
    /// Outlines of inputs and cards.
    pub outline: Color32,
    /// The accent: icons that need a touch of colour, the send button, links.
    pub signal: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
    /// A faint fill under hovered rows, buttons and cards (over any surface).
    pub hover: Color32,
    /// The fill of the active row or segment (over any surface).
    pub active: Color32,
    /// Hairline borders: dividers, the composer and cards at rest.
    pub hairline: Color32,
}

/// The window (and taskbar/dock) icon for the light or dark theme, from `assets/icon` (made by
/// `scripts/make-icons.py`).
pub fn window_icon(dark: bool) -> std::sync::Arc<egui::IconData> {
    let png: &[u8] = if dark {
        include_bytes!("../../assets/icon/window-dark.png")
    } else {
        include_bytes!("../../assets/icon/window-light.png")
    };
    std::sync::Arc::new(
        eframe::icon_data::from_png_bytes(png).expect("bundled icon is a valid PNG"),
    )
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            paper: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            shelf: Color32::from_rgb(0x1E, 0x1E, 0x1E),
            raised: Color32::from_rgb(0x2A, 0x2A, 0x2A),
            selected: Color32::from_rgb(0x35, 0x35, 0x35),
            ink: Color32::from_rgb(0xEC, 0xEC, 0xEC),
            ink2: Color32::from_rgb(0xA1, 0xA1, 0xA1),
            // The spec's #6f6f6f tertiary reads at about 3:1 on the composer; this keeps
            // placeholders and hints at AA.
            pencil: Color32::from_rgb(0x94, 0x94, 0x94),
            rule: Color32::from_rgb(0x2E, 0x2E, 0x2E),
            outline: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            signal: Color32::from_rgb(0xD9, 0x77, 0x57),
            ok: Color32::from_rgb(0x8F, 0xB0, 0x78),
            warn: Color32::from_rgb(0xD9, 0xA8, 0x5B),
            error: Color32::from_rgb(0xE0, 0x7A, 0x6A),
            hover: Color32::from_white_alpha(15),
            active: Color32::from_white_alpha(26),
            hairline: Color32::from_white_alpha(20),
        }
    } else {
        Palette {
            paper: Color32::from_rgb(0xFA, 0xFA, 0xFA),
            shelf: Color32::from_rgb(0xF3, 0xF3, 0xF3),
            raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            selected: Color32::from_rgb(0xE4, 0xE4, 0xE4),
            ink: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            ink2: Color32::from_rgb(0x4A, 0x4A, 0x4A),
            pencil: Color32::from_rgb(0x6B, 0x6B, 0x6B),
            rule: Color32::from_rgb(0xE6, 0xE6, 0xE6),
            outline: Color32::from_rgb(0xD4, 0xD4, 0xD4),
            signal: Color32::from_rgb(0xB0, 0x4E, 0x2C),
            ok: Color32::from_rgb(0x4C, 0x76, 0x32),
            warn: Color32::from_rgb(0x94, 0x62, 0x0F),
            error: Color32::from_rgb(0xB0, 0x3A, 0x2E),
            hover: Color32::from_black_alpha(10),
            active: Color32::from_black_alpha(20),
            hairline: Color32::from_black_alpha(22),
        }
    }
}

pub fn accent(dark: bool) -> Color32 {
    palette(dark).signal
}

pub fn success(dark: bool) -> Color32 {
    palette(dark).ok
}

/// The text style for prose: the model's replies. Uses the same sans as the interface, at
/// [`READING_SIZE`] where replies are drawn (`markdown::show` sets it around them).
pub fn reading() -> TextStyle {
    TextStyle::Body
}

/// Replies read at 16px; the interface around them is 14px.
pub const READING_SIZE: f32 = 16.0;

/// Instrument Sans for the interface and prose, Plex Mono for code, and
/// Phosphor icons as a fallback for each.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_static(bytes).into());
    };
    add(
        "instrument-sans",
        include_bytes!("../../assets/fonts/InstrumentSans.ttf"),
    );
    add(
        "plex-mono",
        include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
    );
    add(
        "phosphor",
        include_bytes!("../../assets/fonts/Phosphor.ttf"),
    );
    add(
        "newsreader",
        include_bytes!("../../assets/fonts/Newsreader-Latin.ttf"),
    );
    add(
        "newsreader-ext",
        include_bytes!("../../assets/fonts/Newsreader-LatinExt.ttf"),
    );
    // egui's defaults stay last, for scripts and emoji these fonts don't cover.
    let families = [
        (
            FontFamily::Proportional,
            "instrument-sans",
            FontFamily::Proportional,
        ),
        (FontFamily::Monospace, "plex-mono", FontFamily::Monospace),
    ];
    let lists: Vec<_> = families
        .iter()
        .map(|(family, primary, base)| {
            let mut list = vec![(*primary).to_owned(), "phosphor".to_owned()];
            list.extend(fonts.families.get(base).cloned().unwrap_or_default());
            (family.clone(), list)
        })
        .collect();
    for (family, list) in lists {
        fonts.families.insert(family, list);
    }
    // The display serif falls back to the interface sans for anything it doesn't cover.
    let mut serif = vec!["newsreader".to_owned(), "newsreader-ext".to_owned()];
    serif.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts
        .families
        .insert(FontFamily::Name("serif".into()), serif);
    ctx.set_fonts(fonts);
}

/// Apply spacing, type scale and palette to both the light and dark styles.
pub fn apply(ctx: &egui::Context) {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            let dark = theme == egui::Theme::Dark;
            let p = palette(dark);
            style.text_styles = [
                (TextStyle::Heading, FontId::proportional(22.0)),
                (TextStyle::Body, FontId::proportional(14.0)),
                (TextStyle::Button, FontId::proportional(14.0)),
                (TextStyle::Small, FontId::proportional(12.0)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();

            let s = &mut style.spacing;
            s.item_spacing = egui::vec2(8.0, 6.0);
            s.button_padding = egui::vec2(10.0, 5.0);
            s.interact_size.y = 28.0;
            s.window_margin = Margin::same(16);
            s.menu_margin = Margin::same(6);
            s.indent = 16.0;
            // A thin bar that widens under the pointer, on every scroll area.
            s.scroll = egui::style::ScrollStyle::thin();

            let v = &mut style.visuals;
            v.panel_fill = p.paper;
            v.window_fill = p.raised;
            v.extreme_bg_color = p.raised;
            v.faint_bg_color = p.raised;
            v.code_bg_color = p.raised;
            v.window_stroke = Stroke::new(1.0, p.outline);
            v.window_corner_radius = CornerRadius::same(14);
            v.menu_corner_radius = CornerRadius::same(10);
            v.window_shadow = Shadow::NONE;
            v.popup_shadow = Shadow::NONE;
            v.hyperlink_color = p.signal;
            v.warn_fg_color = p.warn;
            v.error_fg_color = p.error;
            // Selected rows and selected text: neutral, like the sidebar's current chat.
            v.selection.bg_fill = p.selected;
            v.selection.stroke = Stroke::new(1.0, p.ink);
            v.override_text_color = None;
            // `ui.weak(…)` text: the muted text colour, which meets AA contrast, rather than
            // egui's 60% of the main colour (4.4:1 on the light paper).
            v.weak_text_color = Some(p.pencil);
            v.collapsing_header_frame = false;
            v.indent_has_left_vline = false;
            v.striped = false;

            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = CornerRadius::same(8);
                w.expansion = 0.0;
            }
            v.widgets.noninteractive.bg_fill = p.paper;
            v.widgets.noninteractive.weak_bg_fill = p.paper;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hairline);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
            // Buttons are quiet: a hairline outline, filled on hover.
            v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_fill = p.selected;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.outline);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.hovered.weak_bg_fill = p.hover;
            v.widgets.hovered.bg_fill = p.selected;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.outline);
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.active.weak_bg_fill = p.selected;
            v.widgets.active.bg_fill = p.selected;
            v.widgets.active.bg_stroke = Stroke::new(1.0, p.pencil);
            v.widgets.active.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.open.weak_bg_fill = p.selected;
            v.widgets.open.bg_fill = p.selected;
            v.widgets.open.bg_stroke = Stroke::new(1.0, p.outline);
        });
    }
}

/// How long hover and fill transitions take: 140 ms, or none with "Reduce motion" on.
pub fn set_motion(ctx: &egui::Context, reduce: bool) {
    ctx.all_styles_mut(|style| style.animation_time = if reduce { 0.0 } else { 0.14 });
}

/// The primary action: light-on-dark ink, like a filled pill.
pub fn primary_button<'a>(text: impl Into<String>, dark: bool) -> egui::Button<'a> {
    let p = palette(dark);
    egui::Button::new(egui::RichText::new(text.into()).color(p.paper))
        .fill(p.ink)
        .stroke(Stroke::NONE)
}

/// Text with a leading icon, e.g. `labelled(icon::PLUS, "New chat")`.
pub fn labelled(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    #[test]
    fn window_icons_decode() {
        for dark in [false, true] {
            let icon = super::window_icon(dark);
            assert_eq!((icon.width, icon.height), (256, 256));
        }
    }

    /// WCAG relative luminance of an sRGB colour.
    fn luminance(c: egui::Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Text in either theme reads at WCAG AA (4.5:1) on every surface it is drawn on.
    #[test]
    fn text_colours_meet_wcag_aa() {
        assert!((contrast(egui::Color32::BLACK, egui::Color32::WHITE) - 21.0).abs() < 1e-9);
        for dark in [false, true] {
            let p = super::palette(dark);
            for (text_name, text) in [
                ("ink", p.ink),
                ("ink2", p.ink2),
                ("pencil", p.pencil),
                ("signal", p.signal),
                ("ok", p.ok),
                ("warn", p.warn),
                ("error", p.error),
            ] {
                for (bg_name, bg) in [("paper", p.paper), ("shelf", p.shelf), ("raised", p.raised)]
                {
                    let ratio = contrast(text, bg);
                    assert!(
                        ratio >= 4.5,
                        "{text_name} on {bg_name} ({}) is {ratio:.2}:1",
                        if dark { "dark" } else { "light" }
                    );
                }
            }
            // The selected chat row's own text.
            for text in [p.ink, p.ink2] {
                assert!(contrast(text, p.selected) >= 4.5);
            }
        }
    }
}

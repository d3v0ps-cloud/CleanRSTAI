//! Visual design: a warm charcoal (dark) or warm paper (light) palette with one muted
//! terracotta accent; Instrument Sans for the interface, Newsreader (a serif) for the
//! greeting and the model's replies, IBM Plex Mono for code. Soft 8px corners, quiet
//! borders, no shadows or glows.

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
}

/// The colours of one mode.
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
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            paper: Color32::from_rgb(0x15, 0x15, 0x15),
            shelf: Color32::from_rgb(0x11, 0x11, 0x11),
            raised: Color32::from_rgb(0x20, 0x20, 0x1F),
            selected: Color32::from_rgb(0x35, 0x35, 0x35),
            ink: Color32::from_rgb(0xF0, 0xEF, 0xED),
            ink2: Color32::from_rgb(0xB9, 0xB9, 0xAF),
            pencil: Color32::from_rgb(0x82, 0x81, 0x7C),
            rule: Color32::from_rgb(0x29, 0x29, 0x29),
            outline: Color32::from_rgb(0x4D, 0x4D, 0x4C),
            signal: Color32::from_rgb(0xBC, 0x81, 0x5F),
            ok: Color32::from_rgb(0x8F, 0xB0, 0x78),
            warn: Color32::from_rgb(0xD9, 0xA8, 0x5B),
            error: Color32::from_rgb(0xE0, 0x7A, 0x6A),
        }
    } else {
        Palette {
            paper: Color32::from_rgb(0xFA, 0xF9, 0xF5),
            shelf: Color32::from_rgb(0xF3, 0xF1, 0xEA),
            raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            selected: Color32::from_rgb(0xE6, 0xE3, 0xD9),
            ink: Color32::from_rgb(0x1F, 0x1E, 0x1D),
            ink2: Color32::from_rgb(0x3D, 0x3C, 0x38),
            pencil: Color32::from_rgb(0x7A, 0x77, 0x6E),
            rule: Color32::from_rgb(0xE3, 0xE0, 0xD7),
            outline: Color32::from_rgb(0xD3, 0xCF, 0xC4),
            signal: Color32::from_rgb(0xB0, 0x64, 0x3E),
            ok: Color32::from_rgb(0x4E, 0x7A, 0x34),
            warn: Color32::from_rgb(0x9A, 0x66, 0x10),
            error: Color32::from_rgb(0xB0, 0x3A, 0x2E),
        }
    }
}

pub fn accent(dark: bool) -> Color32 {
    palette(dark).signal
}

pub fn success(dark: bool) -> Color32 {
    palette(dark).ok
}

/// The serif family (Newsreader).
pub const SERIF: &str = "serif";

/// The text style for prose: the model's replies. Interface text uses the sans `Body`.
pub fn reading() -> TextStyle {
    TextStyle::Name("reading".into())
}

/// Instrument Sans for the interface, Newsreader for prose, Plex Mono for code, and
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
        "newsreader",
        include_bytes!("../../assets/fonts/Newsreader.ttf"),
    );
    add(
        "plex-mono",
        include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
    );
    add(
        "phosphor",
        include_bytes!("../../assets/fonts/Phosphor.ttf"),
    );
    // egui's defaults stay last, for scripts and emoji these fonts don't cover.
    let families = [
        (
            FontFamily::Proportional,
            "instrument-sans",
            FontFamily::Proportional,
        ),
        (FontFamily::Monospace, "plex-mono", FontFamily::Monospace),
        (
            FontFamily::Name(SERIF.into()),
            "newsreader",
            FontFamily::Proportional,
        ),
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
    ctx.set_fonts(fonts);
}

/// Apply spacing, type scale and palette to both the light and dark styles.
pub fn apply(ctx: &egui::Context) {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            let dark = theme == egui::Theme::Dark;
            let p = palette(dark);
            let serif = |size| FontId::new(size, FontFamily::Name(SERIF.into()));
            style.text_styles = [
                (TextStyle::Heading, serif(23.0)),
                (TextStyle::Body, FontId::proportional(14.5)),
                (reading(), serif(17.0)),
                (TextStyle::Button, FontId::proportional(14.0)),
                (TextStyle::Small, FontId::proportional(12.5)),
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

            let v = &mut style.visuals;
            v.panel_fill = p.paper;
            v.window_fill = p.raised;
            v.extreme_bg_color = p.raised;
            v.faint_bg_color = p.raised;
            v.code_bg_color = p.raised;
            v.window_stroke = Stroke::new(1.0, p.outline);
            v.window_corner_radius = CornerRadius::same(12);
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
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.rule);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
            // Buttons are quiet: a hairline outline, filled on hover.
            v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            v.widgets.inactive.bg_fill = p.selected;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.outline);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.hovered.weak_bg_fill = p.selected.gamma_multiply(0.7);
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

//! Visual design. The app is a workbench for running models, so it is styled like a printed
//! instrument log rather than a chat toy: warm paper and ink, one vermilion signal colour,
//! a serif for text you read (IBM Plex Serif) and a monospace for the interface and readouts
//! (IBM Plex Mono). Hairline rules instead of cards, near-square corners, no shadows or glows.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Shadow, Stroke, TextStyle,
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
    /// The sidebar: a shade darker than the page.
    pub shelf: Color32,
    /// Code, inputs and other inset areas.
    pub inset: Color32,
    pub ink: Color32,
    /// Secondary text: labels, metadata.
    pub pencil: Color32,
    /// Hairline rules and outlines.
    pub rule: Color32,
    /// The one signal colour: active selection, the model's name, primary actions.
    pub signal: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            paper: Color32::from_rgb(24, 23, 20),
            shelf: Color32::from_rgb(19, 18, 16),
            inset: Color32::from_rgb(33, 31, 27),
            ink: Color32::from_rgb(231, 225, 212),
            pencil: Color32::from_rgb(150, 143, 130),
            rule: Color32::from_rgb(58, 54, 47),
            signal: Color32::from_rgb(236, 108, 64),
            ok: Color32::from_rgb(150, 178, 92),
            warn: Color32::from_rgb(222, 170, 72),
            error: Color32::from_rgb(232, 98, 86),
        }
    } else {
        Palette {
            paper: Color32::from_rgb(246, 243, 235),
            shelf: Color32::from_rgb(236, 232, 221),
            inset: Color32::from_rgb(235, 231, 220),
            ink: Color32::from_rgb(30, 28, 24),
            pencil: Color32::from_rgb(112, 105, 92),
            rule: Color32::from_rgb(208, 200, 184),
            signal: Color32::from_rgb(196, 62, 26),
            ok: Color32::from_rgb(78, 110, 36),
            warn: Color32::from_rgb(160, 104, 10),
            error: Color32::from_rgb(178, 40, 30),
        }
    }
}

pub fn accent(dark: bool) -> Color32 {
    palette(dark).signal
}

pub fn success(dark: bool) -> Color32 {
    palette(dark).ok
}

pub fn border(dark: bool) -> Color32 {
    palette(dark).rule
}

/// Medium-weight monospace, for labels and the wordmark.
pub const MONO_MEDIUM: &str = "mono-medium";
/// Semibold serif, for headings.
pub const SERIF_SEMIBOLD: &str = "serif-semibold";

/// The text style for prose: the model's replies and chat titles. Everything else (labels,
/// buttons, inputs) uses the monospace `Body`, since egui sizes buttons from `Body` too.
pub fn reading() -> TextStyle {
    TextStyle::Name("reading".into())
}

/// Plex Serif for reading, Plex Mono for the interface, Phosphor icons as a fallback for both.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_static(bytes).into());
    };
    add(
        "plex-serif",
        include_bytes!("../../assets/fonts/IBMPlexSerif-Regular.ttf"),
    );
    add(
        "plex-serif-semibold",
        include_bytes!("../../assets/fonts/IBMPlexSerif-SemiBold.ttf"),
    );
    add(
        "plex-mono",
        include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
    );
    add(
        "plex-mono-medium",
        include_bytes!("../../assets/fonts/IBMPlexMono-Medium.ttf"),
    );
    add(
        "phosphor",
        include_bytes!("../../assets/fonts/Phosphor.ttf"),
    );
    // egui's defaults stay last, for scripts and emoji the Plex fonts don't cover.
    let fallback = |primary: &str, family: &FontFamily, fonts: &egui::FontDefinitions| {
        let mut list = vec![primary.to_owned(), "phosphor".to_owned()];
        list.extend(fonts.families.get(family).cloned().unwrap_or_default());
        list
    };
    let families = [
        (
            FontFamily::Proportional,
            "plex-serif",
            FontFamily::Proportional,
        ),
        (FontFamily::Monospace, "plex-mono", FontFamily::Monospace),
        (
            FontFamily::Name(SERIF_SEMIBOLD.into()),
            "plex-serif-semibold",
            FontFamily::Proportional,
        ),
        (
            FontFamily::Name(MONO_MEDIUM.into()),
            "plex-mono-medium",
            FontFamily::Monospace,
        ),
    ];
    let lists: Vec<_> = families
        .iter()
        .map(|(family, primary, base)| (family.clone(), fallback(primary, base, &fonts)))
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
            style.text_styles = [
                (
                    TextStyle::Heading,
                    FontId::new(21.0, FontFamily::Name(SERIF_SEMIBOLD.into())),
                ),
                (TextStyle::Body, FontId::monospace(13.0)),
                (reading(), FontId::proportional(15.5)),
                (TextStyle::Button, FontId::monospace(13.0)),
                (TextStyle::Small, FontId::monospace(11.5)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();

            let s = &mut style.spacing;
            s.item_spacing = egui::vec2(8.0, 6.0);
            s.button_padding = egui::vec2(9.0, 4.0);
            s.interact_size.y = 26.0;
            s.window_margin = Margin::same(16);
            s.menu_margin = Margin::same(4);
            s.indent = 16.0;

            let v = &mut style.visuals;
            v.panel_fill = p.paper;
            v.window_fill = p.paper;
            v.extreme_bg_color = p.inset;
            v.faint_bg_color = p.inset;
            v.code_bg_color = p.inset;
            v.window_stroke = Stroke::new(1.0, p.ink.gamma_multiply(0.5));
            v.window_corner_radius = CornerRadius::same(2);
            v.menu_corner_radius = CornerRadius::same(2);
            v.window_shadow = Shadow::NONE;
            v.popup_shadow = Shadow::NONE;
            v.hyperlink_color = p.signal;
            v.warn_fg_color = p.warn;
            v.error_fg_color = p.error;
            // Selected items and selected text: a faint tint, with the text in signal colour.
            v.selection.bg_fill = p.signal.gamma_multiply(if dark { 0.16 } else { 0.12 });
            v.selection.stroke = Stroke::new(1.0, p.signal);
            v.override_text_color = None;
            v.collapsing_header_frame = false;
            v.indent_has_left_vline = true;
            v.striped = false;

            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = CornerRadius::same(2);
                w.expansion = 0.0;
            }
            v.widgets.noninteractive.bg_fill = p.paper;
            v.widgets.noninteractive.weak_bg_fill = p.paper;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.rule);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
            // Buttons are outlined, not filled; checkboxes and radios (bg_fill) stay visible.
            v.widgets.inactive.weak_bg_fill = p.paper;
            v.widgets.inactive.bg_fill = p.inset;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.rule);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.hovered.weak_bg_fill = p.inset;
            v.widgets.hovered.bg_fill = p.inset;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.ink.gamma_multiply(0.7));
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.active.weak_bg_fill = p.rule;
            v.widgets.active.bg_fill = p.rule;
            v.widgets.active.bg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.active.fg_stroke = Stroke::new(1.0, p.ink);
            v.widgets.open.weak_bg_fill = p.inset;
            v.widgets.open.bg_fill = p.inset;
            v.widgets.open.bg_stroke = Stroke::new(1.0, p.ink.gamma_multiply(0.7));
        });
    }
}

/// The primary action: ink-on-signal, square.
pub fn primary_button<'a>(text: impl Into<String>, dark: bool) -> egui::Button<'a> {
    let p = palette(dark);
    egui::Button::new(RichText::new(text.into()).color(p.paper))
        .fill(p.signal)
        .stroke(Stroke::new(1.0, p.signal))
}

/// A small monospace label in capitals, for section names and the transcript margin.
pub fn caps(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .family(FontFamily::Name(MONO_MEDIUM.into()))
        .size(10.5)
        .extra_letter_spacing(1.2)
}

/// Text with a leading icon, e.g. `labelled(icon::PLUS, "New chat")`.
pub fn labelled(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

/// Paint a horizontal hairline across the current width.
pub fn rule(ui: &mut egui::Ui) {
    let dark = ui.visuals().dark_mode;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette(dark).rule),
    );
}

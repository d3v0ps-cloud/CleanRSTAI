//! The artifact panel beside the chat, and the artifact cards under replies that open it.

use eframe::egui::{self, RichText};
use egui_commonmark::CommonMarkCache;
use uuid::Uuid;

use super::markdown::{self, DrawingCache, DrawingKind};
use super::theme::{self, icon};
use super::widgets::icon_button;
use crate::app::Action;
use crate::artifacts::{Artifact, Kind};

/// The artifact shown in the panel.
pub struct ArtifactView {
    pub conv: Uuid,
    pub message: usize,
    pub index: usize,
    pub tab: Tab,
    /// Move to each new artifact as it arrives (set while the newest one is shown).
    pub follow: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Preview,
    Code,
}

impl ArtifactView {
    pub fn new(conv: Uuid, artifact: &Artifact, newest: bool) -> Self {
        Self {
            conv,
            message: artifact.message,
            index: artifact.index,
            tab: if artifact.kind.has_preview() {
                Tab::Preview
            } else {
                Tab::Code
            },
            follow: newest,
        }
    }

    fn show(&mut self, artifact: &Artifact, newest: bool) {
        if (self.message, self.index) != (artifact.message, artifact.index) {
            // Keep the tab when it still applies, so versions compare like for like.
            let tab = self.tab;
            *self = Self::new(self.conv, artifact, newest);
            if tab == Tab::Code {
                self.tab = Tab::Code;
            }
        }
        self.follow = newest;
    }
}

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Html => icon::GLOBE,
        Kind::Svg => icon::IMAGE,
        Kind::Mermaid => icon::FLOW,
        Kind::Markdown => icon::FILE,
        Kind::Code => icon::CODE,
    }
}

fn kind_label(a: &Artifact) -> String {
    let what = match a.kind {
        Kind::Html => "Web page".to_owned(),
        Kind::Svg => "SVG".to_owned(),
        Kind::Mermaid => "Mermaid".to_owned(),
        Kind::Markdown => "Markdown".to_owned(),
        Kind::Code => crate::artifacts::language_name(&a.lang)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if a.lang.is_empty() {
                    "Code".to_owned()
                } else {
                    a.lang.clone()
                }
            }),
    };
    let lines = a.lines();
    format!("{what} · {lines} line{}", if lines == 1 { "" } else { "s" })
}

/// Lines of source a reply's artifact card shows.
const PREVIEW_LINES: usize = 6;
/// Characters per line in that preview; the card is narrower than the panel.
const PREVIEW_COLUMNS: usize = 100;
/// Height of a drawing's thumbnail in the card.
const THUMBNAIL_HEIGHT: f32 = 140.0;

/// A collapsed preview in a reply that stands in for an artifact: its name, then the first
/// lines of its source or a thumbnail of the drawing. Clicking it sends `open`, which shows
/// the artifact in full in the panel.
pub fn card(
    ui: &mut egui::Ui,
    drawings: &mut DrawingCache,
    a: &Artifact,
    is_open: bool,
    actions: &mut Vec<Action>,
    open: Action,
) {
    let p = theme::palette(ui.visuals().dark_mode);
    let frame = egui::Frame::new()
        .fill(if is_open { p.selected } else { p.raised })
        .stroke(egui::Stroke::new(
            1.0,
            if is_open { p.signal } else { p.outline },
        ))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind_icon(a.kind)).size(22.0).color(p.signal));
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(&a.title).strong()).truncate());
                    ui.label(RichText::new(kind_label(a)).small().color(p.pencil));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let hint = if is_open { "In the panel" } else { "Open" };
                    ui.label(RichText::new(hint).small().color(p.pencil));
                });
            });
            ui.add_space(6.0);
            match a.kind {
                Kind::Svg => markdown::drawing_preview(
                    ui,
                    drawings,
                    DrawingKind::Svg,
                    &a.source,
                    THUMBNAIL_HEIGHT,
                ),
                Kind::Mermaid => markdown::drawing_preview(
                    ui,
                    drawings,
                    DrawingKind::Mermaid,
                    &a.source,
                    THUMBNAIL_HEIGHT,
                ),
                _ => source_peek(ui, a),
            }
        });
    let resp = ui
        .interact(
            frame.response.rect,
            ui.make_persistent_id(("artifact", a.message, a.index)),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if is_open {
            "Shown in the panel"
        } else {
            "Open in the panel"
        });
    let title = a.title.clone();
    resp.widget_info(move || {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Open {title}"))
    });
    if resp.clicked() {
        actions.push(open);
    }
}

/// The first lines of an artifact's source, muted, with how many more there are.
fn source_peek(ui: &mut egui::Ui, a: &Artifact) {
    let p = theme::palette(ui.visuals().dark_mode);
    let total = a.lines();
    let peek: Vec<String> = a
        .source
        .lines()
        .take(PREVIEW_LINES)
        .map(|l| {
            let mut line: String = l.chars().take(PREVIEW_COLUMNS).collect();
            if l.chars().count() > PREVIEW_COLUMNS {
                line.push('…');
            }
            line
        })
        .collect();
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for line in &peek {
                // One label per line, so long lines are cut rather than wrapped.
                ui.add(
                    egui::Label::new(RichText::new(line).monospace().size(12.0).color(p.pencil))
                        .selectable(false)
                        .truncate(),
                );
            }
            if total > PREVIEW_LINES {
                let more = total - PREVIEW_LINES;
                ui.label(
                    RichText::new(format!(
                        "… {more} more line{}",
                        if more == 1 { "" } else { "s" }
                    ))
                    .small()
                    .color(p.pencil),
                );
            }
        });
}

/// The panel's contents. Asks to close the panel when the chat has no artifacts left.
pub fn show(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
    view: &mut ArtifactView,
    artifacts: &[Artifact],
    remote_images: bool,
    actions: &mut Vec<Action>,
) {
    let Some(newest) = artifacts.len().checked_sub(1) else {
        actions.push(Action::CloseArtifact);
        return;
    };
    let mut current = if view.follow {
        newest
    } else {
        artifacts
            .iter()
            .position(|a| (a.message, a.index) == (view.message, view.index))
            .unwrap_or(newest)
    };
    view.show(&artifacts[current], current == newest);
    let p = theme::palette(ui.visuals().dark_mode);

    // Title row: what it is (a menu of every artifact in the chat), then the actions.
    let a = &artifacts[current];
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(kind_icon(a.kind)).size(18.0).color(p.signal));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, icon::CLOSE, "Close the panel").clicked() {
                actions.push(Action::CloseArtifact);
            }
            if a.kind.opens_in_browser()
                && icon_button(ui, icon::ARROW_SQUARE_OUT, "Open in browser").clicked()
            {
                actions.push(Action::OpenArtifactInBrowser(a.clone()));
            }
            if icon_button(ui, icon::DOWNLOAD, "Save as…").clicked() {
                actions.push(Action::SaveArtifact(a.clone()));
            }
            if icon_button(ui, icon::COPY, "Copy").clicked() {
                actions.push(Action::Copy(a.source.clone()));
            }
            // The rest of the row, from the left.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let title = egui::Button::new(RichText::new(&a.title).strong().size(15.0))
                    .right_text(RichText::new(icon::CARET_DOWN).color(p.pencil))
                    .frame(false)
                    .truncate();
                egui::containers::menu::MenuButton::from_button(title)
                    .ui(ui, |ui| {
                        for (i, other) in artifacts.iter().enumerate() {
                            if ui
                                .selectable_label(
                                    i == current,
                                    format!("{}  {}", kind_icon(other.kind), other.title),
                                )
                                .clicked()
                            {
                                current = i;
                            }
                        }
                    })
                    .0
                    .on_hover_text(format!(
                        "{} of {} artifacts in this chat",
                        current + 1,
                        artifacts.len()
                    ));
            });
        });
    });

    // Tabs on the left, versions of this artifact on the right.
    ui.horizontal(|ui| {
        if a.kind.has_preview() {
            ui.selectable_value(&mut view.tab, Tab::Preview, "Preview");
            ui.selectable_value(&mut view.tab, Tab::Code, "Code");
        }
        let versions: Vec<usize> = (0..artifacts.len())
            .filter(|&i| artifacts[i].same_thing(a))
            .collect();
        if versions.len() > 1 {
            let at = versions.iter().position(|&i| i == current).unwrap_or(0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        at + 1 < versions.len(),
                        egui::Button::new(icon::CARET_RIGHT).frame(false),
                    )
                    .on_hover_text("Next version")
                    .clicked()
                {
                    current = versions[at + 1];
                }
                ui.label(
                    RichText::new(format!("Version {} of {}", at + 1, versions.len()))
                        .small()
                        .color(p.pencil),
                );
                if ui
                    .add_enabled(at > 0, egui::Button::new(icon::CARET_LEFT).frame(false))
                    .on_hover_text("Previous version")
                    .clicked()
                {
                    current = versions[at - 1];
                }
            });
        }
    });
    view.show(&artifacts[current], current == newest);
    let a = &artifacts[current];

    if a.kind == Kind::Html {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Web pages run in your browser, not in the app.")
                    .small()
                    .color(p.pencil),
            );
            if ui.small_button("Open in browser").clicked() {
                actions.push(Action::OpenArtifactInBrowser(a.clone()));
            }
        });
    }
    ui.separator();

    let id = ui.make_persistent_id(("artifact-body", view.conv, a.message, a.index));
    egui::ScrollArea::vertical()
        .id_salt(("artifact-scroll", a.message, a.index, view.tab == Tab::Code))
        .auto_shrink(false)
        .show(ui, |ui| match (view.tab, a.kind) {
            (Tab::Preview, Kind::Svg) => {
                markdown::drawing_preview(ui, drawings, DrawingKind::Svg, &a.source, f32::INFINITY)
            }
            (Tab::Preview, Kind::Mermaid) => markdown::drawing_preview(
                ui,
                drawings,
                DrawingKind::Mermaid,
                &a.source,
                f32::INFINITY,
            ),
            (Tab::Preview, Kind::Markdown) => markdown::show(
                ui,
                md_cache,
                drawings,
                id,
                &a.source,
                remote_images,
                None,
                actions,
            ),
            _ => markdown::code_view(ui, md_cache, id, &a.lang, &a.source),
        });
}

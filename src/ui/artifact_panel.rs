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
            // Web pages too: the panel falls back to the code where it can't show them.
            tab: if artifact.kind.has_preview() || artifact.kind == Kind::Html {
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

/// Most a drawing grows to when expanded in the chat.
const INLINE_DRAWING_HEIGHT: f32 = 600.0;

/// An artifact in a reply: one link-style line (its name, kind and size). The artifact itself
/// is shown in the panel; clicking the link expands it here in the chat too, and again folds
/// it away. "Open in panel" at the end sends `open`.
#[allow(clippy::too_many_arguments)]
pub fn card(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
    a: &Artifact,
    is_open: bool,
    remote_images: bool,
    actions: &mut Vec<Action>,
    open: Action,
) {
    let p = theme::palette(ui.visuals().dark_mode);
    let expanded_id = ui.make_persistent_id(("artifact-expanded", a.message, a.index));
    let expanded = ui.data_mut(|d| *d.get_persisted_mut_or_default::<bool>(expanded_id));
    let link = ui.visuals().hyperlink_color;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let caret = if expanded {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        };
        let text = format!("{caret}  {}  {}", kind_icon(a.kind), a.title);
        let resp = ui
            .add(egui::Link::new(RichText::new(text).color(link)))
            .on_hover_text(if expanded {
                "Fold it away"
            } else {
                "Show it here in the chat"
            });
        if resp.clicked() {
            ui.data_mut(|d| d.insert_persisted(expanded_id, !expanded));
        }
        ui.label(RichText::new(kind_label(a)).small().color(p.pencil));
        if is_open {
            ui.label(RichText::new("· in the panel").small().color(p.pencil));
        } else if ui
            .add(
                egui::Button::new(RichText::new("Open in panel").small().color(p.pencil))
                    .frame_when_inactive(false),
            )
            .on_hover_text("Show it in the panel beside the chat")
            .clicked()
        {
            actions.push(open);
        }
    });
    if expanded {
        let id = ui.make_persistent_id(("artifact-inline", a.message, a.index));
        ui.add_space(4.0);
        match a.kind {
            Kind::Svg => markdown::drawing_preview(
                ui,
                drawings,
                DrawingKind::Svg,
                &a.source,
                INLINE_DRAWING_HEIGHT,
            ),
            Kind::Mermaid => markdown::drawing_preview(
                ui,
                drawings,
                DrawingKind::Mermaid,
                &a.source,
                INLINE_DRAWING_HEIGHT,
            ),
            Kind::Markdown => markdown::show(
                ui,
                md_cache,
                drawings,
                id,
                &a.source,
                remote_images,
                None,
                actions,
            ),
            Kind::Html | Kind::Code => markdown::code_view(ui, md_cache, id, &a.lang, &a.source),
        }
    }
}

/// The top bar's button that shows and hides the panel: a window with a bar on the right,
/// the mirror of the sidebar icon (Phosphor has none), in the accent colour while open.
pub fn toggle_button(ui: &mut egui::Ui, open: bool, enabled: bool) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), sense);
    let p = theme::palette(ui.visuals().dark_mode);
    let color = if !enabled {
        ui.visuals().weak_text_color()
    } else if open {
        p.signal
    } else if resp.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let stroke = egui::Stroke::new(1.3, color);
    let r = egui::Rect::from_center_size(rect.center(), egui::vec2(15.0, 13.0));
    let painter = ui.painter();
    painter.rect_stroke(r, 2.5, stroke, egui::StrokeKind::Inside);
    let x = r.right() - 5.5;
    painter.line_segment([egui::pos2(x, r.top()), egui::pos2(x, r.bottom())], stroke);
    if open {
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, r.top()), r.max).shrink(1.0),
            1.5,
            color.gamma_multiply(0.5),
        );
    }
    resp.widget_info(move || {
        egui::WidgetInfo::selected(egui::Role::Button, enabled, open, "Artifact panel")
    });
    resp
}

/// The panel's contents. Asks to close the panel when the chat has no artifacts left. With
/// `live_html`, a web page's Preview tab leaves its area empty and returns it with the page,
/// for the web view laid over it. `own_window` is set where a live preview opens in a window of
/// its own (Linux, while the preview is on): whether that window is open now.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
    view: &mut ArtifactView,
    artifacts: &[Artifact],
    remote_images: bool,
    live_html: Result<(), &str>,
    own_window: Option<bool>,
    actions: &mut Vec<Action>,
) -> Option<(egui::Rect, String)> {
    let Some(newest) = artifacts.len().checked_sub(1) else {
        actions.push(Action::CloseArtifact);
        return None;
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
    let previews = a.kind.has_preview() || (a.kind == Kind::Html && live_html.is_ok());
    ui.horizontal(|ui| {
        if previews {
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

    if a.kind == Kind::Html
        && let Err(why) = live_html
    {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(why).small().color(p.pencil));
            if let Some(open) = own_window {
                let hint = if open {
                    "Its window is open: this brings it to the front"
                } else {
                    "Show the page in a window of its own"
                };
                if ui
                    .small_button("Open live preview")
                    .on_hover_text(hint)
                    .clicked()
                {
                    actions.push(Action::OpenLivePreview(a.clone()));
                }
            }
            if ui.small_button("Open in browser").clicked() {
                actions.push(Action::OpenArtifactInBrowser(a.clone()));
            }
        });
    }
    ui.separator();
    if a.kind == Kind::Html && live_html.is_ok() && view.tab == Tab::Preview {
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        return Some((area, a.source.clone()));
    }

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
    None
}

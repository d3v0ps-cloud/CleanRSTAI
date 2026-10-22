//! Markdown in chat replies: [`CommonMarkViewer`] for text, code, tables and images, plus
//! drawings for ```` ```mermaid ```` and ```` ```svg ```` blocks and raw `<svg>` markup.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use eframe::egui::{self, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use super::theme;
use super::widgets::{code_block, text_actions};
use crate::app::Action;

/// Larger SVGs are shown as code rather than drawn.
const MAX_SVG_BYTES: usize = 2_000_000;
/// Rendered diagrams kept; older ones are dropped when the cache grows past this.
const MAX_CACHED: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DrawingKind {
    Mermaid,
    Svg,
}

/// A part of a reply: markdown text, or a drawing that is shown as an image.
#[derive(Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Markdown(String),
    Drawing {
        kind: DrawingKind,
        /// The Mermaid or SVG source.
        source: String,
        /// The block as written, shown as code if it can't be drawn.
        raw: &'a str,
    },
}

/// Rendered drawings, by hash of kind and source, so each is rendered once.
#[derive(Default)]
pub struct DrawingCache {
    drawings: HashMap<u64, Result<Drawing, String>>,
}

#[derive(Clone)]
struct Drawing {
    uri: String,
    svg: egui::load::Bytes,
}

impl DrawingCache {
    fn get(&mut self, kind: DrawingKind, source: &str) -> &Result<Drawing, String> {
        let mut h = DefaultHasher::new();
        (kind, source).hash(&mut h);
        let key = h.finish();
        if !self.drawings.contains_key(&key) && self.drawings.len() >= MAX_CACHED {
            self.drawings.clear();
        }
        self.drawings.entry(key).or_insert_with(|| {
            let svg = match kind {
                DrawingKind::Mermaid => render_mermaid(source)?,
                DrawingKind::Svg => source.to_owned(),
            };
            if svg.len() > MAX_SVG_BYTES {
                return Err("the SVG is too large to draw".into());
            }
            // egui picks the SVG loader by the `.svg` extension.
            Ok(Drawing {
                uri: format!("bytes://drawing-{key:016x}.svg"),
                svg: svg.into_bytes().into(),
            })
        })
    }
}

fn render_mermaid(source: &str) -> Result<String, String> {
    // The renderer is young; a diagram it can't handle must not take the app down.
    std::panic::catch_unwind(|| mermaid_rs_renderer::render(source))
        .map_err(|_| "the Mermaid renderer failed on this diagram".to_owned())?
        .map_err(|e| format!("{e:#}"))
}

/// Split `text` into markdown and drawings. Only complete top-level blocks become drawings,
/// so a diagram still being streamed shows as code until its closing fence arrives. Unless
/// `remote_images`, web images become plain links.
pub fn segments(text: &str, remote_images: bool) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    // Byte ranges of `text` to drop: the `!` of web images, so they read as links.
    let mut unbang: Vec<usize> = Vec::new();
    let mut drawings: Vec<(Range<usize>, DrawingKind, String)> = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(usize, DrawingKind, String)> = None;

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) if depth == 0 => {
                depth += 1;
                let kind = match lang.split_whitespace().next().unwrap_or("") {
                    l if l.eq_ignore_ascii_case("mermaid") => Some(DrawingKind::Mermaid),
                    l if l.eq_ignore_ascii_case("svg") => Some(DrawingKind::Svg),
                    _ => None,
                };
                if let Some(kind) = kind
                    && fence_is_closed(&text[range.clone()])
                {
                    open = Some((range.start, kind, String::new()));
                }
            }
            // A one-line `<svg>…</svg>` parses as a paragraph of inline HTML.
            Event::Start(Tag::HtmlBlock | Tag::Paragraph) if depth == 0 => {
                depth += 1;
                let block = text[range.clone()].trim();
                if block.starts_with("<svg") && block.ends_with("</svg>") {
                    drawings.push((range, DrawingKind::Svg, block.to_owned()));
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                depth += 1;
                let web = dest_url.starts_with("http://") || dest_url.starts_with("https://");
                if web && !remote_images && text[range.clone()].starts_with('!') {
                    unbang.push(range.start);
                }
            }
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::CodeBlock) => {
                depth -= 1;
                if let Some((start, kind, source)) = open.take() {
                    drawings.push((start..range.end, kind, source));
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(t) => {
                if let Some((.., source)) = &mut open {
                    source.push_str(&t);
                }
            }
            _ => {}
        }
    }

    let mut pos = 0;
    let push_markdown = |from: usize, to: usize, out: &mut Vec<Segment<'_>>| {
        let mut md = String::with_capacity(to - from);
        let mut last = from;
        for &bang in unbang.iter().filter(|&&b| b >= from && b < to) {
            md.push_str(&text[last..bang]);
            last = bang + 1;
        }
        md.push_str(&text[last..to]);
        if !md.trim().is_empty() {
            out.push(Segment::Markdown(md));
        }
    };
    for (range, kind, source) in drawings {
        push_markdown(pos, range.start, &mut out);
        out.push(Segment::Drawing {
            kind,
            source,
            raw: &text[range.clone()],
        });
        pos = range.end;
    }
    push_markdown(pos, text.len(), &mut out);
    out
}

/// Whether a fenced code block's text ends with its closing fence.
fn fence_is_closed(block: &str) -> bool {
    let mut lines = block.trim_end().lines();
    let Some(first) = lines.next() else {
        return false;
    };
    let opener = first.trim_start();
    let fence_char = opener.chars().next().unwrap_or('`');
    let fence_len = opener.chars().take_while(|&c| c == fence_char).count();
    lines.next_back().is_some_and(|last| {
        let last = last.trim();
        last.len() >= fence_len && last.chars().all(|c| c == fence_char)
    })
}

/// Show a reply's markdown, drawing its diagrams and SVGs.
pub fn show(
    ui: &mut egui::Ui,
    md_cache: &mut CommonMarkCache,
    drawings: &mut DrawingCache,
    id: egui::Id,
    text: &str,
    remote_images: bool,
    actions: &mut Vec<Action>,
) {
    for (i, segment) in segments(text, remote_images).into_iter().enumerate() {
        match segment {
            Segment::Markdown(md) => {
                // Prose in the reading serif; code and headings keep their own styles.
                let ui = &mut *ui;
                let previous = ui.style().override_text_style.clone();
                ui.style_mut().override_text_style = Some(theme::reading());
                CommonMarkViewer::new()
                    .max_image_width(Some(ui.available_width() as usize))
                    // Warm themes to sit with the paper-and-ink palette.
                    .syntax_theme_dark("base16-mocha.dark")
                    .syntax_theme_light("Solarized (light)")
                    .show_with_id(id.with(i), ui, md_cache, &md);
                ui.style_mut().override_text_style = previous;
            }
            Segment::Drawing { kind, source, raw } => {
                drawing(ui, drawings, kind, &source, raw, actions)
            }
        }
    }
}

fn drawing(
    ui: &mut egui::Ui,
    cache: &mut DrawingCache,
    kind: DrawingKind,
    source: &str,
    raw: &str,
    actions: &mut Vec<Action>,
) {
    let what = match kind {
        DrawingKind::Mermaid => "diagram",
        DrawingKind::Svg => "SVG",
    };
    match cache.get(kind, source) {
        Ok(d) => {
            let d = d.clone();
            ui.add_space(4.0);
            // Drawings assume a light page, so they sit on near-white in both themes.
            let clicked = egui::Frame::new()
                .fill(egui::Color32::from_rgb(252, 251, 247))
                .stroke(egui::Stroke::new(
                    1.0,
                    theme::border(ui.visuals().dark_mode),
                ))
                .corner_radius(2)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.add(
                        // Natural size, scaled down to fit (not `shrink_to_fit`, which sizes
                        // from the available height: zero here).
                        egui::Image::from_bytes(d.uri.clone(), d.svg.clone())
                            .fit_to_original_size(1.0)
                            .max_width(ui.available_width())
                            .max_height(600.0)
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text("Click to enlarge")
                    .clicked()
                })
                .inner;
            if clicked {
                actions.push(Action::ViewImage(
                    d.uri.clone(),
                    d.svg.clone(),
                    format!("{what}.svg"),
                ));
            }
            match text_actions(ui, &["copy source", "save as svg"]) {
                Some(0) => actions.push(Action::Copy(source.to_owned())),
                Some(1) => actions.push(Action::SaveSvg(d.svg.to_vec())),
                _ => {}
            }
        }
        Err(e) => {
            ui.label(
                RichText::new(format!("couldn't draw this {what}: {e}"))
                    .small()
                    .color(ui.visuals().warn_fg_color),
            );
            code_block(ui, raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawing(kind: DrawingKind, source: &str) -> impl Fn(&Segment<'_>) -> bool {
        move |s| matches!(s, Segment::Drawing { kind: k, source: src, .. } if *k == kind && src == source)
    }

    #[test]
    fn splits_out_closed_diagrams_and_svgs() {
        let text = "Intro\n\n```mermaid\nflowchart LR\n  A-->B\n```\n\nMiddle\n\n\
                    <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"></svg>\n\n\
                    ```svg\n<svg width=\"1\" height=\"1\"/>\n```\n\nEnd";
        let s = segments(text, false);
        assert_eq!(s.len(), 6, "{s:#?}");
        assert_eq!(s[0], Segment::Markdown("Intro\n\n".into()));
        assert!(drawing(DrawingKind::Mermaid, "flowchart LR\n  A-->B\n")(
            &s[1]
        ));
        assert!(matches!(&s[2], Segment::Markdown(m) if m.trim() == "Middle"));
        assert!(
            matches!(&s[3], Segment::Drawing { kind: DrawingKind::Svg, source, .. } if source.starts_with("<svg") && source.ends_with("</svg>"))
        );
        assert!(drawing(
            DrawingKind::Svg,
            "<svg width=\"1\" height=\"1\"/>\n"
        )(&s[4]));
        assert!(matches!(&s[5], Segment::Markdown(m) if m.trim() == "End"));
    }

    #[test]
    fn unclosed_or_nested_blocks_stay_markdown() {
        // Still streaming: no closing fence yet.
        let streaming = "```mermaid\nflowchart LR\n  A-->B\n";
        assert_eq!(
            segments(streaming, false),
            [Segment::Markdown(streaming.into())]
        );
        // Other languages, and blocks inside lists, are left to the markdown viewer.
        let text = "```rust\nfn main() {}\n```\n\n- item\n\n  ```mermaid\n  flowchart LR\n  ```\n";
        assert_eq!(segments(text, false), [Segment::Markdown(text.into())]);
    }

    #[test]
    fn web_images_become_links_unless_allowed() {
        let text =
            "See ![chart](https://example.com/c.png) and ![local](data:image/png;base64,AA==)";
        assert_eq!(
            segments(text, false),
            [Segment::Markdown(
                "See [chart](https://example.com/c.png) and ![local](data:image/png;base64,AA==)"
                    .into()
            )]
        );
        assert_eq!(segments(text, true), [Segment::Markdown(text.into())]);
    }

    #[test]
    fn renders_mermaid_and_reports_errors() {
        let mut cache = DrawingCache::default();
        let ok = cache.get(
            DrawingKind::Mermaid,
            "flowchart LR\n  A[Start] --> B[End]\n",
        );
        let d = ok.as_ref().unwrap();
        assert!(d.uri.ends_with(".svg"));
        assert!(std::str::from_utf8(&d.svg).unwrap().contains("<svg"));
        assert!(
            cache
                .get(DrawingKind::Mermaid, "this is not mermaid")
                .is_err()
        );
    }
}

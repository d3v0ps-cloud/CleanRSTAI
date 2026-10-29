//! Save a reply or an artifact as a Word document, a PDF or an Excel workbook (Settings →
//! General, "Save as files").
//!
//! Everything here is pure: Markdown (or CSV) in, file bytes out. The Markdown is read into a
//! small block model ([`Block`]) and each format is a writer over that model. Word and Excel
//! are plain OOXML written with the `zip` crate already in the tree; PDF is laid out by
//! `genpdf` (pure Rust) with the static fonts in `assets/fonts`.

use std::io::{Cursor, Write};
use std::iter::Peekable;

use anyhow::{Result, anyhow, bail};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use genpdf::style::Style;
use genpdf::{Margins, elements, fonts};

use crate::config::SaveAsSettings;

/// A run of text with the emphasis it had.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

/// The inline text of one table cell or paragraph.
pub type Cell = Vec<Run>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub header: Vec<Cell>,
    pub rows: Vec<Vec<Cell>>,
}

/// The block-level structure of a Markdown document, as far as the saved files show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        runs: Vec<Run>,
    },
    Paragraph(Vec<Run>),
    /// Each item is the blocks inside it (a paragraph, then any nested lists).
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
    /// A code block's text, without its fence.
    Code(String),
    Quote(Vec<Block>),
    Table(Table),
    Rule,
}

/// The file types a reply or artifact can be saved as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Docx,
    Pdf,
    Xlsx,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Docx, Format::Pdf, Format::Xlsx];

    /// The menu entry.
    pub fn label(self) -> &'static str {
        match self {
            Format::Docx => "Word document (.docx)",
            Format::Pdf => "PDF (.pdf)",
            Format::Xlsx => "Excel workbook (.xlsx)",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Docx => "docx",
            Format::Pdf => "pdf",
            Format::Xlsx => "xlsx",
        }
    }

    pub fn enabled(self, settings: &SaveAsSettings) -> bool {
        match self {
            Format::Docx => settings.docx,
            Format::Pdf => settings.pdf,
            Format::Xlsx => settings.xlsx,
        }
    }
}

/// What is being saved: a reply's Markdown, a code artifact, or a CSV/TSV artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Markdown(String),
    Code {
        lang: String,
        text: String,
    },
    /// Comma- or tab-separated values.
    Delimited {
        text: String,
        separator: char,
    },
}

impl Content {
    /// The formats offered for this content, in menu order, among those enabled in Settings.
    /// Excel needs a table: a Markdown table, or a CSV/TSV file.
    pub fn formats(&self, settings: &SaveAsSettings) -> Vec<Format> {
        let xlsx = match self {
            Content::Markdown(md) => has_tables(md),
            Content::Code { .. } => false,
            Content::Delimited { .. } => true,
        };
        Format::ALL
            .into_iter()
            .filter(|f| f.enabled(settings))
            .filter(|f| *f != Format::Xlsx || xlsx)
            .collect()
    }

    /// The content as Markdown: code gets a fence, delimited text becomes a table.
    pub fn to_markdown(&self) -> String {
        match self {
            Content::Markdown(md) => md.clone(),
            Content::Code { lang, text } => {
                let text = text.trim_end_matches('\n');
                let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                format!("{fence}{lang}\n{text}\n{fence}\n")
            }
            Content::Delimited { text, separator } => delimited_markdown(text, *separator),
        }
    }

    /// The file's bytes in `format`.
    pub fn render(&self, format: Format) -> Result<Vec<u8>> {
        match (format, self) {
            (Format::Docx, _) => to_docx(&self.to_markdown()),
            (Format::Pdf, _) => to_pdf(&self.to_markdown()),
            (Format::Xlsx, Content::Markdown(md)) => to_xlsx(md),
            (Format::Xlsx, Content::Delimited { text, separator }) => {
                delimited_to_xlsx(text, *separator)
            }
            (Format::Xlsx, Content::Code { .. }) => bail!("it has no table to save"),
        }
    }
}

impl Content {
    /// What the artifact panel saves for an artifact: Markdown documents, code and CSV or TSV
    /// files. Pages and drawings have no such formats.
    pub fn for_artifact(artifact: &crate::artifacts::Artifact) -> Option<Content> {
        use crate::artifacts::Kind;
        match artifact.kind {
            Kind::Markdown => Some(Content::Markdown(artifact.source.clone())),
            Kind::Code => match artifact.lang.to_ascii_lowercase().as_str() {
                "csv" => Some(Content::Delimited {
                    text: artifact.source.clone(),
                    separator: ',',
                }),
                "tsv" => Some(Content::Delimited {
                    text: artifact.source.clone(),
                    separator: '\t',
                }),
                _ => Some(Content::Code {
                    lang: artifact.lang.clone(),
                    text: artifact.source.clone(),
                }),
            },
            Kind::Html | Kind::Svg | Kind::Mermaid => None,
        }
    }
}

/// A title as a file name without an extension it already carries ("notes.md" is "notes", but
/// "Q3.2 plan" keeps its dot), so the format's extension is added once.
pub fn file_stem(title: &str) -> &str {
    match title.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            stem
        }
        _ => title,
    }
}

/// Whether a Markdown document contains a table.
pub fn has_tables(markdown: &str) -> bool {
    !tables_in(&blocks(markdown)).is_empty()
}

/// Read Markdown into blocks.
pub fn blocks(markdown: &str) -> Vec<Block> {
    let mut events = Parser::new_ext(markdown, crate::artifacts::markdown_options()).peekable();
    parse_blocks(&mut events)
}

/// Blocks up to the end of the enclosing container (consumed), or of the document.
fn parse_blocks<'a, I>(events: &mut Peekable<I>) -> Vec<Block>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut out = Vec::new();
    loop {
        match events.next() {
            None | Some(Event::End(_)) => break,
            Some(Event::Start(Tag::Paragraph)) => {
                let runs = inline_runs(None, events);
                close(events, TagEnd::Paragraph);
                push_paragraph(&mut out, runs);
            }
            Some(Event::Start(Tag::Heading { level, .. })) => {
                let runs = inline_runs(None, events);
                close(events, TagEnd::Heading(level));
                if !is_blank(&runs) {
                    out.push(Block::Heading {
                        level: level as u8,
                        runs,
                    });
                }
            }
            Some(Event::Start(Tag::BlockQuote(_))) => out.push(Block::Quote(parse_blocks(events))),
            Some(Event::Start(Tag::CodeBlock(_))) => {
                let mut text = String::new();
                loop {
                    match events.next() {
                        Some(Event::Text(t)) => text.push_str(&t),
                        None | Some(Event::End(_)) => break,
                        Some(_) => {}
                    }
                }
                out.push(Block::Code(text.trim_end_matches('\n').to_owned()));
            }
            Some(Event::Start(Tag::List(start))) => {
                let mut items = Vec::new();
                loop {
                    match events.next() {
                        Some(Event::Start(Tag::Item)) => items.push(parse_blocks(events)),
                        None | Some(Event::End(_)) => break,
                        Some(_) => {}
                    }
                }
                out.push(Block::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items,
                });
            }
            Some(Event::Start(Tag::Table(_))) => out.push(Block::Table(table(events))),
            Some(Event::Rule) => out.push(Block::Rule),
            // Tight list items and table-less text: inline events directly in a block.
            Some(event) if is_inline(&event) => {
                let runs = inline_runs(Some(event), events);
                push_paragraph(&mut out, runs);
            }
            // Footnotes, metadata and anything else with blocks inside.
            Some(Event::Start(_)) => out.extend(parse_blocks(events)),
            Some(_) => {}
        }
    }
    out
}

fn push_paragraph(out: &mut Vec<Block>, runs: Vec<Run>) {
    if !is_blank(&runs) {
        out.push(Block::Paragraph(runs));
    }
}

fn is_blank(runs: &[Run]) -> bool {
    runs.iter().all(|r| r.text.trim().is_empty())
}

/// Consume `end` if it is next (the events of a block end where its inline content does).
fn close<'a, I>(events: &mut Peekable<I>, end: TagEnd)
where
    I: Iterator<Item = Event<'a>>,
{
    if matches!(events.peek(), Some(Event::End(e)) if *e == end) {
        events.next();
    }
}

fn table<'a, I>(events: &mut Peekable<I>) -> Table
where
    I: Iterator<Item = Event<'a>>,
{
    let mut table = Table {
        header: Vec::new(),
        rows: Vec::new(),
    };
    loop {
        match events.next() {
            Some(Event::Start(Tag::TableHead)) => table.header = row(events),
            Some(Event::Start(Tag::TableRow)) => table.rows.push(row(events)),
            None | Some(Event::End(_)) => break,
            Some(_) => {}
        }
    }
    table
}

fn row<'a, I>(events: &mut Peekable<I>) -> Vec<Cell>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut cells = Vec::new();
    loop {
        match events.next() {
            Some(Event::Start(Tag::TableCell)) => {
                cells.push(inline_runs(None, events));
                close(events, TagEnd::TableCell);
            }
            None | Some(Event::End(_)) => break,
            Some(_) => {}
        }
    }
    cells
}

/// Whether an event belongs to running text rather than to a block.
fn is_inline(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Text(_)
            | Event::Code(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::TaskListMarker(_)
            | Event::FootnoteReference(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Start(
                Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Superscript
                    | Tag::Subscript
                    | Tag::Link { .. }
                    | Tag::Image { .. }
            )
            | Event::End(
                TagEnd::Emphasis
                    | TagEnd::Strong
                    | TagEnd::Strikethrough
                    | TagEnd::Superscript
                    | TagEnd::Subscript
                    | TagEnd::Link
                    | TagEnd::Image
            )
    )
}

/// The runs of the inline events that follow (and `first`, when given).
fn inline_runs<'a, I>(first: Option<Event<'a>>, events: &mut Peekable<I>) -> Vec<Run>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut state = Inline::default();
    if let Some(event) = first {
        state.event(event);
    }
    while matches!(events.peek(), Some(e) if is_inline(e)) {
        if let Some(event) = events.next() {
            state.event(event);
        }
    }
    state.runs
}

#[derive(Default)]
struct Inline {
    runs: Vec<Run>,
    bold: u32,
    italic: u32,
    /// Open links and images: where their text starts, and the address.
    links: Vec<(usize, String, bool)>,
}

impl Inline {
    fn push(&mut self, text: impl Into<String>, code: bool) {
        self.runs.push(Run {
            text: text.into(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            code,
        });
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Text(t) => self.push(t.to_string(), false),
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                self.push(t.to_string(), true)
            }
            Event::SoftBreak => self.push(" ", false),
            Event::HardBreak => self.push("\n", false),
            Event::TaskListMarker(done) => self.push(if done { "[x] " } else { "[ ] " }, false),
            Event::FootnoteReference(label) => self.push(format!("[{label}]"), false),
            Event::Start(Tag::Strong) => self.bold += 1,
            Event::End(TagEnd::Strong) => self.bold = self.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => self.italic += 1,
            Event::End(TagEnd::Emphasis) => self.italic = self.italic.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => {
                self.links
                    .push((self.runs.len(), dest_url.to_string(), false))
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.links
                    .push((self.runs.len(), dest_url.to_string(), true))
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                if let Some((start, dest, _image)) = self.links.pop() {
                    self.close_link(start, &dest);
                }
            }
            // Strikethrough, super- and subscript and raw HTML are written as plain text.
            _ => {}
        }
    }

    /// A link is its text and then its address, unless the text already is the address.
    fn close_link(&mut self, start: usize, dest: &str) {
        let shown: String = self.runs[start..].iter().map(|r| r.text.as_str()).collect();
        let dest = dest.trim();
        if dest.is_empty() || dest.starts_with("data:") || shown.trim() == dest {
            return;
        }
        self.push(format!(" ({dest})"), false);
    }
}

// ----- Word (.docx) ---------------------------------------------------------------------------

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// Text width between A4 margins of 1 inch, in twentieths of a point.
const DOCX_TEXT_WIDTH: u32 = 9026;
/// One indent step for lists and quotes, in twentieths of a point.
const DOCX_STEP: u32 = 360;

const DOCX_CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

const DOCX_DOC_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;

const DOCX_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style></w:styles>"#;

/// A Word document with the Markdown's headings, paragraphs (with bold, italic, code and link
/// addresses), nested lists, code blocks, quotes, tables and rules.
pub fn to_docx(markdown: &str) -> Result<Vec<u8>> {
    let mut body = String::new();
    docx_blocks(&blocks(markdown), 0, false, &mut body);
    if body.is_empty() {
        body.push_str("<w:p/>");
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W_NS}" xmlns:r="{R_NS}"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    zip_parts(&[
        ("[Content_Types].xml", DOCX_CONTENT_TYPES),
        ("_rels/.rels", ROOT_RELS),
        ("word/document.xml", document.as_str()),
        ("word/_rels/document.xml.rels", DOCX_DOC_RELS),
        ("word/styles.xml", DOCX_STYLES),
    ])
}

/// How one Word paragraph is laid out.
#[derive(Default)]
struct Layout {
    style: Option<String>,
    /// Left indent and the hanging indent of a list marker, in twentieths of a point.
    left: u32,
    hanging: u32,
    /// A bar at the left edge (block quotes).
    quote: bool,
    /// A grey background (code lines).
    shaded: bool,
}

fn docx_blocks(blocks: &[Block], indent: u32, quote: bool, out: &mut String) {
    for block in blocks {
        match block {
            Block::Heading { level, runs } => {
                let layout = Layout {
                    style: Some(format!("Heading{}", (*level).clamp(1, 3))),
                    left: indent,
                    quote,
                    ..Layout::default()
                };
                docx_para(out, &layout, runs, false);
            }
            Block::Paragraph(runs) => docx_para(
                out,
                &Layout {
                    left: indent,
                    quote,
                    ..Layout::default()
                },
                runs,
                false,
            ),
            Block::List {
                ordered,
                start,
                items,
            } => {
                for (i, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}.", start + i as u64)
                    } else {
                        "•".to_owned()
                    };
                    let left = indent + DOCX_STEP;
                    let layout = Layout {
                        left,
                        hanging: DOCX_STEP,
                        quote,
                        ..Layout::default()
                    };
                    let rest = match item.first() {
                        Some(Block::Paragraph(runs)) => {
                            let mut runs = runs.clone();
                            runs.insert(
                                0,
                                Run {
                                    text: format!("{marker}\t"),
                                    ..Run::default()
                                },
                            );
                            docx_para(out, &layout, &runs, false);
                            &item[1..]
                        }
                        _ => {
                            let run = Run {
                                text: format!("{marker}\t"),
                                ..Run::default()
                            };
                            docx_para(out, &layout, &[run], false);
                            &item[..]
                        }
                    };
                    docx_blocks(rest, left, quote, out);
                }
            }
            Block::Code(text) => {
                for line in text.split('\n') {
                    let run = Run {
                        text: line.to_owned(),
                        code: true,
                        ..Run::default()
                    };
                    docx_para(
                        out,
                        &Layout {
                            left: indent,
                            quote,
                            shaded: true,
                            ..Layout::default()
                        },
                        &[run],
                        false,
                    );
                }
                // Space after the block.
                out.push_str("<w:p/>");
            }
            Block::Quote(inner) => docx_blocks(inner, indent + DOCX_STEP, true, out),
            Block::Table(table) => {
                docx_table(table, out);
                // Word needs a paragraph between a table and whatever follows it.
                out.push_str("<w:p/>");
            }
            Block::Rule => out.push_str(
                r#"<w:p><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="BFBFBF"/></w:pBdr><w:spacing w:after="240"/></w:pPr></w:p>"#,
            ),
        }
    }
}

fn docx_para(out: &mut String, layout: &Layout, runs: &[Run], force_bold: bool) {
    let mut ppr = String::new();
    if let Some(style) = &layout.style {
        ppr += &format!(r#"<w:pStyle w:val="{style}"/>"#);
    }
    if layout.quote {
        ppr +=
            r#"<w:pBdr><w:left w:val="single" w:sz="18" w:space="8" w:color="BFBFBF"/></w:pBdr>"#;
    }
    if layout.shaded {
        ppr += r#"<w:shd w:val="clear" w:color="auto" w:fill="F3F1EA"/><w:spacing w:after="0"/>"#;
    }
    if layout.left > 0 || layout.hanging > 0 {
        ppr += &format!(
            r#"<w:ind w:left="{}" w:hanging="{}"/>"#,
            layout.left, layout.hanging
        );
    }
    out.push_str("<w:p>");
    if !ppr.is_empty() {
        out.push_str(&format!("<w:pPr>{ppr}</w:pPr>"));
    }
    out.push_str(&docx_runs(runs, force_bold));
    out.push_str("</w:p>");
}

fn docx_runs(runs: &[Run], force_bold: bool) -> String {
    let mut out = String::new();
    for run in runs {
        let mut rpr = String::new();
        if run.code {
            rpr += r#"<w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/>"#;
        }
        if run.bold || force_bold {
            rpr += "<w:b/>";
        }
        if run.italic {
            rpr += "<w:i/>";
        }
        if run.code {
            rpr += r#"<w:sz w:val="19"/>"#;
        }
        out.push_str("<w:r>");
        if !rpr.is_empty() {
            out.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
        }
        out.push_str(&docx_content(&run.text));
        out.push_str("</w:r>");
    }
    out
}

/// Text in a run: tabs and line breaks as their own elements.
fn docx_content(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<w:br/>");
        }
        for (j, part) in line.split('\t').enumerate() {
            if j > 0 {
                out.push_str("<w:tab/>");
            }
            if !part.is_empty() {
                out.push_str(&format!(
                    r#"<w:t xml:space="preserve">{}</w:t>"#,
                    xml_escape(part)
                ));
            }
        }
    }
    out
}

fn docx_table(table: &Table, out: &mut String) {
    let cols = table
        .header
        .len()
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }
    let width = DOCX_TEXT_WIDTH / cols as u32;
    let edge =
        |name: &str| format!(r#"<w:{name} w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>"#);
    out.push_str(&format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="{DOCX_TEXT_WIDTH}" w:type="dxa"/><w:tblBorders>{}{}{}{}{}{}</w:tblBorders><w:tblLayout w:type="fixed"/></w:tblPr><w:tblGrid>"#,
        edge("top"),
        edge("left"),
        edge("bottom"),
        edge("right"),
        edge("insideH"),
        edge("insideV"),
    ));
    for _ in 0..cols {
        out.push_str(&format!(r#"<w:gridCol w:w="{width}"/>"#));
    }
    out.push_str("</w:tblGrid>");
    let mut rows: Vec<(&[Cell], bool)> = Vec::new();
    if !table.header.is_empty() {
        rows.push((&table.header, true));
    }
    rows.extend(table.rows.iter().map(|r| (r.as_slice(), false)));
    for (cells, header) in rows {
        out.push_str("<w:tr>");
        if header {
            out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
        }
        for c in 0..cols {
            out.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:spacing w:after="0"/></w:pPr>{}</w:p></w:tc>"#,
                docx_runs(cells.get(c).map_or(&[], Vec::as_slice), header),
            ));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
}

// ----- PDF ------------------------------------------------------------------------------------

const SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/InstrumentSans-Regular.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/InstrumentSans-Bold.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// Body text size, in points.
const PDF_BODY_SIZE: u8 = 10;

/// A PDF on A4 pages with the Markdown laid out as in [`to_docx`]. Text is set in Instrument
/// Sans and code in IBM Plex Mono. Italics use the regular face (the font has no italic), and
/// characters the fonts lack are not drawn.
pub fn to_pdf(markdown: &str) -> Result<Vec<u8>> {
    let font =
        |data: &[u8]| fonts::FontData::new(data.to_vec(), None).map_err(|e| anyhow!("font: {e}"));
    let sans = fonts::FontFamily {
        regular: font(SANS_REGULAR)?,
        bold: font(SANS_BOLD)?,
        italic: font(SANS_REGULAR)?,
        bold_italic: font(SANS_BOLD)?,
    };
    let mono_data = font(MONO)?;
    let mono_family = fonts::FontFamily {
        regular: mono_data.clone(),
        bold: mono_data.clone(),
        italic: mono_data.clone(),
        bold_italic: mono_data,
    };

    let mut doc = genpdf::Document::new(sans);
    let mono = doc.add_font_family(mono_family);
    doc.set_paper_size(genpdf::PaperSize::A4);
    doc.set_font_size(PDF_BODY_SIZE);
    let mut decorator = genpdf::SimplePageDecorator::new();
    decorator.set_margins(Margins::trbl(20, 20, 20, 20));
    doc.set_page_decorator(decorator);

    let mut layout = elements::LinearLayout::vertical();
    let mut ctx = Pdf { mono };
    ctx.blocks(&blocks(markdown), &mut layout);
    doc.push(layout);

    let mut out = Vec::new();
    doc.render(&mut out).map_err(|e| anyhow!("PDF: {e}"))?;
    Ok(out)
}

/// The fonts a PDF is laid out with.
struct Pdf {
    mono: genpdf::fonts::FontFamily<genpdf::fonts::Font>,
}

impl Pdf {
    fn blocks(&mut self, blocks: &[Block], out: &mut genpdf::elements::LinearLayout) {
        for block in blocks {
            match block {
                Block::Heading { level, runs } => {
                    let size: u8 = match *level {
                        1 => 16,
                        2 => 14,
                        _ => 12,
                    };
                    let mut p = elements::Paragraph::default();
                    for run in runs {
                        let style = self.style(run).with_font_size(size).bold();
                        p.push_styled(pdf_text(&run.text), style);
                    }
                    out.push(p);
                }
                Block::Paragraph(runs) => out.push(self.paragraph(runs, false)),
                Block::List {
                    ordered,
                    start,
                    items,
                } => {
                    let mut inners = Vec::new();
                    for item in items {
                        let mut inner = elements::LinearLayout::vertical();
                        self.blocks(item, &mut inner);
                        inners.push(inner);
                    }
                    // The two list kinds are different types, so each fills its own.
                    if *ordered {
                        let mut list = elements::OrderedList::with_start(*start as usize);
                        for inner in inners {
                            list.push(inner);
                        }
                        out.push(list);
                    } else {
                        let mut list = elements::UnorderedList::new();
                        for inner in inners {
                            list.push(inner);
                        }
                        out.push(list);
                    }
                }
                Block::Code(text) => {
                    let mut inner = elements::LinearLayout::vertical();
                    for line in text.lines() {
                        let line = pdf_text(&line.replace('\t', "    "));
                        let mut p = elements::Paragraph::default();
                        let style = Style::new().with_font_family(self.mono).with_font_size(9);
                        p.push_styled(
                            if line.is_empty() {
                                " ".to_owned()
                            } else {
                                line
                            },
                            style,
                        );
                        inner.push(p);
                    }
                    out.push(elements::FramedElement::new(elements::PaddedElement::new(
                        inner,
                        Margins::all(3),
                    )));
                }
                Block::Quote(inner_blocks) => {
                    let mut inner = elements::LinearLayout::vertical();
                    self.blocks(inner_blocks, &mut inner);
                    out.push(elements::PaddedElement::new(
                        inner,
                        Margins::trbl(0, 0, 0, 8),
                    ));
                }
                Block::Table(table) => self.table(table, out),
                Block::Rule => {
                    let mut p = elements::Paragraph::default();
                    p.push_styled("–".repeat(60), Style::new().with_font_size(8));
                    out.push(p);
                }
            }
            out.push(elements::Break::new(0.5));
        }
    }

    fn style(&self, run: &Run) -> genpdf::style::Style {
        let mut style = genpdf::style::Style::new();
        if run.bold {
            style.set_bold();
        }
        if run.italic {
            style.set_italic();
        }
        if run.code {
            style.set_font_family(self.mono);
        }
        style
    }

    fn paragraph(&self, runs: &[Run], bold: bool) -> genpdf::elements::Paragraph {
        let mut p = genpdf::elements::Paragraph::default();
        for run in runs {
            let mut style = self.style(run);
            if bold {
                style.set_bold();
            }
            p.push_styled(pdf_text(&run.text), style);
        }
        p
    }

    fn table(&self, table: &Table, out: &mut genpdf::elements::LinearLayout) {
        let cols = table
            .header
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return;
        }
        let mut layout = elements::TableLayout::new(vec![1; cols]);
        layout.set_cell_decorator(elements::FrameCellDecorator::new(true, true, false));
        let header = !table.header.is_empty();
        let rows = header
            .then_some(&table.header)
            .into_iter()
            .chain(table.rows.iter());
        for (i, cells) in rows.enumerate() {
            let mut row = layout.row();
            for c in 0..cols {
                let runs = cells.get(c).map_or(&[][..], Vec::as_slice);
                row = row.element(self.paragraph(runs, header && i == 0));
            }
            // Every row has `cols` cells, so this cannot fail.
            let _ = row.push();
        }
        out.push(layout);
    }
}

/// Text as the PDF writer sets it: line breaks and tabs become spaces.
fn pdf_text(text: &str) -> String {
    text.replace(['\n', '\t'], " ")
}

// ----- Excel (.xlsx) and CSV ------------------------------------------------------------------

const XLSX_CONTENT_TYPES_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#;

const XLSX_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;

/// A cell of a workbook sheet.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Text(String),
    Number(f64),
}

struct Sheet {
    name: String,
    rows: Vec<Vec<Value>>,
    /// Whether the first row is a header (bold).
    header: bool,
}

/// An Excel workbook with one sheet per Markdown table ("Table 1", "Table 2", …). Cells that
/// read as numbers are stored as numbers. Errors when the Markdown has no table.
pub fn to_xlsx(markdown: &str) -> Result<Vec<u8>> {
    let blocks = blocks(markdown);
    let tables = tables_in(&blocks);
    if tables.is_empty() {
        bail!("it has no table to save");
    }
    let sheets: Vec<Sheet> = tables
        .iter()
        .enumerate()
        .map(|(i, table)| {
            let mut rows: Vec<Vec<Value>> = Vec::new();
            let header = !table.header.is_empty();
            if header {
                rows.push(
                    table
                        .header
                        .iter()
                        .map(|c| cell_value_for(c, true))
                        .collect(),
                );
            }
            for row in &table.rows {
                rows.push(row.iter().map(|c| cell_value_for(c, false)).collect());
            }
            Sheet {
                name: format!("Table {}", i + 1),
                rows,
                header,
            }
        })
        .collect();
    write_xlsx(&sheets)
}

/// An Excel workbook with the CSV or TSV text as one sheet.
pub fn delimited_to_xlsx(text: &str, separator: char) -> Result<Vec<u8>> {
    let rows = parse_delimited(text, separator);
    if rows.is_empty() {
        bail!("it has no rows to save");
    }
    let rows = rows
        .iter()
        .map(|row| row.iter().map(|c| cell_value(c)).collect())
        .collect();
    write_xlsx(&[Sheet {
        name: "Sheet1".to_owned(),
        rows,
        header: false,
    }])
}

fn cell_value_for(cell: &[Run], header: bool) -> Value {
    let text: String = cell.iter().map(|r| r.text.as_str()).collect();
    if header {
        Value::Text(text.trim().to_owned())
    } else {
        cell_value(text.trim())
    }
}

fn cell_value(text: &str) -> Value {
    match number(text) {
        Some(n) => Value::Number(n),
        None => Value::Text(text.to_owned()),
    }
}

/// A number as a spreadsheet would read it: plain digits with an optional sign and point.
/// Anything with leading zeros, exponents, separators or more than 15 digits stays text, so
/// codes and identifiers keep their form.
pub fn number(text: &str) -> Option<f64> {
    let t = text.trim();
    let body = t.strip_prefix('-').unwrap_or(t);
    let digits = body.chars().filter(char::is_ascii_digit).count();
    if digits == 0 || digits > 15 || !body.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if body.len() > 1 && body.starts_with('0') && !body.starts_with("0.") {
        return None;
    }
    let n: f64 = t.parse().ok()?;
    n.is_finite().then_some(n)
}

/// Every table in the blocks, including those in quotes and lists, in order.
fn tables_in(blocks: &[Block]) -> Vec<&Table> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            Block::Table(table) => out.push(table),
            Block::Quote(inner) => out.extend(tables_in(inner)),
            Block::List { items, .. } => {
                for item in items {
                    out.extend(tables_in(item));
                }
            }
            _ => {}
        }
    }
    out
}

/// Split CSV or TSV text into rows of fields. Quotes follow RFC 4180 (`""` is a quote, fields
/// may hold separators and line breaks); blank lines are skipped.
pub fn parse_delimited(text: &str, separator: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() {
            quoted = true;
        } else if c == separator {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].is_empty()));
    rows
}

/// CSV or TSV as a Markdown table (the first row is the header).
fn delimited_markdown(text: &str, separator: char) -> String {
    let rows = parse_delimited(text, separator);
    let Some((header, body)) = rows.split_first() else {
        return String::new();
    };
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let line = |cells: &[String]| {
        let mut out = String::from("|");
        for c in 0..cols {
            let cell = cells.get(c).map_or("", String::as_str);
            out.push(' ');
            out.push_str(&md_escape(&cell.replace(['\n', '\r'], " ")));
            out.push_str(" |");
        }
        out.push('\n');
        out
    };
    let mut md = line(header);
    md.push('|');
    md.push_str(&" --- |".repeat(cols));
    md.push('\n');
    for row in body {
        md.push_str(&line(row));
    }
    md
}

/// Escape the characters Markdown would read as formatting or as a table divider.
fn md_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\`*_[]<>|&#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The package parts of a workbook with `sheets`.
fn write_xlsx(sheets: &[Sheet]) -> Result<Vec<u8>> {
    let mut sheet_parts: Vec<(String, String)> = Vec::new();
    for (i, sheet) in sheets.iter().enumerate() {
        sheet_parts.push((
            format!("xl/worksheets/sheet{}.xml", i + 1),
            sheet_xml(sheet),
        ));
    }

    let mut content_types = XLSX_CONTENT_TYPES_HEAD.to_owned();
    for i in 1..=sheets.len() {
        content_types.push_str(&format!(
            r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        ));
    }
    content_types.push_str("</Types>");

    let mut workbook = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (i, sheet) in sheets.iter().enumerate() {
        let n = i + 1;
        workbook.push_str(&format!(
            r#"<sheet name="{}" sheetId="{n}" r:id="rId{n}"/>"#,
            xml_escape(&sheet.name)
        ));
        rels.push_str(&format!(
            r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#
        ));
    }
    let styles_id = sheets.len() + 1;
    workbook.push_str("</sheets></workbook>");
    rels.push_str(&format!(
        r#"<Relationship Id="rId{styles_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#
    ));

    let mut parts: Vec<(String, String)> = vec![
        ("[Content_Types].xml".to_owned(), content_types),
        ("_rels/.rels".to_owned(), ROOT_RELS_XLSX.to_owned()),
        ("xl/workbook.xml".to_owned(), workbook),
        ("xl/_rels/workbook.xml.rels".to_owned(), rels),
        ("xl/styles.xml".to_owned(), XLSX_STYLES.to_owned()),
    ];
    parts.extend(sheet_parts);
    zip_parts(&parts)
}

const ROOT_RELS_XLSX: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;

fn sheet_xml(sheet: &Sheet) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (r, row) in sheet.rows.iter().enumerate() {
        out.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (c, value) in row.iter().enumerate() {
            let reference = format!("{}{}", column_name(c), r + 1);
            let style = if sheet.header && r == 0 {
                r#" s="1""#
            } else {
                ""
            };
            match value {
                Value::Text(t) if t.is_empty() => {}
                Value::Text(t) => out.push_str(&format!(
                    r#"<c r="{reference}" t="inlineStr"{style}><is><t xml:space="preserve">{}</t></is></c>"#,
                    xml_escape(t)
                )),
                Value::Number(n) => out.push_str(&format!(
                    r#"<c r="{reference}"{style}><v>{n}</v></c>"#
                )),
            }
        }
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    out
}

/// A column's letters: 0 is A, 25 is Z, 26 is AA.
fn column_name(mut n: usize) -> String {
    let mut name = String::new();
    loop {
        name.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    name
}

// ----- shared -----------------------------------------------------------------------------------

fn zip_parts<N: AsRef<str>, D: AsRef<str>>(parts: &[(N, D)]) -> Result<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name.as_ref(), zip::write::SimpleFileOptions::default())?;
        zip.write_all(data.as_ref().as_bytes())?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Escape text for XML, dropping characters XML cannot hold.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c == '\t' || c == '\n' || c == '\r' || c >= ' ' => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Run {
        Run {
            text: text.to_owned(),
            ..Run::default()
        }
    }

    fn bold(text: &str) -> Run {
        Run {
            bold: true,
            ..run(text)
        }
    }

    fn italic(text: &str) -> Run {
        Run {
            italic: true,
            ..run(text)
        }
    }

    fn code(text: &str) -> Run {
        Run {
            code: true,
            ..run(text)
        }
    }

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![run(text)])
    }

    const REPORT: &str = "# Quarterly report\n\n\
        Revenue grew **12%** in Q3, *mostly* in `EMEA` with a [plan](https://example.com/plan).\n\n\
        ## Details\n\n\
        - one\n  - nested item\n- two\n\n\
        1. first\n2. second\n\n\
        > A quoted line.\n\n\
        ```\nlet x = 1;\n```\n\n\
        | Region | Sales |\n|---|---|\n| North | 420 |\n| South | 3.5 |\n\n\
        ---\n";

    #[test]
    fn reads_headings_paragraphs_and_inline_styles() {
        let b = blocks(REPORT);
        assert_eq!(
            b[0],
            Block::Heading {
                level: 1,
                runs: vec![run("Quarterly report")]
            }
        );
        assert_eq!(
            b[1],
            Block::Paragraph(vec![
                run("Revenue grew "),
                bold("12%"),
                run(" in Q3, "),
                italic("mostly"),
                run(" in "),
                code("EMEA"),
                run(" with a "),
                run("plan"),
                run(" (https://example.com/plan)"),
                run("."),
            ])
        );
        assert_eq!(
            b[2],
            Block::Heading {
                level: 2,
                runs: vec![run("Details")]
            }
        );
    }

    #[test]
    fn reads_nested_lists_quotes_code_tables_and_rules() {
        let b = blocks(REPORT);
        let Block::List {
            ordered: false,
            items,
            ..
        } = &b[3]
        else {
            panic!("bullet list: {:?}", b[3]);
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0], para("one"));
        let Block::List {
            ordered: false,
            items: nested,
            ..
        } = &items[0][1]
        else {
            panic!("nested list: {:?}", items[0]);
        };
        assert_eq!(nested, &vec![vec![para("nested item")]]);
        assert_eq!(items[1], vec![para("two")]);

        assert_eq!(
            b[4],
            Block::List {
                ordered: true,
                start: 1,
                items: vec![vec![para("first")], vec![para("second")]]
            }
        );
        assert_eq!(b[5], Block::Quote(vec![para("A quoted line.")]));
        assert_eq!(b[6], Block::Code("let x = 1;".to_owned()));
        let Block::Table(table) = &b[7] else {
            panic!("table: {:?}", b[7]);
        };
        assert_eq!(table.header, vec![vec![run("Region")], vec![run("Sales")]]);
        assert_eq!(
            table.rows,
            vec![
                vec![vec![run("North")], vec![run("420")]],
                vec![vec![run("South")], vec![run("3.5")]],
            ]
        );
        assert_eq!(b[8], Block::Rule);
        assert_eq!(b.len(), 9);
    }

    #[test]
    fn autolinks_are_not_repeated_and_images_keep_their_text() {
        assert_eq!(
            blocks("<https://a.example>"),
            vec![Block::Paragraph(vec![run("https://a.example")])]
        );
        assert_eq!(
            blocks("![a chart](https://x.example/c.png)"),
            vec![Block::Paragraph(vec![
                run("a chart"),
                run(" (https://x.example/c.png)")
            ])]
        );
        // Images embedded as data URLs show their alt text only.
        assert_eq!(
            blocks("![logo](data:image/png;base64,AAAA)"),
            vec![Block::Paragraph(vec![run("logo")])]
        );
    }

    #[test]
    fn task_items_and_raw_html_read_as_text() {
        let b = blocks("- [x] done\n- [ ] open\n\n<div>raw</div>\n");
        let Block::List { items, .. } = &b[0] else {
            panic!("{b:?}");
        };
        let joined = |b: &Block| match b {
            Block::Paragraph(runs) => runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(joined(&items[0][0]), "[x] done");
        assert_eq!(joined(&items[1][0]), "[ ] open");
        assert!(!format!("{b:?}").contains("<div>"));
    }

    #[test]
    fn csv_and_numbers() {
        assert_eq!(
            parse_delimited("a,\"b, c\",d\n1,\"say \"\"hi\"\"\",\"x\ny\"\r\n\n", ','),
            vec![
                vec!["a".to_owned(), "b, c".to_owned(), "d".to_owned()],
                vec!["1".to_owned(), "say \"hi\"".to_owned(), "x\ny".to_owned()],
            ]
        );
        assert_eq!(parse_delimited("a\tb", '\t'), vec![vec!["a", "b"]]);

        assert_eq!(number("420"), Some(420.0));
        assert_eq!(number(" -3.5 "), Some(-3.5));
        assert_eq!(number("0.25"), Some(0.25));
        assert_eq!(number("007"), None, "codes keep their zeros");
        assert_eq!(number("1,234"), None);
        assert_eq!(number("1e5"), None);
        assert_eq!(number("12345678901234567"), None, "too long to be exact");
        assert_eq!(number("North"), None);
        assert_eq!(number("-"), None);
    }

    #[test]
    fn code_fences_grow_around_backticks() {
        let md = Content::Code {
            lang: "md".into(),
            text: "a ```` b".into(),
        }
        .to_markdown();
        assert!(md.starts_with("`````md\n"), "{md}");
        assert!(
            blocks(&md)
                .iter()
                .any(|b| matches!(b, Block::Code(t) if t == "a ```` b"))
        );
    }

    #[test]
    fn formats_follow_settings_and_tables() {
        let all = SaveAsSettings::default();
        let text = Content::Markdown("Just words.".into());
        assert_eq!(text.formats(&all), vec![Format::Docx, Format::Pdf]);
        let with_table = Content::Markdown("| a |\n|---|\n| 1 |\n".into());
        assert_eq!(with_table.formats(&all), Format::ALL.to_vec());
        let csv = Content::Delimited {
            text: "a,b\n1,2\n".into(),
            separator: ',',
        };
        assert_eq!(csv.formats(&all), Format::ALL.to_vec());
        let code = Content::Code {
            lang: "rust".into(),
            text: "fn main() {}".into(),
        };
        assert_eq!(code.formats(&all), vec![Format::Docx, Format::Pdf]);

        let only_pdf = SaveAsSettings {
            docx: false,
            pdf: true,
            xlsx: false,
        };
        assert_eq!(with_table.formats(&only_pdf), vec![Format::Pdf]);
        assert!(
            with_table
                .formats(&SaveAsSettings {
                    docx: false,
                    pdf: false,
                    xlsx: false
                })
                .is_empty()
        );
        assert!(code.render(Format::Xlsx).is_err());
    }

    #[test]
    fn xlsx_needs_a_table() {
        assert!(to_xlsx("no tables here").is_err());
        assert!(!has_tables("no tables here"));
        assert!(has_tables("> | a |\n> |---|\n> | 1 |\n"));
    }

    #[test]
    fn csv_becomes_a_markdown_table_that_reads_back() {
        let md = delimited_markdown("name,n\n\"a|b\",7\n", ',');
        assert_eq!(md, "| name | n |\n| --- | --- |\n| a\\|b | 7 |\n");
        let b = blocks(&md);
        let Block::Table(t) = &b[0] else {
            panic!("{b:?}");
        };
        let text: String = t.rows[0][0].iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "a|b");
    }

    fn docx_text(bytes: &[u8]) -> String {
        crate::documents::extract(crate::documents::Format::Docx, bytes).unwrap()
    }

    #[test]
    fn docx_round_trips_through_the_document_reader() {
        let bytes = to_docx(REPORT).unwrap();
        assert_eq!(
            crate::documents::detect(&bytes).unwrap(),
            Some(crate::documents::Format::Docx)
        );
        let text = docx_text(&bytes);
        for want in [
            "Quarterly report",
            "Revenue grew 12% in Q3, mostly in EMEA with a plan (https://example.com/plan).",
            "Details",
            "nested item",
            "first",
            "A quoted line.",
            "let x = 1;",
            "Region",
            "Sales",
            "North",
            "420",
            "South",
            "3.5",
        ] {
            assert!(text.contains(want), "missing {want:?} in\n{text}");
        }
    }

    #[test]
    fn docx_escapes_markup_characters() {
        // Raw HTML is dropped; the ampersand and quotes must survive the XML escaping.
        let text = docx_text(&to_docx("Fish & chips <b>not bold</b> \"quoted\"").unwrap());
        assert!(text.contains("Fish & chips"), "{text}");
        assert!(text.contains("\"quoted\""), "{text}");
        assert!(!text.contains("<b>"), "{text}");
    }

    #[test]
    fn xlsx_round_trips_cells_and_number_types() {
        use calamine::{Data, Reader as _};
        let md = "| Region | Sales | Code |\n|---|---|---|\n| North | 420 | 007 |\n| South | 3.5 | |\n\n\
                  | A |\n|---|\n| 1 |\n";
        let bytes = to_xlsx(md).unwrap();
        let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).unwrap();
        assert_eq!(book.sheet_names(), vec!["Table 1", "Table 2"]);
        let first = book.worksheet_range("Table 1").unwrap();
        assert_eq!(
            first.get_value((0, 0)),
            Some(&Data::String("Region".into()))
        );
        assert_eq!(first.get_value((1, 1)), Some(&Data::Float(420.0)));
        assert_eq!(first.get_value((2, 1)), Some(&Data::Float(3.5)));
        assert_eq!(first.get_value((1, 2)), Some(&Data::String("007".into())));
        assert_eq!(first.get_value((2, 0)), Some(&Data::String("South".into())));
        let second = book.worksheet_range("Table 2").unwrap();
        assert_eq!(second.get_value((1, 0)), Some(&Data::Float(1.0)));
    }

    #[test]
    fn csv_round_trips_as_one_sheet() {
        use calamine::{Data, Reader as _};
        let bytes = delimited_to_xlsx("item,price\n\"pens, blue\",2.5\nnote,\n", ',').unwrap();
        let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes)).unwrap();
        assert_eq!(book.sheet_names(), vec!["Sheet1"]);
        let range = book.worksheet_range("Sheet1").unwrap();
        assert_eq!(
            range.get_value((1, 0)),
            Some(&Data::String("pens, blue".into()))
        );
        assert_eq!(range.get_value((1, 1)), Some(&Data::Float(2.5)));
        assert_eq!(range.get_value((2, 0)), Some(&Data::String("note".into())));
    }

    #[test]
    fn column_letters_run_past_z() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    #[test]
    fn pdf_has_the_words_and_several_pages() {
        let mut md = String::from("# Long report\n\nZEBRA-7 marks the start.\n\n");
        for i in 0..150 {
            md.push_str(&format!(
                "Paragraph {i} holds some ordinary sentences so the page fills up quickly.\n\n"
            ));
        }
        md.push_str(
            "| Region | Sales |\n|---|---|\n| North | 420 |\n\n```\nlet answer = 42;\n```\n",
        );
        let bytes = to_pdf(&md).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let text = crate::documents::extract(crate::documents::Format::Pdf, &bytes).unwrap();
        for want in [
            "Long report",
            "ZEBRA-7",
            "Paragraph 149",
            "North",
            "420",
            "let answer = 42;",
        ] {
            assert!(text.contains(want), "missing {want:?}");
        }
        assert!(text.contains("--- Page 2 ---"), "expected several pages");
    }

    #[test]
    fn pdf_of_an_empty_reply_is_still_a_pdf() {
        let bytes = to_pdf("").unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }

    #[test]
    fn artifacts_map_to_their_content() {
        use crate::artifacts::{Artifact, Kind};
        let artifact = |kind, lang: &str| Artifact {
            message: 0,
            index: 0,
            kind,
            lang: lang.to_owned(),
            title: "t".to_owned(),
            named: true,
            source: "a,b\n".to_owned(),
            range: 0..0,
        };
        assert_eq!(
            Content::for_artifact(&artifact(Kind::Markdown, "md")),
            Some(Content::Markdown("a,b\n".into()))
        );
        assert_eq!(
            Content::for_artifact(&artifact(Kind::Code, "CSV")),
            Some(Content::Delimited {
                text: "a,b\n".into(),
                separator: ','
            })
        );
        assert_eq!(
            Content::for_artifact(&artifact(Kind::Code, "tsv")),
            Some(Content::Delimited {
                text: "a,b\n".into(),
                separator: '\t'
            })
        );
        assert_eq!(
            Content::for_artifact(&artifact(Kind::Code, "rust")),
            Some(Content::Code {
                lang: "rust".into(),
                text: "a,b\n".into()
            })
        );
        assert_eq!(Content::for_artifact(&artifact(Kind::Html, "html")), None);
        assert_eq!(Content::for_artifact(&artifact(Kind::Svg, "svg")), None);
        assert_eq!(
            Content::for_artifact(&artifact(Kind::Mermaid, "mermaid")),
            None
        );
    }

    #[test]
    fn file_names_add_the_extension_once() {
        assert_eq!(file_stem("notes.md"), "notes");
        assert_eq!(file_stem("Q3.2 plan"), "Q3.2 plan");
        assert_eq!(file_stem("Q3.2"), "Q3.2");
        assert_eq!(file_stem("Sales report"), "Sales report");
        assert_eq!(file_stem(".hidden"), ".hidden");
    }

    #[test]
    fn older_settings_files_get_all_formats() {
        let mut value = serde_json::to_value(crate::config::GeneralSettings::default()).unwrap();
        value.as_object_mut().unwrap().remove("save_as");
        let general: crate::config::GeneralSettings = serde_json::from_value(value).unwrap();
        assert_eq!(general.save_as, SaveAsSettings::default());
    }
}

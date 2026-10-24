//! Text extraction for document attachments: PDF, Word, Excel and PowerPoint (current and
//! legacy binary formats), OpenDocument text, spreadsheets and presentations, and RTF. The
//! model gets the text; layout, images and most formatting are dropped. Formats are
//! recognised by their content, not their file name.

use std::borrow::Cow;
use std::io::{Cursor, Read};

use anyhow::{Context, Result, anyhow, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// The most a single zip entry may expand to, against zip bombs.
const MAX_ENTRY_BYTES: u64 = 200 * 1024 * 1024;

const CFB_MAGIC: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Docx,
    Doc,
    Odt,
    Rtf,
    Xlsx,
    Xls,
    Ods,
    Pptx,
    Ppt,
    Odp,
}

impl Format {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Doc => "application/msword",
            Self::Odt => "application/vnd.oasis.opendocument.text",
            Self::Rtf => "application/rtf",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Self::Xls => "application/vnd.ms-excel",
            Self::Ods => "application/vnd.oasis.opendocument.spreadsheet",
            Self::Pptx => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            }
            Self::Ppt => "application/vnd.ms-powerpoint",
            Self::Odp => "application/vnd.oasis.opendocument.presentation",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Docx | Self::Doc => "Word document",
            Self::Odt => "OpenDocument text",
            Self::Rtf => "RTF document",
            Self::Xlsx | Self::Xls => "Excel workbook",
            Self::Ods => "OpenDocument spreadsheet",
            Self::Pptx | Self::Ppt => "PowerPoint presentation",
            Self::Odp => "OpenDocument presentation",
        }
    }

    const ALL: [Format; 11] = [
        Self::Pdf,
        Self::Docx,
        Self::Doc,
        Self::Odt,
        Self::Rtf,
        Self::Xlsx,
        Self::Xls,
        Self::Ods,
        Self::Pptx,
        Self::Ppt,
        Self::Odp,
    ];

    pub fn from_mime(mime: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.mime() == mime)
    }
}

/// Recognise a document from its bytes. `Err` for a recognised container that can't be
/// read, such as a password-protected Office file.
pub fn detect(bytes: &[u8]) -> Result<Option<Format>> {
    // PDF readers accept the header anywhere in the first kilobyte.
    if bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
    {
        return Ok(Some(Format::Pdf));
    }
    if bytes.starts_with(b"{\\rtf") {
        return Ok(Some(Format::Rtf));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else {
            return Ok(None);
        };
        if let Ok(mut f) = zip.by_name("mimetype") {
            let mut mime = String::new();
            f.by_ref().take(200).read_to_string(&mut mime).ok();
            return Ok(match mime.trim() {
                m if m.starts_with("application/vnd.oasis.opendocument.text") => Some(Format::Odt),
                m if m.starts_with("application/vnd.oasis.opendocument.spreadsheet") => {
                    Some(Format::Ods)
                }
                m if m.starts_with("application/vnd.oasis.opendocument.presentation") => {
                    Some(Format::Odp)
                }
                _ => None,
            });
        }
        let has = |zip: &mut zip::ZipArchive<_>, name| zip.by_name(name).is_ok();
        return Ok(if has(&mut zip, "word/document.xml") {
            Some(Format::Docx)
        } else if has(&mut zip, "xl/workbook.xml") || has(&mut zip, "xl/workbook.bin") {
            Some(Format::Xlsx)
        } else if has(&mut zip, "ppt/presentation.xml") {
            Some(Format::Pptx)
        } else {
            None
        });
    }
    if bytes.starts_with(CFB_MAGIC) {
        let Ok(cfb) = cfb::CompoundFile::open(Cursor::new(bytes)) else {
            return Ok(None);
        };
        if cfb.is_stream("/EncryptedPackage") {
            bail!("it is password-protected; save an unprotected copy and attach that");
        }
        return Ok(if cfb.is_stream("/WordDocument") {
            Some(Format::Doc)
        } else if cfb.is_stream("/Workbook") || cfb.is_stream("/Book") {
            Some(Format::Xls)
        } else if cfb.is_stream("/PowerPoint Document") {
            Some(Format::Ppt)
        } else {
            None
        });
    }
    Ok(None)
}

/// The text of a document in `format`.
pub fn extract(format: Format, bytes: &[u8]) -> Result<String> {
    let text = match format {
        Format::Pdf => pdf(bytes)?,
        Format::Docx => docx(bytes)?,
        Format::Pptx => pptx(bytes)?,
        Format::Odt | Format::Odp => {
            let xml = zip_entry(
                &mut zip::ZipArchive::new(Cursor::new(bytes))?,
                "content.xml",
            )?;
            xml_text(&xml, Markup::Odf)?
        }
        Format::Xlsx | Format::Xls | Format::Ods => spreadsheet(bytes)?,
        Format::Doc => doc(bytes)?,
        Format::Ppt => ppt(bytes)?,
        Format::Rtf => rtf(bytes),
    };
    let text = tidy(&text);
    if text.is_empty() {
        if format == Format::Pdf {
            bail!("it has no text layer (a scanned PDF?); attach page images instead");
        }
        bail!("it has no text");
    }
    Ok(text)
}

/// Trim trailing spaces on each line and collapse runs of blank lines.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out
}

fn pdf(bytes: &[u8]) -> Result<String> {
    // pdf-extract can panic on malformed files; treat that as an unreadable PDF.
    let pages = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes))
        .map_err(|_| anyhow!("the PDF couldn't be read"))?
        .map_err(|e| match e {
            pdf_extract::OutputError::PdfError(pdf_extract::Error::Decryption(_)) => {
                anyhow!("it is password-protected")
            }
            e => anyhow!("the PDF couldn't be read: {e}"),
        })?;
    let mut out = String::new();
    for (n, page) in pages.iter().enumerate() {
        if page.trim().is_empty() {
            continue;
        }
        if pages.len() > 1 {
            out.push_str(&format!("\n\n--- Page {} ---\n\n", n + 1));
        }
        out.push_str(page);
    }
    Ok(out)
}

fn zip_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<String> {
    let f = zip
        .by_name(name)
        .with_context(|| format!("{name} is missing"))?;
    let size = f.size();
    let bytes =
        read_capped(f, size, MAX_ENTRY_BYTES).with_context(|| format!("couldn't read {name}"))?;
    String::from_utf8(bytes).with_context(|| format!("couldn't read {name}"))
}

/// Read an archive entry that claims to be `declared` bytes, failing (rather than
/// truncating) when it is or turns out to be larger than `cap`.
fn read_capped(r: impl Read, declared: u64, cap: u64) -> Result<Vec<u8>> {
    let too_big = || anyhow!("it expands to more than {} MB", cap / (1024 * 1024));
    if declared > cap {
        return Err(too_big());
    }
    let mut data = Vec::new();
    r.take(cap + 1).read_to_end(&mut data)?;
    if data.len() as u64 > cap {
        return Err(too_big());
    }
    Ok(data)
}

fn docx(bytes: &[u8]) -> Result<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    xml_text(&zip_entry(&mut zip, "word/document.xml")?, Markup::Ooxml)
}

fn pptx(bytes: &[u8]) -> Result<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let slides = pptx_slide_order(&mut zip).unwrap_or_default();
    let slides = if slides.is_empty() {
        // Fall back to the file names: ppt/slides/slide1.xml, slide2.xml, …
        let mut names: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| {
                let n = n.ok()?;
                let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
                Some((num.parse().ok()?, n.to_string()))
            })
            .collect();
        names.sort();
        names.into_iter().map(|(_, n)| n).collect()
    } else {
        slides
    };
    let mut out = String::new();
    for (n, path) in slides.iter().enumerate() {
        out.push_str(&format!("\n\n## Slide {}\n\n", n + 1));
        out.push_str(&xml_text(&zip_entry(&mut zip, path)?, Markup::Ooxml)?);
        let notes = relationships(&mut zip, path)
            .into_iter()
            .find(|r| r.kind.ends_with("/notesSlide"))
            .map(|r| r.target);
        if let Some(notes) = notes
            && let Ok(xml) = zip_entry(&mut zip, &notes)
        {
            let text = xml_text(&xml, Markup::Ooxml)?;
            if !text.trim().is_empty() {
                out.push_str("\n\nNotes:\n");
                out.push_str(&text);
            }
        }
    }
    Ok(out)
}

/// Slide paths in presentation order, from `ppt/presentation.xml` and its relationships.
fn pptx_slide_order<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<Vec<String>> {
    let rels = relationships(zip, "ppt/presentation.xml");
    let xml = zip_entry(zip, "ppt/presentation.xml")?;
    let mut reader = Reader::from_str(&xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "sldId" => {
                // The relationship id is the namespaced `r:id`, not the plain numeric `id`.
                let rid = e.attributes().flatten().find_map(|a| {
                    (a.key.local_name().as_ref() == "id" && a.key.prefix().is_some())
                        .then(|| a.value.to_string())
                });
                if let Some(target) = rid.and_then(|id| rels.iter().find(|r| r.id == id)) {
                    out.push(target.target.clone());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

struct Relationship {
    id: String,
    kind: String,
    /// Path inside the zip.
    target: String,
}

/// The relationships of the part at `part` (from `<dir>/_rels/<file>.rels`), with targets
/// resolved to zip paths. Empty when the part has none.
fn relationships<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    part: &str,
) -> Vec<Relationship> {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let rels_path = if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    };
    let Ok(xml) = zip_entry(zip, &rels_path) else {
        return Vec::new();
    };
    let mut reader = Reader::from_str(&xml);
    let mut out = Vec::new();
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "Relationship" => {
                let attr = |name: &str| {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == name)
                        .map(|a| a.value.to_string())
                        .unwrap_or_default()
                };
                out.push(Relationship {
                    id: attr("Id"),
                    kind: attr("Type"),
                    target: resolve_part(dir, &attr("Target")),
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// Resolve a relationship target against the directory of the part that refers to it.
fn resolve_part(dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_owned();
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Markup {
    /// Word and PowerPoint XML: text only inside `<w:t>` / `<a:t>`.
    Ooxml,
    /// OpenDocument `content.xml`: text inside paragraphs and headings, with collapsible
    /// whitespace.
    Odf,
}

/// The readable text of an Office Open XML or OpenDocument part: paragraphs on their own
/// lines, table cells separated by tabs and rows by newlines, slides of an OpenDocument
/// presentation under `## Slide N` headings.
fn xml_text(xml: &str, markup: Markup) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    // Depth inside elements whose text counts (`t`, or `p`/`h` for ODF).
    let mut in_text = 0usize;
    // Depth inside elements whose text is skipped: OOXML fallbacks (duplicates of the
    // preferred content) and fields such as slide numbers; ODF tracked deletions.
    let mut skipped = 0usize;
    let mut in_cell = 0usize;
    let mut slides = 0usize;

    fn local(e: &BytesStart) -> String {
        e.local_name().as_ref().to_owned()
    }
    let is_text = |name: &str| match markup {
        Markup::Ooxml => name == "t",
        Markup::Odf => name == "p" || name == "h",
    };
    let is_skipped = |name: &str| match markup {
        Markup::Ooxml => name == "Fallback" || name == "fld" || name == "instrText",
        Markup::Odf => name == "tracked-changes" || name == "annotation",
    };
    let is_cell = |name: &str| name == "tc" || name == "table-cell";
    let is_row = |name: &str| name == "tr" || name == "table-row";
    let is_para = |name: &str| name == "p" || name == "h";

    loop {
        let event = reader.read_event()?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let name = local(e);
                let empty = matches!(event, Event::Empty(_));
                if is_skipped(&name) {
                    if !empty {
                        skipped += 1;
                    }
                    continue;
                }
                if skipped > 0 {
                    continue;
                }
                match name.as_str() {
                    "tab" => out.push('\t'),
                    "br" | "cr" | "line-break" => out.push('\n'),
                    // ODF: `<text:s text:c="3"/>` is three spaces.
                    "s" if markup == Markup::Odf => {
                        let n = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.local_name().as_ref() == "c")
                            .and_then(|a| a.value.parse().ok())
                            .unwrap_or(1usize);
                        out.push_str(&" ".repeat(n.min(100)));
                    }
                    "page" if markup == Markup::Odf && !empty => {
                        slides += 1;
                        out.push_str(&format!("\n\n## Slide {slides}\n\n"));
                    }
                    _ => {}
                }
                if !empty {
                    if is_text(&name) {
                        in_text += 1;
                    }
                    if is_cell(&name) {
                        in_cell += 1;
                    }
                }
            }
            Event::End(e) => {
                let name = e.local_name().as_ref().to_owned();
                if is_skipped(&name) {
                    skipped = skipped.saturating_sub(1);
                    continue;
                }
                if skipped > 0 {
                    continue;
                }
                if is_text(&name) {
                    in_text = in_text.saturating_sub(1);
                }
                if is_para(&name) {
                    // Paragraphs inside a table cell stay on the cell's line.
                    out.push(if in_cell > 0 { ' ' } else { '\n' });
                } else if is_cell(&name) {
                    in_cell = in_cell.saturating_sub(1);
                    out.truncate(out.trim_end_matches(' ').len());
                    out.push('\t');
                } else if is_row(&name) {
                    out.push('\n');
                }
            }
            Event::Text(t) if in_text > 0 && skipped == 0 => {
                let text = t.xml10_content();
                if markup == Markup::Odf {
                    out.push_str(&collapse_spaces(&text));
                } else {
                    out.push_str(&text);
                }
            }
            Event::CData(t) if in_text > 0 && skipped == 0 => out.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_text > 0 && skipped == 0 => {
                if let Some(c) = r.resolve_char_ref()? {
                    out.push(c);
                } else {
                    out.push_str(match &*r.xml10_content() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// ODF collapses whitespace in text the way HTML does; explicit spaces use `<text:s>`.
fn collapse_spaces(text: &str) -> Cow<'_, str> {
    if !text.contains(['\n', '\r', '\t', ' ']) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    Cow::Owned(out)
}

/// Every sheet of a workbook (xlsx, xlsm, xlsb, xls, ods) as CSV under a `## Sheet` heading.
fn spreadsheet(bytes: &[u8]) -> Result<String> {
    use calamine::{Data, Reader as _};
    let mut book =
        calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|e| match e {
            calamine::Error::Xlsx(calamine::XlsxError::Password)
            | calamine::Error::Xls(calamine::XlsError::Password) => {
                anyhow!("it is password-protected")
            }
            e => anyhow!("the workbook couldn't be read: {e}"),
        })?;
    let mut out = String::new();
    for name in book.sheet_names() {
        let Ok(range) = book.worksheet_range(&name) else {
            continue;
        };
        out.push_str(&format!("\n\n## Sheet: {name}\n\n"));
        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| match cell {
                    Data::Empty => String::new(),
                    Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
                    Data::DateTime(d) => d
                        .as_datetime()
                        .map(|dt| {
                            if dt.time() == chrono::NaiveTime::MIN {
                                dt.date().to_string()
                            } else {
                                dt.to_string()
                            }
                        })
                        .unwrap_or_else(|| cell.to_string()),
                    Data::Error(e) => format!("#{e:?}"),
                    other => other.to_string(),
                })
                .collect();
            let used = cells
                .iter()
                .rposition(|c| !c.is_empty())
                .map_or(0, |i| i + 1);
            if used == 0 {
                continue;
            }
            let line: Vec<Cow<str>> = cells[..used].iter().map(|c| csv_field(c)).collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
    }
    Ok(out)
}

fn csv_field(s: &str) -> Cow<'_, str> {
    if s.contains([',', '"', '\n', '\r']) {
        Cow::Owned(format!("\"{}\"", s.replace('"', "\"\"")))
    } else {
        Cow::Borrowed(s)
    }
}

fn cfb_stream(cfb: &mut cfb::CompoundFile<Cursor<&[u8]>>, path: &str) -> Result<Vec<u8>> {
    let stream = cfb
        .open_stream(path)
        .with_context(|| format!("{path} is missing"))?;
    let size = stream.len();
    read_capped(stream, size, MAX_ENTRY_BYTES).with_context(|| format!("couldn't read {path}"))
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| anyhow!("the file is truncated"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| anyhow!("the file is truncated"))
}

/// Word 97–2003 (.doc): the main document text, assembled from the piece table.
fn doc(bytes: &[u8]) -> Result<String> {
    let mut cfb = cfb::CompoundFile::open(Cursor::new(bytes))?;
    let word = cfb_stream(&mut cfb, "/WordDocument")?;
    if u16_at(&word, 0)? != 0xA5EC {
        bail!("it isn't a Word 97 or later document");
    }
    let flags = u16_at(&word, 0x0A)?;
    if flags & 0x0100 != 0 {
        bail!("it is password-protected");
    }
    let table = cfb_stream(
        &mut cfb,
        if flags & 0x0200 != 0 {
            "/1Table"
        } else {
            "/0Table"
        },
    )?;

    // File Information Block: fixed base, then three variable-length arrays.
    let csw = u16_at(&word, 32)? as usize;
    let rg_lw = 32 + 2 + csw * 2 + 2;
    let cslw = u16_at(&word, rg_lw - 2)? as usize;
    let ccp_text = u32_at(&word, rg_lw + 3 * 4)? as usize;
    let rg_fc_lcb = rg_lw + cslw * 4 + 2;
    let fc_clx = u32_at(&word, rg_fc_lcb + 33 * 8)? as usize;
    let lcb_clx = u32_at(&word, rg_fc_lcb + 33 * 8 + 4)? as usize;
    let clx = table
        .get(fc_clx..fc_clx + lcb_clx)
        .ok_or_else(|| anyhow!("the file is truncated"))?;

    // Skip the property modifiers (Prc) to reach the piece table (Pcdt).
    let mut i = 0;
    while clx.get(i) == Some(&0x01) {
        i += 3 + u16_at(clx, i + 1)? as usize;
    }
    if clx.get(i) != Some(&0x02) {
        bail!("the document's text table couldn't be found");
    }
    let lcb = u32_at(clx, i + 1)? as usize;
    let plc = clx
        .get(i + 5..i + 5 + lcb)
        .ok_or_else(|| anyhow!("the file is truncated"))?;
    let pieces = lcb.saturating_sub(4) / 12;

    let mut text = String::new();
    for p in 0..pieces {
        let (start, end) = (
            u32_at(plc, p * 4)? as usize,
            u32_at(plc, p * 4 + 4)? as usize,
        );
        let len = end.saturating_sub(start);
        let raw = u32_at(plc, (pieces + 1) * 4 + p * 8 + 2)?;
        if raw & 0x4000_0000 != 0 {
            // Compressed: one byte per character in Windows-1252, at half the offset.
            let at = (raw & 0x3FFF_FFFF) as usize / 2;
            let chunk = word.get(at..at + len).unwrap_or_default();
            text.push_str(
                &encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(chunk)
                    .0,
            );
        } else {
            let at = raw as usize;
            let chunk = word.get(at..at + len * 2).unwrap_or_default();
            text.push_str(&encoding_rs::UTF_16LE.decode_without_bom_handling(chunk).0);
        }
    }
    Ok(word_chars(text.chars().take(ccp_text)))
}

/// Map Word's special characters: paragraph and cell marks, and fields (keep the result,
/// drop the field code).
fn word_chars(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    // For each open field, whether we've passed its separator (and are in its result).
    let mut fields: Vec<bool> = Vec::new();
    for c in chars {
        match c {
            '\u{13}' => fields.push(false),
            '\u{14}' => {
                if let Some(f) = fields.last_mut() {
                    *f = true;
                }
            }
            '\u{15}' => {
                fields.pop();
            }
            _ if fields.iter().any(|in_result| !in_result) => {}
            '\r' | '\u{0b}' | '\u{0c}' => out.push('\n'),
            // A cell mark ends a cell; a second one straight after ends the table row.
            '\u{07}' if out.ends_with('\t') => {
                out.pop();
                out.push('\n');
            }
            '\u{07}' => out.push('\t'),
            '\u{1e}' => out.push('-'),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// PowerPoint 97–2003 (.ppt): slide text, then speaker notes, skipping master slides.
fn ppt(bytes: &[u8]) -> Result<String> {
    let mut cfb = cfb::CompoundFile::open(Cursor::new(bytes))?;
    let stream = cfb_stream(&mut cfb, "/PowerPoint Document")?;

    #[derive(Default)]
    struct Collected {
        /// Text per slide, from the slide list (PowerPoint) or the slides' drawings (others).
        listed: Vec<Vec<String>>,
        drawn: Vec<Vec<String>>,
        notes: Vec<String>,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Place {
        Other,
        SlideList,
        NotesList,
        Slide,
        Notes,
    }

    fn walk(data: &[u8], mut at: usize, end: usize, place: Place, out: &mut Collected) {
        while at + 8 <= end {
            let ver_inst = u16::from_le_bytes([data[at], data[at + 1]]);
            let kind = u16::from_le_bytes([data[at + 2], data[at + 3]]);
            let len = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]])
                as usize;
            let body = at + 8;
            let next = body.saturating_add(len).min(end);
            let text = match kind {
                0x0FA0 => Some(
                    encoding_rs::UTF_16LE
                        .decode_without_bom_handling(&data[body..next])
                        .0
                        .into_owned(),
                ),
                0x0FA8 => Some(
                    encoding_rs::WINDOWS_1252
                        .decode_without_bom_handling(&data[body..next])
                        .0
                        .into_owned(),
                ),
                _ => None,
            };
            if let Some(text) = text {
                // "*" is the placeholder for slide numbers, dates and footers.
                let text = text.replace(['\r', '\u{0b}'], "\n");
                if !text.trim().is_empty() && text.trim() != "*" {
                    match place {
                        Place::SlideList => {
                            if let Some(s) = out.listed.last_mut() {
                                s.push(text);
                            }
                        }
                        Place::Slide => {
                            if let Some(s) = out.drawn.last_mut() {
                                s.push(text);
                            }
                        }
                        Place::Notes | Place::NotesList => out.notes.push(text),
                        Place::Other => {}
                    }
                }
            } else if kind == 0x03F3 && place == Place::SlideList {
                // SlidePersistAtom: the next slide's text follows.
                out.listed.push(Vec::new());
            } else if ver_inst & 0xF == 0xF {
                let inner = match kind {
                    // Main masters and handouts hold placeholder prompts, not content.
                    0x03F8 | 0x0FC9 => None,
                    0x0FF0 => match ver_inst >> 4 {
                        0 => Some(Place::SlideList),
                        2 => Some(Place::NotesList),
                        _ => None,
                    },
                    0x03EE => {
                        out.drawn.push(Vec::new());
                        Some(Place::Slide)
                    }
                    0x03F0 => Some(Place::Notes),
                    _ => Some(place),
                };
                if let Some(inner) = inner {
                    walk(data, body, next, inner, out);
                }
            }
            at = next;
        }
    }

    let mut found = Collected::default();
    walk(&stream, 0, stream.len(), Place::Other, &mut found);
    let count = found.listed.len().max(found.drawn.len());
    let mut out = String::new();
    for n in 0..count {
        out.push_str(&format!("\n\n## Slide {}\n\n", n + 1));
        // PowerPoint keeps placeholder text in the slide list and other text boxes in the
        // drawing; other writers use only one. Merge them without repeating anything.
        let mut seen: Vec<&String> = Vec::new();
        for t in found
            .listed
            .get(n)
            .into_iter()
            .chain(found.drawn.get(n))
            .flatten()
        {
            if !seen.contains(&t) {
                seen.push(t);
                out.push_str(t);
                out.push('\n');
            }
        }
    }
    found.notes.dedup();
    if !found.notes.is_empty() {
        out.push_str("\n\n## Notes\n\n");
        out.push_str(&found.notes.join("\n\n"));
    }
    Ok(out)
}

/// The plain text of an RTF document.
fn rtf(bytes: &[u8]) -> String {
    // Destinations whose content isn't document text.
    const SKIP: &[&str] = &[
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "pict",
        "object",
        "fldinst",
        "header",
        "headerl",
        "headerr",
        "headerf",
        "footer",
        "footerl",
        "footerr",
        "footerf",
        "themedata",
        "colorschememapping",
        "latentstyles",
        "datastore",
        "listtable",
        "listoverridetable",
        "rsidtbl",
        "xmlnstbl",
        "generator",
        "pgdsctbl",
        "filetbl",
        "revtbl",
        "mmathPr",
    ];
    let mut out = String::new();
    // (skipping, unicode fallback length) per group.
    let mut stack: Vec<(bool, usize)> = Vec::new();
    let (mut skip, mut uc) = (false, 1usize);
    // Fallback characters still to drop after a `\uN`.
    let mut drop = 0usize;
    // Bytes from `\'hh` escapes, decoded together so multi-byte code pages work.
    let mut pending: Vec<u8> = Vec::new();
    let flush = |pending: &mut Vec<u8>, out: &mut String, skip: bool| {
        if !pending.is_empty() {
            if !skip {
                out.push_str(
                    &encoding_rs::WINDOWS_1252
                        .decode_without_bom_handling(pending)
                        .0,
                );
            }
            pending.clear();
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' || bytes.get(i + 1) != Some(&b'\'') {
            flush(&mut pending, &mut out, skip);
        }
        match b {
            b'{' => {
                stack.push((skip, uc));
                i += 1;
            }
            b'}' => {
                (skip, uc) = stack.pop().unwrap_or((false, 1));
                i += 1;
            }
            b'\\' => {
                let Some(&next) = bytes.get(i + 1) else { break };
                if next.is_ascii_alphabetic() {
                    let start = i + 1;
                    let mut j = start;
                    while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    let word = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
                    let num_start = j;
                    if j < bytes.len() && bytes[j] == b'-' {
                        j += 1;
                    }
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    let param: Option<i32> = std::str::from_utf8(&bytes[num_start..j])
                        .ok()
                        .and_then(|s| s.parse().ok());
                    if j < bytes.len() && bytes[j] == b' ' {
                        j += 1;
                    }
                    i = j;
                    if drop > 0 {
                        drop -= 1;
                        continue;
                    }
                    if SKIP.contains(&word) {
                        skip = true;
                        continue;
                    }
                    if skip {
                        continue;
                    }
                    match word {
                        "par" | "line" | "row" | "sect" | "page" => out.push('\n'),
                        "tab" | "cell" => out.push('\t'),
                        "emdash" => out.push('—'),
                        "endash" => out.push('–'),
                        "bullet" => out.push('•'),
                        "lquote" => out.push('‘'),
                        "rquote" => out.push('’'),
                        "ldblquote" => out.push('“'),
                        "rdblquote" => out.push('”'),
                        "uc" => uc = param.unwrap_or(1).max(0) as usize,
                        "u" => {
                            let code = param.unwrap_or(0);
                            let code = if code < 0 { code + 65536 } else { code } as u32;
                            out.extend(char::from_u32(code));
                            drop = uc;
                        }
                        _ => {}
                    }
                } else {
                    i += 2;
                    match next {
                        b'\'' => {
                            let hex = bytes
                                .get(i..i + 2)
                                .and_then(|h| std::str::from_utf8(h).ok());
                            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                                i += 2;
                                if drop > 0 {
                                    drop -= 1;
                                } else {
                                    pending.push(v);
                                }
                            }
                        }
                        b'*' => skip = true,
                        b'~' if !skip => out.push('\u{a0}'),
                        b'_' if !skip => out.push('-'),
                        b'\n' | b'\r' if !skip => out.push('\n'),
                        b'\\' | b'{' | b'}' if !skip => {
                            if drop > 0 {
                                drop -= 1;
                            } else {
                                out.push(next as char);
                            }
                        }
                        _ => {}
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if drop > 0 {
                    drop -= 1;
                } else if !skip {
                    pending.push(b);
                }
                i += 1;
            }
        }
    }
    flush(&mut pending, &mut out, skip);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_entries_fail_instead_of_truncating() {
        const MB: u64 = 1024 * 1024;
        let data = vec![b'x'; (MB + 1) as usize];
        assert_eq!(
            read_capped(&data[..MB as usize], MB, MB).unwrap().len(),
            MB as usize
        );
        // Too big by its declared size, or by what it actually holds (a lying header).
        assert!(read_capped(&data[..10], 2 * MB, MB).is_err());
        let err = read_capped(&data[..], 10, MB).unwrap_err();
        assert!(err.to_string().contains("more than 1 MB"), "{err}");
    }

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!(
            "{}/tests/fixtures/documents/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn read(name: &str, expect: Format) -> String {
        let bytes = fixture(name);
        assert_eq!(detect(&bytes).unwrap(), Some(expect), "{name}");
        extract(expect, &bytes).unwrap()
    }

    /// The report fixture (`report.*`) was written by LibreOffice from the same HTML.
    fn assert_report(name: &str, format: Format) {
        let text = read(name, format);
        for want in [
            "Quarterly report",
            "Revenue grew 12% in Q3 – café sales led.",
            "North",
            "420",
            "Second paragraph with marker ZEBRA-7.",
        ] {
            assert!(text.contains(want), "{name}: missing {want:?} in\n{text}");
        }
    }

    #[test]
    fn word_processing_formats() {
        assert_report("report.docx", Format::Docx);
        assert_report("report.doc", Format::Doc);
        assert_report("report.odt", Format::Odt);
        assert_report("report.rtf", Format::Rtf);
        assert_report("report.pdf", Format::Pdf);
        // Table cells share a line, one line per row.
        for (name, format) in [("report.docx", Format::Docx), ("report.doc", Format::Doc)] {
            let text = read(name, format);
            assert!(
                text.contains("Region\tSales\nNorth\t420\n"),
                "{name}: {text:?}"
            );
        }
    }

    #[test]
    fn spreadsheets_become_csv() {
        for (name, format) in [
            ("sales.xlsx", Format::Xlsx),
            ("sales.xls", Format::Xls),
            ("sales.ods", Format::Ods),
        ] {
            let text = read(name, format);
            assert!(text.starts_with("## Sheet: "), "{name}: {text}");
            assert!(
                text.ends_with("Region,Sales,Note\nNorth,420,\"big, round\"\nSouth,310.5,café"),
                "{name}: {text}"
            );
        }
    }

    #[test]
    fn presentations_in_slide_order_with_notes() {
        for (name, format) in [("deck.pptx", Format::Pptx), ("deck.odp", Format::Odp)] {
            let text = read(name, format);
            let one = text.find("## Slide 1").expect(name);
            let roadmap = text.find("Roadmap 2027").expect(name);
            let two = text.find("## Slide 2").expect(name);
            let budget = text.find("Budget café").expect(name);
            assert!(
                one < roadmap && roadmap < two && two < budget,
                "{name}: {text}"
            );
            assert!(
                text.contains("Ship the app icon\nAdd Windows runner"),
                "{name}: {text}"
            );
            assert!(text.contains("Speaker note PELICAN"), "{name}: {text}");
            assert!(!text.contains("Click to edit"), "{name}: {text}");
        }
    }

    /// A minimal PowerPoint 97 file: one slide's text in the slide list, another in a
    /// slide drawing, a master slide's prompt and a notes page.
    #[test]
    fn legacy_powerpoint_records() {
        fn rec(ver_inst: u16, kind: u16, body: &[u8]) -> Vec<u8> {
            let mut v = ver_inst.to_le_bytes().to_vec();
            v.extend(kind.to_le_bytes());
            v.extend((body.len() as u32).to_le_bytes());
            v.extend(body);
            v
        }
        fn chars(s: &str) -> Vec<u8> {
            rec(
                0,
                0x0FA0,
                &s.encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
        }
        let slide_list = rec(
            0x000F,
            0x0FF0,
            &[
                rec(0, 0x03F3, &[0; 20]),
                chars("Listed title"),
                rec(0, 0x0FA8, b"Bytes body"),
            ]
            .concat(),
        );
        let document = rec(0x000F, 0x03E8, &slide_list);
        let master = rec(0x000F, 0x03F8, &chars("Click to edit Master title style"));
        let slide1 = rec(
            0x000F,
            0x03EE,
            &[chars("Listed title"), chars("Extra box")].concat(),
        );
        let slide2 = rec(0x000F, 0x03EE, &chars("Drawn only\rsecond line"));
        let notes = rec(0x000F, 0x03F0, &[chars("*"), chars("A note")].concat());
        let stream = [document, master, slide1, slide2, notes].concat();

        let mut cfb = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        std::io::Write::write_all(
            &mut cfb.create_stream("/PowerPoint Document").unwrap(),
            &stream,
        )
        .unwrap();
        let bytes = cfb.into_inner().into_inner();

        assert_eq!(detect(&bytes).unwrap(), Some(Format::Ppt));
        let text = extract(Format::Ppt, &bytes).unwrap();
        assert_eq!(
            text,
            "## Slide 1\n\nListed title\nBytes body\nExtra box\n\n## Slide 2\n\n\
             Drawn only\nsecond line\n\n## Notes\n\nA note"
        );
    }

    #[test]
    fn rtf_escapes_and_destinations() {
        let rtf = br"{\rtf1\ansi\ansicpg1252{\fonttbl{\f0 Arial;}}{\*\generator Test;}
{\info{\title Hidden}}Caf\'e9 \u8364?\par
Line\tab two {\field{\*\fldinst HYPERLINK x}{\fldrslt link}}\par
\{braces\}}";
        assert_eq!(detect(rtf).unwrap(), Some(Format::Rtf));
        assert_eq!(
            extract(Format::Rtf, rtf).unwrap(),
            "Café €\nLine\ttwo link\n{braces}"
        );
    }

    #[test]
    fn rejects_other_files() {
        assert_eq!(detect(b"plain text").unwrap(), None);
        // A zip that isn't an Office or OpenDocument file.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"hi").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert_eq!(detect(&bytes).unwrap(), None);
        // A password-protected Office file is a compound file with an encrypted package.
        let mut cfb = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        cfb.create_stream("/EncryptedPackage").unwrap();
        let bytes = cfb.into_inner().into_inner();
        assert!(detect(&bytes).unwrap_err().to_string().contains("password"));
    }

    #[test]
    fn resolves_relationship_targets() {
        assert_eq!(
            resolve_part("ppt", "slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(
            resolve_part("ppt/slides", "../notesSlides/notesSlide1.xml"),
            "ppt/notesSlides/notesSlide1.xml"
        );
        assert_eq!(resolve_part("ppt", "/ppt/slides/s.xml"), "ppt/slides/s.xml");
    }
}

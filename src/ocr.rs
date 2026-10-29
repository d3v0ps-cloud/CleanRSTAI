//! OCR for scanned PDFs: a PDF with no text layer has its page images read out with lopdf, and
//! each image is transcribed by a vision model (its page is sent to a provider) or by the
//! `tesseract` program on this computer. The text comes back page by page, as an attachment.

use std::collections::HashSet;
use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::attach::Attachment;
use crate::config::{ProviderConfig, Settings};
use crate::llm;

/// Sent with each page image to the vision model.
pub const PROMPT: &str = "Transcribe all text on this page exactly. Output only the text.";

/// A PDF with fewer non-space characters than this, per page on average, has no text layer.
const TEXT_CHARS_PER_PAGE: usize = 20;

/// The longest one tesseract run may take.
const TESSERACT_TIMEOUT: Duration = Duration::from_secs(120);

/// Which engine reads the page images.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrEngine {
    /// A vision model from a provider; each page image is sent to it.
    #[default]
    VisionModel,
    /// The `tesseract` program on this computer.
    Tesseract,
}

/// OCR for scanned PDFs (Settings → Context & files).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OcrSettings {
    /// Read scanned PDFs when they are attached.
    pub enabled: bool,
    pub engine: OcrEngine,
    /// Provider profile for the vision model; `None` means the active provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The tesseract program, by name on PATH or as a full path.
    pub tesseract_path: String,
    /// Tesseract language codes, joined with `+` (e.g. `eng+deu`).
    pub languages: String,
    /// Pages read from each PDF (1–200).
    pub max_pages: u32,
}

impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            engine: OcrEngine::VisionModel,
            provider: None,
            tesseract_path: "tesseract".into(),
            languages: "eng".into(),
            max_pages: 20,
        }
    }
}

impl OcrSettings {
    /// Clamp the numbers, tidy the text fields, and forget a provider that no longer exists.
    pub fn normalize(&mut self, provider_exists: impl Fn(&str) -> bool) {
        self.max_pages = self.max_pages.clamp(1, 200);
        self.tesseract_path = self.tesseract_path.trim().to_owned();
        if self.tesseract_path.is_empty() {
            self.tesseract_path = "tesseract".into();
        }
        let languages: String = self
            .languages
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+'))
            .collect();
        self.languages = if languages.is_empty() {
            "eng".into()
        } else {
            languages
        };
        if self
            .provider
            .as_deref()
            .is_some_and(|id| id.trim().is_empty() || !provider_exists(id))
        {
            self.provider = None;
        }
    }
}

/// What a scanned PDF is read with, copied from the settings when it is attached.
pub struct Job {
    settings: OcrSettings,
    provider: ProviderConfig,
    http: reqwest::Client,
}

impl Job {
    /// `None` when OCR is off. The vision model is the chosen provider, or the active one.
    pub fn from_settings(settings: &Settings, http: &reqwest::Client) -> Option<Self> {
        let o = &settings.general.ocr;
        if !o.enabled {
            return None;
        }
        let mut provider = o
            .provider
            .as_deref()
            .and_then(|id| settings.providers.iter().find(|p| p.id == id))
            .unwrap_or_else(|| settings.provider())
            .clone();
        // One page per request, read in full: streaming would only add parsing.
        provider.stream = false;
        Some(Self {
            settings: o.clone(),
            provider,
            http: http.clone(),
        })
    }

    fn label(&self) -> String {
        match self.settings.engine {
            OcrEngine::VisionModel => format!("a vision model ({})", self.provider.model.trim()),
            OcrEngine::Tesseract => "Tesseract".into(),
        }
    }
}

/// Whether a PDF's pages hold too little text to count as having a text layer.
pub fn is_scanned(pages: &[String]) -> bool {
    let chars: usize = pages
        .iter()
        .map(|p| p.chars().filter(|c| !c.is_whitespace()).count())
        .sum();
    !pages.is_empty() && chars < TEXT_CHARS_PER_PAGE * pages.len()
}

/// One image of a page, as a file a vision model or tesseract can read.
#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    /// The page's number, from 1.
    pub page: u32,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// The images of the first pages of a PDF, and what was left out.
#[derive(Debug, Default, PartialEq)]
pub struct PageImages {
    pub images: Vec<PageImage>,
    pub notes: Vec<String>,
}

/// The image XObjects of the first `max_pages` pages. Images in encodings this reader does not
/// handle are left out, with a note.
pub fn page_images(pdf: &[u8], max_pages: usize) -> Result<PageImages> {
    let doc = Document::load_mem(pdf).map_err(|e| anyhow!("the PDF couldn't be read: {e}"))?;
    let pages = doc.get_pages();
    let mut out = PageImages::default();
    if pages.len() > max_pages {
        out.notes.push(format!(
            "only the first {max_pages} of {} pages were read (the page limit in Settings → \
             Context & files)",
            pages.len()
        ));
    }
    for (&number, &id) in pages.iter().take(max_pages) {
        let found = page_image_streams(&doc, id);
        if found.is_empty() {
            out.notes.push(format!("page {number} has no image"));
        }
        for stream in found {
            match image_bytes(&doc, stream) {
                Ok((mime, bytes)) => out.images.push(PageImage {
                    page: number,
                    mime,
                    bytes,
                }),
                Err(why) => out
                    .notes
                    .push(format!("an image on page {number} was skipped: {why}")),
            }
        }
    }
    Ok(out)
}

/// Follow an object reference, if there is one.
fn resolve<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    doc.dereference(object).ok().map(|(_, o)| o)
}

/// The image XObjects a page uses, from its own and its inherited resources, each once.
fn page_image_streams(doc: &Document, page: ObjectId) -> Vec<&Stream> {
    let Ok((direct, resource_ids)) = doc.get_page_resources(page) else {
        return Vec::new();
    };
    let mut dicts: Vec<&Dictionary> = direct.into_iter().collect();
    dicts.extend(
        resource_ids
            .into_iter()
            .filter_map(|id| doc.get_dictionary(id).ok()),
    );
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for dict in dicts {
        let xobjects = dict
            .get(b"XObject")
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_dict().ok());
        let Some(xobjects) = xobjects else {
            continue;
        };
        for (_, value) in xobjects.iter() {
            let Ok(id) = value.as_reference() else {
                continue;
            };
            let Ok(Object::Stream(stream)) = doc.get_object(id) else {
                continue;
            };
            let is_image = stream
                .dict
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|n| n == b"Image");
            if is_image && seen.insert(id) {
                found.push(stream);
            }
        }
    }
    found
}

/// The image as a file a model can read: JPEG bytes as they are, or 8-bit grey or RGB pixels
/// (Flate-compressed or not) encoded as PNG. Other encodings say why they were skipped.
fn image_bytes(
    doc: &Document,
    stream: &Stream,
) -> std::result::Result<(&'static str, Vec<u8>), String> {
    let dict = &stream.dict;
    if dict
        .get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false)
    {
        return Err("it is a stencil mask".into());
    }
    let filters = filter_names(doc, dict);
    let channels = colour_channels(doc, dict);
    let only: Option<&[u8]> = match filters.as_slice() {
        [] => None,
        [one] => Some(one.as_slice()),
        _ => return Err("it has several filters".into()),
    };
    match only {
        None | Some(b"FlateDecode") => {
            let bits = dict
                .get(b"BitsPerComponent")
                .and_then(Object::as_i64)
                .unwrap_or(0);
            if bits != 8 {
                return Err(format!("it has {bits} bits per component, not 8"));
            }
            let channels = match channels {
                Some(c @ (1 | 3)) => c,
                Some(_) => return Err("it is CMYK".into()),
                None => return Err("its colour space isn't supported".into()),
            };
            let (w, h) = dimensions(dict)?;
            let raw = if only.is_none() {
                stream.content.clone()
            } else {
                stream
                    .decompressed_content()
                    .map_err(|e| format!("it couldn't be decompressed: {e}"))?
            };
            encode_png(w, h, channels, raw).map(|png| ("image/png", png))
        }
        Some(b"DCTDecode") => {
            if channels == Some(4) {
                return Err("it is a CMYK JPEG".into());
            }
            Ok(("image/jpeg", stream.content.clone()))
        }
        Some(b"JPXDecode") => Err("it is JPEG 2000".into()),
        Some(b"CCITTFaxDecode") => Err("it is CCITT fax".into()),
        Some(b"JBIG2Decode") => Err("it is JBIG2".into()),
        Some(_) => Err("its encoding isn't supported".into()),
    }
}

/// The filter names of a stream: one name, or an array of them.
fn filter_names(doc: &Document, dict: &Dictionary) -> Vec<Vec<u8>> {
    let Ok(filter) = dict.get(b"Filter") else {
        return Vec::new();
    };
    match resolve(doc, filter) {
        Some(Object::Name(name)) => vec![name.clone()],
        Some(Object::Array(items)) => items
            .iter()
            .filter_map(|item| resolve(doc, item))
            .filter_map(|o| o.as_name().ok())
            .map(<[u8]>::to_vec)
            .collect(),
        _ => Vec::new(),
    }
}

/// The number of colour components of an image's colour space, when it is one of the simple
/// kinds: device grey, RGB or CMYK, or an ICC profile with that many.
fn colour_channels(doc: &Document, dict: &Dictionary) -> Option<u8> {
    let space = resolve(doc, dict.get(b"ColorSpace").ok()?)?;
    match space {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" => Some(1),
            b"DeviceRGB" => Some(3),
            b"DeviceCMYK" => Some(4),
            _ => None,
        },
        Object::Array(items) => {
            if items.first()?.as_name().ok()? != b"ICCBased" {
                return None;
            }
            let profile = resolve(doc, items.get(1)?)?.as_stream().ok()?;
            let n = profile.dict.get(b"N").and_then(Object::as_i64).ok()?;
            u8::try_from(n).ok().filter(|n| matches!(n, 1 | 3 | 4))
        }
        _ => None,
    }
}

fn dimensions(dict: &Dictionary) -> std::result::Result<(u32, u32), String> {
    let size = |key: &[u8]| {
        dict.get(key)
            .and_then(Object::as_i64)
            .ok()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
    };
    match (size(b"Width"), size(b"Height")) {
        (Some(w), Some(h)) => Ok((w, h)),
        _ => Err("its size is missing".into()),
    }
}

fn encode_png(w: u32, h: u32, channels: u8, raw: Vec<u8>) -> std::result::Result<Vec<u8>, String> {
    let expected = w as usize * h as usize * channels as usize;
    if raw.len() != expected {
        return Err(format!(
            "its data is {} bytes, not the {expected} its size needs",
            raw.len()
        ));
    }
    let image = if channels == 1 {
        image::GrayImage::from_raw(w, h, raw).map(image::DynamicImage::ImageLuma8)
    } else {
        image::RgbImage::from_raw(w, h, raw).map(image::DynamicImage::ImageRgb8)
    }
    .ok_or("its pixels don't fit its size")?;
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("it couldn't be encoded as PNG: {e}"))?;
    Ok(out.into_inner())
}

/// One page image through the vision model: its transcription.
async fn transcribe_with_model(job: &Job, image: &PageImage) -> Result<String> {
    let url = format!("data:{};base64,{}", image.mime, B64.encode(&image.bytes));
    let messages: Vec<Value> = vec![json!({"role": "user", "content": [
        {"type": "text", "text": PROMPT},
        {"type": "image_url", "image_url": {"url": url}},
    ]})];
    let limit = Duration::from_secs(job.provider.timeout_secs.max(10));
    let turn = tokio::time::timeout(
        limit,
        llm::chat_completion(&job.http, &job.provider, messages, &[], |_| {}, |_| {}),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "the vision model didn't answer within {} s",
            limit.as_secs()
        )
    })??;
    Ok(turn.content.trim().to_owned())
}

/// One page image through the tesseract program: its text.
async fn transcribe_with_tesseract(job: &Job, image: &PageImage) -> Result<String> {
    let ext = if image.mime == "image/png" {
        "png"
    } else {
        "jpg"
    };
    let path = std::env::temp_dir().join(format!(
        "cleanrstai-ocr-{}.{ext}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&path, &image.bytes).context("couldn't write a page image for Tesseract")?;
    let mut command = tokio::process::Command::new(&job.settings.tesseract_path);
    command
        .arg(&path)
        .arg("stdout")
        .arg("-l")
        .arg(&job.settings.languages)
        .kill_on_drop(true);
    let run = tokio::time::timeout(TESSERACT_TIMEOUT, command.output()).await;
    let _ = std::fs::remove_file(&path);
    let output = run
        .map_err(|_| {
            anyhow!(
                "Tesseract took longer than {} minutes",
                TESSERACT_TIMEOUT.as_secs() / 60
            )
        })?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!(
                    "Tesseract isn't installed or isn't at \"{}\"; install it or set its path \
                     in Settings → Context & files",
                    job.settings.tesseract_path
                )
            } else {
                anyhow!("Tesseract couldn't run: {e}")
            }
        })?;
    if !output.status.success() {
        bail!(
            "Tesseract failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The document text from `(page, text)` pairs: a note on where it came from, then each page
/// under a `--- Page N ---` heading. Pages without text are left out.
pub fn assemble(label: &str, pages: &[(u32, String)], notes: &[String]) -> String {
    let mut out =
        format!("[Text read from scanned pages by OCR with {label}; it may contain mistakes.]");
    for note in notes {
        out.push_str(&format!("\n[{note}]"));
    }
    for (page, text) in pages {
        if !text.trim().is_empty() {
            out.push_str(&format!("\n\n--- Page {page} ---\n\n{}", text.trim()));
        }
    }
    out
}

/// Read a scanned PDF with OCR, as an attachment named `name`.
pub async fn attach(job: &Job, name: String, pdf: Vec<u8>) -> Result<Attachment> {
    let max_pages = job.settings.max_pages as usize;
    let PageImages { images, notes } =
        tokio::task::spawn_blocking(move || page_images(&pdf, max_pages))
            .await
            .context("the PDF reader stopped")??;
    if images.is_empty() {
        bail!("no page image could be read from it ({})", notes.join("; "));
    }
    let mut pages: Vec<(u32, String)> = Vec::new();
    for image in &images {
        let text = match job.settings.engine {
            OcrEngine::VisionModel => transcribe_with_model(job, image).await,
            OcrEngine::Tesseract => transcribe_with_tesseract(job, image).await,
        }?;
        match pages.last_mut() {
            Some((page, all)) if *page == image.page => {
                all.push_str("\n\n");
                all.push_str(&text);
            }
            _ => pages.push((image.page, text)),
        }
    }
    if pages.iter().all(|(_, text)| text.trim().is_empty()) {
        bail!("OCR found no text in it");
    }
    Ok(Attachment::ocr_text(
        name,
        assemble(&job.label(), &pages, &notes),
    ))
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder as _;
    use lopdf::{Dictionary, Document, Stream, dictionary};
    use serde_json::json;

    use super::*;
    use crate::config::Settings;
    use crate::documents::{Extracted, Format};
    use crate::testutil::{self, Response};

    /// A 32 × 32 JPEG of a gradient, from the image crate.
    fn jpeg() -> Vec<u8> {
        let raw: Vec<u8> = (0..32)
            .flat_map(|y| (0..32).flat_map(move |x| [x * 7, y * 7, 128]))
            .collect();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut out)
            .write_image(&raw, 32, 32, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    /// An image XObject with the given colour space, bit depth, filter and data.
    fn image_stream(
        space: &str,
        bits: i64,
        size: i64,
        filter: Option<&str>,
        data: Vec<u8>,
    ) -> Stream {
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => size,
            "Height" => size,
            "ColorSpace" => Object::Name(space.as_bytes().to_vec()),
            "BitsPerComponent" => bits,
        };
        if let Some(f) = filter {
            dict.set("Filter", Object::Name(f.as_bytes().to_vec()));
        }
        Stream::new(dict, data)
    }

    /// A PDF with one page per image, each page drawing its image and holding no text.
    fn scanned_pdf(images: Vec<Stream>) -> Vec<u8> {
        let mut doc = Document::new();
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        let count = images.len() as i64;
        for image in images {
            let image_id = doc.add_object(image);
            let content = doc.add_object(Stream::new(
                Dictionary::new(),
                b"q 612 0 0 792 0 0 cm /Im1 Do Q".to_vec(),
            ));
            let mut xobjects = Dictionary::new();
            xobjects.set("Im1", image_id);
            let mut resources = Dictionary::new();
            resources.set("XObject", xobjects);
            let page = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Contents" => content,
                "Resources" => resources,
            });
            kids.push(Object::Reference(page));
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// Page 1: a JPEG. Page 2: a 64 × 64 Flate-compressed RGB image of one colour (it
    /// compresses, so the filter is kept). Returns the PDF and the JPEG's bytes.
    fn two_page_scan() -> (Vec<u8>, Vec<u8>) {
        let jpg = jpeg();
        let mut flat = image_stream("DeviceRGB", 8, 64, None, [200, 30, 30].repeat(64 * 64));
        flat.compress().unwrap();
        assert_eq!(
            flat.dict.get(b"Filter").unwrap().as_name().unwrap(),
            b"FlateDecode"
        );
        let dct = image_stream("DeviceRGB", 8, 32, Some("DCTDecode"), jpg.clone());
        (scanned_pdf(vec![dct, flat]), jpg)
    }

    fn enabled_settings(base_url: &str) -> Settings {
        let mut s = Settings::default();
        s.providers[0].base_url = base_url.into();
        s.providers[0].model = "vision-test".into();
        s.providers[0].stream = false;
        s.general.ocr.enabled = true;
        s
    }

    #[test]
    fn a_page_with_little_text_is_scanned() {
        assert!(is_scanned(&["".into()]));
        assert!(is_scanned(&["   \n\t ".into()]));
        // Nineteen characters on each of two pages: still under the 20 per page average.
        assert!(is_scanned(&["a".repeat(19), "b".repeat(19)]));
        assert!(!is_scanned(&["The quarterly report, page one.".into()]));
        assert!(!is_scanned(&["".into(), "a".repeat(40)]));
        assert!(!is_scanned(&[]));
    }

    #[test]
    fn scanned_pdfs_are_detected_and_their_images_read() {
        let (pdf, jpg) = two_page_scan();

        // The text layer is empty, so extraction reports the scan.
        assert_eq!(
            crate::documents::extract_or_scanned(Format::Pdf, &pdf).unwrap(),
            Extracted::Scanned { pages: 2 }
        );

        let read = page_images(&pdf, 20).unwrap();
        assert_eq!(read.images.len(), 2, "{:?}", read.notes);
        assert!(read.notes.is_empty(), "{:?}", read.notes);
        // JPEG bytes are passed on as they are.
        assert_eq!(
            read.images[0],
            PageImage {
                page: 1,
                mime: "image/jpeg",
                bytes: jpg
            }
        );
        // Raw RGB becomes a PNG of the same size.
        assert_eq!(read.images[1].page, 2);
        assert_eq!(read.images[1].mime, "image/png");
        let png = image::load_from_memory(&read.images[1].bytes).unwrap();
        assert_eq!((png.width(), png.height()), (64, 64));
        assert_eq!(png.to_rgb8().get_pixel(10, 10).0, [200, 30, 30]);
    }

    #[test]
    fn the_page_limit_and_unsupported_images_are_noted() {
        let (pdf, _) = two_page_scan();
        let read = page_images(&pdf, 1).unwrap();
        assert_eq!(read.images.len(), 1);
        assert_eq!(read.images[0].page, 1);
        assert!(
            read.notes
                .iter()
                .any(|n| n.contains("only the first 1 of 2 pages"))
        );

        let jpx = image_stream("DeviceRGB", 8, 8, Some("JPXDecode"), vec![0; 4]);
        let read = page_images(&scanned_pdf(vec![jpx]), 20).unwrap();
        assert!(read.images.is_empty());
        assert_eq!(
            read.notes,
            vec!["an image on page 1 was skipped: it is JPEG 2000".to_owned()]
        );
    }

    #[test]
    fn the_attachment_says_so_when_ocr_is_off() {
        let (pdf, _) = two_page_scan();
        let reading =
            crate::attach::Attachment::read_bytes("scan.pdf".into(), pdf.clone()).unwrap();
        assert!(matches!(reading, crate::attach::Reading::Scanned(_)));
        let attached = crate::attach::Attachment::from_bytes("scan.pdf".into(), pdf).unwrap();
        assert_eq!(
            attached.text.as_deref(),
            Some(crate::attach::NO_TEXT_LAYER_NOTE)
        );
        assert!(
            attached
                .text
                .unwrap()
                .contains("turn on OCR in Settings → Context & files")
        );

        let mut settings = enabled_settings("http://127.0.0.1:9");
        settings.general.ocr.enabled = false;
        assert!(Job::from_settings(&settings, &llm::build_http_client()).is_none());
    }

    #[tokio::test]
    async fn the_vision_model_transcribes_each_page() {
        let (pdf, jpg) = two_page_scan();
        let (base, log) = testutil::serve(|_| {
            Response::json(json!({
                "choices": [{"message": {"role": "assistant", "content": " Transcribed text "},
                             "finish_reason": "stop"}]
            }))
        })
        .await;
        let settings = enabled_settings(&base);
        let job = Job::from_settings(&settings, &llm::build_http_client()).unwrap();
        let attached = attach(&job, "scan.pdf".into(), pdf).await.unwrap();

        let text = attached.text.unwrap();
        assert!(
            text.starts_with(
                "[Text read from scanned pages by OCR with a vision model (vision-test)"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "--- Page 1 ---\n\nTranscribed text\n\n--- Page 2 ---\n\nTranscribed text"
            ),
            "{text}"
        );
        assert_eq!(attached.mime, "application/pdf");

        let requests = log.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/chat/completions");
        let body = requests[0].json();
        assert_eq!(body["model"], "vision-test");
        let parts = &body["messages"][0]["content"];
        assert_eq!(parts[0]["text"], PROMPT);
        assert_eq!(parts[1]["type"], "image_url");
        let url = parts[1]["image_url"]["url"].as_str().unwrap();
        assert_eq!(url, format!("data:image/jpeg;base64,{}", B64.encode(&jpg)));
    }

    #[tokio::test]
    async fn the_chosen_provider_is_the_vision_model() {
        let (base, log) = testutil::serve(|_| {
            Response::json(json!({"choices": [{"message": {"content": "ok"}}]}))
        })
        .await;
        let mut settings = enabled_settings("http://127.0.0.1:9");
        settings.providers.push(ProviderConfig {
            id: "ocr-provider".into(),
            base_url: base,
            model: "other-vision".into(),
            stream: false,
            ..Default::default()
        });
        settings.general.ocr.provider = Some("ocr-provider".into());
        let job = Job::from_settings(&settings, &llm::build_http_client()).unwrap();
        let (pdf, _) = two_page_scan();
        attach(&job, "scan.pdf".into(), pdf).await.unwrap();
        let requests = log.lock().unwrap();
        assert_eq!(requests[0].json()["model"], "other-vision");
    }

    #[tokio::test]
    async fn a_missing_tesseract_is_a_clear_error() {
        let mut settings = enabled_settings("http://127.0.0.1:9");
        settings.general.ocr.engine = OcrEngine::Tesseract;
        settings.general.ocr.tesseract_path = "/nonexistent/cleanrstai-tesseract".into();
        let job = Job::from_settings(&settings, &llm::build_http_client()).unwrap();
        let (pdf, _) = two_page_scan();
        let err = attach(&job, "scan.pdf".into(), pdf).await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("Tesseract isn't installed"), "{text}");
        assert!(text.contains("/nonexistent/cleanrstai-tesseract"), "{text}");
    }

    #[tokio::test]
    #[ignore = "needs the tesseract program on PATH"]
    async fn tesseract_runs_on_a_page_image() {
        if std::process::Command::new("tesseract")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("tesseract isn't installed; skipping");
            return;
        }
        let mut settings = enabled_settings("http://127.0.0.1:9");
        settings.general.ocr.engine = OcrEngine::Tesseract;
        let job = Job::from_settings(&settings, &llm::build_http_client()).unwrap();
        let (pdf, _) = two_page_scan();
        let image = page_images(&pdf, 1).unwrap().images.remove(0);
        // A plain gradient has no words; the point is that the program runs and answers.
        transcribe_with_tesseract(&job, &image).await.unwrap();
    }

    #[test]
    fn the_text_is_assembled_page_by_page() {
        let text = assemble(
            "Tesseract",
            &[(1, "First".into()), (2, "  ".into()), (3, "Third".into())],
            &["page 4 has no image".into()],
        );
        assert_eq!(
            text,
            "[Text read from scanned pages by OCR with Tesseract; it may contain mistakes.]\n\
             [page 4 has no image]\n\n--- Page 1 ---\n\nFirst\n\n--- Page 3 ---\n\nThird"
        );
    }

    #[test]
    fn settings_load_from_older_files_and_normalise() {
        // Settings written before OCR existed load it off, with the defaults.
        let s: Settings = serde_json::from_str(r#"{"general": {"zoom": 1.5}}"#).unwrap();
        assert_eq!(s.general.ocr, OcrSettings::default());
        assert!(!s.general.ocr.enabled);
        assert_eq!(s.general.ocr.max_pages, 20);
        assert_eq!(s.general.ocr.languages, "eng");

        let json = serde_json::to_string(&OcrSettings::default()).unwrap();
        assert!(!json.contains("provider"), "{json}");
        assert!(json.contains(r#""engine":"vision_model""#), "{json}");

        let mut o = OcrSettings {
            max_pages: 500,
            tesseract_path: "  ".into(),
            languages: "eng; rm".into(),
            provider: Some("gone".into()),
            ..Default::default()
        };
        o.normalize(|_| false);
        assert_eq!(o.max_pages, 200);
        assert_eq!(o.tesseract_path, "tesseract");
        assert_eq!(o.languages, "engrm");
        assert_eq!(o.provider, None);
        o.max_pages = 0;
        o.normalize(|_| true);
        assert_eq!(o.max_pages, 1);
    }
}

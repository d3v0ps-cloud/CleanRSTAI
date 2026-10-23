//! File and image attachments on messages.

use std::path::Path;

use anyhow::{Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// The largest text file or document that can be attached.
const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
/// Text beyond this many characters is cut, with a note saying so.
const MAX_TEXT_CHARS: usize = 1_000_000;

/// An image (base64 `data`) or text (`text`) attached to a message. Documents (PDF, Office,
/// OpenDocument, RTF) are attached as their extracted text, with the document's MIME type.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Base64-encoded bytes, for images.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Recognise an image from its first bytes.
pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

impl Attachment {
    pub fn from_path(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        if std::fs::metadata(path)?.len() > MAX_FILE_BYTES as u64 {
            bail!("{name} is larger than 50 MB");
        }
        let bytes = std::fs::read(path)?;
        Self::from_bytes(name, bytes)
    }

    pub fn from_bytes(name: String, bytes: Vec<u8>) -> Result<Self> {
        if let Some(mime) = image_mime(&bytes) {
            if bytes.len() > MAX_IMAGE_BYTES {
                bail!("{name} is larger than 20 MB");
            }
            return Ok(Self::image(name, mime.into(), &bytes));
        }
        if bytes.len() > MAX_FILE_BYTES {
            bail!("{name} is larger than 50 MB");
        }
        let detected = crate::documents::detect(&bytes)
            .map_err(|e| anyhow::anyhow!("{name} couldn't be read: {e:#}"))?;
        if let Some(format) = detected {
            let text = crate::documents::extract(format, &bytes)
                .map_err(|e| anyhow::anyhow!("{name} couldn't be read: {e:#}"))?;
            return Ok(Self::text(name, format.mime(), text));
        }
        match decode_text(&bytes) {
            Some(text) => Ok(Self::text(name, "text/plain", text)),
            None => bail!(
                "{name} isn't a supported file: attach text, images (PNG, JPEG, GIF, WebP), \
                 PDFs, Office or OpenDocument files, or RTF"
            ),
        }
    }

    fn text(name: String, mime: &str, text: String) -> Self {
        Self {
            name,
            mime: mime.into(),
            text: Some(truncate(text)),
            data: None,
        }
    }

    pub fn image(name: String, mime: String, bytes: &[u8]) -> Self {
        Self {
            name,
            mime,
            text: None,
            data: Some(B64.encode(bytes)),
        }
    }

    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/") && self.data.is_some()
    }

    pub fn bytes(&self) -> Option<Vec<u8>> {
        B64.decode(self.data.as_deref()?).ok()
    }

    pub fn data_url(&self) -> Option<String> {
        Some(format!(
            "data:{};base64,{}",
            self.mime,
            self.data.as_deref()?
        ))
    }

    /// Short human-readable size, e.g. "12 lines", "PDF · 80 lines" or "340 KB".
    pub fn size_label(&self) -> String {
        if let Some(t) = &self.text {
            let lines = t.lines().count();
            let lines = format!("{lines} line{}", if lines == 1 { "" } else { "s" });
            return match crate::documents::Format::from_mime(&self.mime) {
                Some(f) => format!("{} · {lines}", f.label()),
                None => lines,
            };
        }
        let bytes = self.data.as_ref().map(|d| d.len() * 3 / 4).unwrap_or(0);
        if bytes >= 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{} KB", bytes.div_ceil(1024))
        }
    }
}

/// Decode a text file: UTF-8 (with or without a byte order mark), UTF-16 with a byte order
/// mark, or else Windows-1252 for older single-byte files. `None` for binary data.
pub(crate) fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, malformed) = encoding.decode_without_bom_handling(&bytes[bom..]);
        return (!malformed && !text.contains('\0')).then(|| text.into_owned());
    }
    // Binary data has NULs or other control characters that text doesn't.
    let control = bytes
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B))
        .count();
    if bytes.contains(&0) || control * 100 > bytes.len() {
        return None;
    }
    Some(match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        // Not UTF-8: most likely an older single-byte encoding.
        Err(_) => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    })
}

/// Cut text longer than [`MAX_TEXT_CHARS`], saying how much was left out.
fn truncate(text: String) -> String {
    let Some((cut, _)) = text.char_indices().nth(MAX_TEXT_CHARS) else {
        return text;
    };
    let total = text.chars().count();
    format!(
        "{}\n\n[Truncated: showing the first {MAX_TEXT_CHARS} of {total} characters.]",
        &text[..cut]
    )
}

/// The message text sent to the model, with attached text files appended.
pub fn text_with_files(content: &str, attachments: &[Attachment]) -> String {
    let mut out = content.to_owned();
    for a in attachments {
        if let Some(text) = &a.text {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!(
                "<file name=\"{}\">\n{}\n</file>",
                a.name,
                text.trim_end()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44,
        0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF,
        0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn classifies_images_and_text() {
        let img = Attachment::from_bytes("a.bin".into(), PNG_1X1.to_vec()).unwrap();
        assert!(img.is_image());
        assert_eq!(img.mime, "image/png");
        assert_eq!(img.bytes().unwrap(), PNG_1X1);
        assert!(
            img.data_url()
                .unwrap()
                .starts_with("data:image/png;base64,iVBOR")
        );

        let txt = Attachment::from_bytes("notes.md".into(), b"# Hi\nthere\n".to_vec()).unwrap();
        assert!(!txt.is_image());
        assert_eq!(txt.size_label(), "2 lines");

        assert!(Attachment::from_bytes("x.exe".into(), vec![0x4D, 0x5A, 0, 0, 0xFF]).is_err());
        let big = Attachment::from_bytes("big.txt".into(), vec![b'a'; MAX_TEXT_CHARS + 5]).unwrap();
        let big = big.text.unwrap();
        assert!(big.starts_with("aaaa"));
        assert!(big.ends_with(&format!(
            "[Truncated: showing the first {MAX_TEXT_CHARS} of {} characters.]",
            MAX_TEXT_CHARS + 5
        )));
    }

    #[test]
    fn decodes_text_encodings() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("Grüße".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode_text(&utf16).as_deref(), Some("Grüße"));
        assert_eq!(decode_text(b"\xEF\xBB\xBFbom").as_deref(), Some("bom"));
        assert_eq!(
            decode_text(b"caf\xE9 au lait").as_deref(),
            Some("café au lait")
        );
        assert_eq!(decode_text(b"\x01\x02\x03\x04binary\x05"), None);
    }

    #[test]
    fn attaches_documents_as_text() {
        let path = format!(
            "{}/tests/fixtures/documents/report.docx",
            env!("CARGO_MANIFEST_DIR")
        );
        let a = Attachment::from_path(std::path::Path::new(&path)).unwrap();
        assert_eq!(a.name, "report.docx");
        assert!(a.mime.ends_with("wordprocessingml.document"));
        assert!(a.text.as_deref().unwrap().contains("ZEBRA-7"));
        assert!(a.size_label().starts_with("Word document · "));
        let out = text_with_files("Summarise", &[a]);
        assert!(out.starts_with("Summarise\n\n<file name=\"report.docx\">\nQuarterly report\n"));
    }

    #[test]
    fn inlines_text_files() {
        let f = Attachment::from_bytes("a.rs".into(), b"fn main() {}\n".to_vec()).unwrap();
        let img = Attachment::from_bytes("p.png".into(), PNG_1X1.to_vec()).unwrap();
        let out = text_with_files("Review this", &[f, img]);
        assert_eq!(
            out,
            "Review this\n\n<file name=\"a.rs\">\nfn main() {}\n</file>"
        );
        assert_eq!(text_with_files("", &[]), "");
    }
}

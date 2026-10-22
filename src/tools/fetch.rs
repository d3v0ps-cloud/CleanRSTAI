//! The built-in `fetch` tool: GET a web URL and give the model its content as text: web
//! pages converted to readable text, plain text and JSON as they are, images as images.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use reqwest::header;
use serde_json::Value;

use crate::attach::Attachment;
use crate::mcp::ToolOutput;

/// Characters returned per call unless the model asks for another amount.
const DEFAULT_MAX_CHARS: usize = 20_000;
/// The most characters one call may return.
const MAX_CHARS_LIMIT: usize = 100_000;
/// Bytes read from the response; anything after this is dropped.
const MAX_BYTES: usize = 5 * 1024 * 1024;
/// Width that HTML is laid out at when converted to text.
const TEXT_WIDTH: usize = 120;

pub fn spec_description() -> String {
    "Fetch a web URL (http or https) and read its content. Web pages come back as readable \
     text with their links listed, plain text and JSON as they are, and images as images. \
     Long content is returned in parts: if the result says it was truncated, call again with \
     the start_index it gives to read on."
        .into()
}

pub fn spec_parameters() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "url": {"type": "string", "description": "The http or https URL to fetch."},
            "max_length": {
                "type": "integer",
                "description": format!("Most characters to return (default {DEFAULT_MAX_CHARS}, at most {MAX_CHARS_LIMIT}).")
            },
            "start_index": {
                "type": "integer",
                "description": "Character to start at, to continue reading a truncated result (default 0)."
            },
            "raw": {
                "type": "boolean",
                "description": "Return a web page's HTML source instead of converting it to text."
            }
        },
        "required": ["url"]
    })
}

struct Args {
    url: reqwest::Url,
    max_length: usize,
    start_index: usize,
    raw: bool,
}

fn parse_args(args: &Value) -> Result<Args> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .context("missing \"url\"")?
        .trim();
    let url = reqwest::Url::parse(url).with_context(|| format!("\"{url}\" isn't a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!(
            "only http and https URLs can be fetched, not {}:",
            url.scheme()
        );
    }
    let count = |key: &str, default: usize| -> Result<usize> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v
                .as_u64()
                .map(|n| n as usize)
                .with_context(|| format!("\"{key}\" must be a whole number")),
        }
    };
    Ok(Args {
        url,
        max_length: count("max_length", DEFAULT_MAX_CHARS)?.clamp(1, MAX_CHARS_LIMIT),
        start_index: count("start_index", 0)?,
        raw: args.get("raw").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// What a response's body is, from its `Content-Type` and, failing that, its first bytes.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Html,
    Text,
    Image(&'static str),
    Other(String),
}

fn kind_of(content_type: &str, body: &[u8]) -> Kind {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if let Some(image) = crate::attach::image_mime(body) {
        return Kind::Image(image);
    }
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => Kind::Html,
        m if m.starts_with("text/")
            || m.ends_with("+json")
            || m.ends_with("+xml")
            || matches!(
                m,
                "application/json"
                    | "application/xml"
                    | "application/javascript"
                    | "application/x-javascript"
                    | "application/rss+xml"
                    | "application/atom+xml"
            ) =>
        {
            Kind::Text
        }
        // Servers that don't say: look at the content.
        "" | "application/octet-stream" => {
            let head = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
            let head = head.trim_start();
            if head.starts_with("<!doctype html") || head.starts_with("<html") {
                Kind::Html
            } else if std::str::from_utf8(&body[..body.len().min(4096)]).is_ok()
                && !body.contains(&0)
            {
                Kind::Text
            } else {
                Kind::Other(if mime.is_empty() {
                    "unknown".into()
                } else {
                    mime
                })
            }
        }
        _ => Kind::Other(mime),
    }
}

/// Fetch `args.url` and describe what came back for the model.
pub async fn run(http: &reqwest::Client, args: &Value, timeout: Duration) -> Result<ToolOutput> {
    let args = parse_args(args)?;
    let resp = http
        .get(args.url.clone())
        .header(
            header::USER_AGENT,
            concat!(
                "CleanRSTAI/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/d3v0ps-cloud/CleanRSTAI)"
            ),
        )
        .header(
            header::ACCEPT,
            "text/html,application/xhtml+xml,text/plain;q=0.9,application/json;q=0.9,*/*;q=0.5",
        )
        .timeout(timeout)
        .send()
        .await
        .with_context(|| format!("couldn't fetch {}", args.url))?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    let mut body = Vec::new();
    let mut cut_off = false;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("the download was interrupted")?;
        let room = MAX_BYTES - body.len();
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            cut_off = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }

    let mut header = format!("URL: {final_url}\nStatus: {status}\n");
    if !content_type.is_empty() {
        header.push_str(&format!("Content-Type: {content_type}\n"));
    }
    let kind = kind_of(&content_type, &body);
    if let Kind::Image(mime) = kind {
        if cut_off {
            bail!(
                "the image at {final_url} is larger than {} MB",
                MAX_BYTES / (1024 * 1024)
            );
        }
        let name = final_url.rsplit('/').next().unwrap_or("image").to_owned();
        return Ok(ToolOutput {
            text: format!(
                "{header}\nAn image ({mime}, {} bytes); it is attached.",
                body.len()
            ),
            is_error: !status.is_success(),
            images: vec![Attachment::image(name, mime.into(), &body)],
        });
    }
    let text = match kind {
        Kind::Html if !args.raw => html2text::from_read(body.as_slice(), TEXT_WIDTH)
            .context("couldn't convert the page to text")?,
        Kind::Html | Kind::Text => String::from_utf8_lossy(&body).into_owned(),
        Kind::Other(mime) => bail!(
            "{final_url} returned {mime} content (HTTP {status}); only web pages, text, JSON \
             and images can be read"
        ),
        Kind::Image(_) => unreachable!(),
    };

    let total = text.chars().count();
    let start = args.start_index.min(total);
    let page: String = text.chars().skip(start).take(args.max_length).collect();
    let end = start + page.chars().count();
    let mut out = format!("{header}\n{page}");
    if end < total {
        out.push_str(&format!(
            "\n\n[Truncated: characters {start}–{end} of {total}. Call fetch again with \
             start_index={end} to read on.]"
        ));
    } else if start > 0 && page.is_empty() {
        out.push_str(&format!(
            "[Nothing left: the content has {total} characters.]"
        ));
    }
    if cut_off {
        out.push_str(&format!(
            "\n\n[Only the first {} MB of the response was downloaded.]",
            MAX_BYTES / (1024 * 1024)
        ));
    }
    Ok(ToolOutput {
        text: out,
        is_error: !status.is_success(),
        images: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};
    use serde_json::json;

    const PAGE: &str = "<!doctype html><html><head><title>T</title><style>p{}</style></head>\
        <body><h1>Hello</h1><p>Some <b>bold</b> text and <a href=\"/next\">a link</a>.</p>\
        <script>var x = 1;</script></body></html>";

    async fn fetch(args: Value) -> Result<ToolOutput> {
        run(&reqwest::Client::new(), &args, Duration::from_secs(10)).await
    }

    #[tokio::test]
    async fn converts_pages_to_text_and_keeps_text_as_is() {
        let (base, log) = serve(|r| match r.path.as_str() {
            "/page" => Response::with_type(200, "text/html; charset=utf-8", PAGE),
            "/data" => Response::json(json!({"a": 1})),
            "/missing" => Response::with_type(404, "text/plain", "no such page"),
            _ => Response::status(500),
        })
        .await;

        let out = fetch(json!({"url": format!("{base}/page")})).await.unwrap();
        assert!(!out.is_error);
        assert!(out.text.contains("Status: 200 OK"), "{}", out.text);
        assert!(out.text.contains("Hello"), "{}", out.text);
        assert!(out.text.contains("bold"), "{}", out.text);
        assert!(!out.text.contains("<p>"), "{}", out.text);
        assert!(!out.text.contains("var x"), "{}", out.text);
        assert!(out.text.contains("/next"), "links are kept: {}", out.text);
        let ua = log.lock().unwrap()[0]
            .header("user-agent")
            .unwrap()
            .to_owned();
        assert!(ua.starts_with("CleanRSTAI/"), "{ua}");

        let raw = fetch(json!({"url": format!("{base}/page"), "raw": true}))
            .await
            .unwrap();
        assert!(raw.text.contains("<h1>Hello</h1>"), "{}", raw.text);

        let data = fetch(json!({"url": format!("{base}/data")})).await.unwrap();
        assert!(data.text.ends_with("{\"a\":1}"), "{}", data.text);

        let missing = fetch(json!({"url": format!("{base}/missing")}))
            .await
            .unwrap();
        assert!(missing.is_error);
        assert!(missing.text.contains("404"), "{}", missing.text);
        assert!(missing.text.contains("no such page"), "{}", missing.text);
    }

    #[tokio::test]
    async fn pages_through_long_content() {
        let body = "0123456789".repeat(30);
        let (base, _) = serve(move |_| Response::with_type(200, "text/plain", body.clone())).await;
        let url = format!("{base}/long");
        let first = fetch(json!({"url": url, "max_length": 100})).await.unwrap();
        assert!(
            first.text.contains("characters 0–100 of 300"),
            "{}",
            first.text
        );
        assert!(first.text.contains("start_index=100"), "{}", first.text);
        let last = fetch(json!({"url": url, "max_length": 250, "start_index": 100}))
            .await
            .unwrap();
        assert!(!last.text.contains("Truncated"), "{}", last.text);
        assert!(last.text.ends_with("0123456789"), "{}", last.text);
    }

    #[tokio::test]
    async fn returns_images_as_attachments_and_refuses_binaries() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let (base, _) = serve(move |r| match r.path.as_str() {
            "/pic" => Response::with_type(200, "image/png", png.clone()),
            _ => Response::with_type(200, "application/zip", vec![0x50, 0x4B, 3, 4, 0]),
        })
        .await;
        let pic = fetch(json!({"url": format!("{base}/pic")})).await.unwrap();
        assert_eq!(pic.images.len(), 1);
        assert_eq!(pic.images[0].mime, "image/png");
        let zip = fetch(json!({"url": format!("{base}/a.zip")})).await;
        let err = format!("{:#}", zip.unwrap_err());
        assert!(err.contains("application/zip"), "{err}");
    }

    #[tokio::test]
    async fn rejects_bad_urls() {
        for (url, expected) in [
            ("file:///etc/passwd", "only http and https"),
            ("not a url", "isn't a valid URL"),
        ] {
            let err = format!("{:#}", fetch(json!({"url": url})).await.unwrap_err());
            assert!(err.contains(expected), "{url}: {err}");
        }
        let err = format!("{:#}", fetch(json!({})).await.unwrap_err());
        assert!(err.contains("missing \"url\""), "{err}");
    }

    #[test]
    fn sniffs_content_without_a_type() {
        assert_eq!(kind_of("", b"<!DOCTYPE html><p>x"), Kind::Html);
        assert_eq!(
            kind_of("application/octet-stream", b"plain words"),
            Kind::Text
        );
        assert_eq!(kind_of("application/ld+json", b"{}"), Kind::Text);
        assert_eq!(kind_of("", &[0u8, 1, 2, 3]), Kind::Other("unknown".into()));
    }
}

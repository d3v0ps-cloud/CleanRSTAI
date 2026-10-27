//! The web search tool: asks the service chosen in Settings (DuckDuckGo, a SearXNG instance,
//! Brave or Tavily) and lists the results for the model, which can read a page with the fetch
//! tool. The service and its address come from Settings, never from the model, so the app's
//! own HTTP client is used (a SearXNG instance may well run on this computer).

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use reqwest::header;
use serde_json::{Value, json};

use crate::config::{MAX_SEARCH_RESULTS, SearchBackend, WebSearchSettings};

const DUCKDUCKGO: &str = "https://html.duckduckgo.com";
const BRAVE: &str = "https://api.search.brave.com";
const TAVILY: &str = "https://api.tavily.com";
const MAX_SNIPPET_CHARS: usize = 400;

pub fn spec_description(settings: &WebSearchSettings) -> String {
    format!(
        "Search the web ({}) and get a list of results: title, address and a short snippet. \
         Use it for current events, facts you're unsure of, documentation and anything after \
         your training data. Then read the most useful pages with the web fetch tool, when it \
         is available, rather than relying on snippets alone.",
        settings.backend.label()
    )
}

pub fn spec_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "What to search for, as you would type it into a search engine."},
            "count": {"type": "integer", "description": "How many results to return (optional; the user's setting by default)."}
        },
        "required": ["query"]
    })
}

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub async fn run(
    http: &reqwest::Client,
    settings: &WebSearchSettings,
    args: &Value,
    timeout: Duration,
) -> Result<String> {
    let base = match settings.backend {
        SearchBackend::DuckDuckGo => DUCKDUCKGO,
        SearchBackend::Searxng => settings.searxng_url.as_str(),
        SearchBackend::Brave => BRAVE,
        SearchBackend::Tavily => TAVILY,
    };
    run_at(http, settings, base, args, timeout).await
}

/// [`run`] against `base` instead of the service's own address (for tests).
async fn run_at(
    http: &reqwest::Client,
    settings: &WebSearchSettings,
    base: &str,
    args: &Value,
    timeout: Duration,
) -> Result<String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .context("missing \"query\"")?;
    let count = args
        .get("count")
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, MAX_SEARCH_RESULTS as u64) as usize)
        .unwrap_or(settings.max_results as usize);
    if settings.backend.needs_key() && settings.api_key.trim().is_empty() {
        bail!(
            "web search needs an API key for {}; add one in Settings → Built-in Tools",
            settings.backend.label()
        );
    }
    if base.trim().is_empty() {
        bail!(
            "web search needs the address of a SearXNG instance; set it in Settings → Built-in Tools"
        );
    }
    let base = base.trim_end_matches('/');
    let mut hits = match settings.backend {
        SearchBackend::DuckDuckGo => duckduckgo(http, base, query, settings, timeout).await?,
        SearchBackend::Searxng => searxng(http, base, query, settings, timeout).await?,
        SearchBackend::Brave => brave(http, base, query, count, settings, timeout).await?,
        SearchBackend::Tavily => tavily(http, base, query, count, settings, timeout).await?,
    };
    hits.truncate(count);
    Ok(describe(query, settings.backend, &hits))
}

fn user_agent() -> &'static str {
    concat!(
        "CleanRSTAI/",
        env!("CARGO_PKG_VERSION"),
        " (+https://github.com/",
        crate::update::repo!(),
        ")"
    )
}

async fn get_text(req: reqwest::RequestBuilder, service: &str) -> Result<String> {
    let resp = req
        .send()
        .await
        .with_context(|| format!("couldn't reach {service}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail: String = text.chars().take(300).collect();
        bail!("{service} answered {status}: {}", detail.trim());
    }
    Ok(text)
}

async fn duckduckgo(
    http: &reqwest::Client,
    base: &str,
    query: &str,
    settings: &WebSearchSettings,
    timeout: Duration,
) -> Result<Vec<Hit>> {
    let req = http
        .get(format!("{base}/html/"))
        .query(&[
            ("q", query),
            ("kp", if settings.safe_search { "1" } else { "-2" }),
        ])
        .header(header::USER_AGENT, user_agent())
        .header(header::ACCEPT, "text/html")
        .timeout(timeout);
    let html = get_text(req, "DuckDuckGo").await?;
    let hits = parse_duckduckgo(&html);
    if hits.is_empty() && html.contains("anomaly") {
        bail!(
            "DuckDuckGo asked for a human check instead of answering; try again later, or choose \
             another search service in Settings → Built-in Tools"
        );
    }
    Ok(hits)
}

/// Results from DuckDuckGo's HTML page: each `result__a` link, and the `result__snippet` that
/// follows it. Ads (links through `y.js`) are skipped.
pub fn parse_duckduckgo(html: &str) -> Vec<Hit> {
    static LINK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
            .unwrap()
    });
    static SNIPPET: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"(?s)class="result__snippet"[^>]*>(.*?)</(?:a|td|div)>"#).unwrap()
    });
    let links: Vec<_> = LINK.captures_iter(html).collect();
    let mut hits = Vec::new();
    for (i, cap) in links.iter().enumerate() {
        let whole = cap.get(0).unwrap();
        let end = links
            .get(i + 1)
            .map_or(html.len(), |next| next.get(0).unwrap().start());
        let url = result_url(&decode_entities(&cap[1]));
        let Some(url) = url.filter(|u| !u.contains("duckduckgo.com/y.js")) else {
            continue;
        };
        let snippet = SNIPPET
            .captures(&html[whole.end()..end])
            .map(|s| plain_text(&s[1]))
            .unwrap_or_default();
        hits.push(Hit {
            title: plain_text(&cap[2]),
            url,
            snippet,
        });
    }
    hits
}

/// The address a DuckDuckGo result points to: its `uddg` parameter, or the link itself.
fn result_url(href: &str) -> Option<String> {
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_owned()
    };
    let parsed = reqwest::Url::parse(&absolute).ok()?;
    if let Some((_, target)) = parsed.query_pairs().find(|(k, _)| k == "uddg") {
        return Some(target.into_owned());
    }
    matches!(parsed.scheme(), "http" | "https").then_some(absolute)
}

async fn searxng(
    http: &reqwest::Client,
    base: &str,
    query: &str,
    settings: &WebSearchSettings,
    timeout: Duration,
) -> Result<Vec<Hit>> {
    let req = http
        .get(format!("{base}/search"))
        .query(&[
            ("q", query),
            ("format", "json"),
            ("safesearch", if settings.safe_search { "1" } else { "0" }),
        ])
        .header(header::USER_AGENT, user_agent())
        .header(header::ACCEPT, "application/json")
        .timeout(timeout);
    let text = get_text(req, "SearXNG").await.map_err(|e| {
        if e.to_string().contains("403") {
            e.context("the SearXNG instance refused JSON results; add `json` to `search.formats` in its settings.yml")
        } else {
            e
        }
    })?;
    let v: Value = serde_json::from_str(&text).context("SearXNG didn't answer with JSON")?;
    Ok(hits_from(&v["results"], "title", "url", "content"))
}

async fn brave(
    http: &reqwest::Client,
    base: &str,
    query: &str,
    count: usize,
    settings: &WebSearchSettings,
    timeout: Duration,
) -> Result<Vec<Hit>> {
    let req = http
        .get(format!("{base}/res/v1/web/search"))
        .query(&[
            ("q", query),
            ("count", &count.min(20).to_string()),
            (
                "safesearch",
                if settings.safe_search {
                    "strict"
                } else {
                    "off"
                },
            ),
        ])
        .header("X-Subscription-Token", settings.api_key.trim())
        .header(header::ACCEPT, "application/json")
        .timeout(timeout);
    let text = get_text(req, "Brave Search").await?;
    let v: Value = serde_json::from_str(&text).context("Brave Search didn't answer with JSON")?;
    Ok(hits_from(
        &v["web"]["results"],
        "title",
        "url",
        "description",
    ))
}

async fn tavily(
    http: &reqwest::Client,
    base: &str,
    query: &str,
    count: usize,
    settings: &WebSearchSettings,
    timeout: Duration,
) -> Result<Vec<Hit>> {
    let req = http
        .post(format!("{base}/search"))
        .bearer_auth(settings.api_key.trim())
        .json(&json!({"query": query, "max_results": count, "search_depth": "basic"}))
        .timeout(timeout);
    let text = get_text(req, "Tavily").await?;
    let v: Value = serde_json::from_str(&text).context("Tavily didn't answer with JSON")?;
    Ok(hits_from(&v["results"], "title", "url", "content"))
}

fn hits_from(list: &Value, title: &str, url: &str, snippet: &str) -> Vec<Hit> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some(Hit {
                title: plain_text(r[title].as_str().unwrap_or_default()),
                url: r[url].as_str().filter(|u| !u.is_empty())?.to_owned(),
                snippet: plain_text(r[snippet].as_str().unwrap_or_default()),
            })
        })
        .collect()
}

/// The results as text for the model.
fn describe(query: &str, backend: SearchBackend, hits: &[Hit]) -> String {
    if hits.is_empty() {
        return format!(
            "No results for {query:?} ({}). Try other words.",
            backend.label()
        );
    }
    let mut out = format!("Results for {query:?} ({}):\n", backend.label());
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!("\n{}. {}\n   {}\n", i + 1, h.title, h.url));
        if !h.snippet.is_empty() {
            out.push_str(&format!("   {}\n", clip(&h.snippet, MAX_SNIPPET_CHARS)));
        }
    }
    out
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// `html` without its tags and entities, on one line.
fn plain_text(html: &str) -> String {
    static TAG: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"<[^>]*>").unwrap());
    let text = decode_entities(&TAG.replace_all(html, ""));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(s: &str) -> String {
    static ENTITY: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
    ENTITY
        .replace_all(s, |c: &regex::Captures| {
            let e = &c[1];
            let ch = if let Some(hex) = e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
            } else if let Some(dec) = e.strip_prefix('#') {
                dec.parse().ok().and_then(char::from_u32)
            } else {
                match e {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    _ => None,
                }
            };
            ch.map_or_else(|| c[0].to_owned(), String::from)
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};

    const DDG: &str = r##"
<div class="result results_links results_links_deep web-result ">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2F&amp;rut=abc">Rust <b>Programming</b> Language</a>
  </h2>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x">A language empowering everyone to build reliable &amp; efficient software.</a>
</div>
<div class="result result--ad">
  <a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=x">Buy now</a>
  <a class="result__snippet" href="#">An ad</a>
</div>
<div class="result">
  <a rel="nofollow" class="result__a" href="https://doc.rust-lang.org/book/">The Book</a>
</div>"##;

    #[test]
    fn parses_duckduckgo_results_and_skips_ads() {
        let hits = parse_duckduckgo(DDG);
        assert_eq!(
            hits,
            [
                Hit {
                    title: "Rust Programming Language".into(),
                    url: "https://www.rust-lang.org/".into(),
                    snippet:
                        "A language empowering everyone to build reliable & efficient software."
                            .into(),
                },
                Hit {
                    title: "The Book".into(),
                    url: "https://doc.rust-lang.org/book/".into(),
                    snippet: String::new(),
                },
            ]
        );
        assert!(parse_duckduckgo("<html>nothing</html>").is_empty());
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &amp; b &#39;c&#x27; &lt;d&gt; &bogus;"),
            "a & b 'c' <d> &bogus;"
        );
        assert_eq!(plain_text("  <b>x</b>\n y "), "x y");
    }

    fn settings(backend: SearchBackend) -> WebSearchSettings {
        WebSearchSettings {
            backend,
            api_key: "key-1".into(),
            max_results: 2,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn asks_each_service_and_lists_results() {
        let (base, log) = serve(|r| match r.path.split('?').next().unwrap() {
            "/html/" => Response::with_type(200, "text/html", DDG),
            "/search" if r.method == "GET" => Response::json(json!({"results": [
                {"title": "S1", "url": "https://a.example/", "content": "first"},
                {"title": "S2", "url": "https://b.example/", "content": "second"},
                {"title": "S3", "url": "https://c.example/", "content": "third"}]})),
            "/search" => Response::json(json!({"results": [
                {"title": "T1", "url": "https://t.example/", "content": "tavily"}]})),
            "/res/v1/web/search" => Response::json(json!({"web": {"results": [
                {"title": "B1", "url": "https://brave.example/", "description": "<strong>brave</strong>"}]}})),
            _ => Response::status(404),
        })
        .await;
        let http = reqwest::Client::new();
        let t = Duration::from_secs(5);
        let args = json!({"query": "rust lang"});

        let out = run_at(&http, &settings(SearchBackend::DuckDuckGo), &base, &args, t)
            .await
            .unwrap();
        assert!(
            out.starts_with("Results for \"rust lang\" (DuckDuckGo):"),
            "{out}"
        );
        assert!(
            out.contains("1. Rust Programming Language\n   https://www.rust-lang.org/"),
            "{out}"
        );

        // SearXNG: the setting's count (2) caps the list; `count` raises it.
        let out = run_at(&http, &settings(SearchBackend::Searxng), &base, &args, t)
            .await
            .unwrap();
        assert!(out.contains("2. S2") && !out.contains("S3"), "{out}");
        let more = json!({"query": "rust lang", "count": 3});
        let out = run_at(&http, &settings(SearchBackend::Searxng), &base, &more, t)
            .await
            .unwrap();
        assert!(out.contains("3. S3"), "{out}");

        let out = run_at(&http, &settings(SearchBackend::Brave), &base, &args, t)
            .await
            .unwrap();
        assert!(
            out.contains("1. B1\n   https://brave.example/\n   brave"),
            "{out}"
        );
        let out = run_at(&http, &settings(SearchBackend::Tavily), &base, &args, t)
            .await
            .unwrap();
        assert!(out.contains("T1"), "{out}");

        let log = log.lock().unwrap();
        let brave = log.iter().find(|r| r.path.starts_with("/res/")).unwrap();
        assert_eq!(brave.header("x-subscription-token"), Some("key-1"));
        assert!(brave.path.contains("safesearch=strict"), "{}", brave.path);
        let tavily = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(tavily.header("authorization"), Some("Bearer key-1"));
        assert_eq!(tavily.json()["query"], "rust lang");
    }

    #[tokio::test]
    async fn explains_what_is_missing() {
        let http = reqwest::Client::new();
        let t = Duration::from_secs(5);
        let mut s = settings(SearchBackend::Brave);
        s.api_key.clear();
        let e = run_at(&http, &s, BRAVE, &json!({"query": "x"}), t)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("API key"), "{e}");
        let s = settings(SearchBackend::Searxng);
        let e = run(&http, &s, &json!({"query": "x"}), t).await.unwrap_err();
        assert!(e.to_string().contains("SearXNG"), "{e}");
        let e = run_at(&http, &s, "http://x", &json!({}), t)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("query"), "{e}");
    }
}

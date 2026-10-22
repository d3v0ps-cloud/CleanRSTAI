//! Check GitHub Releases for a newer version of the app.

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

pub const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/d3v0ps-cloud/CleanRSTAI/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Version without the leading `v`, e.g. `0.3.0`.
    pub version: String,
    pub url: String,
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateCheck {
    UpToDate,
    Available(Release),
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Parse `v1.2.3` / `1.2.3-beta.1` into comparable numbers. Pre-releases sort before the
/// release they precede.
fn parse_version(v: &str) -> Option<(u64, u64, u64, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once(['-', '+']) {
        Some((core, _)) => (core, v.contains('-')),
        None => (v, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    Some((major, minor, patch, !pre))
}

/// Whether `candidate` is a newer version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

pub async fn check(http: &reqwest::Client, url: &str, current: &str) -> Result<UpdateCheck> {
    let resp = http
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .context("couldn't reach GitHub")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateCheck::UpToDate); // no releases published yet
    }
    if !resp.status().is_success() {
        bail!("GitHub answered HTTP {}", resp.status());
    }
    let r: ApiRelease = resp
        .json()
        .await
        .context("unexpected response from GitHub")?;
    if r.draft || r.prerelease || !is_newer(&r.tag_name, current) {
        return Ok(UpdateCheck::UpToDate);
    }
    Ok(UpdateCheck::Available(Release {
        version: r.tag_name.trim_start_matches('v').to_owned(),
        url: r.html_url,
        notes: r.body.unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Response, serve};

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("nightly", "0.2.0"));
        assert!(is_newer("v1", "0.9.0"));
    }

    #[tokio::test]
    async fn checks_latest_release() {
        let (base, _) = serve(|req| match req.path.as_str() {
            "/new" => Response::json(serde_json::json!({
                "tag_name": "v9.0.0",
                "html_url": "https://example.com/r/v9.0.0",
                "body": "Notes",
            })),
            "/same" => Response::json(serde_json::json!({
                "tag_name": "v0.2.0", "html_url": "x",
            })),
            "/pre" => Response::json(serde_json::json!({
                "tag_name": "v9.0.0-beta", "html_url": "x", "prerelease": true,
            })),
            "/none" => Response::status(404),
            _ => Response::status(500),
        })
        .await;
        let http = reqwest::Client::new();
        let got = check(&http, &format!("{base}/new"), "0.2.0").await.unwrap();
        assert_eq!(
            got,
            UpdateCheck::Available(Release {
                version: "9.0.0".into(),
                url: "https://example.com/r/v9.0.0".into(),
                notes: "Notes".into(),
            })
        );
        for path in ["same", "pre", "none"] {
            let got = check(&http, &format!("{base}/{path}"), "0.2.0")
                .await
                .unwrap();
            assert_eq!(got, UpdateCheck::UpToDate, "{path}");
        }
        assert!(check(&http, &format!("{base}/err"), "0.2.0").await.is_err());
    }
}

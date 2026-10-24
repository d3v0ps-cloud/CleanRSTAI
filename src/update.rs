//! Check GitHub Releases for a newer version of the app, and install it in place.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

/// The GitHub repository (`owner/name`), as a literal for `concat!`. Also named in
/// `scripts/release-local.sh`.
macro_rules! repo {
    () => {
        "d3v0ps-cloud/CleanRSTAI"
    };
}
pub(crate) use repo;

pub const LATEST_RELEASE_URL: &str =
    concat!("https://api.github.com/repos/", repo!(), "/releases/latest");

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Version without the leading `v`, e.g. `0.3.0`.
    pub version: String,
    pub url: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
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
    #[serde(default)]
    assets: Vec<Asset>,
}

/// The releases API URL for the "Update from" setting: `owner/repo` and `github.com/owner/repo`
/// links become GitHub's `releases/latest` endpoint; any other URL is used as given (it must
/// answer with a GitHub-style release, as Gitea and Forgejo do).
pub fn release_api_url(setting: &str) -> String {
    let s = setting.trim().trim_end_matches('/');
    let repo = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("github.com/"))
        .or_else(|| (!s.contains("://")).then_some(s));
    if let Some(path) = repo {
        let mut parts = path.split('/').filter(|p| !p.is_empty());
        if let (Some(owner), Some(name)) = (parts.next(), parts.next()) {
            let name = name.trim_end_matches(".git");
            return format!("https://api.github.com/repos/{owner}/{name}/releases/latest");
        }
    }
    s.to_owned()
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
        // Also what a private repository answers without a sign-in, so not "up to date".
        bail!("no public release at {url} (is the repository private?)");
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
        assets: r.assets,
    }))
}

/// The checksum file the release workflow publishes next to the builds.
const SUMS_NAME: &str = "SHA256SUMS.txt";

/// How a release file is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Package {
    /// `.tar.gz` holding the bare `cleanrstai` executable (Linux).
    Tarball,
    /// `.zip` holding `CleanRSTAI.app`, made by `ditto` (macOS).
    AppZip,
}

/// The release file this build updates itself from, or `None` where it can't.
pub fn platform_package() -> Option<(String, Package)> {
    if cfg!(target_os = "macos") {
        Some(("CleanRSTAI-macos-universal.zip".into(), Package::AppZip))
    } else if cfg!(target_os = "linux") {
        Some((
            format!("CleanRSTAI-linux-{}.tar.gz", std::env::consts::ARCH),
            Package::Tarball,
        ))
    } else {
        None
    }
}

/// Whether this build can install `r` itself, rather than sending the user to the
/// release page.
pub fn can_install(r: &Release) -> bool {
    platform_package().is_some_and(|(name, _)| {
        r.assets.iter().any(|a| a.name == name) && r.assets.iter().any(|a| a.name == SUMS_NAME)
    }) && install_target().is_ok()
}

/// What an update replaces: the `.app` bundle on macOS, the executable elsewhere.
pub fn install_target() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .context("couldn't find the running executable")?;
    if cfg!(target_os = "macos") {
        exe.ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(Path::to_path_buf)
            .context("the app isn't running from a .app bundle")
    } else {
        Ok(exe)
    }
}

/// Download `r`'s file for this platform, check it against the release's SHA256SUMS.txt
/// and put it in place of `target`. The running app keeps working; the new version starts
/// with [`relaunch`]. `progress` gets the bytes downloaded so far and the total, if known.
pub async fn install(
    http: &reqwest::Client,
    r: &Release,
    (name, package): (&str, Package),
    target: &Path,
    progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    let find = |n: &str| {
        r.assets
            .iter()
            .find(|a| a.name == n)
            .with_context(|| format!("release {} has no {n}", r.version))
    };
    let (asset, sums) = (find(name)?, find(SUMS_NAME)?);
    let sums = get(http, &sums.url)
        .await?
        .text()
        .await
        .context("couldn't download the checksums")?;
    let expected =
        expected_sha256(&sums, name).with_context(|| format!("{SUMS_NAME} doesn't list {name}"))?;

    let resp = get(http, &asset.url).await?;
    let total = resp.content_length();
    let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt as _;
    while let Some(chunk) = stream.next().await {
        data.extend_from_slice(&chunk.context("download interrupted")?);
        progress(data.len() as u64, total);
    }
    use sha2::Digest as _;
    let got = format!("{:x}", sha2::Sha256::digest(&data));
    if !got.eq_ignore_ascii_case(&expected) {
        bail!("{name} doesn't match its checksum; nothing was changed");
    }

    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || unpack_into_place(&data, package, &target))
        .await
        .context("the install task failed")?
}

async fn get(http: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    http.get(url)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("couldn't download {url}"))
}

/// The hash listed for `name` in `sha256sum` output (`<hex>  <name>`, or `*<name>`).
fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_owned())
    })
}

/// Unpack the archive in a staging folder next to `target` (so the final move is a rename
/// on the same file system), then swap it in.
fn unpack_into_place(data: &[u8], package: Package, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .context("the install location has no parent folder")?;
    let staging = parent.join(format!(".cleanrstai-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging).with_context(|| {
        format!(
            "can't write to {}; download the update from the release page instead",
            parent.display()
        )
    })?;
    let result = (|| {
        let archive = staging.join("download");
        std::fs::write(&archive, data).context("couldn't save the download")?;
        let unpacked = staging.join("unpacked");
        std::fs::create_dir(&unpacked)?;
        match package {
            Package::Tarball => {
                run(std::process::Command::new("tar")
                    .args(["--no-same-owner", "-xzf"])
                    .arg(&archive)
                    .arg("-C")
                    .arg(&unpacked))?;
                let new = unpacked.join("cleanrstai");
                if !new.is_file() {
                    bail!("the download doesn't contain the cleanrstai executable");
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
                }
                // Renaming over a running executable is fine on Unix: the old file lives
                // on until the process exits.
                std::fs::rename(&new, target).context("couldn't replace the executable")
            }
            Package::AppZip => {
                // `ditto` keeps the bundle's permissions, symlinks and signature intact.
                run(std::process::Command::new("ditto")
                    .args(["-x", "-k"])
                    .arg(&archive)
                    .arg(&unpacked))?;
                let new = std::fs::read_dir(&unpacked)?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .find(|p| p.extension().is_some_and(|e| e == "app"))
                    .context("the download doesn't contain an .app")?;
                replace_dir(&new, target, &staging.join("previous.app"))
            }
        }
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Swap the folder `new` in for `target`, moving the old one to `backup` (and back again
/// if the swap fails).
fn replace_dir(new: &Path, target: &Path, backup: &Path) -> Result<()> {
    std::fs::rename(target, backup)
        .with_context(|| format!("couldn't move {} aside", target.display()))?;
    if let Err(e) = std::fs::rename(new, target) {
        let _ = std::fs::rename(backup, target);
        return Err(e).context("couldn't put the new version in place");
    }
    Ok(())
}

fn run(cmd: &mut std::process::Command) -> Result<()> {
    let out = cmd
        .output()
        .with_context(|| format!("couldn't run {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!(
            "{:?} failed: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Start the installed `target` (from [`install_target`]) as a new process. The caller
/// then closes this one.
pub fn relaunch(target: &Path) -> Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-n").arg(target);
        c
    } else {
        std::process::Command::new(target)
    };
    cmd.spawn().context("couldn't start the new version")?;
    Ok(())
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
                "assets": [{"name": "a.tar.gz", "browser_download_url": "https://example.com/a"}],
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
                assets: vec![Asset {
                    name: "a.tar.gz".into(),
                    url: "https://example.com/a".into(),
                }],
            })
        );
        for path in ["same", "pre"] {
            let got = check(&http, &format!("{base}/{path}"), "0.2.0")
                .await
                .unwrap();
            assert_eq!(got, UpdateCheck::UpToDate, "{path}");
        }
        for path in ["none", "err"] {
            assert!(
                check(&http, &format!("{base}/{path}"), "0.2.0")
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn release_api_urls() {
        let api = "https://api.github.com/repos/me/fork/releases/latest";
        for s in [
            "me/fork",
            "https://github.com/me/fork",
            "github.com/me/fork/",
            "https://github.com/me/fork.git",
            "https://github.com/me/fork/releases",
            api,
        ] {
            assert_eq!(release_api_url(s), api, "{s}");
        }
        assert_eq!(release_api_url(LATEST_RELEASE_URL), LATEST_RELEASE_URL);
        let gitea = "https://git.example.com/api/v1/repos/me/fork/releases/latest";
        assert_eq!(release_api_url(gitea), gitea);
    }

    #[test]
    fn reads_checksums() {
        let sums = "abc123  CleanRSTAI-linux-x86_64.tar.gz\ndef456 *SHA256SUMS.txt\n";
        assert_eq!(
            expected_sha256(sums, "CleanRSTAI-linux-x86_64.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            expected_sha256(sums, "SHA256SUMS.txt").as_deref(),
            Some("def456")
        );
        assert_eq!(expected_sha256(sums, "CleanRSTAI-linux-x86.tar.gz"), None);
    }

    #[test]
    fn replace_dir_swaps_and_restores() {
        let dir = std::env::temp_dir().join(format!("cleanrstai-upd-{}", uuid::Uuid::new_v4()));
        let (new, target) = (dir.join("new.app"), dir.join("App.app"));
        std::fs::create_dir_all(&new).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(new.join("v"), "2").unwrap();
        std::fs::write(target.join("v"), "1").unwrap();
        replace_dir(&new, &target, &dir.join("old.app")).unwrap();
        assert_eq!(std::fs::read_to_string(target.join("v")).unwrap(), "2");
        assert_eq!(std::fs::read_to_string(dir.join("old.app/v")).unwrap(), "1");
        // A missing new version leaves the installed one where it was.
        assert!(replace_dir(&dir.join("missing"), &target, &dir.join("old2.app")).is_err());
        assert_eq!(std::fs::read_to_string(target.join("v")).unwrap(), "2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Download, verify and swap in a tarball, the way Linux builds update.
    #[cfg(unix)]
    #[tokio::test]
    async fn installs_a_verified_tarball() {
        use sha2::Digest as _;
        let dir = std::env::temp_dir().join(format!("cleanrstai-upd-{}", uuid::Uuid::new_v4()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("cleanrstai"), "new build").unwrap();
        let tarball = dir.join("pkg.tar.gz");
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(&src)
            .arg("cleanrstai")
            .status()
            .unwrap();
        assert!(ok.success());
        let bytes = std::fs::read(&tarball).unwrap();
        let sum = format!("{:x}", sha2::Sha256::digest(&bytes));

        let good_sums = format!("{sum}  pkg.tar.gz\n");
        let bad_sums = format!("{}  pkg.tar.gz\n", "0".repeat(64));
        let (base, _) = serve(move |req| match req.path.as_str() {
            "/pkg.tar.gz" => Response::with_type(200, "application/gzip", bytes.clone()),
            "/good" => Response::with_type(200, "text/plain", good_sums.clone()),
            "/bad" => Response::with_type(200, "text/plain", bad_sums.clone()),
            _ => Response::status(404),
        })
        .await;
        let release = |sums: &str| Release {
            version: "9.0.0".into(),
            url: String::new(),
            notes: String::new(),
            assets: vec![
                Asset {
                    name: "pkg.tar.gz".into(),
                    url: format!("{base}/pkg.tar.gz"),
                },
                Asset {
                    name: SUMS_NAME.into(),
                    url: format!("{base}/{sums}"),
                },
            ],
        };
        let target = dir.join("bin/cleanrstai");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "old build").unwrap();
        let http = reqwest::Client::new();
        let pkg = ("pkg.tar.gz", Package::Tarball);

        // A checksum mismatch changes nothing.
        let err = install(&http, &release("bad"), pkg, &target, |_, _| {})
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old build");

        let seen = std::sync::Mutex::new(0);
        install(&http, &release("good"), pkg, &target, |done, _| {
            *seen.lock().unwrap() = done
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new build");
        assert!(*seen.lock().unwrap() > 0);
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        // The staging folder is cleaned up.
        let left: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("cleanrstai")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

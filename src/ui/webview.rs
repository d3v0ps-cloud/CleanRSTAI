//! The live preview of web page artifacts: a native web view (WKWebView on macOS, WebView2 on
//! Windows) laid over the artifact panel's preview area. The page gets a Content-Security-Policy
//! from Settings (scripts and network on or off); links open in the browser, never in the view.
//! Linux builds have no web view, so web pages open in the browser there.
//!
//! The view is a native child window drawn above egui, so the app hides it whenever something
//! else (settings, the palette, a menu) could sit on top of it.

use eframe::egui;

use crate::config::HtmlPreviewSettings;

/// Whether this build can show web pages in the app.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The Content-Security-Policy for a preview, or `None` when it may do anything.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn policy(s: &HtmlPreviewSettings) -> Option<String> {
    match (s.scripts, s.network) {
        (true, true) => None,
        (false, true) => Some("script-src 'none'".into()),
        (scripts, false) => Some(format!(
            "default-src 'none'; img-src data: blob:; media-src data: blob:; font-src data:; \
             style-src 'unsafe-inline' data:; connect-src 'none'; frame-src 'none'; \
             form-action 'none'; script-src {}",
            if scripts {
                "'unsafe-inline' 'unsafe-eval' blob: data:"
            } else {
                "'none'"
            }
        )),
    }
}

/// `html` as the preview loads it: with the policy's `<meta>` first (after a doctype, so the
/// page keeps standards mode).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn page(html: &str, s: &HtmlPreviewSettings) -> String {
    let Some(policy) = policy(s) else {
        return html.to_owned();
    };
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">");
    let trimmed = html.trim_start();
    let lead = html.len() - trimmed.len();
    if trimmed
        .get(..9)
        .is_some_and(|s| s.eq_ignore_ascii_case("<!doctype"))
        && let Some(end) = trimmed.find('>')
    {
        let at = lead + end + 1;
        return format!("{}{meta}{}", &html[..at], &html[at..]);
    }
    format!("{meta}{html}")
}

/// The web view, while one exists.
#[derive(Default)]
pub struct HtmlPreview {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    shown: Option<native::Shown>,
    /// Why the web view couldn't be made, shown in the panel instead.
    pub error: Option<String>,
}

impl HtmlPreview {
    /// Show `html` over `rect` (in points) in `frame`'s window.
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "windows")),
        allow(unused_variables)
    )]
    pub fn show(
        &mut self,
        frame: &eframe::Frame,
        rect: egui::Rect,
        pixels_per_point: f32,
        html: &str,
        s: &HtmlPreviewSettings,
    ) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if self.error.is_none()
            && let Err(e) = native::show(&mut self.shown, frame, rect, pixels_per_point, html, s)
        {
            self.error = Some(e);
            self.shown = None;
        }
    }

    /// Hide the view (it is kept, so showing it again is quick).
    pub fn hide(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(shown) = &mut self.shown {
            native::hide(shown);
        }
    }

    /// Drop the view, e.g. when the settings change.
    pub fn reset(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.shown = None;
        }
        self.error = None;
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use std::hash::{DefaultHasher, Hash, Hasher};

    use eframe::egui;
    use wry::dpi::{PhysicalPosition, PhysicalSize};

    use crate::config::HtmlPreviewSettings;

    pub struct Shown {
        webview: wry::WebView,
        /// Hash of the page loaded.
        page: u64,
        bounds: [i32; 4],
        visible: bool,
    }

    fn rect(bounds: [i32; 4]) -> wry::Rect {
        wry::Rect {
            position: PhysicalPosition::new(bounds[0], bounds[1]).into(),
            size: PhysicalSize::new(bounds[2].max(1) as u32, bounds[3].max(1) as u32).into(),
        }
    }

    /// Only the page itself (and in-page anchors) loads in the view; links open in the browser.
    fn navigate(url: String) -> bool {
        if url.starts_with("about:") || url.starts_with("data:") {
            return true;
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            let _ = open::that_detached(&url);
        }
        false
    }

    pub fn show(
        shown: &mut Option<Shown>,
        frame: &eframe::Frame,
        area: egui::Rect,
        ppp: f32,
        html: &str,
        s: &HtmlPreviewSettings,
    ) -> Result<(), String> {
        let page = super::page(html, s);
        let mut h = DefaultHasher::new();
        page.hash(&mut h);
        let hash = h.finish();
        let bounds = [
            (area.min.x * ppp).round() as i32,
            (area.min.y * ppp).round() as i32,
            (area.width() * ppp).round() as i32,
            (area.height() * ppp).round() as i32,
        ];
        match shown {
            Some(v) => {
                if v.page != hash {
                    v.webview.load_html(&page).map_err(|e| e.to_string())?;
                    v.page = hash;
                }
                if v.bounds != bounds {
                    v.webview
                        .set_bounds(rect(bounds))
                        .map_err(|e| e.to_string())?;
                    v.bounds = bounds;
                }
                if !v.visible {
                    v.webview.set_visible(true).map_err(|e| e.to_string())?;
                    v.visible = true;
                }
            }
            None => {
                let webview = wry::WebViewBuilder::new()
                    .with_html(page)
                    .with_bounds(rect(bounds))
                    .with_devtools(false)
                    .with_navigation_handler(navigate)
                    .with_new_window_req_handler(|url, _| {
                        navigate(url);
                        wry::NewWindowResponse::Deny
                    })
                    .build_as_child(frame)
                    .map_err(|e| e.to_string())?;
                *shown = Some(Shown {
                    webview,
                    page: hash,
                    bounds,
                    visible: true,
                });
            }
        }
        Ok(())
    }

    pub fn hide(shown: &mut Shown) {
        if shown.visible {
            let _ = shown.webview.set_visible(false);
            shown.visible = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_follows_the_settings() {
        let s = |scripts, network| HtmlPreviewSettings {
            enabled: true,
            scripts,
            network,
        };
        assert_eq!(policy(&s(true, true)), None);
        assert_eq!(policy(&s(false, true)).unwrap(), "script-src 'none'");
        let offline = policy(&s(true, false)).unwrap();
        assert!(offline.starts_with("default-src 'none'"), "{offline}");
        assert!(offline.contains("connect-src 'none'"), "{offline}");
        assert!(offline.ends_with("script-src 'unsafe-inline' 'unsafe-eval' blob: data:"));
        assert!(
            policy(&s(false, false))
                .unwrap()
                .ends_with("script-src 'none'")
        );
    }

    #[test]
    fn the_policy_goes_after_the_doctype() {
        let s = HtmlPreviewSettings::default();
        let out = page("\n<!DOCTYPE html><html><body>hi</body></html>", &s);
        assert!(
            out.starts_with("\n<!DOCTYPE html><meta http-equiv=\"Content-Security-Policy\""),
            "{out}"
        );
        assert!(out.ends_with("\"><html><body>hi</body></html>"), "{out}");
        let out = page("<p>hi</p>", &s);
        assert!(
            out.starts_with("<meta http-equiv") && out.ends_with("<p>hi</p>"),
            "{out}"
        );
        let open = HtmlPreviewSettings { network: true, ..s };
        assert_eq!(page("<p>hi</p>", &open), "<p>hi</p>");
    }
}

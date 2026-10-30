//! The menu bar (macOS) or system tray (Windows) icon behind "Keep running when the window is
//! closed" (Settings → Background). Closing the window then hides it, and replies and MCP
//! connections keep running until Quit is chosen from the icon's menu.
//!
//! The decisions are pure and tested here; the platform code only builds the icon and reads its
//! events. Linux has no tray: tray-icon needs GTK there and the release builds don't link it,
//! so the Linux build has a stub that never creates an icon.

/// What a tray click or menu item asks the app to do.
#[cfg_attr(
    not(any(target_os = "macos", windows)),
    allow(dead_code, reason = "only the platform code makes these")
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the window again and focus it.
    Show,
    /// Quit: the next close request closes the app instead of hiding it.
    Quit,
}

/// What a close request does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Close the window and end the app.
    Quit,
    /// Hide the window; the tray icon brings it back.
    Hide,
}

/// Whether this build can show a tray icon at all.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

/// Decides what closing the window does. It hides only when the setting is on, a tray icon
/// exists to bring the window back, and the user hasn't chosen Quit.
pub fn on_close(keep_running: bool, tray_exists: bool, quitting: bool) -> CloseAction {
    if keep_running && tray_exists && !quitting {
        CloseAction::Hide
    } else {
        CloseAction::Quit
    }
}

/// The checkbox label in Settings → Background.
pub fn setting_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Keep running in the menu bar when the window is closed"
    } else {
        "Keep running in the system tray when the window is closed"
    }
}

/// The notification sent the first time the window hides to the tray in an app run.
pub fn hidden_notice() -> &'static str {
    if cfg!(target_os = "macos") {
        "CleanRSTAI is still running in the menu bar"
    } else {
        "CleanRSTAI is still running in the system tray"
    }
}

#[cfg(any(target_os = "macos", windows))]
mod platform {
    use super::Command;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    const SHOW_ID: &str = "show";
    const QUIT_ID: &str = "quit";
    /// Square size of the icon: 22 pt at 2x on macOS, 16 pt at 200% on Windows.
    const SIZE_MAC: u32 = 44;
    const SIZE_WINDOWS: u32 = 32;

    /// The icon in the menu bar or tray. Dropping it removes the icon, so the app keeps it for as
    /// long as the setting is on.
    pub struct Tray {
        _icon: TrayIcon,
    }

    impl Tray {
        /// Builds the icon, or `None` if the OS refuses it (the close button then quits as
        /// before). Must run on the main thread.
        pub fn new(dark: bool) -> Option<Tray> {
            let built = match build(dark) {
                Ok(tray) => tray,
                Err(e) => {
                    log::warn!("tray icon unavailable: {e}");
                    return None;
                }
            };
            Some(Tray { _icon: built })
        }

        /// Drains the clicks and menu choices since the last call.
        pub fn poll(&self) -> Vec<Command> {
            let mut out = Vec::new();
            while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
                // Windows: a left click shows the window (the menu is on the right click).
                // macOS: the menu opens on any click, and its Show item does the same.
                let left_click = matches!(
                    event,
                    tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    }
                );
                if cfg!(windows) && left_click {
                    out.push(Command::Show);
                }
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                match event.id.as_ref() {
                    SHOW_ID => out.push(Command::Show),
                    QUIT_ID => out.push(Command::Quit),
                    _ => {}
                }
            }
            out
        }
    }

    fn build(dark: bool) -> Result<TrayIcon, Box<dyn std::error::Error>> {
        let size = if cfg!(windows) {
            SIZE_WINDOWS
        } else {
            SIZE_MAC
        };
        let window = crate::ui::theme::window_icon(dark);
        let rgba = image::RgbaImage::from_raw(window.width, window.height, window.rgba.clone())
            .ok_or("the window icon has the wrong size")?;
        let scaled =
            image::imageops::resize(&rgba, size, size, image::imageops::FilterType::Lanczos3);
        let icon = Icon::from_rgba(scaled.into_raw(), size, size)?;

        let menu = Menu::new();
        let show = MenuItem::with_id(SHOW_ID, "Show CleanRSTAI", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])?;

        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("CleanRSTAI")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(cfg!(not(windows)))
            .build()?;
        Ok(tray)
    }
}

#[cfg(any(target_os = "macos", windows))]
pub use platform::Tray;

/// Linux: no tray, so `new` never returns one and nothing is polled.
#[cfg(not(any(target_os = "macos", windows)))]
#[allow(dead_code, reason = "a tray is never created on this platform")]
pub struct Tray(());

#[cfg(not(any(target_os = "macos", windows)))]
impl Tray {
    pub fn new(_dark: bool) -> Option<Tray> {
        None
    }

    pub fn poll(&self) -> Vec<Command> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_hides_only_when_kept_running_with_a_tray_and_not_quitting() {
        assert_eq!(on_close(true, true, false), CloseAction::Hide);
        // Setting off: the window closes as it always did.
        assert_eq!(on_close(false, true, false), CloseAction::Quit);
        // No tray icon (Linux, or the OS refused it): nothing would bring the window back.
        assert_eq!(on_close(true, false, false), CloseAction::Quit);
        // Quit chosen from the tray: the close goes through.
        assert_eq!(on_close(true, true, true), CloseAction::Quit);
        assert_eq!(on_close(false, false, true), CloseAction::Quit);
    }

    #[test]
    fn notices_and_labels_name_the_platform_surface() {
        let label = setting_label();
        assert!(label.starts_with("Keep running in the "));
        assert!(label.ends_with(" when the window is closed"));
        assert!(hidden_notice().starts_with("CleanRSTAI is still running in the "));
        if cfg!(target_os = "macos") {
            assert!(label.contains("menu bar") && hidden_notice().contains("menu bar"));
        } else {
            assert!(label.contains("system tray") && hidden_notice().contains("system tray"));
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn linux_has_no_tray() {
        const { assert!(!SUPPORTED) };
        assert!(Tray::new(false).is_none());
    }
}

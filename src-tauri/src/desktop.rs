//! Living on the desktop (WHIT-824, design §5.7): one instance, a tray icon,
//! starting at login if the person asks, and the `whity-agent://` link.
//!
//! A link can come from any web page, so it does nothing by itself: a valid one
//! opens the window with the workspace's address filled in, and signing in stays
//! the person's own step.

use crate::oauth;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_OPEN: &str = "open";
const TRAY_QUIT: &str = "quit";
/// Started at login, the app waits in the tray rather than opening its window.
pub const HIDDEN_ARG: &str = "--hidden";

/// A workspace address from a link, waiting for the webview to ask for it.
#[derive(Default)]
pub struct PendingLink(pub Mutex<Option<String>>);

/// The tray's menu items, kept to relabel when the language changes.
pub struct TrayLabels {
    open: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

/// The workspace address in `whity-agent://connect?instance=<address>`, checked
/// like an address the person typed. Anything else is ignored.
pub fn connect_address(link: &str) -> Option<String> {
    let url = url::Url::parse(link).ok()?;
    if url.scheme() != "whity-agent" || url.host_str() != Some("connect") {
        return None;
    }
    let instance = url
        .query_pairs()
        .find(|(key, _)| key == "instance")
        .map(|(_, value)| value.into_owned())?;
    oauth::normalize_address(&instance)
        .ok()
        .map(|origin| origin.as_str().trim_end_matches('/').to_string())
}

/// Bring the window forward.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Act on any `whity-agent://` links among a process's arguments or an OS event.
pub fn handle_links<'a>(app: &AppHandle, links: impl IntoIterator<Item = &'a str>) {
    for address in links.into_iter().filter_map(connect_address) {
        *app.state::<PendingLink>().0.lock().unwrap() = Some(address.clone());
        let _ = app.emit("connect-link", address);
        show_window(app);
    }
}

/// The tray icon: left click opens the window, the menu opens it or quits.
pub fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, TRAY_OPEN, "Open Whity Agent", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Whity Agent")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_OPEN => show_window(app),
            TRAY_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayLabels { open, quit });
    Ok(())
}

/// The tray in the language the person chose; the webview holds both catalogs.
pub fn set_tray_labels(app: &AppHandle, open: &str, quit: &str) -> Result<(), String> {
    let labels = app.state::<TrayLabels>();
    labels.open.set_text(open).map_err(|e| e.to_string())?;
    labels.quit.set_text(quit).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connect_link_gives_a_checked_address() {
        assert_eq!(
            connect_address("whity-agent://connect?instance=app.whity.dev").as_deref(),
            Some("https://app.whity.dev")
        );
        assert_eq!(
            connect_address(
                "whity-agent://connect?instance=https%3A%2F%2Fapp.whity.dev%2Fhome%3Fx%3D1"
            )
            .as_deref(),
            Some("https://app.whity.dev"),
            "an address, not a page"
        );
    }

    #[test]
    fn anything_else_is_ignored() {
        for link in [
            "https://connect?instance=app.whity.dev",
            "whity-agent://disconnect?instance=app.whity.dev",
            "whity-agent://connect",
            "whity-agent://connect?instance=",
            "whity-agent://connect?instance=http%3A%2F%2Fevil.example",
            "whity-agent://connect?instance=https%3A%2F%2Fuser%3Apass%40evil.example",
            "--hidden",
            "not a link",
        ] {
            assert_eq!(connect_address(link), None, "{link}");
        }
    }
}

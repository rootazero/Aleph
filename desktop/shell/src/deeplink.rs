//! `aleph://` deep links.
//!
//! Registers the `aleph://` URL scheme and turns an incoming link into two
//! pure-I/O actions: bring the window forward, and hand the raw URL to the
//! Panel as a DOM event. The shell never interprets the link — the Panel
//! (R2/R4) decides what, if anything, to do with it.

use tauri::{AppHandle, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

/// Wire deep-link handling. On Windows and Linux the scheme is registered
/// with the OS at runtime; on macOS it is declared in the app bundle's
/// Info.plist, generated from `tauri.conf.json`.
pub fn setup(app: &AppHandle) {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    if let Err(e) = app.deep_link().register_all() {
        tracing::warn!("could not register the aleph:// scheme: {e}");
    }

    // Fires for the link that cold-started the app and for every link
    // delivered while it is already running.
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            handle_url(&handle, url.as_str());
        }
    });
}

/// React to one `aleph://` URL: focus the window, then deliver the raw URL
/// to the Panel as an `aleph:deep-link` DOM event for it to route.
fn handle_url(app: &AppHandle, url: &str) {
    tracing::info!("deep link received: {url}");
    crate::focus_window(app);

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.eval(deep_link_script(url)) {
        tracing::debug!("could not deliver the deep link to the Panel: {e}");
    }
}

/// Build the JS that dispatches the deep link into the Panel document, with
/// the URL escaped for a single-quoted string literal.
fn deep_link_script(url: &str) -> String {
    let safe = url.replace('\\', "\\\\").replace('\'', "\\'");
    format!("window.dispatchEvent(new CustomEvent('aleph:deep-link',{{detail:'{safe}'}}))")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_script_escapes_quotes_and_backslashes() {
        let js = deep_link_script("aleph://x'y\\z");
        assert!(js.contains("aleph://x\\'y\\\\z"));
        assert!(js.starts_with("window.dispatchEvent"));
    }
}

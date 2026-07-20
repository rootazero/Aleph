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
    // URLs commonly carry auth codes / tokens in the query (e.g.
    // `aleph://oauth?code=...&state=...`) which must never reach an
    // info-level log. Strip the query and fragment so the log only
    // records "what scheme did the user invoke", and drop to debug
    // for the full URL behind a feature most operators never enable.
    tracing::info!("deep link received: {}", redacted_for_log(url));
    if tracing::level_filters::LevelFilter::current()
        >= tracing::level_filters::LevelFilter::DEBUG
    {
        tracing::debug!("deep link full URL: {url}");
    }
    crate::focus_window(app);

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.eval(deep_link_script(url)) {
        tracing::debug!("could not deliver the deep link to the Panel: {e}");
    }
}

/// Return the URL with query string and fragment stripped, so secrets
/// carried in either segment never reach an info-level log. Falls back to
/// the input unchanged when parsing fails.
fn redacted_for_log(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let mut s = u.clone();
            s.set_query(None);
            s.set_fragment(None);
            s.to_string()
        }
        Err(_) => {
            // Not a parseable absolute URL — best effort: drop the
            // first `?` or `#` and everything after.
            let end = url
                .find('?')
                .or_else(|| url.find('#'))
                .unwrap_or(url.len());
            url[..end].to_string()
        }
    }
}

/// Build the JS that dispatches the deep link into the Panel document, with
/// the URL escaped for a single-quoted string literal.
fn deep_link_script(url: &str) -> String {
    // serde_json emits a fully-escaped, JS-safe double-quoted string literal —
    // it covers backslashes, quotes AND control chars (newline/CR) that the old
    // hand-rolled replace() missed, so a crafted `aleph://` payload cannot break
    // out of the literal. `to_string` on a &str is infallible; fall back to an
    // empty literal defensively.
    let safe = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".to_string());
    format!("window.dispatchEvent(new CustomEvent('aleph:deep-link',{{detail:{safe}}}))")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_script_escapes_quotes_and_backslashes() {
        let js = deep_link_script("aleph://x'y\\z");
        // serde_json double-quotes the literal: backslash escaped, single quote
        // left bare (valid inside a double-quoted JS string).
        assert!(js.contains(r#""aleph://x'y\\z""#));
        assert!(js.starts_with("window.dispatchEvent"));
    }

    #[test]
    fn deep_link_script_escapes_control_chars() {
        let js = deep_link_script("aleph://x\ny");
        // A raw newline would terminate a JS string literal; it must be escaped.
        assert!(!js.contains('\n'));
        assert!(js.contains("\\n"));
    }
}

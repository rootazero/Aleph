// Individual browser tools — focused, single-responsibility browser actions.
//
// Each tool wraps a ProfileManager and implements AlephTool for one operation.

pub mod click;
pub mod console;
pub mod cookies;
pub mod dialog;
pub mod drag;
pub mod emulate;
pub mod evaluate;
pub mod fill_form;
pub mod hover;
pub mod navigate;
pub mod network;
pub mod open;
pub mod pdf;
pub mod press_key;
pub mod profile_tool;
pub mod resize;
pub mod screenshot;
pub mod scroll;
pub mod select;
pub mod session;
pub mod snapshot;
pub mod tabs;
pub mod type_text;
pub mod upload;
pub mod wait_for;

use crate::browser::backend::BrowserBackend;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
use crate::browser::error::BrowserError;
use crate::browser::manager::ProfileManager;
use crate::browser::playwright_cli_backend::PlaywrightCliBackend;
use crate::browser::profile::BrowserDriver;

/// Parse one `list_tabs` line into `(id, url)`.
///
/// Handles both the Chrome DevTools MCP format `"N: URL"` and the Playwright
/// CLI format `"Tab N: URL"`, and strips a trailing annotation such as
/// `" [selected]"` from the URL. Returns `None` for lines without a numeric id.
fn parse_tab_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    // Normalize "Tab N: URL" → "N: URL" so both formats share one parser.
    let rest = line.strip_prefix("Tab ").unwrap_or(line);
    let colon = rest.find(": ")?;
    let id = rest.get(..colon)?.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let url_part = rest.get(colon + 2..)?.trim();
    // Strip a trailing " [selected]" / " [active]" style annotation so the URL
    // round-trips through a strict parser.
    let url = match url_part.rfind(" [") {
        Some(pos) if url_part.ends_with(']') => url_part.get(..pos).unwrap_or(url_part).trim(),
        _ => url_part,
    };
    Some((id.to_string(), url.to_string()))
}

/// The active (most recent) tab id, or `None` if no tabs are open.
/// Uses the last entry because newly opened tabs append to the list.
fn parse_active_tab_id(tabs_text: &str) -> Option<String> {
    tabs_text
        .lines()
        .filter_map(parse_tab_line)
        .map(|(id, _)| id)
        .next_back()
}

/// The current URL of `tab_id` as reported by `list_tabs`, if present.
fn extract_tab_url(tabs_text: &str, tab_id: &str) -> Option<String> {
    tabs_text
        .lines()
        .filter_map(parse_tab_line)
        .rfind(|(id, _)| id == tab_id)
        .map(|(_, url)| url)
}

/// Returns `Some(violation)` if the active tab's current http(s) URL is blocked
/// by the SSRF policy. Non-http schemes (`about:blank`, `chrome://`, …) carry no
/// network target and are skipped.
fn current_page_block(manager: &ProfileManager, tabs_text: &str, tab_id: &str) -> Option<String> {
    let url = extract_tab_url(tabs_text, tab_id)?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    manager.check_url(&url).err().map(|v| v.to_string())
}

/// Get the active (most recent) tab from the backend, or return an error if none open.
async fn get_active_tab(backend: &dyn BrowserBackend) -> Result<String, BrowserError> {
    let tabs_text = backend.list_tabs().await?;
    parse_active_tab_id(&tabs_text)
        .ok_or_else(|| BrowserError::ActionFailed("No tabs open. Use browser_open first.".into()))
}

/// Create the appropriate backend for the given profile.
///
/// Headless mode follows the profile-level override, falling back to the global
/// `playwright_cli.headless` default (resolved via `ProfileManager::resolve_headless`).
pub(crate) fn make_backend(manager: &ProfileManager, profile: &str) -> Box<dyn BrowserBackend> {
    manager.record_activity(profile);
    match manager.get_driver(profile) {
        Some(BrowserDriver::ExistingSession) => Box::new(ChromeMcpBackend::new(
            manager.get_chrome_mcp_driver(),
            profile.to_string(),
            manager.get_ssrf_guard(),
        )),
        Some(BrowserDriver::Managed) | None => Box::new(PlaywrightCliBackend::new(
            manager.get_playwright_cli_driver(),
            profile.to_string(),
            manager.get_ssrf_guard(),
            manager.resolve_headless(profile),
        )),
    }
}

/// Create the appropriate backend and resolve the active tab ID.
pub(crate) async fn make_backend_and_tab(
    manager: &ProfileManager,
    profile: &str,
) -> Result<(Box<dyn BrowserBackend>, String), BrowserError> {
    let backend = make_backend(manager, profile);
    let tab_id = get_active_tab(backend.as_ref()).await?;
    Ok((backend, tab_id))
}

/// Like [`make_backend_and_tab`], but additionally asserts the active tab's
/// CURRENT url passes the SSRF policy before any page-content read.
///
/// Navigation-time guards (`browser_open` / `browser_navigate` goto) only vet
/// the URL being navigated *to*. A page can still reach a forbidden internal
/// origin afterwards via an HTTP redirect, a JS-driven `location` change, or
/// back/forward history — none of which re-pass the navigation guard. Content
/// reads (snapshot, console, network, screenshot, pdf, evaluate) would then
/// exfiltrate that internal content. This closes that read-time bypass
/// (openclaw #78526 / GHSA-2x93-h3hg-2xfp).
///
/// Interaction/navigation tools deliberately keep using [`make_backend_and_tab`]
/// so the agent can always navigate *away* from a blocked page.
pub(crate) async fn make_backend_and_tab_guarded(
    manager: &ProfileManager,
    profile: &str,
) -> Result<(Box<dyn BrowserBackend>, String), BrowserError> {
    let backend = make_backend(manager, profile);
    let tabs_text = backend.list_tabs().await?;
    let tab_id = parse_active_tab_id(&tabs_text).ok_or_else(|| {
        BrowserError::ActionFailed("No tabs open. Use browser_open first.".into())
    })?;
    if let Some(violation) = current_page_block(manager, &tabs_text, &tab_id) {
        return Err(BrowserError::NavigationFailed(format!(
            "current page blocked by SSRF policy ({violation}); \
             navigate to an allowed URL before reading page content"
        )));
    }
    Ok((backend, tab_id))
}

pub use click::{BrowserClickArgs, BrowserClickOutput, BrowserClickTool};
pub use console::{BrowserConsoleArgs, BrowserConsoleOutput, BrowserConsoleTool};
pub use cookies::{BrowserCookiesArgs, BrowserCookiesOutput, BrowserCookiesTool};
pub use dialog::{BrowserDialogArgs, BrowserDialogOutput, BrowserDialogTool};
pub use drag::{BrowserDragArgs, BrowserDragOutput, BrowserDragTool};
pub use emulate::{BrowserEmulateArgs, BrowserEmulateOutput, BrowserEmulateTool};
pub use evaluate::{BrowserEvaluateArgs, BrowserEvaluateOutput, BrowserEvaluateTool};
pub use fill_form::{BrowserFillFormArgs, BrowserFillFormOutput, BrowserFillFormTool};
pub use hover::{BrowserHoverArgs, BrowserHoverOutput, BrowserHoverTool};
pub use navigate::{BrowserNavigateArgs, BrowserNavigateOutput, BrowserNavigateTool};
pub use network::{BrowserNetworkArgs, BrowserNetworkOutput, BrowserNetworkTool};
pub use open::{BrowserOpenArgs, BrowserOpenOutput, BrowserOpenTool};
pub use pdf::{BrowserPdfArgs, BrowserPdfOutput, BrowserPdfTool};
pub use press_key::{BrowserPressKeyArgs, BrowserPressKeyOutput, BrowserPressKeyTool};
pub use profile_tool::{BrowserProfileArgs, BrowserProfileOutput, BrowserProfileTool};
pub use resize::{BrowserResizeArgs, BrowserResizeOutput, BrowserResizeTool};
pub use screenshot::{BrowserScreenshotArgs, BrowserScreenshotOutput, BrowserScreenshotTool};
pub use scroll::{BrowserScrollArgs, BrowserScrollOutput, BrowserScrollTool};
pub use select::{BrowserSelectArgs, BrowserSelectOutput, BrowserSelectTool};
pub use session::{BrowserSessionArgs, BrowserSessionOutput, BrowserSessionTool};
pub use snapshot::{BrowserSnapshotArgs, BrowserSnapshotOutput, BrowserSnapshotTool};
pub use tabs::{BrowserTabsArgs, BrowserTabsOutput, BrowserTabsTool};
pub use type_text::{BrowserTypeArgs, BrowserTypeOutput, BrowserTypeTool};
pub use upload::{BrowserUploadArgs, BrowserUploadOutput, BrowserUploadTool};
pub use wait_for::{BrowserWaitForArgs, BrowserWaitForOutput, BrowserWaitForTool};

/// Default browser profile name, used by serde `default` attributes across all browser tools.
pub(crate) fn default_profile() -> String {
    "default".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[test]
    fn parse_tab_line_handles_both_formats_and_annotations() {
        assert_eq!(
            parse_tab_line("1: https://example.com [selected]"),
            Some(("1".into(), "https://example.com".into()))
        );
        assert_eq!(
            parse_tab_line("Tab 2: http://10.0.0.1/x"),
            Some(("2".into(), "http://10.0.0.1/x".into()))
        );
        assert_eq!(
            parse_tab_line("3: about:blank"),
            Some(("3".into(), "about:blank".into()))
        );
        // Non-numeric / malformed lines are ignored.
        assert_eq!(parse_tab_line("no colon here"), None);
        assert_eq!(parse_tab_line("Tab x: http://a"), None);
    }

    #[test]
    fn parse_active_tab_id_picks_last() {
        let text = "1: https://a.com\n2: https://b.com\n3: https://c.com";
        assert_eq!(parse_active_tab_id(text).as_deref(), Some("3"));
        assert_eq!(parse_active_tab_id("").as_deref(), None);
    }

    #[test]
    fn extract_tab_url_matches_id() {
        let text = "1: https://a.com\n2: http://10.0.0.1/x [selected]";
        assert_eq!(
            extract_tab_url(text, "2").as_deref(),
            Some("http://10.0.0.1/x")
        );
        assert_eq!(extract_tab_url(text, "9"), None);
    }

    #[test]
    fn current_page_block_flags_internal_http_urls() {
        // Default browser SSRF policy blocks private/loopback/link-local.
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // Cloud metadata endpoint reached via redirect → blocked.
        assert!(
            current_page_block(&manager, "1: http://169.254.169.254/latest/meta-data", "1")
                .is_some()
        );

        // Loopback → blocked.
        assert!(current_page_block(&manager, "1: http://127.0.0.1:9000/", "1").is_some());

        // Public URL → allowed.
        assert!(current_page_block(&manager, "1: https://example.com/", "1").is_none());

        // Non-http schemes carry no network target → skipped.
        assert!(current_page_block(&manager, "1: about:blank", "1").is_none());

        // No matching tab → nothing to check.
        assert!(current_page_block(&manager, "1: http://127.0.0.1/", "2").is_none());
    }
}

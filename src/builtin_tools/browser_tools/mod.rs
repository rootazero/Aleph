// Individual browser tools — focused, single-responsibility browser actions.
//
// Each tool wraps a ProfileManager and implements AlephTool for one operation.

pub mod click;
pub mod console;
pub mod evaluate;
pub mod fill_form;
pub mod navigate;
pub mod open;
pub mod press_key;
pub mod profile_tool;
pub mod screenshot;
pub mod select;
pub mod snapshot;
pub mod tabs;
pub mod type_text;
pub mod wait_for;

use crate::browser::backend::BrowserBackend;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
use crate::browser::error::BrowserError;
use crate::browser::manager::ProfileManager;
use crate::browser::playwright_cli_backend::PlaywrightCliBackend;
use crate::browser::profile::BrowserDriver;

/// Get the active (most recent) tab from the backend, or return an error if none open.
/// Uses last() because newly opened tabs appear at the end of the list.
async fn get_active_tab(backend: &dyn BrowserBackend) -> Result<String, BrowserError> {
    let tabs_text = backend.list_tabs().await?;
    // Parse the last numeric id from text lines like "1: URL [selected]" or "Tab 1: URL"
    let last_id = tabs_text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            // Chrome DevTools MCP format: "N: URL"
            if let Some(colon_pos) = line.find(": ") {
                let id_str = line.get(..colon_pos)?.trim();
                if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                    return Some(id_str.to_string());
                }
            }
            // Playwright CLI format: "Tab N: URL"
            if let Some(rest) = line.strip_prefix("Tab ") {
                if let Some(colon_pos) = rest.find(": ") {
                    let id_str = rest.get(..colon_pos)?.trim();
                    return Some(id_str.to_string());
                }
            }
            None
        })
        .last();
    last_id
        .ok_or_else(|| BrowserError::ActionFailed("No tabs open. Use browser_open first.".into()))
}

/// Create the appropriate backend for the given profile.
pub(crate) fn make_backend(manager: &ProfileManager, profile: &str) -> Box<dyn BrowserBackend> {
    manager.record_activity(profile);
    match manager.get_driver(profile) {
        Some(BrowserDriver::ExistingSession) => Box::new(ChromeMcpBackend::new(
            manager.get_chrome_mcp_driver(),
            profile.to_string(),
        )),
        Some(BrowserDriver::Managed) | None => Box::new(PlaywrightCliBackend::new(
            manager.get_playwright_cli_driver(),
            profile.to_string(),
            manager.get_ssrf_guard(),
            true, // headless default; profile-level override applied via get_backend()
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

pub use click::{BrowserClickArgs, BrowserClickOutput, BrowserClickTool};
pub use console::{BrowserConsoleArgs, BrowserConsoleOutput, BrowserConsoleTool};
pub use evaluate::{BrowserEvaluateArgs, BrowserEvaluateOutput, BrowserEvaluateTool};
pub use fill_form::{BrowserFillFormArgs, BrowserFillFormOutput, BrowserFillFormTool};
pub use navigate::{BrowserNavigateArgs, BrowserNavigateOutput, BrowserNavigateTool};
pub use open::{BrowserOpenArgs, BrowserOpenOutput, BrowserOpenTool};
pub use press_key::{BrowserPressKeyArgs, BrowserPressKeyOutput, BrowserPressKeyTool};
pub use profile_tool::{BrowserProfileArgs, BrowserProfileOutput, BrowserProfileTool};
pub use screenshot::{BrowserScreenshotArgs, BrowserScreenshotOutput, BrowserScreenshotTool};
pub use select::{BrowserSelectArgs, BrowserSelectOutput, BrowserSelectTool};
pub use snapshot::{BrowserSnapshotArgs, BrowserSnapshotOutput, BrowserSnapshotTool};
pub use tabs::{BrowserTabsArgs, BrowserTabsOutput, BrowserTabsTool};
pub use type_text::{BrowserTypeArgs, BrowserTypeOutput, BrowserTypeTool};
pub use wait_for::{BrowserWaitForArgs, BrowserWaitForOutput, BrowserWaitForTool};

/// Default browser profile name, used by serde `default` attributes across all browser tools.
pub(crate) fn default_profile() -> String {
    "default".into()
}

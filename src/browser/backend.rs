//! `BrowserBackend` trait — text-first unified contract for browser drivers.

use std::path::Path;

use async_trait::async_trait;

use super::error::BrowserError;
use super::profile::BrowserDriver;
use super::types::{
    ActionTarget, CookieOp, EmulateOptions, HistoryNav, ScreenshotOpts, ScreenshotOutput,
    ScrollDirection, SnapshotOutput, TabId, TabLine, WaitCondition,
};

#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;
    async fn list_tabs(&self) -> Result<Vec<TabLine>, BrowserError>;
    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError>;
    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError>;
    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError>;
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn scroll(
        &self,
        tab_id: &str,
        target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError>;
    async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError>;
    async fn snapshot(&self, tab_id: &str) -> Result<SnapshotOutput, BrowserError>;
    /// Evaluate `js` in the page and return **the value it produced** — not a
    /// transcript of the call.
    ///
    /// The distinction is load-bearing, not stylistic: `wait_probe` searches
    /// this string for a sentinel that is a literal inside every probe it
    /// builds, so a backend that echoes the script back makes the search true
    /// on the first poll and every `wait_for` on that driver silently reports
    /// "found". The managed driver did exactly that for as long as it existed
    /// (`playwright-cli eval` prints `### Ran Playwright code` under the
    /// result); see `playwright_cli::parse_result_value`.
    ///
    /// A failure the backend cannot express as a value (a thrown script) may be
    /// returned as its diagnostic text, provided that text does not restate the
    /// script.
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError>;
    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError>;

    /// Send a raw key press to the focused element.
    ///
    /// Required method — all three backends have a native key primitive.
    async fn press_key(&self, tab_id: &str, key: &str) -> Result<(), BrowserError>;

    /// Navigate the tab's history: back, forward, or reload the current page.
    ///
    /// Required method — all three backends implement it via a native history
    /// primitive so the command waits for the resulting navigation to complete.
    async fn history(&self, tab_id: &str, nav: HistoryNav) -> Result<(), BrowserError>;

    /// Double-click an element.
    ///
    /// Coordinate double-click is a **per-backend capability**, not a property
    /// of the verb: the CDP backend dispatches two press/release pairs at the
    /// point, while the playwright-cli and Chrome `DevTools` MCP backends have
    /// only a ref-taking native primitive and refuse a coordinate with
    /// [`BrowserError::ActionFailed`] naming the ref requirement. A caller that
    /// needs it on an unnamed element uses a `driver = "cdp"` profile.
    ///
    /// Required method — all three backends have a native double-click.
    async fn dblclick(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;

    /// Wait until `condition` holds on the tab, within `timeout_ms`.
    /// Returns `Ok(false)` on timeout — an absent condition is an answer, not
    /// an error. The default impl polls `evaluate` with a JS probe (see
    /// [`super::wait_probe`]); backends with a native wait primitive override
    /// the arms they support (Chrome `DevTools` MCP overrides `Text`).
    ///
    /// A real shared default, not a capability stub: the managed Playwright
    /// backend has no wait command at all and runs exactly this body.
    async fn wait_for(
        &self,
        tab_id: &str,
        condition: &WaitCondition,
        timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        super::wait_probe::poll_wait_for(self, tab_id, condition, timeout_ms).await
    }

    /// Console messages captured for the tab.
    ///
    /// Required method — all three backends have a native console listing.
    async fn console_messages(&self, tab_id: &str) -> Result<String, BrowserError>;

    /// Network request log for the tab.
    ///
    /// Required method — all three backends have a native network listing.
    async fn network_log(&self, tab_id: &str) -> Result<String, BrowserError>;

    /// Print-to-PDF — writes PDF to `output_path`.
    ///
    /// Not universal: the managed Playwright backend has a `pdf` command and
    /// the CDP backend has `Page.printToPDF`, so this default is served by
    /// exactly one backend — the existing-session one — and therefore names the
    /// driver that does serve it. A bare "pdf not supported" leaves the model
    /// unable to tell whether the action, the profile, or the page is at fault,
    /// while the remedy is always the same.
    async fn pdf(&self, _tab_id: &str, _output_path: &Path) -> Result<(), BrowserError> {
        Err(unsupported_by_driver("pdf", BrowserDriver::Managed))
    }

    /// Bring the given tab to the foreground / make it the active page.
    ///
    /// Required method — Chrome `DevTools` MCP selects natively, the Playwright
    /// CLI via `tab-select`, the CDP backend via `Target.activateTarget`.
    ///
    /// Note for callers: the selection only survives if whoever asks "which tab
    /// is active" next honors the driver's own `[selected]` marker — see
    /// [`super::tab_registry::active_tab`], which takes the rows
    /// [`Self::list_tabs`] returns and is the single source for that question.
    async fn switch_tab(&self, tab_id: &str) -> Result<(), BrowserError>;

    /// Respond to a pending native dialog (alert / confirm / prompt / beforeunload).
    /// `action` is "accept" or "dismiss"; `prompt_text` is the text to fill into a
    /// prompt before accepting (ignored for non-prompt dialogs).
    ///
    /// Required method — all three backends have a native dialog primitive.
    async fn handle_dialog(
        &self,
        tab_id: &str,
        action: &str,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError>;

    /// Drag-and-drop from one snapshot element onto another.
    /// Both targets must be snapshot refs (coordinates unsupported).
    ///
    /// Required method — all three backends have a native drag.
    async fn drag(
        &self,
        tab_id: &str,
        from: ActionTarget,
        to: ActionTarget,
    ) -> Result<(), BrowserError>;

    /// Attach one or more local files to a file input.
    /// `target` identifies the `<input type=file>` element (required by the
    /// existing-session/MCP backend; ignored by the managed CLI backend, which
    /// targets the page's file chooser directly).
    ///
    /// Required method — all three backends have a native upload.
    async fn upload(
        &self,
        tab_id: &str,
        target: Option<ActionTarget>,
        paths: &[String],
    ) -> Result<(), BrowserError>;

    /// Resize the browser viewport / window to `width` × `height` CSS pixels.
    ///
    /// Required method — all three backends have a native resize.
    async fn resize(&self, tab_id: &str, width: u32, height: u32) -> Result<(), BrowserError>;

    /// Apply environment/device emulation overrides (color scheme, geolocation,
    /// network/CPU throttling, extra HTTP headers, user-agent) to a tab.
    /// Only the fields set in `opts` are applied.
    ///
    /// Required method — all three backends implement it (the managed one accepts
    /// only the subset the Playwright CLI can express, and names the rest).
    async fn emulate(&self, tab_id: &str, opts: &EmulateOptions) -> Result<(), BrowserError>;

    /// Persist the browser's storage state (cookies + localStorage — the
    /// authentication state) to an absolute file path, so a logged-in session
    /// can be reused later.
    ///
    /// Not universal: the managed Playwright backend has a storage-state
    /// primitive and the CDP backend assembles the same `storageState` shape
    /// out of `Network.getAllCookies` plus the active origin's `localStorage`,
    /// so only the existing-session backend takes this default (see
    /// [`Self::pdf`] for why it names the driver it speaks for).
    async fn save_state(&self, _path: &Path) -> Result<(), BrowserError> {
        Err(unsupported_by_driver("save_state", BrowserDriver::Managed))
    }

    /// Restore a previously-saved storage state from an absolute file path,
    /// re-establishing cookies + localStorage.
    ///
    /// Not universal — see [`Self::save_state`].
    async fn load_state(&self, _path: &Path) -> Result<(), BrowserError> {
        Err(unsupported_by_driver("load_state", BrowserDriver::Managed))
    }

    /// Run a cookie-management operation, returning the backend's textual output
    /// (a cookie listing for `List` / `Get`; empty/confirmation for mutations).
    ///
    /// Not universal — see [`Self::save_state`].
    async fn cookies(&self, _op: &CookieOp) -> Result<String, BrowserError> {
        Err(unsupported_by_driver("cookies", BrowserDriver::Managed))
    }

    /// Fill several fields in one call.
    ///
    /// A real shared default, not a capability stub: the managed Playwright
    /// backend has no batch-fill command and runs exactly this loop; the MCP
    /// backend overrides it with its native `fill_form`.
    async fn fill_form(
        &self,
        tab_id: &str,
        fields: &[(ActionTarget, String)],
    ) -> Result<usize, BrowserError> {
        let mut filled = 0;
        for (target, value) in fields {
            self.fill(tab_id, target.clone(), value).await?;
            filled += 1;
        }
        Ok(filled)
    }
}

/// Error returned by the trait's one-sided defaults.
///
/// Every default left in this trait that is not a shared implementation
/// (`wait_for`, `fill_form`) is served by some backends and not others. The
/// message therefore names the DRIVER that serves it, spelled the way a config
/// file spells it, rather than describing one particular backend's internals:
/// the old text said "the Chrome DevTools MCP server exposes no {op}
/// primitive", which was true when there were two drivers and became a lie
/// about the third the day it existed — and it was a lie in the expensive
/// direction, telling a `cdp` profile's reader to go configure Chrome MCP.
pub(crate) fn unsupported_by_driver(op: &'static str, supported_by: BrowserDriver) -> BrowserError {
    BrowserError::ActionFailed(format!(
        "{op} is not available on this profile's driver — a profile with \
         driver = \"{}\" serves it. Change the profile's driver, or use one that already \
         has it (the default profile is a good starting point).",
        supported_by.as_wire()
    ))
}

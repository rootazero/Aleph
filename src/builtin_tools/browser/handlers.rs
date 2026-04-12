//! Action handlers for the browser tool (private implementation).

use super::types::{BrowserArgs, BrowserOutput};
use crate::browser::{ActionTarget, BrowserConfig, BrowserRuntime, ScreenshotOpts};
use crate::error::Result;

/// Extract an [`ActionTarget`] from the tool arguments.
///
/// Priority: `ref_id` > `selector`. Returns an error string if neither is set.
pub(super) fn resolve_action_target(
    args: &BrowserArgs,
) -> std::result::Result<ActionTarget, String> {
    if let Some(ref ref_id) = args.ref_id {
        Ok(ActionTarget::Ref {
            ref_id: ref_id.clone(),
        })
    } else if let Some(ref _css) = args.selector {
        // CSS selectors are no longer supported — callers should use ref_id from a snapshot.
        Err("CSS selector targeting is no longer supported. Use 'ref_id' from a browser_snapshot.".to_string())
    } else {
        Err(
            "This action requires a target element. Provide 'ref_id' (from a snapshot) \
             or 'selector' (CSS selector)."
                .to_string(),
        )
    }
}

impl super::BrowserTool {
    /// Require the browser to be running and return a guard. The caller must
    /// hold the guard for the duration of the operation.
    ///
    /// Returns `Err(BrowserOutput)` with a user-friendly message when no
    /// browser is running.
    async fn require_running(
        &self,
    ) -> std::result::Result<tokio::sync::MutexGuard<'_, Option<BrowserRuntime>>, BrowserOutput>
    {
        let guard = self.runtime.lock().await;
        if guard.is_none() {
            return Err(BrowserOutput::err(
                "Browser is not running. Use action 'start' to launch a browser first.",
            ));
        }
        Ok(guard)
    }

    /// Extract tab_id from args or return an error output.
    fn require_tab_id(args: &BrowserArgs) -> std::result::Result<&str, BrowserOutput> {
        args.tab_id.as_deref().ok_or_else(|| {
            BrowserOutput::err("This action requires 'tab_id'. Use 'list_tabs' to see open tabs.")
        })
    }

    // ── Start / Stop ────────────────────────────────────────────────────

    pub(super) async fn handle_start(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let mut guard = self.runtime.lock().await;
        if guard.is_some() {
            return Ok(BrowserOutput::ok(
                "Browser is already running. Use 'stop' first if you need to restart.",
            ));
        }

        let config = BrowserConfig {
            headless: args.headless.unwrap_or(false),
            ..BrowserConfig::default()
        };

        match BrowserRuntime::start(config).await {
            Ok(rt) => {
                *guard = Some(rt);
                Ok(BrowserOutput::ok("Browser started successfully."))
            }
            Err(e) => Ok(BrowserOutput::err(format!("Failed to start browser: {e}"))),
        }
    }

    pub(super) async fn handle_stop(&self) -> Result<BrowserOutput> {
        let mut guard = self.runtime.lock().await;
        match guard.take() {
            Some(rt) => match rt.stop().await {
                Ok(()) => Ok(BrowserOutput::ok("Browser stopped.")),
                Err(e) => Ok(BrowserOutput::err(format!("Error stopping browser: {e}"))),
            },
            None => Ok(BrowserOutput::ok("Browser was not running.")),
        }
    }

    // ── Tab management ──────────────────────────────────────────────────

    pub(super) async fn handle_open_tab(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let url = match args.url.as_deref() {
            Some(u) => u,
            None => return Ok(BrowserOutput::err("'open_tab' requires a 'url' parameter.")),
        };

        let mut guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_mut().unwrap();
        match rt.open_tab(url).await {
            Ok(tab_id) => Ok(BrowserOutput::ok_data_msg(
                format!("Tab opened: {tab_id}"),
                serde_json::json!({ "tab_id": tab_id }),
            )),
            Err(e) => Ok(BrowserOutput::err(format!("Failed to open tab: {e}"))),
        }
    }

    pub(super) async fn handle_close_tab(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };

        let mut guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_mut().unwrap();
        match rt.close_tab(tab_id).await {
            Ok(()) => Ok(BrowserOutput::ok(format!("Tab {tab_id} closed."))),
            Err(e) => Ok(BrowserOutput::err(format!("Failed to close tab: {e}"))),
        }
    }

    pub(super) async fn handle_list_tabs(&self) -> Result<BrowserOutput> {
        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        let tabs = rt.list_tabs().await;
        Ok(BrowserOutput::ok_data(
            serde_json::to_value(&tabs).unwrap_or_default(),
        ))
    }

    // ── Navigation ──────────────────────────────────────────────────────

    pub(super) async fn handle_navigate(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let url = match args.url.as_deref() {
            Some(u) => u,
            None => return Ok(BrowserOutput::err("'navigate' requires a 'url' parameter.")),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.navigate(tab_id, url).await {
            Ok(()) => Ok(BrowserOutput::ok(format!("Navigated to {url}"))),
            Err(e) => Ok(BrowserOutput::err(format!("Navigation failed: {e}"))),
        }
    }

    // ── Element interactions ────────────────────────────────────────────

    pub(super) async fn handle_click(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let target = match resolve_action_target(args) {
            Ok(t) => t,
            Err(msg) => return Ok(BrowserOutput::err(msg)),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.click(tab_id, target).await {
            Ok(()) => Ok(BrowserOutput::ok("Clicked.")),
            Err(e) => Ok(BrowserOutput::err(format!("Click failed: {e}"))),
        }
    }

    pub(super) async fn handle_type(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let target = match resolve_action_target(args) {
            Ok(t) => t,
            Err(msg) => return Ok(BrowserOutput::err(msg)),
        };
        let text = match args.text.as_deref() {
            Some(t) => t,
            None => return Ok(BrowserOutput::err("'type' requires a 'text' parameter.")),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.type_text(tab_id, target, text).await {
            Ok(()) => Ok(BrowserOutput::ok("Text typed.")),
            Err(e) => Ok(BrowserOutput::err(format!("Type failed: {e}"))),
        }
    }

    pub(super) async fn handle_fill(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let target = match resolve_action_target(args) {
            Ok(t) => t,
            Err(msg) => return Ok(BrowserOutput::err(msg)),
        };
        let text = match args.text.as_deref() {
            Some(t) => t,
            None => return Ok(BrowserOutput::err("'fill' requires a 'text' parameter.")),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.fill(tab_id, target, text).await {
            Ok(()) => Ok(BrowserOutput::ok("Value filled.")),
            Err(e) => Ok(BrowserOutput::err(format!("Fill failed: {e}"))),
        }
    }

    pub(super) async fn handle_scroll(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let target =
            resolve_action_target(args).unwrap_or(ActionTarget::Coordinates { x: 0.0, y: 0.0 });
        let direction = match args.direction.clone() {
            Some(d) => d,
            None => {
                return Ok(BrowserOutput::err(
                    "'scroll' requires a 'direction' parameter (up/down/left/right).",
                ))
            }
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.scroll(tab_id, target, direction).await {
            Ok(()) => Ok(BrowserOutput::ok("Scrolled.")),
            Err(e) => Ok(BrowserOutput::err(format!("Scroll failed: {e}"))),
        }
    }

    pub(super) async fn handle_hover(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let target = match resolve_action_target(args) {
            Ok(t) => t,
            Err(msg) => return Ok(BrowserOutput::err(msg)),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.hover(tab_id, target).await {
            Ok(()) => Ok(BrowserOutput::ok("Hovered.")),
            Err(e) => Ok(BrowserOutput::err(format!("Hover failed: {e}"))),
        }
    }

    // ── Observation ─────────────────────────────────────────────────────

    pub(super) async fn handle_screenshot(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let opts = ScreenshotOpts {
            full_page: args.full_page.unwrap_or(false),
            ..ScreenshotOpts::default()
        };

        let rt = guard.as_ref().unwrap();
        match rt.screenshot(tab_id, opts).await {
            Ok(result) => Ok(BrowserOutput::ok_data(
                serde_json::to_value(&result).unwrap_or_default(),
            )),
            Err(e) => Ok(BrowserOutput::err(format!("Screenshot failed: {e}"))),
        }
    }

    pub(super) async fn handle_snapshot(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.snapshot(tab_id).await {
            Ok(snap) => Ok(BrowserOutput::ok_data(
                serde_json::to_value(&snap).unwrap_or_default(),
            )),
            Err(e) => Ok(BrowserOutput::err(format!("Snapshot failed: {e}"))),
        }
    }

    // ── JavaScript ──────────────────────────────────────────────────────

    pub(super) async fn handle_evaluate(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let js = match args.js.as_deref() {
            Some(j) => j,
            None => return Ok(BrowserOutput::err("'evaluate' requires a 'js' parameter.")),
        };

        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        match rt.evaluate(tab_id, js).await {
            Ok(value) => Ok(BrowserOutput::ok_data(value)),
            Err(e) => Ok(BrowserOutput::err(format!("JS evaluation failed: {e}"))),
        }
    }
}

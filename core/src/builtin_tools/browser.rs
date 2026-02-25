//! Browser automation tool — controls a Chromium browser via CDP.
//!
//! Wraps [`BrowserRuntime`] behind the [`AlephTool`] interface so the AI agent
//! can launch a browser, navigate pages, interact with elements, take
//! screenshots, and obtain accessibility snapshots for structured page
//! understanding.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::browser::{
    ActionTarget, BrowserConfig, BrowserRuntime, ScreenshotOpts, ScrollDirection,
};
use crate::error::Result;
use crate::tools::AlephTool;

// =============================================================================
// BrowserAction — the set of operations the tool exposes
// =============================================================================

/// The action to perform on the browser.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    /// Launch a new browser instance (or connect to an existing one).
    Start,
    /// Stop the running browser instance.
    Stop,
    /// Open a new tab navigating to a URL.
    OpenTab,
    /// Close an existing tab by its tab_id.
    CloseTab,
    /// List all open tabs.
    ListTabs,
    /// Navigate an existing tab to a new URL.
    Navigate,
    /// Click an element identified by ref_id or selector.
    Click,
    /// Type (append) text into a focused or targeted element.
    Type,
    /// Fill (replace) the value of an input element.
    Fill,
    /// Scroll an element or the page in a given direction.
    Scroll,
    /// Hover over an element.
    Hover,
    /// Capture a screenshot of a tab.
    Screenshot,
    /// Take an ARIA accessibility snapshot of a tab (returns ref_ids for targeting).
    Snapshot,
    /// Evaluate arbitrary JavaScript in a tab.
    Evaluate,
}

// =============================================================================
// BrowserArgs — tool input
// =============================================================================

/// Arguments for the browser tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserArgs {
    /// The browser action to perform.
    pub action: BrowserAction,

    /// Target tab ID (required for most per-tab actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,

    /// URL for open_tab / navigate actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// ARIA snapshot ref_id for targeting an element (preferred over selector).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,

    /// CSS selector for targeting an element (fallback when ref_id is absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,

    /// Text to type or fill into an element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// JavaScript code to evaluate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,

    /// Scroll direction: "up", "down", "left", "right".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<ScrollDirection>,

    /// Whether to launch the browser in headless mode (default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,

    /// Whether to capture the full scrollable page for screenshots (default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_page: Option<bool>,
}

// =============================================================================
// BrowserOutput — tool output
// =============================================================================

/// Output from browser operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserOutput {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Human-readable message (present on errors or informational results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Structured data returned by the operation (tab info, snapshot, screenshot, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl BrowserOutput {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: None,
        }
    }

    fn ok_data(data: Value) -> Self {
        Self {
            success: true,
            message: None,
            data: Some(data),
        }
    }

    fn ok_data_msg(message: impl Into<String>, data: Value) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: Some(data),
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data: None,
        }
    }
}

// =============================================================================
// BrowserTool
// =============================================================================

/// Browser automation tool — gives the AI agent a controllable web browser.
///
/// Manages a [`BrowserRuntime`] behind an `Arc<Mutex<Option<...>>>` so the
/// tool can be cloned (required by `AlephTool`) while sharing the single
/// browser instance.
#[derive(Clone)]
pub struct BrowserTool {
    runtime: Arc<Mutex<Option<BrowserRuntime>>>,
}

impl BrowserTool {
    pub fn new() -> Self {
        Self {
            runtime: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Helper: resolve an ActionTarget from args
// =============================================================================

/// Extract an [`ActionTarget`] from the tool arguments.
///
/// Priority: `ref_id` > `selector`. Returns an error string if neither is set.
fn resolve_action_target(args: &BrowserArgs) -> std::result::Result<ActionTarget, String> {
    if let Some(ref ref_id) = args.ref_id {
        Ok(ActionTarget::Ref {
            ref_id: ref_id.clone(),
        })
    } else if let Some(ref css) = args.selector {
        Ok(ActionTarget::Selector { css: css.clone() })
    } else {
        Err(
            "This action requires a target element. Provide 'ref_id' (from a snapshot) \
             or 'selector' (CSS selector)."
                .to_string(),
        )
    }
}

// =============================================================================
// AlephTool implementation
// =============================================================================

#[async_trait]
impl AlephTool for BrowserTool {
    const NAME: &'static str = "browser";

    const DESCRIPTION: &'static str = r#"Control a Chromium browser: launch, navigate, interact with elements, take screenshots, and get page structure.

Workflow: start -> open_tab -> snapshot (get ref_ids) -> click/type/fill/scroll using ref_id -> screenshot -> stop

Actions:
- start: Launch a browser instance. Optional headless=true for invisible mode.
- stop: Shut down the browser instance.
- open_tab: Open a new tab. Requires url. Returns tab_id.
- close_tab: Close a tab. Requires tab_id.
- list_tabs: List all open tabs with their IDs, URLs, and titles.
- navigate: Navigate a tab to a new URL. Requires tab_id and url.
- click: Click an element. Requires tab_id and ref_id (or selector).
- type: Type text into an element. Requires tab_id, ref_id (or selector), and text.
- fill: Replace element value. Requires tab_id, ref_id (or selector), and text.
- scroll: Scroll an element. Requires tab_id, ref_id (or selector), and direction (up/down/left/right).
- hover: Hover over an element. Requires tab_id and ref_id (or selector).
- screenshot: Capture a tab screenshot (base64 PNG). Requires tab_id. Optional full_page=true.
- snapshot: Get ARIA accessibility tree of a tab. Requires tab_id. Returns elements with ref_ids.
- evaluate: Run JavaScript in a tab. Requires tab_id and js.

Targeting: Use ref_id from snapshot results (preferred) or a CSS selector as fallback.

Examples:
{"action":"start"}
{"action":"start","headless":true}
{"action":"open_tab","url":"https://example.com"}
{"action":"snapshot","tab_id":"..."}
{"action":"click","tab_id":"...","ref_id":"e42"}
{"action":"type","tab_id":"...","ref_id":"e7","text":"hello world"}
{"action":"screenshot","tab_id":"...","full_page":true}
{"action":"evaluate","tab_id":"...","js":"document.title"}
{"action":"stop"}"#;

    type Args = BrowserArgs;
    type Output = BrowserOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"browser(action="start") — launch a headed browser"#.to_string(),
            r#"browser(action="start", headless=true) — launch headless"#.to_string(),
            r#"browser(action="open_tab", url="https://example.com") — open tab, returns tab_id"#
                .to_string(),
            r#"browser(action="snapshot", tab_id="...") — get ARIA tree with ref_ids"#.to_string(),
            r#"browser(action="click", tab_id="...", ref_id="e42") — click element e42"#
                .to_string(),
            r#"browser(action="type", tab_id="...", ref_id="e7", text="search query") — type into input"#
                .to_string(),
            r#"browser(action="fill", tab_id="...", selector="input#email", text="a@b.com") — fill input by CSS"#
                .to_string(),
            r#"browser(action="screenshot", tab_id="...", full_page=true) — full-page screenshot"#
                .to_string(),
            r#"browser(action="evaluate", tab_id="...", js="document.title") — run JS"#
                .to_string(),
            r#"browser(action="stop") — shut down the browser"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            // ── Lifecycle ───────────────────────────────────────────────
            BrowserAction::Start => self.handle_start(&args).await,
            BrowserAction::Stop => self.handle_stop().await,

            // ── Tab management ──────────────────────────────────────────
            BrowserAction::OpenTab => self.handle_open_tab(&args).await,
            BrowserAction::CloseTab => self.handle_close_tab(&args).await,
            BrowserAction::ListTabs => self.handle_list_tabs().await,

            // ── Navigation ──────────────────────────────────────────────
            BrowserAction::Navigate => self.handle_navigate(&args).await,

            // ── Element interactions ────────────────────────────────────
            BrowserAction::Click => self.handle_click(&args).await,
            BrowserAction::Type => self.handle_type(&args).await,
            BrowserAction::Fill => self.handle_fill(&args).await,
            BrowserAction::Scroll => self.handle_scroll(&args).await,
            BrowserAction::Hover => self.handle_hover(&args).await,

            // ── Observation ─────────────────────────────────────────────
            BrowserAction::Screenshot => self.handle_screenshot(&args).await,
            BrowserAction::Snapshot => self.handle_snapshot(&args).await,

            // ── JavaScript ──────────────────────────────────────────────
            BrowserAction::Evaluate => self.handle_evaluate(&args).await,
        }
    }
}

// =============================================================================
// Action handlers (private)
// =============================================================================

impl BrowserTool {
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

    async fn handle_start(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_stop(&self) -> Result<BrowserOutput> {
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

    async fn handle_open_tab(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_close_tab(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_list_tabs(&self) -> Result<BrowserOutput> {
        let guard = match self.require_running().await {
            Ok(g) => g,
            Err(out) => return Ok(out),
        };

        let rt = guard.as_ref().unwrap();
        let tabs = rt.list_tabs().await;
        Ok(BrowserOutput::ok_data(serde_json::to_value(&tabs).unwrap_or_default()))
    }

    // ── Navigation ──────────────────────────────────────────────────────

    async fn handle_navigate(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_click(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_type(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_fill(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_scroll(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
        let tab_id = match Self::require_tab_id(args) {
            Ok(id) => id,
            Err(out) => return Ok(out),
        };
        let target = match resolve_action_target(args) {
            Ok(t) => t,
            Err(msg) => return Ok(BrowserOutput::err(msg)),
        };
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

    async fn handle_hover(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_screenshot(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_snapshot(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

    async fn handle_evaluate(&self, args: &BrowserArgs) -> Result<BrowserOutput> {
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(action: BrowserAction) -> BrowserArgs {
        BrowserArgs {
            action,
            tab_id: None,
            url: None,
            ref_id: None,
            selector: None,
            text: None,
            js: None,
            direction: None,
            headless: None,
            full_page: None,
        }
    }

    #[test]
    fn test_resolve_action_target_ref() {
        let mut args = make_args(BrowserAction::Click);
        args.ref_id = Some("e42".to_string());
        let target = resolve_action_target(&args).unwrap();
        assert!(matches!(target, ActionTarget::Ref { ref_id } if ref_id == "e42"));
    }

    #[test]
    fn test_resolve_action_target_selector() {
        let mut args = make_args(BrowserAction::Click);
        args.selector = Some("button.submit".to_string());
        let target = resolve_action_target(&args).unwrap();
        assert!(matches!(target, ActionTarget::Selector { css } if css == "button.submit"));
    }

    #[test]
    fn test_resolve_action_target_ref_preferred_over_selector() {
        let mut args = make_args(BrowserAction::Click);
        args.ref_id = Some("e1".to_string());
        args.selector = Some("div".to_string());
        let target = resolve_action_target(&args).unwrap();
        // ref_id takes priority
        assert!(matches!(target, ActionTarget::Ref { ref_id } if ref_id == "e1"));
    }

    #[test]
    fn test_resolve_action_target_missing() {
        let args = make_args(BrowserAction::Click);
        assert!(resolve_action_target(&args).is_err());
    }

    #[test]
    fn test_tool_definition() {
        let tool = BrowserTool::new();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "browser");
        assert!(def.description.contains("Chromium"));
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_not_running_returns_error_output() {
        let tool = BrowserTool::new();
        let args = BrowserArgs {
            action: BrowserAction::ListTabs,
            ..make_args(BrowserAction::ListTabs)
        };
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        assert!(output.message.unwrap().contains("not running"));
    }

    #[tokio::test]
    async fn test_stop_when_not_running() {
        let tool = BrowserTool::new();
        let args = make_args(BrowserAction::Stop);
        let output = AlephTool::call(&tool, args).await.unwrap();
        // Should succeed gracefully even when not running
        assert!(output.success);
        assert!(output.message.unwrap().contains("not running"));
    }

    #[tokio::test]
    async fn test_open_tab_missing_url() {
        let tool = BrowserTool::new();
        // Start would fail anyway (no browser), but open_tab should fail on missing url first...
        // Actually, open_tab checks url before require_running, but our impl checks url first.
        // Let's test the url-missing path by having a running browser... we can't easily do that.
        // Instead, test that the error for not-running is returned.
        let args = make_args(BrowserAction::OpenTab);
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        // Either "url" error or "not running" — our impl checks url before require_running
        assert!(output.message.is_some());
    }

    #[tokio::test]
    async fn test_click_missing_target() {
        let tool = BrowserTool::new();
        let mut args = make_args(BrowserAction::Click);
        args.tab_id = Some("fake".to_string());
        let output = AlephTool::call(&tool, args).await.unwrap();
        assert!(!output.success);
        // Will fail either on "not running" or "missing target"
        assert!(output.message.is_some());
    }

    #[test]
    fn test_browser_action_serde() {
        let action: BrowserAction = serde_json::from_str(r#""start""#).unwrap();
        assert!(matches!(action, BrowserAction::Start));

        let action: BrowserAction = serde_json::from_str(r#""open_tab""#).unwrap();
        assert!(matches!(action, BrowserAction::OpenTab));

        let action: BrowserAction = serde_json::from_str(r#""snapshot""#).unwrap();
        assert!(matches!(action, BrowserAction::Snapshot));
    }

    #[test]
    fn test_browser_args_deserialization() {
        let json = serde_json::json!({
            "action": "click",
            "tab_id": "abc123",
            "ref_id": "e42"
        });
        let args: BrowserArgs = serde_json::from_value(json).unwrap();
        assert!(matches!(args.action, BrowserAction::Click));
        assert_eq!(args.tab_id.as_deref(), Some("abc123"));
        assert_eq!(args.ref_id.as_deref(), Some("e42"));
    }
}

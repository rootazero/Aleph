//! Browser automation tool — controls a Chromium browser via CDP.
//!
//! Wraps [`BrowserRuntime`] behind the [`AlephTool`] interface so the AI agent
//! can launch a browser, navigate pages, interact with elements, take
//! screenshots, and obtain accessibility snapshots for structured page
//! understanding.

mod handlers;
mod types;

#[cfg(test)]
mod tests;

pub use types::{BrowserAction, BrowserArgs, BrowserOutput};

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::approval::{ActionRequest, ActionType, ApprovalDecision, ApprovalPolicy};
use crate::browser::BrowserRuntime;
use crate::error::Result;
use crate::tools::AlephTool;

/// Browser automation tool — gives the AI agent a controllable web browser.
///
/// Manages a [`BrowserRuntime`] behind an `Arc<Mutex<Option<...>>>` so the
/// tool can be cloned (required by `AlephTool`) while sharing the single
/// browser instance.
#[derive(Clone)]
pub struct BrowserTool {
    runtime: Arc<Mutex<Option<BrowserRuntime>>>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserTool {
    pub fn new() -> Self {
        Self {
            runtime: Arc::new(Mutex::new(None)),
            approval_policy: None,
        }
    }

    /// Attach an approval policy to gate sensitive actions.
    ///
    /// When a policy is set, mutating actions (OpenTab, Navigate, Click, Type,
    /// Fill, Evaluate) are checked before execution. Read-only actions
    /// (Screenshot, Snapshot, Scroll, Hover, ListTabs, etc.) are always allowed.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }

    /// Check the approval policy for a sensitive action.
    ///
    /// Returns `None` if the action is allowed (or no policy is configured),
    /// or `Some(BrowserOutput)` if the action is denied or requires user
    /// confirmation.
    async fn check_approval(&self, action_type: ActionType, target: &str) -> Option<BrowserOutput> {
        let policy = self.approval_policy.as_ref()?;

        let request = ActionRequest {
            action_type,
            target: target.to_string(),
            agent_id: String::new(), // TODO: plumb agent_id from agent loop call context
            context: String::new(),  // TODO: populate with action description for audit
            timestamp: chrono::Utc::now(),
        };

        let decision = policy.check(&request).await;

        match decision {
            ApprovalDecision::Allow => {
                policy.record(&request, &decision).await;
                None
            }
            ApprovalDecision::Deny { ref reason } => {
                policy.record(&request, &decision).await;
                Some(BrowserOutput::err(format!(
                    "Action denied by approval policy: {reason}"
                )))
            }
            ApprovalDecision::Ask { ref prompt } => {
                // Don't record yet — record() should be called after user responds
                Some(BrowserOutput {
                    success: false,
                    message: Some(format!("Approval required: {prompt}")),
                    data: Some(serde_json::json!({
                        "approval_required": true,
                        "prompt": prompt,
                    })),
                })
            }
        }
    }
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

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
            // ── Lifecycle (no approval needed) ──────────────────────────
            BrowserAction::Start => self.handle_start(&args).await,
            BrowserAction::Stop => self.handle_stop().await,

            // ── Tab management ───────────────────────────────────────────
            BrowserAction::OpenTab => {
                if let Some(out) = self
                    .check_approval(
                        ActionType::BrowserNavigate,
                        args.url.as_deref().unwrap_or(""),
                    )
                    .await
                {
                    return Ok(out);
                }
                self.handle_open_tab(&args).await
            }
            BrowserAction::CloseTab => self.handle_close_tab(&args).await,
            BrowserAction::ListTabs => self.handle_list_tabs().await,

            // ── Navigation (approval required) ──────────────────────────
            BrowserAction::Navigate => {
                if let Some(out) = self
                    .check_approval(
                        ActionType::BrowserNavigate,
                        args.url.as_deref().unwrap_or(""),
                    )
                    .await
                {
                    return Ok(out);
                }
                self.handle_navigate(&args).await
            }

            // ── Element interactions (approval for mutating actions) ────
            BrowserAction::Click => {
                let target_str = args
                    .ref_id
                    .as_deref()
                    .or(args.selector.as_deref())
                    .unwrap_or("");
                if let Some(out) = self
                    .check_approval(ActionType::BrowserClick, target_str)
                    .await
                {
                    return Ok(out);
                }
                self.handle_click(&args).await
            }
            BrowserAction::Type => {
                if let Some(out) = self
                    .check_approval(ActionType::BrowserType, args.text.as_deref().unwrap_or(""))
                    .await
                {
                    return Ok(out);
                }
                self.handle_type(&args).await
            }
            BrowserAction::Fill => {
                if let Some(out) = self
                    .check_approval(ActionType::BrowserFill, args.text.as_deref().unwrap_or(""))
                    .await
                {
                    return Ok(out);
                }
                self.handle_fill(&args).await
            }
            BrowserAction::Scroll => self.handle_scroll(&args).await,
            BrowserAction::Hover => self.handle_hover(&args).await,

            // ── Observation (no approval needed) ────────────────────────
            BrowserAction::Screenshot => self.handle_screenshot(&args).await,
            BrowserAction::Snapshot => self.handle_snapshot(&args).await,

            // ── JavaScript (approval required) ──────────────────────────
            BrowserAction::Evaluate => {
                if let Some(out) = self
                    .check_approval(
                        ActionType::BrowserEvaluate,
                        args.js.as_deref().unwrap_or(""),
                    )
                    .await
                {
                    return Ok(out);
                }
                self.handle_evaluate(&args).await
            }
        }
    }
}

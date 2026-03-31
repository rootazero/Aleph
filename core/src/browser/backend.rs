//! BrowserBackend trait — unified contract for browser driver implementations.

use async_trait::async_trait;

use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, ConsoleMessage, ScreenshotOpts, ScreenshotResult, ScrollDirection,
    TabId, TabInfo,
};

/// Unified interface for browser operations, implemented by both
/// `ManagedBackend` (chromiumoxide) and `ChromeMcpBackend` (Chrome DevTools MCP).
#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;
    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError>;
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
    ) -> Result<ScreenshotResult, BrowserError>;
    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError>;
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError>;
    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError>;

    /// Press a keyboard key.
    async fn press_key(&self, _tab_id: &str, _key: &str) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed(
            "press_key not supported by this backend".into(),
        ))
    }

    /// Wait for specific text to appear on the page.
    async fn wait_for_text(
        &self,
        _tab_id: &str,
        _text: &str,
        _timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        Err(BrowserError::ActionFailed(
            "wait_for_text not supported by this backend".into(),
        ))
    }

    /// Read browser console messages.
    async fn console_messages(&self, _tab_id: &str) -> Result<Vec<ConsoleMessage>, BrowserError> {
        Err(BrowserError::ActionFailed(
            "console_messages not supported by this backend".into(),
        ))
    }

    /// Fill multiple form fields in a batch (default: sequential fills).
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

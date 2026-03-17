//! BrowserBackend trait — unified contract for browser driver implementations.

use async_trait::async_trait;

use super::error::BrowserError;
use super::types::{
    ActionTarget, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection, TabId, TabInfo,
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
    async fn type_text(&self, tab_id: &str, target: ActionTarget, text: &str) -> Result<(), BrowserError>;
    async fn fill(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError>;
    async fn scroll(&self, tab_id: &str, target: ActionTarget, direction: ScrollDirection) -> Result<(), BrowserError>;
    async fn screenshot(&self, tab_id: &str, opts: ScreenshotOpts) -> Result<ScreenshotResult, BrowserError>;
    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError>;
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError>;
    async fn select(&self, tab_id: &str, target: ActionTarget, value: &str) -> Result<(), BrowserError>;
}

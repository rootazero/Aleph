//! ManagedBackend — BrowserBackend implementation wrapping BrowserRuntime (chromiumoxide).

use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::runtime::BrowserRuntime;
use super::types::{
    ActionTarget, AriaSnapshot, ScreenshotOpts, ScreenshotResult, ScrollDirection, TabId, TabInfo,
};

/// BrowserBackend backed by a managed chromiumoxide BrowserRuntime.
pub struct ManagedBackend {
    runtime: Arc<Mutex<BrowserRuntime>>,
}

impl ManagedBackend {
    pub fn new(runtime: Arc<Mutex<BrowserRuntime>>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl BrowserBackend for ManagedBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.runtime.lock().await.open_tab(url).await
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.runtime.lock().await.close_tab(tab_id).await
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, BrowserError> {
        Ok(self.runtime.lock().await.list_tabs().await)
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.runtime.lock().await.navigate(tab_id, url).await
    }

    async fn click(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.runtime.lock().await.click(tab_id, target).await
    }

    async fn type_text(
        &self,
        tab_id: &str,
        target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        self.runtime
            .lock()
            .await
            .type_text(tab_id, target, text)
            .await
    }

    async fn fill(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.runtime.lock().await.fill(tab_id, target, value).await
    }

    async fn hover(&self, tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.runtime.lock().await.hover(tab_id, target).await
    }

    async fn scroll(
        &self,
        tab_id: &str,
        target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        self.runtime
            .lock()
            .await
            .scroll(tab_id, target, direction)
            .await
    }

    async fn screenshot(
        &self,
        tab_id: &str,
        opts: ScreenshotOpts,
    ) -> Result<ScreenshotResult, BrowserError> {
        self.runtime.lock().await.screenshot(tab_id, opts).await
    }

    async fn snapshot(&self, tab_id: &str) -> Result<AriaSnapshot, BrowserError> {
        self.runtime.lock().await.snapshot(tab_id).await
    }

    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<serde_json::Value, BrowserError> {
        self.runtime.lock().await.evaluate(tab_id, js).await
    }

    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        // BrowserRuntime doesn't have a select method yet.
        // Implement via JS evaluation as a reasonable fallback.
        let escaped_value = serde_json::to_string(value).map_err(|e| {
            BrowserError::ActionFailed(format!("Failed to escape select value: {e}"))
        })?;
        let js = match &target {
            ActionTarget::Ref { ref_id } => {
                let escaped_ref = serde_json::to_string(ref_id).map_err(|e| {
                    BrowserError::ActionFailed(format!("Failed to escape ref_id: {e}"))
                })?;
                format!(
                    r#"(() => {{ const el = document.querySelector('[data-ref=' + {escaped_ref} + ']'); if (el) {{ el.value = {escaped_value}; el.dispatchEvent(new Event('change')); return true; }} return false; }})()"#
                )
            }
            ActionTarget::Selector { css } => {
                let escaped_css = serde_json::to_string(css).map_err(|e| {
                    BrowserError::ActionFailed(format!("Failed to escape CSS selector: {e}"))
                })?;
                format!(
                    r#"(() => {{ const el = document.querySelector({escaped_css}); if (el) {{ el.value = {escaped_value}; el.dispatchEvent(new Event('change')); return true; }} return false; }})()"#
                )
            }
            ActionTarget::Coordinates { .. } => {
                return Err(BrowserError::ActionFailed(
                    "Cannot select by coordinates".to_string(),
                ));
            }
        };
        self.runtime.lock().await.evaluate(tab_id, &js).await?;
        Ok(())
    }
}

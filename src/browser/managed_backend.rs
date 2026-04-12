//! ManagedBackend — BrowserBackend implementation wrapping BrowserRuntime (chromiumoxide).
//! NOTE: This backend is pending deletion in Task 10. Methods are stubbed to satisfy the
//! new BrowserBackend trait; they return errors rather than real implementations.

use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::runtime::BrowserRuntime;
use super::types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
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

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        Err(BrowserError::ActionFailed("pending migration".into()))
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
        _tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        Err(BrowserError::ActionFailed("pending migration".into()))
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        Err(BrowserError::ActionFailed("pending migration".into()))
    }

    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError> {
        let val = self.runtime.lock().await.evaluate(tab_id, js).await?;
        Ok(val.to_string())
    }

    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
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

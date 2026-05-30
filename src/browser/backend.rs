//! BrowserBackend trait — text-first unified contract for browser drivers.

use std::path::Path;

use async_trait::async_trait;

use super::error::BrowserError;
use super::types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
};

#[async_trait]
pub trait BrowserBackend: Send + Sync {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError>;
    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError>;
    async fn list_tabs(&self) -> Result<String, BrowserError>;
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
    async fn evaluate(&self, tab_id: &str, js: &str) -> Result<String, BrowserError>;
    async fn select(
        &self,
        tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError>;

    async fn press_key(&self, _tab_id: &str, _key: &str) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("press_key not supported".into()))
    }

    async fn wait_for_text(
        &self,
        _tab_id: &str,
        _text: &str,
        _timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        Err(BrowserError::ActionFailed(
            "wait_for_text not supported".into(),
        ))
    }

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Err(BrowserError::ActionFailed(
            "console_messages not supported".into(),
        ))
    }

    async fn network_log(&self, _tab_id: &str) -> Result<String, BrowserError> {
        Err(BrowserError::ActionFailed(
            "network_log not supported".into(),
        ))
    }

    /// Print-to-PDF — writes PDF to `output_path`. Default impl returns Unsupported.
    async fn pdf(&self, _tab_id: &str, _output_path: &Path) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("pdf not supported".into()))
    }

    /// Bring the given tab to the foreground / make it the active page.
    /// Default impl returns Unsupported — only backends with a real notion of
    /// active page (e.g. Chrome DevTools MCP) override.
    async fn switch_tab(&self, _tab_id: &str) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed(
            "switch_tab not supported".into(),
        ))
    }

    /// Respond to a pending native dialog (alert / confirm / prompt / beforeunload).
    /// `action` is "accept" or "dismiss"; `prompt_text` is the text to fill into a
    /// prompt before accepting (ignored for non-prompt dialogs).
    /// Default impl returns Unsupported.
    async fn handle_dialog(
        &self,
        _tab_id: &str,
        _action: &str,
        _prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed(
            "handle_dialog not supported".into(),
        ))
    }

    /// Drag-and-drop from one snapshot element onto another.
    /// Both targets must be snapshot refs (coordinates unsupported).
    /// Default impl returns Unsupported.
    async fn drag(
        &self,
        _tab_id: &str,
        _from: ActionTarget,
        _to: ActionTarget,
    ) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("drag not supported".into()))
    }

    /// Attach one or more local files to a file input.
    /// `target` identifies the `<input type=file>` element (required by the
    /// existing-session/MCP backend; ignored by the managed CLI backend, which
    /// targets the page's file chooser directly). Default impl returns Unsupported.
    async fn upload(
        &self,
        _tab_id: &str,
        _target: Option<ActionTarget>,
        _paths: &[String],
    ) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("upload not supported".into()))
    }

    /// Resize the browser viewport / window to `width` × `height` CSS pixels.
    /// Default impl returns Unsupported.
    async fn resize(&self, _tab_id: &str, _width: u32, _height: u32) -> Result<(), BrowserError> {
        Err(BrowserError::ActionFailed("resize not supported".into()))
    }

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

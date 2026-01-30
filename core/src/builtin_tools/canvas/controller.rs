//! Canvas Controller
//!
//! Manages canvas state and dispatches commands to the rendering backend.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::a2ui::{parse_jsonl, A2uiMessage, SurfaceManager};
use super::types::{
    CanvasAction, CanvasState, CanvasToolArgs, CanvasToolOutput, SnapshotFormat, WindowPlacement,
};

/// Canvas controller for managing canvas state and operations
pub struct CanvasController {
    /// Current canvas state
    state: Arc<RwLock<CanvasState>>,
    /// A2UI surface manager
    surfaces: Arc<RwLock<SurfaceManager>>,
    /// Backend for rendering operations
    backend: Arc<dyn CanvasBackend>,
}

impl CanvasController {
    /// Create a new canvas controller with a backend
    pub fn new(backend: Arc<dyn CanvasBackend>) -> Self {
        Self {
            state: Arc::new(RwLock::new(CanvasState::default())),
            surfaces: Arc::new(RwLock::new(SurfaceManager::new())),
            backend,
        }
    }

    /// Create a controller with a no-op backend (for testing)
    pub fn new_noop() -> Self {
        Self::new(Arc::new(NoOpBackend))
    }

    /// Get current canvas state
    pub async fn get_state(&self) -> CanvasState {
        self.state.read().await.clone()
    }

    /// Execute a canvas action
    pub async fn execute(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        debug!(action = %args.action, "Executing canvas action");

        match args.action {
            CanvasAction::Present => self.handle_present(args).await,
            CanvasAction::Hide => self.handle_hide().await,
            CanvasAction::Navigate => self.handle_navigate(args).await,
            CanvasAction::Eval => self.handle_eval(args).await,
            CanvasAction::Snapshot => self.handle_snapshot(args).await,
            CanvasAction::A2uiPush => self.handle_a2ui_push(args).await,
            CanvasAction::A2uiReset => self.handle_a2ui_reset(args).await,
        }
    }

    async fn handle_present(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        let placement = args.placement.unwrap_or_default();
        let url = args.url.clone();

        // Update state
        {
            let mut state = self.state.write().await;
            state.visible = true;
            state.placement = placement.clone();
            if let Some(ref u) = url {
                state.current_url = Some(u.clone());
            }
        }

        // Call backend
        match self.backend.present(placement, url.as_deref()).await {
            Ok(()) => {
                if let Some(url) = url {
                    CanvasToolOutput::success_with_url(CanvasAction::Present, url)
                } else {
                    CanvasToolOutput::success(CanvasAction::Present)
                }
            }
            Err(e) => CanvasToolOutput::failed(CanvasAction::Present, e),
        }
    }

    async fn handle_hide(&self) -> CanvasToolOutput {
        // Update state
        {
            let mut state = self.state.write().await;
            state.visible = false;
        }

        // Call backend
        match self.backend.hide().await {
            Ok(()) => CanvasToolOutput::success(CanvasAction::Hide),
            Err(e) => CanvasToolOutput::failed(CanvasAction::Hide, e),
        }
    }

    async fn handle_navigate(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        let url = match args.url {
            Some(u) => u,
            None => {
                return CanvasToolOutput::failed(
                    CanvasAction::Navigate,
                    "URL is required for navigate action",
                )
            }
        };

        // Update state
        {
            let mut state = self.state.write().await;
            state.current_url = Some(url.clone());
        }

        // Call backend
        match self.backend.navigate(&url).await {
            Ok(()) => CanvasToolOutput::success_with_url(CanvasAction::Navigate, url),
            Err(e) => CanvasToolOutput::failed(CanvasAction::Navigate, e),
        }
    }

    async fn handle_eval(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        let javascript = match args.javascript {
            Some(js) => js,
            None => {
                return CanvasToolOutput::failed(
                    CanvasAction::Eval,
                    "JavaScript code is required for eval action",
                )
            }
        };

        // Call backend
        match self.backend.eval(&javascript).await {
            Ok(result) => {
                if let Some(r) = result {
                    CanvasToolOutput::success_with_eval(CanvasAction::Eval, r)
                } else {
                    CanvasToolOutput::success(CanvasAction::Eval)
                }
            }
            Err(e) => CanvasToolOutput::failed(CanvasAction::Eval, e),
        }
    }

    async fn handle_snapshot(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        let format = args.format.unwrap_or(SnapshotFormat::Png);
        let max_width = args.max_width;
        let quality = args.quality.unwrap_or(90);

        // Call backend
        match self.backend.snapshot(format, max_width, quality).await {
            Ok(data) => {
                CanvasToolOutput::success_with_snapshot(CanvasAction::Snapshot, data, format.mime_type())
            }
            Err(e) => CanvasToolOutput::failed(CanvasAction::Snapshot, e),
        }
    }

    async fn handle_a2ui_push(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        let jsonl = match args.jsonl {
            Some(j) => j,
            None => {
                return CanvasToolOutput::failed(
                    CanvasAction::A2uiPush,
                    "JSONL content is required for a2ui_push action",
                )
            }
        };

        // Parse JSONL
        let messages = match parse_jsonl(&jsonl) {
            Ok(m) => m,
            Err(e) => return CanvasToolOutput::failed(CanvasAction::A2uiPush, e.to_string()),
        };

        // Apply to surface manager
        {
            let mut surfaces = self.surfaces.write().await;
            for msg in &messages {
                surfaces.apply(msg);
            }

            // Update state with active surfaces
            let mut state = self.state.write().await;
            state.surfaces = surfaces.list().iter().map(|s| s.to_string()).collect();
        }

        // Call backend
        match self.backend.push_a2ui(&messages).await {
            Ok(()) => CanvasToolOutput::success(CanvasAction::A2uiPush),
            Err(e) => CanvasToolOutput::failed(CanvasAction::A2uiPush, e),
        }
    }

    async fn handle_a2ui_reset(&self, args: CanvasToolArgs) -> CanvasToolOutput {
        let surface_id = args.surface_id;

        // Clear surfaces
        {
            let mut surfaces = self.surfaces.write().await;
            if let Some(id) = &surface_id {
                surfaces.clear_surface(id);
            } else {
                surfaces.clear();
            }

            // Update state
            let mut state = self.state.write().await;
            state.surfaces = surfaces.list().iter().map(|s| s.to_string()).collect();
        }

        // Call backend
        match self.backend.reset_a2ui(surface_id.as_deref()).await {
            Ok(()) => CanvasToolOutput::success(CanvasAction::A2uiReset),
            Err(e) => CanvasToolOutput::failed(CanvasAction::A2uiReset, e),
        }
    }
}

/// Backend trait for canvas rendering operations
#[async_trait::async_trait]
pub trait CanvasBackend: Send + Sync {
    /// Present the canvas window
    async fn present(&self, placement: WindowPlacement, url: Option<&str>) -> Result<(), String>;

    /// Hide the canvas window
    async fn hide(&self) -> Result<(), String>;

    /// Navigate to a URL
    async fn navigate(&self, url: &str) -> Result<(), String>;

    /// Execute JavaScript and return result
    async fn eval(&self, javascript: &str) -> Result<Option<String>, String>;

    /// Capture a screenshot (returns base64)
    async fn snapshot(
        &self,
        format: SnapshotFormat,
        max_width: Option<u32>,
        quality: u8,
    ) -> Result<String, String>;

    /// Push A2UI messages
    async fn push_a2ui(&self, messages: &[A2uiMessage]) -> Result<(), String>;

    /// Reset A2UI state
    async fn reset_a2ui(&self, surface_id: Option<&str>) -> Result<(), String>;
}

/// No-op backend for testing
pub struct NoOpBackend;

#[async_trait::async_trait]
impl CanvasBackend for NoOpBackend {
    async fn present(&self, _placement: WindowPlacement, _url: Option<&str>) -> Result<(), String> {
        debug!("NoOpBackend: present");
        Ok(())
    }

    async fn hide(&self) -> Result<(), String> {
        debug!("NoOpBackend: hide");
        Ok(())
    }

    async fn navigate(&self, url: &str) -> Result<(), String> {
        debug!("NoOpBackend: navigate to {}", url);
        Ok(())
    }

    async fn eval(&self, javascript: &str) -> Result<Option<String>, String> {
        debug!("NoOpBackend: eval {} bytes", javascript.len());
        Ok(Some("undefined".to_string()))
    }

    async fn snapshot(
        &self,
        format: SnapshotFormat,
        _max_width: Option<u32>,
        _quality: u8,
    ) -> Result<String, String> {
        debug!("NoOpBackend: snapshot {:?}", format);
        // Return a minimal valid base64 PNG (1x1 transparent pixel)
        Ok("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string())
    }

    async fn push_a2ui(&self, messages: &[A2uiMessage]) -> Result<(), String> {
        debug!("NoOpBackend: push_a2ui {} messages", messages.len());
        Ok(())
    }

    async fn reset_a2ui(&self, surface_id: Option<&str>) -> Result<(), String> {
        debug!("NoOpBackend: reset_a2ui {:?}", surface_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_controller_present() {
        let controller = CanvasController::new_noop();

        let output = controller.execute(CanvasToolArgs::present()).await;
        assert!(output.success);
        assert_eq!(output.action, CanvasAction::Present);

        let state = controller.get_state().await;
        assert!(state.visible);
    }

    #[tokio::test]
    async fn test_controller_hide() {
        let controller = CanvasController::new_noop();

        // First present
        controller.execute(CanvasToolArgs::present()).await;

        // Then hide
        let output = controller.execute(CanvasToolArgs::hide()).await;
        assert!(output.success);

        let state = controller.get_state().await;
        assert!(!state.visible);
    }

    #[tokio::test]
    async fn test_controller_navigate() {
        let controller = CanvasController::new_noop();

        let output = controller
            .execute(CanvasToolArgs::navigate("http://example.com"))
            .await;
        assert!(output.success);
        assert_eq!(output.url, Some("http://example.com".to_string()));

        let state = controller.get_state().await;
        assert_eq!(state.current_url, Some("http://example.com".to_string()));
    }

    #[tokio::test]
    async fn test_controller_navigate_missing_url() {
        let controller = CanvasController::new_noop();

        let mut args = CanvasToolArgs::navigate("");
        args.url = None;

        let output = controller.execute(args).await;
        assert!(!output.success);
        assert!(output.error.is_some());
    }

    #[tokio::test]
    async fn test_controller_eval() {
        let controller = CanvasController::new_noop();

        let output = controller
            .execute(CanvasToolArgs::eval("console.log('test')"))
            .await;
        assert!(output.success);
        assert!(output.eval_result.is_some());
    }

    #[tokio::test]
    async fn test_controller_snapshot() {
        let controller = CanvasController::new_noop();

        let output = controller.execute(CanvasToolArgs::snapshot()).await;
        assert!(output.success);
        assert!(output.snapshot_data.is_some());
        assert_eq!(output.snapshot_mime, Some("image/png".to_string()));
    }

    #[tokio::test]
    async fn test_controller_a2ui_push() {
        let controller = CanvasController::new_noop();

        let jsonl = r#"{"surfaceUpdate":{"surfaceId":"main","components":[{"id":"root","type":"container"}]}}"#;
        let output = controller.execute(CanvasToolArgs::a2ui_push(jsonl)).await;
        assert!(output.success);

        let state = controller.get_state().await;
        assert!(state.surfaces.contains(&"main".to_string()));
    }

    #[tokio::test]
    async fn test_controller_a2ui_reset() {
        let controller = CanvasController::new_noop();

        // First push some content
        let jsonl = r#"{"surfaceUpdate":{"surfaceId":"main","components":[]}}"#;
        controller.execute(CanvasToolArgs::a2ui_push(jsonl)).await;

        // Then reset
        let output = controller.execute(CanvasToolArgs::a2ui_reset()).await;
        assert!(output.success);

        let state = controller.get_state().await;
        assert!(state.surfaces.is_empty());
    }
}

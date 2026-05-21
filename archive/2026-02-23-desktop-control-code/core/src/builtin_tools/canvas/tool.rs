//! Canvas Tool Implementation
//!
//! Agent-facing tool for canvas operations.

use async_trait::async_trait;
use std::sync::Arc;
use tracing::debug;

use crate::error::Result;
use crate::tools::AlephTool;

use super::controller::{CanvasBackend, CanvasController};
use super::types::{CanvasToolArgs, CanvasToolOutput};

/// Canvas tool for visual rendering operations
///
/// This tool provides agents with the ability to display visual content,
/// execute JavaScript, capture screenshots, and render A2UI components.
#[derive(Clone)]
pub struct CanvasTool {
    /// Canvas controller
    controller: Arc<CanvasController>,
}

impl CanvasTool {
    /// Tool identifier
    pub const NAME: &'static str = "canvas";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Control a visual canvas for rendering content. \
        Actions: present (show canvas), hide, navigate (load URL), eval (run JavaScript), \
        snapshot (capture screenshot), a2ui_push (render A2UI components), a2ui_reset (clear state). \
        Use for displaying visualizations, web content, or interactive UI.";

    /// Create a new canvas tool with a controller
    pub fn new(controller: Arc<CanvasController>) -> Self {
        Self { controller }
    }

    /// Create a canvas tool with a custom backend
    pub fn with_backend(backend: Arc<dyn CanvasBackend>) -> Self {
        Self {
            controller: Arc::new(CanvasController::new(backend)),
        }
    }

    /// Create a canvas tool with no-op backend (for testing)
    pub fn new_noop() -> Self {
        Self {
            controller: Arc::new(CanvasController::new_noop()),
        }
    }

    /// Get the controller
    pub fn controller(&self) -> &Arc<CanvasController> {
        &self.controller
    }
}

#[async_trait]
impl AlephTool for CanvasTool {
    const NAME: &'static str = "canvas";
    const DESCRIPTION: &'static str = "Control a visual canvas for rendering content. \
        Actions: present (show canvas), hide, navigate (load URL), eval (run JavaScript), \
        snapshot (capture screenshot), a2ui_push (render A2UI components), a2ui_reset (clear state). \
        Use for displaying visualizations, web content, or interactive UI.";

    type Args = CanvasToolArgs;
    type Output = CanvasToolOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(action = %args.action, "Canvas tool called");
        Ok(self.controller.execute(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::canvas::types::CanvasAction;

    #[tokio::test]
    async fn test_canvas_tool_present() {
        let tool = CanvasTool::new_noop();
        let output = tool.call(CanvasToolArgs::present()).await.unwrap();
        assert!(output.success);
        assert_eq!(output.action, CanvasAction::Present);
    }

    #[tokio::test]
    async fn test_canvas_tool_navigate() {
        let tool = CanvasTool::new_noop();
        let output = tool
            .call(CanvasToolArgs::navigate("http://example.com"))
            .await
            .unwrap();
        assert!(output.success);
        assert_eq!(output.url, Some("http://example.com".to_string()));
    }

    #[tokio::test]
    async fn test_canvas_tool_eval() {
        let tool = CanvasTool::new_noop();
        let output = tool
            .call(CanvasToolArgs::eval("1 + 1"))
            .await
            .unwrap();
        assert!(output.success);
        assert!(output.eval_result.is_some());
    }

    #[tokio::test]
    async fn test_canvas_tool_snapshot() {
        let tool = CanvasTool::new_noop();
        let output = tool.call(CanvasToolArgs::snapshot()).await.unwrap();
        assert!(output.success);
        assert!(output.snapshot_data.is_some());
    }

    #[tokio::test]
    async fn test_canvas_tool_a2ui() {
        let tool = CanvasTool::new_noop();

        // Push
        let jsonl = r#"{"surfaceUpdate":{"surfaceId":"test","components":[]}}"#;
        let output = tool.call(CanvasToolArgs::a2ui_push(jsonl)).await.unwrap();
        assert!(output.success);

        // Reset
        let output = tool.call(CanvasToolArgs::a2ui_reset()).await.unwrap();
        assert!(output.success);
    }
}

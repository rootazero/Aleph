// Browser resize tool — set the viewport / window dimensions.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_resize tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserResizeArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
}

/// Output from the browser_resize tool.
#[derive(Debug, Serialize)]
pub struct BrowserResizeOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Resizes the browser viewport (test responsive layouts, mobile vs desktop).
#[derive(Clone)]
pub struct BrowserResizeTool {
    manager: Arc<ProfileManager>,
}

impl BrowserResizeTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserResizeTool {
    const NAME: &'static str = "browser_resize";
    const DESCRIPTION: &'static str =
        "Resize the browser viewport to the given width and height in CSS pixels";
    type Args = BrowserResizeArgs;
    type Output = BrowserResizeOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if args.width == 0 || args.height == 0 {
            return Err(AlephError::invalid_input(
                "browser_resize requires non-zero width and height",
            ));
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.resize(&tab_id, args.width, args.height).await {
                Ok(()) => Ok(BrowserResizeOutput {
                    success: true,
                    message: Some(format!(
                        "Resized to {}x{} in profile '{}'",
                        args.width, args.height, args.profile
                    )),
                }),
                Err(e) => Ok(BrowserResizeOutput {
                    success: false,
                    message: Some(format!("Resize failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserResizeOutput {
                success: false,
                message: Some(format!("{e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_resize_zero_dimension_is_input_error() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserResizeTool::new(manager);
        let result = tool
            .call(BrowserResizeArgs {
                profile: "default".into(),
                width: 0,
                height: 600,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resize_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserResizeTool::new(manager);
        let result = tool
            .call(BrowserResizeArgs {
                profile: "default".into(),
                width: 1280,
                height: 720,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

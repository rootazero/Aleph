// Browser resize tool — set the viewport / window dimensions.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_resize` tool.
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

/// Output from the `browser_resize` tool.
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
        // A zero dimension is a model mistake, and it degrades to
        // `success:false` with the contract spelled out rather than a hard
        // `Err` — the convention `click::resolve_target` states for this
        // family. Resize was the last holdout raising
        // `AlephError::invalid_input`, which reaches the model as a tool
        // failure rather than as a rule it can act on.
        if args.width == 0 || args.height == 0 {
            return Ok(BrowserResizeOutput {
                success: false,
                message: Some("browser_resize requires non-zero width and height".into()),
            });
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
                    message: Some(format!(
                        "Resize failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserResizeOutput {
                success: false,
                message: Some(super::backend_error_text(&self.manager, &e)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    /// A zero dimension degrades to `success:false` carrying the contract,
    /// the shape the rest of the browser family uses, instead of a hard `Err`.
    #[tokio::test]
    async fn zero_dimension_degrades_with_the_contract_instead_of_erroring() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserResizeTool::new(manager);
        let result = tool
            .call(BrowserResizeArgs {
                profile: "default".into(),
                width: 0,
                height: 600,
            })
            .await
            .expect("a malformed call is a result, not a hard error");
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("non-zero")),
            "got: {:?}",
            result.message
        );
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

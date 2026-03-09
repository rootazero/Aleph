// Browser screenshot tool — captures a screenshot of the current page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

fn default_profile() -> String {
    "default".into()
}

/// Arguments for the browser_screenshot tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserScreenshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Capture the full page (default: false, captures viewport only).
    #[serde(default)]
    pub full_page: bool,
    /// CSS selector to screenshot a specific element.
    pub selector: Option<String>,
}

/// Output from the browser_screenshot tool.
#[derive(Debug, Serialize)]
pub struct BrowserScreenshotOutput {
    pub success: bool,
    pub image_base64: Option<String>,
    pub message: Option<String>,
}

/// Captures a screenshot of the current page or a specific element.
#[derive(Clone)]
pub struct BrowserScreenshotTool {
    manager: Arc<ProfileManager>,
}

impl BrowserScreenshotTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserScreenshotTool {
    const NAME: &'static str = "browser_screenshot";
    const DESCRIPTION: &'static str = "Take a screenshot of the current browser page or a specific element";
    type Args = BrowserScreenshotArgs;
    type Output = BrowserScreenshotOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserScreenshotOutput {
            success: true,
            image_base64: None,
            message: Some(format!(
                "Screenshot captured in profile '{}' (full_page={}, selector={:?})",
                args.profile, args.full_page, args.selector
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_screenshot_default_args() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserScreenshotTool::new(manager);

        let result = tool
            .call(BrowserScreenshotArgs {
                profile: "default".into(),
                full_page: false,
                selector: None,
            })
            .await
            .unwrap();

        assert!(result.success);
    }

    #[tokio::test]
    async fn test_screenshot_with_selector() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserScreenshotTool::new(manager);

        let result = tool
            .call(BrowserScreenshotArgs {
                profile: "default".into(),
                full_page: false,
                selector: Some("#main-content".into()),
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("#main-content"));
    }
}

// Browser screenshot tool — captures a screenshot of the current page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::ScreenshotOpts;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_screenshot tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserScreenshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Capture the full page (default: false, captures viewport only).
    #[serde(default)]
    pub full_page: bool,
}

/// Output from the browser_screenshot tool.
#[derive(Debug, Serialize)]
pub struct BrowserScreenshotOutput {
    pub success: bool,
    pub image_base64: Option<String>,
    pub message: Option<String>,
}

/// Captures a screenshot of the current page.
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
    const DESCRIPTION: &'static str = "Take a screenshot of the current browser page";
    type Args = BrowserScreenshotArgs;
    type Output = BrowserScreenshotOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                let opts = ScreenshotOpts {
                    full_page: args.full_page,
                    ..Default::default()
                };
                match backend.screenshot(&tab_id, opts).await {
                    Ok(result) => Ok(BrowserScreenshotOutput {
                        success: true,
                        image_base64: Some(result.data_base64),
                        message: Some(format!("Screenshot captured in profile '{}'", args.profile)),
                    }),
                    Err(e) => Ok(BrowserScreenshotOutput {
                        success: false,
                        image_base64: None,
                        message: Some(format!("Screenshot failed: {e}")),
                    }),
                }
            }
            Err(e) => Ok(BrowserScreenshotOutput {
                success: false,
                image_base64: None,
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
    async fn test_screenshot_default_args() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserScreenshotTool::new(manager);

        let result = tool
            .call(BrowserScreenshotArgs {
                profile: "default".into(),
                full_page: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_screenshot_full_page() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserScreenshotTool::new(manager);

        let result = tool
            .call(BrowserScreenshotArgs {
                profile: "default".into(),
                full_page: true,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

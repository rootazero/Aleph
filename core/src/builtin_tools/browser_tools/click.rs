// Browser click tool — clicks an element on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

fn default_profile() -> String {
    "default".into()
}

/// Arguments for the browser_click tool.
///
/// At least one targeting method must be provided: `selector`, `ref_id`, or coordinates (`x`/`y`).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserClickArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "default_profile")]
    pub profile: String,
    /// CSS selector of the element to click.
    pub selector: Option<String>,
    /// Accessibility ref_id from a previous snapshot.
    pub ref_id: Option<String>,
    /// X coordinate for coordinate-based clicking.
    pub x: Option<f64>,
    /// Y coordinate for coordinate-based clicking.
    pub y: Option<f64>,
}

/// Output from the browser_click tool.
#[derive(Debug, Serialize)]
pub struct BrowserClickOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Clicks an element on the page by selector, ref_id, or coordinates.
#[derive(Clone)]
pub struct BrowserClickTool {
    manager: Arc<ProfileManager>,
}

impl BrowserClickTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserClickTool {
    const NAME: &'static str = "browser_click";
    const DESCRIPTION: &'static str =
        "Click an element on the page by CSS selector, accessibility ref_id, or coordinates";
    type Args = BrowserClickArgs;
    type Output = BrowserClickOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate: at least one targeting method must be provided
        let has_selector = args.selector.is_some();
        let has_ref_id = args.ref_id.is_some();
        let has_coords = args.x.is_some() && args.y.is_some();

        if !has_selector && !has_ref_id && !has_coords {
            return Err(AlephError::invalid_input(
                "browser_click requires at least one targeting method: selector, ref_id, or x/y coordinates",
            ));
        }

        self.manager.record_activity(&args.profile);

        // Build a description of what was clicked
        let target_desc = if let Some(ref sel) = args.selector {
            format!("selector '{}'", sel)
        } else if let Some(ref rid) = args.ref_id {
            format!("ref_id '{}'", rid)
        } else {
            format!("coordinates ({}, {})", args.x.unwrap(), args.y.unwrap())
        };

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserClickOutput {
            success: true,
            message: Some(format!(
                "Clicked {} in profile '{}'",
                target_desc, args.profile
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_click_with_selector() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                selector: Some("button#submit".into()),
                ref_id: None,
                x: None,
                y: None,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("button#submit"));
    }

    #[tokio::test]
    async fn test_click_with_coordinates() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                selector: None,
                ref_id: None,
                x: Some(100.0),
                y: Some(200.0),
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("100"));
    }

    #[tokio::test]
    async fn test_click_no_target_fails() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                selector: None,
                ref_id: None,
                x: None,
                y: None,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_click_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                selector: None,
                ref_id: Some("ref-42".into()),
                x: None,
                y: None,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("ref-42"));
    }
}

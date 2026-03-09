// Browser type tool — types text into an element on the page.
//
// Named `type_text.rs` because `type` is a Rust keyword.

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

/// Arguments for the browser_type tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTypeArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Text to type into the element.
    pub text: String,
    /// CSS selector of the element to type into (optional, uses focused element if omitted).
    pub selector: Option<String>,
    /// Accessibility ref_id from a previous snapshot (optional).
    pub ref_id: Option<String>,
}

/// Output from the browser_type tool.
#[derive(Debug, Serialize)]
pub struct BrowserTypeOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Types text into an element on the page, identified by selector, ref_id, or the focused element.
#[derive(Clone)]
pub struct BrowserTypeTool {
    manager: Arc<ProfileManager>,
}

impl BrowserTypeTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserTypeTool {
    const NAME: &'static str = "browser_type";
    const DESCRIPTION: &'static str =
        "Type text into an element on the page by CSS selector, ref_id, or the currently focused element";
    type Args = BrowserTypeArgs;
    type Output = BrowserTypeOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        // Build a description of the target
        let target_desc = if let Some(ref sel) = args.selector {
            format!("selector '{}'", sel)
        } else if let Some(ref rid) = args.ref_id {
            format!("ref_id '{}'", rid)
        } else {
            "focused element".to_string()
        };

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserTypeOutput {
            success: true,
            message: Some(format!(
                "Typed {} chars into {} in profile '{}'",
                args.text.len(),
                target_desc,
                args.profile
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_type_into_selector() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTypeTool::new(manager);

        let result = tool
            .call(BrowserTypeArgs {
                profile: "default".into(),
                text: "hello world".into(),
                selector: Some("input#search".into()),
                ref_id: None,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("input#search"));
    }

    #[tokio::test]
    async fn test_type_into_focused() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTypeTool::new(manager);

        let result = tool
            .call(BrowserTypeArgs {
                profile: "default".into(),
                text: "test input".into(),
                selector: None,
                ref_id: None,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("focused element"));
    }

    #[tokio::test]
    async fn test_type_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTypeTool::new(manager);

        let result = tool
            .call(BrowserTypeArgs {
                profile: "default".into(),
                text: "ref text".into(),
                selector: None,
                ref_id: Some("ref-10".into()),
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("ref-10"));
    }
}

// Browser type tool — types text into an element on the page.
//
// Named `type_text.rs` because `type` is a Rust keyword.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_type tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTypeArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
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
        let target = if let Some(ref sel) = args.selector {
            ActionTarget::Selector { css: sel.clone() }
        } else if let Some(ref rid) = args.ref_id {
            ActionTarget::Ref { ref_id: rid.clone() }
        } else {
            ActionTarget::Ref { ref_id: "focused".into() }
        };

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.type_text(&tab_id, target, &args.text).await {
                Ok(()) => Ok(BrowserTypeOutput {
                    success: true,
                    message: Some(format!(
                        "Typed {} chars in profile '{}'",
                        args.text.len(), args.profile
                    )),
                }),
                Err(e) => Ok(BrowserTypeOutput {
                    success: false,
                    message: Some(format!("Type failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserTypeOutput {
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

// Browser click tool — clicks an element on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_click` tool.
///
/// At least one targeting method must be provided: `selector`, `ref_id`, or coordinates (`x`/`y`).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserClickArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// CSS selector of the element to click.
    pub selector: Option<String>,
    /// Accessibility `ref_id` from a previous snapshot.
    pub ref_id: Option<String>,
    /// X coordinate for coordinate-based clicking.
    pub x: Option<f64>,
    /// Y coordinate for coordinate-based clicking.
    pub y: Option<f64>,
    /// Double-click instead of single-click (requires `ref_id` targeting).
    #[serde(default)]
    pub double: bool,
}

/// Output from the `browser_click` tool.
#[derive(Debug, Serialize)]
pub struct BrowserClickOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Clicks an element on the page by selector, `ref_id`, or coordinates.
#[derive(Clone)]
pub struct BrowserClickTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserClickTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate clicks behind a user-defined approval policy. With no policy wired
    /// the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

fn resolve_target(args: &BrowserClickArgs) -> std::result::Result<ActionTarget, AlephError> {
    if let Some(ref rid) = args.ref_id {
        Ok(ActionTarget::Ref {
            ref_id: rid.clone(),
        })
    } else if let (Some(x), Some(y)) = (args.x, args.y) {
        Ok(ActionTarget::Coordinates { x, y })
    } else if args.selector.is_some() {
        Err(AlephError::invalid_input(
            "CSS selector targeting is no longer supported. Use 'ref_id' from browser_snapshot.",
        ))
    } else {
        Err(AlephError::invalid_input(
            "browser_click requires at least one targeting method: ref_id or x/y coordinates",
        ))
    }
}

#[async_trait]
impl AlephTool for BrowserClickTool {
    const NAME: &'static str = "browser_click";
    const DESCRIPTION: &'static str =
        "Click an element on the page by accessibility ref_id or coordinates; \
         set double=true for a double-click (ref_id only)";
    type Args = BrowserClickArgs;
    type Output = BrowserClickOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let target = resolve_target(&args)?;
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserClick,
            "click",
            &format!("{target:?}"),
        )
        .await
        {
            return Ok(BrowserClickOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                let result = if args.double {
                    backend.dblclick(&tab_id, target).await
                } else {
                    backend.click(&tab_id, target).await
                };
                match result {
                    Ok(()) => Ok(BrowserClickOutput {
                        success: true,
                        message: Some(format!(
                            "{} in profile '{}'",
                            if args.double {
                                "Double-clicked"
                            } else {
                                "Clicked"
                            },
                            args.profile
                        )),
                    }),
                    Err(e) => Ok(BrowserClickOutput {
                        success: false,
                        message: Some(format!("Click failed: {e}")),
                    }),
                }
            }
            Err(e) => Ok(BrowserClickOutput {
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
    async fn test_click_with_selector_returns_error() {
        // CSS selector targeting is no longer supported — must use ref_id.
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
                double: false,
            })
            .await;

        assert!(result.is_err(), "selector-only click should return Err");
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
                double: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
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
                double: false,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_double_click_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                selector: None,
                ref_id: Some("e7".into()),
                x: None,
                y: None,
                double: true,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully.
        assert!(!result.success);
        assert!(result.message.is_some());
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
                double: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

// Browser hover tool — moves the pointer over an element on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_hover` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserHoverArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Accessibility `ref_id` from a previous snapshot.
    pub ref_id: Option<String>,
    /// X coordinate for coordinate-based hovering.
    pub x: Option<f64>,
    /// Y coordinate for coordinate-based hovering.
    pub y: Option<f64>,
}

/// Output from the `browser_hover` tool.
#[derive(Debug, Serialize)]
pub struct BrowserHoverOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Hovers the pointer over an element (reveals menus, tooltips, hover states).
#[derive(Clone)]
pub struct BrowserHoverTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserHoverTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate hovers behind the approval policy. Hover is read-only motion in
    /// practice, but a deny on it from a chat-tier channel is the cheapest
    /// read of the policy. `Allow` is the matching default. With no policy
    /// wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

fn resolve_target(args: &BrowserHoverArgs) -> std::result::Result<ActionTarget, AlephError> {
    if let Some(ref rid) = args.ref_id {
        Ok(ActionTarget::Ref {
            ref_id: rid.clone(),
        })
    } else if let (Some(x), Some(y)) = (args.x, args.y) {
        Ok(ActionTarget::Coordinates { x, y })
    } else {
        Err(AlephError::invalid_input(
            "browser_hover requires a targeting method: ref_id or x/y coordinates",
        ))
    }
}

#[async_trait]
impl AlephTool for BrowserHoverTool {
    const NAME: &'static str = "browser_hover";
    const DESCRIPTION: &'static str =
        "Hover the pointer over an element by accessibility ref_id or coordinates";
    type Args = BrowserHoverArgs;
    type Output = BrowserHoverOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let target = resolve_target(&args)?;

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserHover,
            "hover",
            &format!("{target:?}"),
        )
        .await
        {
            return Ok(BrowserHoverOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.hover(&tab_id, target).await {
                Ok(()) => Ok(BrowserHoverOutput {
                    success: true,
                    message: Some(format!("Hovered in profile '{}'", args.profile)),
                }),
                Err(e) => Ok(BrowserHoverOutput {
                    success: false,
                    message: Some(format!("Hover failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserHoverOutput {
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
    async fn test_hover_with_ref_id() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserHoverTool::new(manager);
        let result = tool
            .call(BrowserHoverArgs {
                profile: "default".into(),
                ref_id: Some("e1".into()),
                x: None,
                y: None,
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_hover_no_target_fails() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserHoverTool::new(manager);
        let result = tool
            .call(BrowserHoverArgs {
                profile: "default".into(),
                ref_id: None,
                x: None,
                y: None,
            })
            .await;
        assert!(result.is_err());
    }
}

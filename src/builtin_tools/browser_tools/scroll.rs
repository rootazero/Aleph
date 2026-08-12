// Browser scroll tool — scrolls the page viewport in a direction.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::{ActionTarget, ScrollDirection};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Direction to scroll the viewport.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

impl From<ScrollDir> for ScrollDirection {
    fn from(d: ScrollDir) -> Self {
        match d {
            ScrollDir::Up => Self::Up,
            ScrollDir::Down => Self::Down,
            ScrollDir::Left => Self::Left,
            ScrollDir::Right => Self::Right,
        }
    }
}

/// Arguments for the `browser_scroll` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserScrollArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Direction to scroll: up, down, left, or right.
    pub direction: ScrollDir,
}

/// Output from the `browser_scroll` tool.
#[derive(Debug, Serialize)]
pub struct BrowserScrollOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Scrolls the page viewport in a direction (reveals off-screen content).
#[derive(Clone)]
pub struct BrowserScrollTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserScrollTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate scroll behind the approval policy. Read-only motion in practice,
    /// `Allow` is the matching default. With no policy wired the tool behaves
    /// exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserScrollTool {
    const NAME: &'static str = "browser_scroll";
    const DESCRIPTION: &'static str =
        "Scroll the page viewport up, down, left, or right to reveal off-screen content";
    type Args = BrowserScrollArgs;
    type Output = BrowserScrollOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserScroll,
            "scroll",
            &format!("{:?}", args.direction),
        )
        .await
        {
            return Ok(BrowserScrollOutput {
                success: false,
                message: Some(message),
            });
        }
        let direction: ScrollDirection = args.direction.clone().into();
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                // Scroll targets the viewport, not a specific element — the backend
                // ignores the target, so a viewport-origin placeholder is passed.
                let target = ActionTarget::Coordinates { x: 0.0, y: 0.0 };
                match backend.scroll(&tab_id, target, direction).await {
                    Ok(()) => Ok(BrowserScrollOutput {
                        success: true,
                        message: Some(format!(
                            "Scrolled {:?} in profile '{}'",
                            args.direction, args.profile
                        )),
                    }),
                    Err(e) => Ok(BrowserScrollOutput {
                        success: false,
                        message: Some(format!(
                            "Scroll failed: {}",
                            super::backend_error_text(&self.manager, &e)
                        )),
                    }),
                }
            }
            Err(e) => Ok(BrowserScrollOutput {
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

    #[tokio::test]
    async fn test_scroll_down() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserScrollTool::new(manager);
        let result = tool
            .call(BrowserScrollArgs {
                profile: "default".into(),
                direction: ScrollDir::Down,
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

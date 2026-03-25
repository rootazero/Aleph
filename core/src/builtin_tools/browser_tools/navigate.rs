// Browser navigate tool — go back, forward, or refresh the current page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Navigation action to perform.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavigateAction {
    /// Go back to the previous page.
    Back,
    /// Go forward to the next page.
    Forward,
    /// Refresh the current page.
    Refresh,
}

/// Arguments for the browser_navigate tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserNavigateArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Navigation action to perform.
    pub action: NavigateAction,
}

/// Output from the browser_navigate tool.
#[derive(Debug, Serialize)]
pub struct BrowserNavigateOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Navigates the browser: go back, forward, or refresh.
#[derive(Clone)]
pub struct BrowserNavigateTool {
    manager: Arc<ProfileManager>,
}

impl BrowserNavigateTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserNavigateTool {
    const NAME: &'static str = "browser_navigate";
    const DESCRIPTION: &'static str =
        "Navigate browser: go back, forward, or refresh the current page";
    type Args = BrowserNavigateArgs;
    type Output = BrowserNavigateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let js = match args.action {
            NavigateAction::Back => "history.back()",
            NavigateAction::Forward => "history.forward()",
            NavigateAction::Refresh => "location.reload()",
        };
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.evaluate(&tab_id, js).await {
                Ok(_) => Ok(BrowserNavigateOutput {
                    success: true,
                    message: Some(format!("Navigated {:?} in profile '{}'", args.action, args.profile)),
                }),
                Err(e) => Ok(BrowserNavigateOutput {
                    success: false,
                    message: Some(format!("Navigation failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserNavigateOutput {
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
    async fn test_navigate_back() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserNavigateTool::new(manager);

        let result = tool
            .call(BrowserNavigateArgs {
                profile: "default".into(),
                action: NavigateAction::Back,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_navigate_forward() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserNavigateTool::new(manager);

        let result = tool
            .call(BrowserNavigateArgs {
                profile: "default".into(),
                action: NavigateAction::Forward,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_navigate_refresh() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserNavigateTool::new(manager);

        let result = tool
            .call(BrowserNavigateArgs {
                profile: "default".into(),
                action: NavigateAction::Refresh,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

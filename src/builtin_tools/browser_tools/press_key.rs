// Browser press_key tool — presses a keyboard key.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserPressKeyArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Key to press (e.g. "Enter", "Tab", "Escape", "`ArrowDown`", "Backspace").
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct BrowserPressKeyOutput {
    pub success: bool,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct BrowserPressKeyTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserPressKeyTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate key presses behind the approval policy. `Allow` is the matching
    /// default (a denied `browser_click` should not block a follow-up
    /// `browser_press_key`). With no policy wired the tool behaves exactly as
    /// before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserPressKeyTool {
    const NAME: &'static str = "browser_press_key";
    const DESCRIPTION: &'static str =
        "Press a keyboard key in the browser (Enter, Tab, Escape, ArrowDown, Backspace, etc.)";
    type Args = BrowserPressKeyArgs;
    type Output = BrowserPressKeyOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserPressKey,
            "press_key",
            &args.key,
        )
        .await
        {
            return Ok(BrowserPressKeyOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.press_key(&tab_id, &args.key).await {
                Ok(()) => Ok(BrowserPressKeyOutput {
                    success: true,
                    message: Some(format!("Pressed '{}'", args.key)),
                }),
                Err(e) => Ok(BrowserPressKeyOutput {
                    success: false,
                    message: Some(format!("Press key failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserPressKeyOutput {
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
    async fn test_press_key() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserPressKeyTool::new(manager);
        let result = tool
            .call(BrowserPressKeyArgs {
                profile: "default".into(),
                key: "Enter".into(),
            })
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }
}

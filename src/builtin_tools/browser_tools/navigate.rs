// Browser navigate tool — go back, forward, refresh, or go to a URL.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::HistoryNav;
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
    /// Navigate the current tab to a new URL (SSRF-checked).
    Goto {
        /// The URL to navigate to.
        url: String,
    },
}

/// Arguments for the `browser_navigate` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserNavigateArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Navigation action to perform.
    pub action: NavigateAction,
}

/// Output from the `browser_navigate` tool.
#[derive(Debug, Serialize)]
pub struct BrowserNavigateOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Navigates the browser: go back, forward, refresh, or go to a new URL.
#[derive(Clone)]
pub struct BrowserNavigateTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserNavigateTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate navigation behind a user-defined approval policy (allow/deny/ask
    /// per `~/.aleph/approval-policy.json`). With no policy wired the tool
    /// behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserNavigateTool {
    const NAME: &'static str = "browser_navigate";
    const DESCRIPTION: &'static str =
        "Navigate browser: go back, forward, refresh, or go to a new URL in the current tab";
    type Args = BrowserNavigateArgs;
    type Output = BrowserNavigateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Approval policy gate (user-defined allow/deny/ask). Runs above the
        // SSRF check below: SSRF is a fixed security floor, this is the user's
        // configurable allow/blocklist (e.g. block `*://malicious.com/*`).
        let nav_target = match &args.action {
            NavigateAction::Goto { url } => url.as_str(),
            NavigateAction::Back => "back",
            NavigateAction::Forward => "forward",
            NavigateAction::Refresh => "refresh",
        };
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserNavigate,
            "navigate",
            nav_target,
        )
        .await
        {
            return Ok(BrowserNavigateOutput {
                success: false,
                message: Some(message),
            });
        }

        // Goto navigates the current tab to a new URL — routed through the
        // backend's `navigate` (SSRF-checked) rather than a JS history call.
        if let NavigateAction::Goto { url } = &args.action {
            if let Err(violation) = self.manager.check_navigation(url).await {
                return Ok(BrowserNavigateOutput {
                    success: false,
                    message: Some(format!("Blocked: {violation}")),
                });
            }
            return Ok(
                match super::make_backend_and_tab(&self.manager, &args.profile).await {
                    Ok((backend, tab_id)) => match backend.navigate(&tab_id, url).await {
                        Ok(()) => BrowserNavigateOutput {
                            success: true,
                            message: Some(format!(
                                "Navigated to {url} in profile '{}'",
                                args.profile
                            )),
                        },
                        Err(e) => BrowserNavigateOutput {
                            success: false,
                            message: Some(format!("Navigation failed: {e}")),
                        },
                    },
                    Err(e) => BrowserNavigateOutput {
                        success: false,
                        message: Some(format!("{e}")),
                    },
                },
            );
        }

        // Back/forward/refresh route through the backend's `history` method:
        // native go-back/go-forward/reload on the managed CLI, native
        // `navigate_page {type}` on Chrome DevTools MCP. (The old path passed a
        // bare `history.back()` expression to `evaluate`, but both eval
        // surfaces require an arrow-function body — the call never executed.)
        let nav = match args.action {
            NavigateAction::Back => HistoryNav::Back,
            NavigateAction::Forward => HistoryNav::Forward,
            NavigateAction::Refresh => HistoryNav::Refresh,
            NavigateAction::Goto { .. } => unreachable!("Goto handled above"),
        };
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.history(&tab_id, nav).await {
                Ok(_) => Ok(BrowserNavigateOutput {
                    success: true,
                    message: Some(format!("Navigated {nav:?} in profile '{}'", args.profile)),
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

    #[tokio::test]
    async fn test_navigate_goto_blocks_ssrf() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserNavigateTool::new(manager);

        let result = tool
            .call(BrowserNavigateArgs {
                profile: "default".into(),
                action: NavigateAction::Goto {
                    url: "http://127.0.0.1:8080/admin".into(),
                },
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.message.unwrap().contains("Blocked"));
    }

    #[tokio::test]
    async fn test_navigate_blocked_by_approval_policy() {
        use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};
        use async_trait::async_trait;

        struct DenyAll;
        #[async_trait]
        impl ApprovalPolicy for DenyAll {
            async fn check(&self, _req: &ActionRequest) -> ApprovalDecision {
                ApprovalDecision::Deny {
                    reason: "blocked in test".into(),
                }
            }
            async fn record(&self, _req: &ActionRequest, _dec: &ApprovalDecision) {}
        }

        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserNavigateTool::new(manager)
            .with_approval_policy(Arc::new(DenyAll) as Arc<dyn ApprovalPolicy>);

        let result = tool
            .call(BrowserNavigateArgs {
                profile: "default".into(),
                action: NavigateAction::Goto {
                    url: "https://example.com/".into(),
                },
            })
            .await
            .unwrap();

        // The policy short-circuits before any browser/SSRF work runs.
        assert!(!result.success);
        assert!(result
            .message
            .unwrap()
            .contains("denied by approval policy"));
    }

    #[tokio::test]
    async fn test_navigate_goto_allows_public_url() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserNavigateTool::new(manager);

        let result = tool
            .call(BrowserNavigateArgs {
                profile: "default".into(),
                action: NavigateAction::Goto {
                    url: "https://example.com/".into(),
                },
            })
            .await
            .unwrap();

        // Public URL passes SSRF; without a running browser it degrades gracefully.
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

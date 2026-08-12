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
        // SSRF + secret-exfiltration floor runs FIRST, the same order
        // `browser_open` uses. It is deterministic and free, and a URL it
        // refuses can never be navigated to however the approval resolves —
        // asking first spends a user approval (and, under an `Ask` policy, a
        // human interruption) on an action that was already decided.
        // Deterministic policy beats interactive approval, the rule
        // `check_input_secret_block` states for the input-side gate.
        if let NavigateAction::Goto { url } = &args.action {
            if let Err(violation) = self.manager.check_navigation(url).await {
                return Ok(BrowserNavigateOutput {
                    success: false,
                    message: Some(format!("Blocked: {violation}")),
                });
            }
        }

        // Approval policy gate: the user's configurable allow/blocklist (e.g.
        // block `*://malicious.com/*`) layered on top of that fixed floor.
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
        // backend's `navigate` (SSRF-checked above) rather than a JS history
        // call.
        if let NavigateAction::Goto { url } = &args.action {
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
                            message: Some(format!(
                                "Navigation failed: {}",
                                super::backend_error_text(&self.manager, &e)
                            )),
                        },
                    },
                    Err(e) => BrowserNavigateOutput {
                        success: false,
                        message: Some(super::backend_error_text(&self.manager, &e)),
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
                    message: Some(format!(
                        "Navigation failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserNavigateOutput {
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

    /// Make `example.com` resolve to a public address.
    ///
    /// The SSRF floor now runs before the approval gate, so a test that means
    /// to exercise the gate has to get past the floor first — and in a
    /// sandboxed test process nothing resolves, so every public host would
    /// otherwise be refused as unresolvable. Returns the scope guard; dropping
    /// it restores the previous resolver.
    fn public_dns() -> crate::security::ssrf::dns::test_hook::ResolverScope {
        let mut hosts = std::collections::HashMap::new();
        hosts.insert("example.com".to_string(), vec!["8.8.8.8".parse().unwrap()]);
        crate::security::ssrf::dns::test_hook::ResolverScope::install(hosts)
    }

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

        let _dns = public_dns();
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

        // For a URL the SSRF floor allows, the policy is still what decides —
        // and it short-circuits before any browser work runs.
        assert!(!result.success);
        assert!(result
            .message
            .unwrap()
            .contains("denied by approval policy"));
    }

    /// An SSRF-refused URL must be refused BEFORE the approval policy is
    /// consulted. The floor is deterministic: the navigation cannot happen
    /// whichever way the approval resolves, so asking spends a user approval
    /// — and under an `Ask` policy a human interruption — on a decided action.
    ///
    /// `browser_open` has always ordered it this way; this tool had it
    /// inverted, so the two faces of the same trust surface disagreed.
    #[tokio::test]
    async fn ssrf_refusal_precedes_the_approval_gate() {
        use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Counts consultations so the test can assert the gate was never
        /// reached — an `Ask` verdict alone would only prove which message won.
        struct CountingAsk(Arc<AtomicUsize>);
        #[async_trait]
        impl ApprovalPolicy for CountingAsk {
            async fn check(&self, _req: &ActionRequest) -> ApprovalDecision {
                self.0.fetch_add(1, Ordering::SeqCst);
                ApprovalDecision::Ask {
                    prompt: "approve?".into(),
                }
            }
            async fn record(&self, _req: &ActionRequest, _dec: &ApprovalDecision) {}
        }

        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = Arc::new(ProfileManager::new(config));
        let asked = Arc::new(AtomicUsize::new(0));
        let tool = BrowserNavigateTool::new(manager)
            .with_approval_policy(
                Arc::new(CountingAsk(Arc::clone(&asked))) as Arc<dyn ApprovalPolicy>
            );

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
        assert!(
            result.message.unwrap().contains("Blocked"),
            "the SSRF verdict must be what the model is told"
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "the approval policy must not be consulted for a URL the floor already refused"
        );
    }

    #[tokio::test]
    async fn test_navigate_goto_allows_public_url() {
        let _dns = public_dns();
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

        // Public URL passes SSRF; without a running browser it degrades
        // gracefully. Assert on WHICH failure: with the floor now running
        // first, "message is some" would be satisfied just as well by a
        // `Blocked:` verdict, which is the opposite of what this pins.
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| !m.contains("Blocked")),
            "the URL must clear the SSRF floor: {:?}",
            result.message
        );
    }
}

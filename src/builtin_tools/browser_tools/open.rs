// Browser open tool — opens a URL in a managed browser profile.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_open` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserOpenArgs {
    /// URL to open.
    pub url: String,
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
}

/// Output from the `browser_open` tool.
#[derive(Debug, Serialize)]
pub struct BrowserOpenOutput {
    pub success: bool,
    pub tab_id: Option<String>,
    pub message: Option<String>,
}

/// Opens a URL in a managed browser profile with SSRF protection.
#[derive(Clone)]
pub struct BrowserOpenTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserOpenTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate `browser_open` behind the same approval policy as
    /// `browser_navigate` — opening a fresh tab is the same trust surface as
    /// navigating an existing one, so a deny on a host must deny both. With
    /// no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserOpenTool {
    const NAME: &'static str = "browser_open";
    const DESCRIPTION: &'static str = "Open a URL in a headless browser. Default profile is fast and invisible. Only use profile=\"user\" when the user explicitly asks to use their Chrome browser with logged-in sessions.";
    type Args = BrowserOpenArgs;
    type Output = BrowserOpenOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // SSRF + secret-exfiltration check before creating backend
        if let Err(violation) = self.manager.check_navigation(&args.url).await {
            return Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(format!("Blocked: {violation}")),
            });
        }

        // Approval gate mirrors `browser_navigate`: a host the user has
        // already denied should not be reachable via a fresh tab.
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserOpen,
            "open",
            &args.url,
        )
        .await
        {
            return Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(message),
            });
        }

        let backend = match super::make_backend(&self.manager, &args.profile) {
            Ok(b) => b,
            Err(e) => {
                return Ok(BrowserOpenOutput {
                    success: false,
                    tab_id: None,
                    message: Some(e.to_string()),
                });
            }
        };
        match backend.open_tab(&args.url).await {
            Ok(tab_id) => Ok(BrowserOpenOutput {
                success: true,
                tab_id: Some(tab_id),
                message: Some(format!("Opened {} in profile '{}'", args.url, args.profile)),
            }),
            Err(e) => Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(format!("Failed to open tab: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_browser_open_ssrf_blocks_private() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        let result = tool
            .call(BrowserOpenArgs {
                url: "http://localhost:3000/admin".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.message.unwrap().contains("Blocked"));
    }

    #[tokio::test]
    async fn test_browser_open_blocks_ssrf_private_ip() {
        let mut config = BrowserSystemConfig::default();
        config.policy.block_private = true;
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        for url in &[
            "http://10.0.0.1/secret",
            "http://172.16.0.1/internal",
            "http://192.168.1.1/router",
            "http://[::1]/",
        ] {
            let result = tool
                .call(BrowserOpenArgs {
                    url: url.to_string(),
                    profile: "default".into(),
                })
                .await
                .unwrap();

            assert!(!result.success, "Should block {}", url);
            assert!(
                result.message.as_ref().unwrap().contains("Blocked"),
                "Should have Blocked message for {}",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_browser_open_blocked_domain_list() {
        use crate::browser::network_policy::SsrfConfig;

        let config = BrowserSystemConfig {
            policy: SsrfConfig {
                block_private: false,
                blocked_domains: vec!["*.evil.com".to_string(), "malware.org".to_string()],
                allowed_domains: vec![],
                block_secrets_in_url: false,
                block_secrets_in_input: false,
                redact_secrets_in_content: false,
            },
            ..Default::default()
        };

        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        // Should block evil.com subdomain
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://sub.evil.com/payload".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.as_ref().unwrap().contains("Blocked"));

        // Should block malware.org
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://malware.org/payload".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.as_ref().unwrap().contains("Blocked"));

        // Should allow normal domains (passes policy, but fails without a running browser)
        let result = tool
            .call(BrowserOpenArgs {
                url: "https://safe.com".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_browser_open_allowlist_mode() {
        use crate::browser::network_policy::SsrfConfig;

        let config = BrowserSystemConfig {
            policy: SsrfConfig {
                block_private: false,
                blocked_domains: vec![],
                allowed_domains: vec!["*.allowed.com".to_string()],
                block_secrets_in_url: false,
                block_secrets_in_input: false,
                redact_secrets_in_content: false,
            },
            ..Default::default()
        };

        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        // Should allow allowed.com subdomain (passes policy, but fails without a running browser)
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://app.allowed.com/page".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());

        // Should block non-allowed domain
        let result = tool
            .call(BrowserOpenArgs {
                url: "http://other.com/page".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.as_ref().unwrap().contains("Blocked"));
    }

    #[tokio::test]
    async fn test_browser_open_allows_public() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserOpenTool::new(manager);

        let result = tool
            .call(BrowserOpenArgs {
                url: "https://example.com".into(),
                profile: "default".into(),
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

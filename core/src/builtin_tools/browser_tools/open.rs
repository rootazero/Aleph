// Browser open tool — opens a URL in a managed browser profile.

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

/// Arguments for the browser_open tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserOpenArgs {
    /// URL to open.
    pub url: String,
    /// Browser profile name (default: "default").
    #[serde(default = "default_profile")]
    pub profile: String,
}

/// Output from the browser_open tool.
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
}

impl BrowserOpenTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserOpenTool {
    const NAME: &'static str = "browser_open";
    const DESCRIPTION: &'static str = "Open a URL in a managed browser profile";
    type Args = BrowserOpenArgs;
    type Output = BrowserOpenOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // SSRF check
        if let Err(violation) = self.manager.check_url(&args.url) {
            return Ok(BrowserOpenOutput {
                success: false,
                tab_id: None,
                message: Some(format!("Blocked: {violation}")),
            });
        }

        self.manager.record_activity(&args.profile);

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserOpenOutput {
            success: true,
            tab_id: Some("tab-1".into()),
            message: Some(format!("Opened {} in profile '{}'", args.url, args.profile)),
        })
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

        assert!(result.success);
        assert!(result.tab_id.is_some());
    }
}

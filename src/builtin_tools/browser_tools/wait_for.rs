// Browser wait_for tool — waits for text to appear on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

fn default_timeout() -> u64 {
    5000
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserWaitForArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Text to wait for on the page.
    pub text: String,
    /// Timeout in milliseconds (default: 5000).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct BrowserWaitForOutput {
    pub success: bool,
    pub found: bool,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct BrowserWaitForTool {
    manager: Arc<ProfileManager>,
}

impl BrowserWaitForTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserWaitForTool {
    const NAME: &'static str = "browser_wait_for";
    const DESCRIPTION: &'static str =
        "Wait for specific text to appear on the page (useful after navigation or actions)";
    type Args = BrowserWaitForArgs;
    type Output = BrowserWaitForOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                match backend
                    .wait_for_text(&tab_id, &args.text, args.timeout_ms)
                    .await
                {
                    Ok(found) => Ok(BrowserWaitForOutput {
                        success: true,
                        found,
                        message: Some(format!(
                            "Text '{}' {}",
                            args.text,
                            if found { "found" } else { "not found" }
                        )),
                    }),
                    Err(e) => Ok(BrowserWaitForOutput {
                        success: false,
                        found: false,
                        message: Some(format!("Wait failed: {e}")),
                    }),
                }
            }
            Err(e) => Ok(BrowserWaitForOutput {
                success: false,
                found: false,
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
    async fn test_wait_for() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserWaitForTool::new(manager);
        let result = tool
            .call(BrowserWaitForArgs {
                profile: "default".into(),
                text: "Loading".into(),
                timeout_ms: 1000,
            })
            .await
            .unwrap();
        assert!(!result.success); // No browser running
    }
}

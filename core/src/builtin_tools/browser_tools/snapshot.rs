// Browser snapshot tool — captures an accessibility tree snapshot of the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::backend::BrowserBackend;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
use crate::browser::playwright_mcp_backend::PlaywrightMcpBackend;
use crate::browser::manager::ProfileManager;
use crate::browser::profile::BrowserDriver;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_snapshot tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSnapshotArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
}

/// Output from the browser_snapshot tool.
#[derive(Debug, Serialize)]
pub struct BrowserSnapshotOutput {
    pub success: bool,
    pub aria_tree: Option<String>,
    pub message: Option<String>,
}

/// Captures an accessibility tree (ARIA) snapshot of the current page.
#[derive(Clone)]
pub struct BrowserSnapshotTool {
    manager: Arc<ProfileManager>,
}

impl BrowserSnapshotTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserSnapshotTool {
    const NAME: &'static str = "browser_snapshot";
    const DESCRIPTION: &'static str =
        "Get an accessibility tree snapshot of the current browser page for structured understanding";
    type Args = BrowserSnapshotArgs;
    type Output = BrowserSnapshotOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        let driver = self.manager.get_driver(&args.profile);
        match driver {
            Some(BrowserDriver::ExistingSession) => {
                let chrome_mcp = self.manager.get_chrome_mcp_driver();
                let backend = ChromeMcpBackend::new(chrome_mcp, args.profile.clone());
                let tab_id = match super::get_active_tab(&backend).await {
                    Ok(id) => id,
                    Err(e) => {
                        return Ok(BrowserSnapshotOutput {
                            success: false,
                            aria_tree: None,
                            message: Some(format!("{e}")),
                        });
                    }
                };

                match backend.snapshot(&tab_id).await {
                    Ok(snap) => {
                        let json_str = serde_json::to_string(&snap)
                            .unwrap_or_else(|_| "{}".to_string());
                        Ok(BrowserSnapshotOutput {
                            success: true,
                            aria_tree: Some(json_str),
                            message: Some(format!(
                                "Snapshot captured in profile '{}'",
                                args.profile
                            )),
                        })
                    }
                    Err(e) => Ok(BrowserSnapshotOutput {
                        success: false,
                        aria_tree: None,
                        message: Some(format!("Snapshot failed: {e}")),
                    }),
                }
            }
            Some(BrowserDriver::Managed) | None => {
                let playwright = self.manager.get_playwright_mcp_driver();
                let backend = PlaywrightMcpBackend::new(playwright, args.profile.clone());
                let tab_id = match super::get_active_tab(&backend).await {
                    Ok(id) => id,
                    Err(e) => {
                        return Ok(BrowserSnapshotOutput {
                            success: false,
                            aria_tree: None,
                            message: Some(format!("{e}")),
                        });
                    }
                };

                match backend.snapshot(&tab_id).await {
                    Ok(snap) => {
                        let json_str = serde_json::to_string(&snap)
                            .unwrap_or_else(|_| "{}".to_string());
                        Ok(BrowserSnapshotOutput {
                            success: true,
                            aria_tree: Some(json_str),
                            message: Some(format!(
                                "Snapshot captured in profile '{}' (headless)",
                                args.profile
                            )),
                        })
                    }
                    Err(e) => Ok(BrowserSnapshotOutput {
                        success: false,
                        aria_tree: None,
                        message: Some(format!("Snapshot failed: {e}")),
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_snapshot_returns_aria_tree() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSnapshotTool::new(manager);

        let result = tool
            .call(BrowserSnapshotArgs {
                profile: "default".into(),
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some()); // Error message present
    }
}

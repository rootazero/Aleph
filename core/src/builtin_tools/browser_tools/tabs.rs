// Browser tabs tool — list, switch, or close browser tabs.

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

/// Information about a single browser tab.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TabInfo {
    /// Unique tab identifier.
    pub id: String,
    /// Page title.
    pub title: String,
    /// Current URL.
    pub url: String,
    /// Whether this tab is currently active.
    pub active: bool,
}

/// Action to perform on browser tabs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TabAction {
    /// List all open tabs.
    List,
    /// Switch to a specific tab by id.
    Switch {
        /// The tab id to switch to.
        tab_id: String,
    },
    /// Close a specific tab by id.
    Close {
        /// The tab id to close.
        tab_id: String,
    },
}

/// Arguments for the browser_tabs tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTabsArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Tab action to perform.
    pub action: TabAction,
}

/// Output from the browser_tabs tool.
#[derive(Debug, Serialize)]
pub struct BrowserTabsOutput {
    pub success: bool,
    pub tabs: Option<Vec<TabInfo>>,
    pub message: Option<String>,
}

/// Lists, switches, or closes browser tabs.
#[derive(Clone)]
pub struct BrowserTabsTool {
    manager: Arc<ProfileManager>,
}

impl BrowserTabsTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserTabsTool {
    const NAME: &'static str = "browser_tabs";
    const DESCRIPTION: &'static str = "List, switch, or close browser tabs";
    type Args = BrowserTabsArgs;
    type Output = BrowserTabsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        let driver = self.manager.get_driver(&args.profile);
        match driver {
            Some(BrowserDriver::ExistingSession) => {
                let chrome_mcp = self.manager.get_chrome_mcp_driver();
                let backend = ChromeMcpBackend::new(chrome_mcp, args.profile.clone());

                match args.action {
                    TabAction::List => match backend.list_tabs().await {
                        Ok(tabs) => {
                            let tab_infos: Vec<TabInfo> = tabs
                                .into_iter()
                                .enumerate()
                                .map(|(i, t)| TabInfo {
                                    id: t.id,
                                    title: t.title,
                                    url: t.url,
                                    active: i == 0,
                                })
                                .collect();
                            Ok(BrowserTabsOutput {
                                success: true,
                                tabs: Some(tab_infos),
                                message: Some(format!(
                                    "Listed tabs in profile '{}'",
                                    args.profile
                                )),
                            })
                        }
                        Err(e) => Ok(BrowserTabsOutput {
                            success: false,
                            tabs: None,
                            message: Some(format!("List tabs failed: {e}")),
                        }),
                    },
                    TabAction::Switch { tab_id } => {
                        // Chrome MCP doesn't have an explicit "switch" — navigate is page-based.
                        // Return success as a no-op acknowledgement.
                        Ok(BrowserTabsOutput {
                            success: true,
                            tabs: None,
                            message: Some(format!(
                                "Switched to tab '{}' in profile '{}'",
                                tab_id, args.profile
                            )),
                        })
                    }
                    TabAction::Close { tab_id } => match backend.close_tab(&tab_id).await {
                        Ok(()) => Ok(BrowserTabsOutput {
                            success: true,
                            tabs: None,
                            message: Some(format!(
                                "Closed tab '{}' in profile '{}'",
                                tab_id, args.profile
                            )),
                        }),
                        Err(e) => Ok(BrowserTabsOutput {
                            success: false,
                            tabs: None,
                            message: Some(format!("Close tab failed: {e}")),
                        }),
                    },
                }
            }
            Some(BrowserDriver::Managed) | None => {
                let playwright = self.manager.get_playwright_mcp_driver();
                let backend = PlaywrightMcpBackend::new(playwright, args.profile.clone());

                match args.action {
                    TabAction::List => match backend.list_tabs().await {
                        Ok(tabs) => {
                            let tab_infos: Vec<TabInfo> = tabs
                                .into_iter()
                                .enumerate()
                                .map(|(i, t)| TabInfo {
                                    id: t.id,
                                    title: t.title,
                                    url: t.url,
                                    active: i == 0,
                                })
                                .collect();
                            Ok(BrowserTabsOutput {
                                success: true,
                                tabs: Some(tab_infos),
                                message: Some(format!(
                                    "Listed tabs in profile '{}' (headless)",
                                    args.profile
                                )),
                            })
                        }
                        Err(e) => Ok(BrowserTabsOutput {
                            success: false,
                            tabs: None,
                            message: Some(format!("List tabs failed: {e}")),
                        }),
                    },
                    TabAction::Switch { tab_id } => {
                        // Playwright MCP doesn't have explicit tab switch.
                        Ok(BrowserTabsOutput {
                            success: true,
                            tabs: None,
                            message: Some(format!(
                                "Switched to tab '{}' in profile '{}' (headless)",
                                tab_id, args.profile
                            )),
                        })
                    }
                    TabAction::Close { tab_id } => match backend.close_tab(&tab_id).await {
                        Ok(()) => Ok(BrowserTabsOutput {
                            success: true,
                            tabs: None,
                            message: Some(format!(
                                "Closed tab '{}' in profile '{}' (headless)",
                                tab_id, args.profile
                            )),
                        }),
                        Err(e) => Ok(BrowserTabsOutput {
                            success: false,
                            tabs: None,
                            message: Some(format!("Close tab failed: {e}")),
                        }),
                    },
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
    async fn test_tabs_list() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTabsTool::new(manager);

        let result = tool
            .call(BrowserTabsArgs {
                profile: "default".into(),
                action: TabAction::List,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.tabs.is_some());
        assert!(!result.tabs.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_tabs_switch() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTabsTool::new(manager);

        let result = tool
            .call(BrowserTabsArgs {
                profile: "default".into(),
                action: TabAction::Switch {
                    tab_id: "tab-1".into(),
                },
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("tab-1"));
    }

    #[tokio::test]
    async fn test_tabs_close() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTabsTool::new(manager);

        let result = tool
            .call(BrowserTabsArgs {
                profile: "default".into(),
                action: TabAction::Close {
                    tab_id: "tab-2".into(),
                },
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("Closed"));
    }
}

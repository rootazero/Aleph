// Browser tabs tool — list, switch, or close browser tabs.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
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

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        match args.action {
            TabAction::List => Ok(BrowserTabsOutput {
                success: true,
                tabs: Some(vec![TabInfo {
                    id: "tab-1".into(),
                    title: "Placeholder Tab".into(),
                    url: "about:blank".into(),
                    active: true,
                }]),
                message: Some(format!("Listed tabs in profile '{}'", args.profile)),
            }),
            TabAction::Switch { tab_id } => Ok(BrowserTabsOutput {
                success: true,
                tabs: None,
                message: Some(format!(
                    "Switched to tab '{}' in profile '{}'",
                    tab_id, args.profile
                )),
            }),
            TabAction::Close { tab_id } => Ok(BrowserTabsOutput {
                success: true,
                tabs: None,
                message: Some(format!(
                    "Closed tab '{}' in profile '{}'",
                    tab_id, args.profile
                )),
            }),
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

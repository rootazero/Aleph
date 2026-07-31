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

/// Arguments for the `browser_tabs` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTabsArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Tab action to perform.
    pub action: TabAction,
}

/// Output from the `browser_tabs` tool.
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
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
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
        let backend = match super::make_backend(&self.manager, &args.profile) {
            Ok(b) => b,
            Err(e) => {
                return Ok(BrowserTabsOutput {
                    success: false,
                    tabs: None,
                    message: Some(e.to_string()),
                });
            }
        };

        match args.action {
            TabAction::List => match backend.list_tabs().await {
                Ok(tabs_text) => {
                    // `list` is a content read like any other: tab URLs are
                    // site-controlled and can embed credentials (session ids,
                    // OAuth codes in query strings), so route the raw listing
                    // through the shared egress chokepoint — size budget,
                    // credential redaction, injection-boundary fencing —
                    // before parsing. Fence / truncation-marker lines don't
                    // match the "N: URL" shape and are skipped by the parser.
                    let tabs_text = super::redact_and_wrap(&self.manager, &tabs_text);
                    // Parse "N: URL [selected]" or "Tab N: URL" lines into TabInfo structs.
                    let mut tab_infos: Vec<TabInfo> = tabs_text
                        .lines()
                        .filter_map(|line| {
                            let line = line.trim();
                            // Chrome DevTools MCP: "N: URL [selected]"
                            if let Some(colon_pos) = line.find(": ") {
                                let id_str = line.get(..colon_pos)?.trim();
                                if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty()
                                {
                                    let rest = line.get(colon_pos + 2..)?;
                                    let url = if let Some(bracket) = rest.rfind(" [") {
                                        rest.get(..bracket).unwrap_or(rest)
                                    } else {
                                        rest
                                    };
                                    return Some(TabInfo {
                                        id: id_str.to_string(),
                                        title: String::new(),
                                        url: url.to_string(),
                                        active: false,
                                    });
                                }
                            }
                            // Playwright CLI: "Tab N: URL"
                            if let Some(rest) = line.strip_prefix("Tab ") {
                                if let Some(colon_pos) = rest.find(": ") {
                                    let id_str = rest.get(..colon_pos)?.trim();
                                    let url = rest.get(colon_pos + 2..)?;
                                    return Some(TabInfo {
                                        id: id_str.to_string(),
                                        title: String::new(),
                                        url: url.to_string(),
                                        active: false,
                                    });
                                }
                            }
                            None
                        })
                        .collect();
                    // The active/most-recent tab is the LAST entry — the
                    // convention every other tool here relies on (mod.rs
                    // parse_active_tab_id uses .next_back(); chrome_mcp_backend
                    // treats the newest tab as last). Marking i==0 active would
                    // report a different tab than click/type/snapshot operate on.
                    if let Some(last) = tab_infos.last_mut() {
                        last.active = true;
                    }
                    Ok(BrowserTabsOutput {
                        success: true,
                        tabs: Some(tab_infos),
                        message: Some(format!("Listed tabs in profile '{}'", args.profile)),
                    })
                }
                Err(e) => Ok(BrowserTabsOutput {
                    success: false,
                    tabs: None,
                    message: Some(format!("List tabs failed: {e}")),
                }),
            },
            TabAction::Switch { tab_id } => match backend.switch_tab(&tab_id).await {
                Ok(()) => Ok(BrowserTabsOutput {
                    success: true,
                    tabs: None,
                    message: Some(format!(
                        "Switched to tab '{}' in profile '{}'",
                        tab_id, args.profile
                    )),
                }),
                Err(e) => Ok(BrowserTabsOutput {
                    success: false,
                    tabs: None,
                    message: Some(format!("Switch tab failed: {e}")),
                }),
            },
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
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

        // Switch now routes to backend.switch_tab() — without a running browser
        // the call fails gracefully rather than lying about success.
        assert!(!result.success);
        assert!(result.message.is_some());
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

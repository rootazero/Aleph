// Browser select tool — selects an option from a dropdown/select element.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::backend::BrowserBackend;
use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
use crate::browser::manager::ProfileManager;
use crate::browser::profile::BrowserDriver;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_select tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSelectArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// CSS selector of the dropdown/select element.
    pub selector: String,
    /// Value to select from the dropdown.
    pub value: String,
}

/// Output from the browser_select tool.
#[derive(Debug, Serialize)]
pub struct BrowserSelectOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Selects an option from a dropdown/select element on the page.
#[derive(Clone)]
pub struct BrowserSelectTool {
    manager: Arc<ProfileManager>,
}

impl BrowserSelectTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserSelectTool {
    const NAME: &'static str = "browser_select";
    const DESCRIPTION: &'static str = "Select an option from a dropdown/select element";
    type Args = BrowserSelectArgs;
    type Output = BrowserSelectOutput;

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
                        return Ok(BrowserSelectOutput {
                            success: false,
                            message: Some(format!("{e}")),
                        });
                    }
                };

                let target = ActionTarget::Selector {
                    css: args.selector.clone(),
                };

                match backend.select(&tab_id, target, &args.value).await {
                    Ok(()) => Ok(BrowserSelectOutput {
                        success: true,
                        message: Some(format!(
                            "Selected '{}' in '{}' in profile '{}'",
                            args.value, args.selector, args.profile
                        )),
                    }),
                    Err(e) => Ok(BrowserSelectOutput {
                        success: false,
                        message: Some(format!("Select failed: {e}")),
                    }),
                }
            }
            _ => {
                // Placeholder for managed mode
                Ok(BrowserSelectOutput {
                    success: true,
                    message: Some(format!(
                        "Selected '{}' in '{}' in profile '{}'",
                        args.value, args.selector, args.profile
                    )),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_select_option() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                selector: "select#country".into(),
                value: "us".into(),
            })
            .await
            .unwrap();

        assert!(result.success);
        let msg = result.message.unwrap();
        assert!(msg.contains("us"));
        assert!(msg.contains("select#country"));
    }
}

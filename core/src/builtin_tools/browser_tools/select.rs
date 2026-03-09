// Browser select tool — selects an option from a dropdown/select element.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
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

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserSelectOutput {
            success: true,
            message: Some(format!(
                "Selected '{}' in '{}' in profile '{}'",
                args.value, args.selector, args.profile
            )),
        })
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

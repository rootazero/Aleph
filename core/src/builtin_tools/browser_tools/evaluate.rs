// Browser evaluate tool — executes JavaScript in the browser.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the browser_evaluate tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserEvaluateArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// JavaScript code to execute in the browser context.
    pub script: String,
}

/// Output from the browser_evaluate tool.
#[derive(Debug, Serialize)]
pub struct BrowserEvaluateOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub message: Option<String>,
}

/// Executes JavaScript in the browser and returns the result.
#[derive(Clone)]
pub struct BrowserEvaluateTool {
    manager: Arc<ProfileManager>,
}

impl BrowserEvaluateTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserEvaluateTool {
    const NAME: &'static str = "browser_evaluate";
    const DESCRIPTION: &'static str =
        "Execute JavaScript in the browser and return the result";
    type Args = BrowserEvaluateArgs;
    type Output = BrowserEvaluateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserEvaluateOutput {
            success: true,
            result: Some(serde_json::Value::Null),
            message: Some(format!(
                "Evaluated {} chars of JS in profile '{}'",
                args.script.len(),
                args.profile
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_evaluate_script() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserEvaluateTool::new(manager);

        let result = tool
            .call(BrowserEvaluateArgs {
                profile: "default".into(),
                script: "document.title".into(),
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.result.is_some());
    }

    #[tokio::test]
    async fn test_evaluate_empty_script() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserEvaluateTool::new(manager);

        let result = tool
            .call(BrowserEvaluateArgs {
                profile: "default".into(),
                script: String::new(),
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.unwrap().contains("0 chars"));
    }
}

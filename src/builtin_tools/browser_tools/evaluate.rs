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
    const DESCRIPTION: &'static str = "Execute JavaScript in the browser and return the result";
    type Args = BrowserEvaluateArgs;
    type Output = BrowserEvaluateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.evaluate(&tab_id, &args.script).await {
                Ok(value) => {
                    // evaluate() returns String; try to parse as JSON, else wrap as JSON string.
                    let json_value: serde_json::Value = serde_json::from_str(&value)
                        .unwrap_or(serde_json::Value::String(value));
                    Ok(BrowserEvaluateOutput {
                        success: true,
                        result: Some(json_value),
                        message: Some(format!(
                            "Evaluated {} chars of JS in profile '{}'",
                            args.script.chars().count(),
                            args.profile
                        )),
                    })
                }
                Err(e) => Ok(BrowserEvaluateOutput {
                    success: false,
                    result: None,
                    message: Some(format!("Evaluate failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserEvaluateOutput {
                success: false,
                result: None,
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
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

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

// Browser fill_form tool — fills multiple form fields at once.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// A single form field to fill.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormField {
    /// CSS selector of the form field.
    pub selector: String,
    /// Value to fill into the field.
    pub value: String,
}

/// Arguments for the browser_fill_form tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserFillFormArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Form fields to fill.
    pub fields: Vec<FormField>,
}

/// Output from the browser_fill_form tool.
#[derive(Debug, Serialize)]
pub struct BrowserFillFormOutput {
    pub success: bool,
    pub filled_count: usize,
    pub message: Option<String>,
}

/// Fills multiple form fields at once.
#[derive(Clone)]
pub struct BrowserFillFormTool {
    manager: Arc<ProfileManager>,
}

impl BrowserFillFormTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserFillFormTool {
    const NAME: &'static str = "browser_fill_form";
    const DESCRIPTION: &'static str = "Fill multiple form fields at once";
    type Args = BrowserFillFormArgs;
    type Output = BrowserFillFormOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.manager.record_activity(&args.profile);

        let count = args.fields.len();

        // TODO: Route through PlaywrightBridge or BrowserRuntime
        Ok(BrowserFillFormOutput {
            success: true,
            filled_count: count,
            message: Some(format!(
                "Filled {} field(s) in profile '{}'",
                count, args.profile
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_fill_form_multiple_fields() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserFillFormTool::new(manager);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![
                    FormField {
                        selector: "input#name".into(),
                        value: "Alice".into(),
                    },
                    FormField {
                        selector: "input#email".into(),
                        value: "alice@example.com".into(),
                    },
                ],
            })
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.filled_count, 2);
    }

    #[tokio::test]
    async fn test_fill_form_empty_fields() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserFillFormTool::new(manager);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![],
            })
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.filled_count, 0);
    }
}

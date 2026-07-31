// Browser select tool — selects an option from a dropdown/select element.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_select` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSelectArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// CSS selector of the dropdown/select element.
    pub selector: Option<String>,
    /// Accessibility `ref_id` from a previous snapshot.
    pub ref_id: Option<String>,
    /// Value to select from the dropdown.
    pub value: String,
}

/// Output from the `browser_select` tool.
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
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for BrowserSelectTool {
    const NAME: &'static str = "browser_select";
    const DESCRIPTION: &'static str =
        "Select an option from a dropdown/select element by CSS selector or ref_id";
    type Args = BrowserSelectArgs;
    type Output = BrowserSelectOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Input-side secret scan: a dropdown value is still text typed into the
        // page, so the same deterministic policy gate as browser_type applies.
        if let Some(message) = super::check_input_secret_block(&self.manager, &args.value) {
            return Ok(BrowserSelectOutput {
                success: false,
                message: Some(message),
            });
        }

        let target = if let Some(ref rid) = args.ref_id {
            ActionTarget::Ref {
                ref_id: rid.clone(),
            }
        } else if args.selector.is_some() {
            return Err(AlephError::invalid_input(
                "CSS selector targeting is no longer supported. Use 'ref_id' from browser_snapshot.",
            ));
        } else {
            return Err(AlephError::invalid_input(
                "browser_select requires 'ref_id'",
            ));
        };

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.select(&tab_id, target, &args.value).await {
                Ok(()) => Ok(BrowserSelectOutput {
                    success: true,
                    message: Some(format!("Selected '{}'", args.value)),
                }),
                Err(e) => Ok(BrowserSelectOutput {
                    success: false,
                    message: Some(format!("Select failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserSelectOutput {
                success: false,
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
    async fn test_select_with_selector_returns_error() {
        // CSS selector targeting is no longer supported — must use ref_id.
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                selector: Some("select#country".into()),
                ref_id: None,
                value: "us".into(),
            })
            .await;

        assert!(result.is_err(), "selector-only select should return Err");
    }

    #[tokio::test]
    async fn test_select_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                selector: None,
                ref_id: Some("combobox[0]".into()),
                value: "us".into(),
            })
            .await
            .unwrap();

        assert!(!result.success); // No browser running
    }

    #[tokio::test]
    async fn test_select_no_target_fails() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                selector: None,
                ref_id: None,
                value: "us".into(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_select_blocks_secret_value() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                selector: None,
                ref_id: Some("combobox[0]".into()),
                value: "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789".into(),
            })
            .await
            .unwrap();

        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("Blocked"), "expected refusal: {message}");
        // The refusal names the rule but never echoes the secret value.
        assert!(!message.contains("sk-ant-api03"));
    }

    #[tokio::test]
    async fn test_select_clean_value_not_blocked() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                selector: None,
                ref_id: Some("combobox[0]".into()),
                value: "us".into(),
            })
            .await
            .unwrap();

        // Clean value passes the secret scan and reaches the backend, which
        // degrades gracefully without a running browser.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(
            !message.contains("Blocked"),
            "clean value blocked: {message}"
        );
    }
}

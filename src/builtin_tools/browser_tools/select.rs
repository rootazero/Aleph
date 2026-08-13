// Browser select tool — selects an option from a dropdown/select element.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_select` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSelectArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Accessibility `ref_id` of the dropdown/select element, from a previous
    /// snapshot.
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
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserSelectTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate dropdown selection behind the approval policy. The pick is page-
    /// state-changing (and frequently page-navigating), so `Ask` is the
    /// matching default. With no policy wired the tool behaves exactly as
    /// before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserSelectTool {
    const NAME: &'static str = "browser_select";
    const DESCRIPTION: &'static str =
        "Select an option from a dropdown/select element, addressed by the ref_id a \
         browser_snapshot reported for it";
    type Args = BrowserSelectArgs;
    type Output = BrowserSelectOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate first: a malformed call is a model mistake and must not
        // consume a user approval or touch the page. It degrades to
        // success:false with the contract spelled out, never a hard `Err` —
        // the shape exec.rs and wait_for.rs already state in prose.
        let Some(ref ref_id) = args.ref_id else {
            return Ok(BrowserSelectOutput {
                success: false,
                message: Some(
                    "browser_select requires 'ref_id': call browser_snapshot and pass the \
                     ref_id it reports for the dropdown."
                        .into(),
                ),
            });
        };
        let target = ActionTarget::Ref {
            ref_id: ref_id.clone(),
        };

        // Input-side secret scan: a dropdown value is still text typed into the
        // page, so the same deterministic policy gate as browser_type applies.
        if let Some(message) = super::check_input_secret_block(&self.manager, &args.value) {
            return Ok(BrowserSelectOutput {
                success: false,
                message: Some(message),
            });
        }

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserSelect,
            "select",
            &args.value,
        )
        .await
        {
            return Ok(BrowserSelectOutput {
                success: false,
                message: Some(message),
            });
        }

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.select(&tab_id, target, &args.value).await {
                Ok(()) => Ok(BrowserSelectOutput {
                    success: true,
                    message: Some(format!("Selected '{}'", args.value)),
                }),
                Err(e) => Ok(BrowserSelectOutput {
                    success: false,
                    message: Some(format!(
                        "Select failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserSelectOutput {
                success: false,
                message: Some(super::backend_error_text(&self.manager, &e)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserSystemConfig;

    #[tokio::test]
    async fn test_select_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                ref_id: Some("combobox[0]".into()),
                value: "us".into(),
            })
            .await
            .unwrap();

        // ref_id is the supported targeting method and still reaches the
        // backend, which fails gracefully with no browser running.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(!message.contains("requires 'ref_id'"), "got: {message}");
    }

    #[tokio::test]
    async fn test_select_no_target_is_graceful_failure() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                ref_id: None,
                value: "us".into(),
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("browser_snapshot")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_select_malformed_call_does_not_consume_approval() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserSelect, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserSelectTool::new(manager).with_approval_policy(policy);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
                ref_id: None,
                value: "us".into(),
            })
            .await
            .unwrap();

        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("browser_snapshot"), "got: {message}");
        assert!(!message.contains("denied"), "got: {message}");
    }

    #[tokio::test]
    async fn test_select_blocks_secret_value() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserSelectTool::new(manager);

        let result = tool
            .call(BrowserSelectArgs {
                profile: "default".into(),
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

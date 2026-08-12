// Browser dialog tool — responds to native JS dialogs (alert/confirm/prompt).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// How to respond to a pending native dialog.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DialogAction {
    /// Accept the dialog (OK on alert/confirm, submit on prompt).
    Accept,
    /// Dismiss the dialog (Cancel).
    Dismiss,
}

/// Arguments for the `browser_dialog` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserDialogArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Whether to accept or dismiss the dialog.
    pub action: DialogAction,
    /// Text to fill into a prompt dialog before accepting (ignored otherwise).
    #[serde(default)]
    pub prompt_text: Option<String>,
}

/// Output from the `browser_dialog` tool.
#[derive(Debug, Serialize)]
pub struct BrowserDialogOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Responds to native browser dialogs (alert, confirm, prompt, beforeunload).
#[derive(Clone)]
pub struct BrowserDialogTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserDialogTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate dialog dismissal / acceptance behind the approval policy. A `prompt`
    /// dialog can submit text into the page; `Ask` is the matching default. With
    /// no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserDialogTool {
    const NAME: &'static str = "browser_dialog";
    const DESCRIPTION: &'static str =
        "Respond to a native browser dialog (alert/confirm/prompt) by accepting or dismissing it";
    type Args = BrowserDialogArgs;
    type Output = BrowserDialogOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Input-side secret scan: prompt-dialog text is typed into the page,
        // so the same deterministic policy gate as browser_type applies.
        if let Some(ref text) = args.prompt_text {
            if let Some(message) = super::check_input_secret_block(&self.manager, text) {
                return Ok(BrowserDialogOutput {
                    success: false,
                    message: Some(message),
                });
            }
        }

        let action = match args.action {
            DialogAction::Accept => "accept",
            DialogAction::Dismiss => "dismiss",
        };

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserDialog,
            "dialog",
            &format!("{action}{}", args.prompt_text.as_deref().unwrap_or("")),
        )
        .await
        {
            return Ok(BrowserDialogOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                match backend
                    .handle_dialog(&tab_id, action, args.prompt_text.as_deref())
                    .await
                {
                    Ok(()) => Ok(BrowserDialogOutput {
                        success: true,
                        message: Some(format!("Dialog {action}ed in profile '{}'", args.profile)),
                    }),
                    Err(e) => Ok(BrowserDialogOutput {
                        success: false,
                        message: Some(format!(
                            "Dialog handling failed: {}",
                            super::backend_error_text(&self.manager, &e)
                        )),
                    }),
                }
            }
            Err(e) => Ok(BrowserDialogOutput {
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
    async fn test_dialog_accept_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserDialogTool::new(manager);
        let result = tool
            .call(BrowserDialogArgs {
                profile: "default".into(),
                action: DialogAction::Accept,
                prompt_text: None,
            })
            .await
            .unwrap();
        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_dialog_blocks_secret_prompt_text() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserDialogTool::new(manager);
        let result = tool
            .call(BrowserDialogArgs {
                profile: "default".into(),
                action: DialogAction::Accept,
                prompt_text: Some("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789".into()),
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
    async fn test_dialog_clean_prompt_text_not_blocked() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserDialogTool::new(manager);
        let result = tool
            .call(BrowserDialogArgs {
                profile: "default".into(),
                action: DialogAction::Accept,
                prompt_text: Some("yes, I confirm".into()),
            })
            .await
            .unwrap();

        // Clean prompt text passes the secret scan and reaches the backend,
        // which degrades gracefully without a running browser.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(
            !message.contains("Blocked"),
            "clean prompt blocked: {message}"
        );
    }
}

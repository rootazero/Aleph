// Browser upload tool — attach local files to a file input.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_upload` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserUploadArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Absolute paths of the local files to upload.
    pub paths: Vec<String>,
    /// Accessibility `ref_id` of the `<input type=file>` element. Required for the
    /// existing-session profile; optional for the managed profile, which targets
    /// the page's file chooser directly.
    pub ref_id: Option<String>,
}

/// Output from the `browser_upload` tool.
#[derive(Debug, Serialize)]
pub struct BrowserUploadOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Attaches one or more local files to a file input on the page.
#[derive(Clone)]
pub struct BrowserUploadTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserUploadTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate file uploads behind the approval policy — local files egressing to a
    /// host the page chooses is privacy-sensitive. `Ask` is the matching default.
    /// With no policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserUploadTool {
    const NAME: &'static str = "browser_upload";
    const DESCRIPTION: &'static str =
        "Attach one or more local files to a file input (provide ref_id from a snapshot for the \
         existing-session profile)";
    type Args = BrowserUploadArgs;
    type Output = BrowserUploadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if args.paths.is_empty() {
            return Err(AlephError::invalid_input(
                "browser_upload requires at least one file path in `paths`",
            ));
        }
        let target = args.ref_id.as_ref().map(|rid| ActionTarget::Ref {
            ref_id: rid.clone(),
        });

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserUpload,
            "upload",
            &format!("{} file(s): {}", args.paths.len(), args.paths.join(", ")),
        )
        .await
        {
            return Ok(BrowserUploadOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.upload(&tab_id, target, &args.paths).await {
                Ok(()) => Ok(BrowserUploadOutput {
                    success: true,
                    message: Some(format!(
                        "Uploaded {} file(s) in profile '{}'",
                        args.paths.len(),
                        args.profile
                    )),
                }),
                Err(e) => Ok(BrowserUploadOutput {
                    success: false,
                    message: Some(format!("Upload failed: {e}")),
                }),
            },
            Err(e) => Ok(BrowserUploadOutput {
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
    async fn test_upload_empty_paths_is_input_error() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserUploadTool::new(manager);
        let result = tool
            .call(BrowserUploadArgs {
                profile: "default".into(),
                paths: vec![],
                ref_id: Some("e1".into()),
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_upload_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserUploadTool::new(manager);
        let result = tool
            .call(BrowserUploadArgs {
                profile: "default".into(),
                paths: vec!["/tmp/a.txt".into()],
                ref_id: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

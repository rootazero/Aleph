// Browser upload tool — attach local files to a file input.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::path::Path;

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_upload` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserUploadArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Absolute paths of the local files to upload. Subject to the same
    /// protected-location denylist as the file tools.
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
        // Validate before the approval check: a malformed call is a model
        // mistake and must not consume a user approval or touch the page. It
        // degrades to success:false with the contract spelled out rather than a
        // hard `Err`, matching the rest of the family.
        if args.paths.is_empty() {
            return Ok(BrowserUploadOutput {
                success: false,
                message: Some("browser_upload requires at least one file path in `paths`".into()),
            });
        }

        // Every path goes through the FILE layer's sole path resolver — the
        // same one `file_ops` uses — so the credential denylist and the
        // operator's `[sandbox] deny_read_globs` floor bind this reader too.
        // A file has two faces that can read it; uploading `~/.aws/credentials`
        // into a page's form was the second face, and `deny_read_globs` bound
        // only the first. `None` for the output-dir override means a relative
        // path is refused rather than resolved somewhere unexpected — this
        // tool's contract is absolute paths.
        let denied = get_denied_paths();
        let mut resolved = Vec::with_capacity(args.paths.len());
        for path in &args.paths {
            match check_and_resolve_path(Path::new(path), &denied, None) {
                Ok(p) => resolved.push(p.display().to_string()),
                Err(e) => {
                    return Ok(BrowserUploadOutput {
                        success: false,
                        message: Some(e.to_string()),
                    });
                }
            }
        }

        let target = args.ref_id.as_ref().map(|rid| ActionTarget::Ref {
            ref_id: rid.clone(),
        });

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserUpload,
            "upload",
            &format!("{} file(s): {}", resolved.len(), resolved.join(", ")),
        )
        .await
        {
            return Ok(BrowserUploadOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            // Upload the RESOLVED paths: they are what the deny check ran
            // against, so handing the raw spellings to the backend would let a
            // symlink or an `FsScope` rebase point somewhere else.
            Ok((backend, tab_id)) => match backend.upload(&tab_id, target, &resolved).await {
                Ok(()) => Ok(BrowserUploadOutput {
                    success: true,
                    message: Some(format!(
                        "Uploaded {} file(s) in profile '{}'",
                        resolved.len(),
                        args.profile
                    )),
                }),
                Err(e) => Ok(BrowserUploadOutput {
                    success: false,
                    message: Some(format!(
                        "Upload failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserUploadOutput {
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
    async fn test_upload_empty_paths_is_graceful_failure() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserUploadTool::new(manager);
        let result = tool
            .call(BrowserUploadArgs {
                profile: "default".into(),
                paths: vec![],
                ref_id: Some("e1".into()),
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("at least one file path")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_upload_refuses_a_protected_path_before_the_gate() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        // Allow uploads outright: the deny check must still refuse, proving it
        // runs before the approval gate and before any backend lookup.
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserUpload, DefaultDecision::Allow);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserUploadTool::new(manager).with_approval_policy(policy);
        let result = tool
            .call(BrowserUploadArgs {
                profile: "default".into(),
                paths: vec!["~/.aws/credentials".into()],
                ref_id: Some("e1".into()),
            })
            .await
            .unwrap();
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(
            message.contains("protected location"),
            "expected a deny verdict, got: {message}"
        );
    }

    #[tokio::test]
    async fn test_upload_refuses_a_relative_path() {
        // The contract is absolute paths; a relative one has no anchor here and
        // must be refused rather than resolved somewhere unexpected.
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserUploadTool::new(manager);
        let result = tool
            .call(BrowserUploadArgs {
                profile: "default".into(),
                paths: vec!["relative/file.txt".into()],
                ref_id: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.is_some());
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
        // An allowed path passes the deny check and reaches the backend, which
        // fails gracefully with no browser running.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(!message.contains("protected location"), "got: {message}");
    }
}

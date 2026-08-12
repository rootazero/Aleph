// Browser type tool — types text into an element on the page.
//
// Named `type_text.rs` because `type` is a Rust keyword.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_type` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTypeArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Text to type into the element.
    pub text: String,
    /// Accessibility `ref_id` of the element to type into, from a previous
    /// `browser_snapshot`.
    pub ref_id: Option<String>,
}

/// Output from the `browser_type` tool.
#[derive(Debug, Serialize)]
pub struct BrowserTypeOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Types text into an element on the page, identified by its snapshot `ref_id`.
#[derive(Clone)]
pub struct BrowserTypeTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserTypeTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate typing behind a user-defined approval policy. With no policy wired
    /// the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserTypeTool {
    const NAME: &'static str = "browser_type";
    const DESCRIPTION: &'static str =
        "Type text into an element on the page, addressed by the ref_id a browser_snapshot \
         reported for it";
    type Args = BrowserTypeArgs;
    type Output = BrowserTypeOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // No-target typing is gone. It used to lower to `ActionTarget::Ref {
        // ref_id: "focused" }`, and NOTHING under `src/browser/` ever read that
        // literal — both backends treat a `Ref` as a snapshot handle, so the
        // promised "type into whatever holds focus" behaviour reached the page
        // as a lookup for an element named `focused` and failed. A channel with
        // zero consumers is CUT, not reconnected (R10); the message names the
        // one targeting method that does work.
        let Some(ref ref_id) = args.ref_id else {
            return Ok(BrowserTypeOutput {
                success: false,
                message: Some(
                    "browser_type requires 'ref_id': call browser_snapshot and pass the ref_id \
                     it reports for the field you want to type into."
                        .into(),
                ),
            });
        };
        let target = ActionTarget::Ref {
            ref_id: ref_id.clone(),
        };

        // Input-side secret scan runs BEFORE the approval check: deterministic
        // policy beats interactive approval (and is cheaper).
        if let Some(message) = super::check_input_secret_block(&self.manager, &args.text) {
            return Ok(BrowserTypeOutput {
                success: false,
                message: Some(message),
            });
        }

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserType,
            "type",
            &args.text,
        )
        .await
        {
            return Ok(BrowserTypeOutput {
                success: false,
                message: Some(message),
            });
        }

        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.type_text(&tab_id, target, &args.text).await {
                Ok(()) => Ok(BrowserTypeOutput {
                    success: true,
                    message: Some(format!(
                        "Typed {} chars in profile '{}'",
                        args.text.chars().count(),
                        args.profile
                    )),
                }),
                Err(e) => Ok(BrowserTypeOutput {
                    success: false,
                    message: Some(format!(
                        "Type failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserTypeOutput {
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
    async fn test_type_without_ref_id_names_the_supported_targeting() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTypeTool::new(manager);

        let result = tool
            .call(BrowserTypeArgs {
                profile: "default".into(),
                text: "test input".into(),
                ref_id: None,
            })
            .await
            .unwrap();

        // The old no-target path claimed to type into the focused element and
        // silently did nothing of the sort; it now says what to do instead.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("ref_id"), "got: {message}");
        assert!(message.contains("browser_snapshot"), "got: {message}");
    }

    #[tokio::test]
    async fn test_type_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserTypeTool::new(manager);

        let result = tool
            .call(BrowserTypeArgs {
                profile: "default".into(),
                text: "ref text".into(),
                ref_id: Some("ref-10".into()),
            })
            .await
            .unwrap();

        // ref_id is the supported targeting method and still reaches the
        // backend, which degrades gracefully without a running browser.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(!message.contains("requires 'ref_id'"), "got: {message}");
    }

    #[tokio::test]
    async fn test_type_malformed_call_does_not_consume_approval() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserType, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserTypeTool::new(manager).with_approval_policy(policy);

        let result = tool
            .call(BrowserTypeArgs {
                profile: "default".into(),
                text: "hello".into(),
                ref_id: None,
            })
            .await
            .unwrap();

        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("ref_id"), "got: {message}");
        assert!(!message.contains("denied"), "got: {message}");
    }
}

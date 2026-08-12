// Browser drag tool — drag-and-drop between two snapshot elements.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_drag` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserDragArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Accessibility `ref_id` of the source element to drag (from a snapshot).
    pub from_ref: String,
    /// Accessibility `ref_id` of the target element to drop onto (from a snapshot).
    pub to_ref: String,
}

/// Output from the `browser_drag` tool.
#[derive(Debug, Serialize)]
pub struct BrowserDragOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Drags one element onto another (reordering lists, sliders, drag-and-drop uploads).
#[derive(Clone)]
pub struct BrowserDragTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserDragTool {
    pub const fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate drag-and-drop behind the approval policy. Drag frequently triggers
    /// page navigation / submit / upload; `Ask` is the matching default. With no
    /// policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserDragTool {
    const NAME: &'static str = "browser_drag";
    const DESCRIPTION: &'static str =
        "Drag one element and drop it onto another, using accessibility ref_ids from a snapshot";
    type Args = BrowserDragArgs;
    type Output = BrowserDragOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let from = ActionTarget::Ref {
            ref_id: args.from_ref.clone(),
        };
        let to = ActionTarget::Ref {
            ref_id: args.to_ref.clone(),
        };

        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserDrag,
            "drag",
            &format!("{} -> {}", args.from_ref, args.to_ref),
        )
        .await
        {
            return Ok(BrowserDragOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => match backend.drag(&tab_id, from, to).await {
                Ok(()) => Ok(BrowserDragOutput {
                    success: true,
                    message: Some(format!(
                        "Dragged '{}' onto '{}' in profile '{}'",
                        args.from_ref, args.to_ref, args.profile
                    )),
                }),
                Err(e) => Ok(BrowserDragOutput {
                    success: false,
                    message: Some(format!(
                        "Drag failed: {}",
                        super::backend_error_text(&self.manager, &e)
                    )),
                }),
            },
            Err(e) => Ok(BrowserDragOutput {
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
    async fn test_drag_degrades_without_browser() {
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserDragTool::new(manager);
        let result = tool
            .call(BrowserDragArgs {
                profile: "default".into(),
                from_ref: "e1".into(),
                to_ref: "e2".into(),
            })
            .await
            .unwrap();
        // Without a running browser, the tool degrades gracefully (no panic).
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

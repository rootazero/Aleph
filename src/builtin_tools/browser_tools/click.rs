// Browser click tool — clicks an element on the page.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `browser_click` tool.
///
/// At least one targeting method must be provided: `ref_id` or coordinates (`x`/`y`).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserClickArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Accessibility `ref_id` from a previous snapshot.
    pub ref_id: Option<String>,
    /// X coordinate for coordinate-based clicking.
    pub x: Option<f64>,
    /// Y coordinate for coordinate-based clicking.
    pub y: Option<f64>,
    /// Double-click instead of single-click (requires `ref_id` targeting).
    #[serde(default)]
    pub double: bool,
}

/// Output from the `browser_click` tool.
#[derive(Debug, Serialize)]
pub struct BrowserClickOutput {
    pub success: bool,
    pub message: Option<String>,
}

/// Clicks an element on the page by `ref_id` or viewport coordinates.
#[derive(Clone)]
pub struct BrowserClickTool {
    manager: Arc<ProfileManager>,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserClickTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate clicks behind a user-defined approval policy. With no policy wired
    /// the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

/// Lower the model's targeting arguments to an [`ActionTarget`].
///
/// Returns the contract as a message rather than an error: a malformed request
/// degrades to `success:false` with the contract spelled out, never a hard
/// `Err` — the convention `exec.rs` and `wait_for.rs` already state in prose,
/// and previously the one place this family disagreed with itself (click and
/// select hard-errored while type and fill_form did not, so the same mistake
/// reached the model two different ways).
fn resolve_target(args: &BrowserClickArgs) -> std::result::Result<ActionTarget, String> {
    if let Some(ref rid) = args.ref_id {
        Ok(ActionTarget::Ref {
            ref_id: rid.clone(),
        })
    } else if let (Some(x), Some(y)) = (args.x, args.y) {
        // `double` is a ref-only capability, and saying so here is what makes
        // the claim true. Neither driver has a coordinate double-click —
        // `BrowserBackend::dblclick` states the constraint and both
        // implementations reject anything but a ref — so a coordinate
        // `double: true` was always going to fail, but only AFTER the approval
        // gate had spent a user approval and the tab had been resolved. The
        // arg doc and the tool DESCRIPTION both already advertised the
        // restriction; nothing enforced it.
        if args.double {
            return Err(
                "browser_click double=true requires ref_id targeting: neither browser driver \
                 has a coordinate-based double-click. Call browser_snapshot and pass the \
                 ref_id it reports."
                    .into(),
            );
        }
        Ok(ActionTarget::Coordinates { x, y })
    } else {
        Err(
            "browser_click requires at least one targeting method: ref_id (from \
             browser_snapshot) or x/y coordinates"
                .into(),
        )
    }
}

#[async_trait]
impl AlephTool for BrowserClickTool {
    const NAME: &'static str = "browser_click";
    const DESCRIPTION: &'static str =
        "Click an element on the page by accessibility ref_id or coordinates; \
         set double=true for a double-click (ref_id only)";
    type Args = BrowserClickArgs;
    type Output = BrowserClickOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Validate before the approval check: a malformed call is a model
        // mistake and must not consume a user approval or touch the page.
        let target = match resolve_target(&args) {
            Ok(t) => t,
            Err(message) => {
                return Ok(BrowserClickOutput {
                    success: false,
                    message: Some(message),
                });
            }
        };
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserClick,
            "click",
            &format!("{target:?}"),
        )
        .await
        {
            return Ok(BrowserClickOutput {
                success: false,
                message: Some(message),
            });
        }
        match super::make_backend_and_tab(&self.manager, &args.profile).await {
            Ok((backend, tab_id)) => {
                let result = if args.double {
                    backend.dblclick(&tab_id, target).await
                } else {
                    backend.click(&tab_id, target).await
                };
                match result {
                    Ok(()) => Ok(BrowserClickOutput {
                        success: true,
                        message: Some(format!(
                            "{} in profile '{}'",
                            if args.double {
                                "Double-clicked"
                            } else {
                                "Clicked"
                            },
                            args.profile
                        )),
                    }),
                    Err(e) => Ok(BrowserClickOutput {
                        success: false,
                        message: Some(format!(
                            "Click failed: {}",
                            super::backend_error_text(&self.manager, &e)
                        )),
                    }),
                }
            }
            Err(e) => Ok(BrowserClickOutput {
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
    async fn test_click_with_coordinates() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: Some(100.0),
                y: Some(200.0),
                double: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_click_no_target_is_graceful_failure() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: None,
                y: None,
                double: false,
            })
            .await
            .unwrap();

        // A malformed call degrades to success:false with the contract spelled
        // out — the same shape type/fill_form/batch/wait_for already use.
        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("ref_id")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_click_malformed_call_does_not_consume_approval() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        // Deny clicks outright: a targetless call must still report the
        // targeting contract, proving validation ran before the gate.
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserClick, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserClickTool::new(manager).with_approval_policy(policy);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: None,
                y: None,
                double: false,
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("ref_id") && !m.contains("denied")),
            "got: {:?}",
            result.message
        );
    }

    #[tokio::test]
    async fn test_double_click_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: Some("e7".into()),
                x: None,
                y: None,
                double: true,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully.
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    /// `double: true` with coordinates is a call neither driver can serve
    /// (`BrowserBackend::dblclick` takes a ref only), so it must be refused
    /// with the contract — and refused before the approval gate, since a
    /// rejected-by-construction call must not spend a user approval.
    #[tokio::test]
    async fn double_click_by_coordinates_is_refused_before_the_approval_gate() {
        use crate::approval::{ActionRequest, ApprovalDecision, ApprovalPolicy};
        use async_trait::async_trait;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingAllow(Arc<AtomicUsize>);
        #[async_trait]
        impl ApprovalPolicy for CountingAllow {
            async fn check(&self, _req: &ActionRequest) -> ApprovalDecision {
                self.0.fetch_add(1, Ordering::SeqCst);
                ApprovalDecision::Allow
            }
            async fn record(&self, _req: &ActionRequest, _dec: &ApprovalDecision) {}
        }

        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let asked = Arc::new(AtomicUsize::new(0));
        let tool = BrowserClickTool::new(manager)
            .with_approval_policy(
                Arc::new(CountingAllow(Arc::clone(&asked))) as Arc<dyn ApprovalPolicy>
            );

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: None,
                x: Some(10.0),
                y: Some(20.0),
                double: true,
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("double=true requires ref_id")),
            "got: {:?}",
            result.message
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "a call the backend cannot serve must not consume an approval"
        );
    }

    #[tokio::test]
    async fn test_click_with_ref_id() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserClickTool::new(manager);

        let result = tool
            .call(BrowserClickArgs {
                profile: "default".into(),
                ref_id: Some("ref-42".into()),
                x: None,
                y: None,
                double: false,
            })
            .await
            .unwrap();

        // Without a running browser, tools degrade gracefully
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

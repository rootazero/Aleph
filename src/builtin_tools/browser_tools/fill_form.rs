// Browser fill_form tool — fills multiple form fields at once.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::browser::manager::ProfileManager;
use crate::browser::types::ActionTarget;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// A single form field to fill.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FormField {
    /// ARIA snapshot `ref_id` of the form field.
    #[serde(default)]
    pub ref_id: Option<String>,
    /// Value to fill into the field.
    pub value: String,
}

/// Arguments for the `browser_fill_form` tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BrowserFillFormArgs {
    /// Browser profile name (default: "default").
    #[serde(default = "crate::builtin_tools::browser_tools::default_profile")]
    pub profile: String,
    /// Form fields to fill.
    pub fields: Vec<FormField>,
}

/// Output from the `browser_fill_form` tool.
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
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl BrowserFillFormTool {
    pub fn new(manager: Arc<ProfileManager>) -> Self {
        Self {
            manager,
            approval_policy: None,
        }
    }

    /// Gate form filling behind a user-defined approval policy. With no policy
    /// wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for BrowserFillFormTool {
    const NAME: &'static str = "browser_fill_form";
    const DESCRIPTION: &'static str =
        "Fill multiple form fields at once; address each field by the ref_id a \
         browser_snapshot reported for it";
    type Args = BrowserFillFormArgs;
    type Output = BrowserFillFormOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Lower every field to an `ActionTarget` FIRST — before the secret
        // scan, the approval check and the backend lookup. A malformed call is
        // a model mistake and must not consume a user approval or touch the
        // page (the rule `exec.rs::plan_actions` already states); this used to
        // run after both, so a form with one unaddressable field spent the
        // user's approval before discovering it could not be filled.
        // An empty batch is the degenerate case of the same mistake, and it
        // used to be the expensive one: the loop below has nothing to reject,
        // so the call spent a user approval, resolved a tab, and came back
        // `success: true, filled_count: 0` — a no-op reported as a completed
        // fill. `browser_upload` refuses its empty `paths` for this reason.
        if args.fields.is_empty() {
            return Ok(BrowserFillFormOutput {
                success: false,
                filled_count: 0,
                message: Some("browser_fill_form requires at least one field in `fields`".into()),
            });
        }

        let mut targets = Vec::with_capacity(args.fields.len());
        for field in &args.fields {
            let Some(ref ref_id) = field.ref_id else {
                return Ok(BrowserFillFormOutput {
                    success: false,
                    filled_count: 0,
                    message: Some(
                        "each field requires 'ref_id': call browser_snapshot and pass the \
                         ref_id it reports for each input."
                            .into(),
                    ),
                });
            };
            targets.push((
                ActionTarget::Ref {
                    ref_id: ref_id.clone(),
                },
                field.value.clone(),
            ));
        }

        // Input-side secret scan runs BEFORE the approval check: deterministic
        // policy beats interactive approval (and is cheaper). Every field value
        // is scanned — a single credential-shaped value anywhere in the batch
        // refuses the whole fill.
        for field in &args.fields {
            if let Some(message) = super::check_input_secret_block(&self.manager, &field.value) {
                return Ok(BrowserFillFormOutput {
                    success: false,
                    filled_count: 0,
                    message: Some(message),
                });
            }
        }

        let fill_target = format!(
            "{} field(s): {}",
            args.fields.len(),
            args.fields
                .iter()
                .filter_map(|f| f.ref_id.as_deref())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(message) = super::check_browser_approval(
            self.approval_policy.as_ref(),
            ActionType::BrowserFill,
            "fill_form",
            &fill_target,
        )
        .await
        {
            return Ok(BrowserFillFormOutput {
                success: false,
                filled_count: 0,
                message: Some(message),
            });
        }

        let (backend, tab_id) =
            match super::make_backend_and_tab(&self.manager, &args.profile).await {
                Ok(pair) => pair,
                Err(e) => {
                    return Ok(BrowserFillFormOutput {
                        success: false,
                        filled_count: 0,
                        message: Some(super::backend_error_text(&self.manager, &e)),
                    });
                }
            };

        match backend.fill_form(&tab_id, &targets).await {
            Ok(filled) => Ok(BrowserFillFormOutput {
                success: true,
                filled_count: filled,
                message: Some(format!(
                    "Filled {} field(s) in profile '{}'",
                    filled, args.profile
                )),
            }),
            Err(e) => Ok(BrowserFillFormOutput {
                success: false,
                filled_count: 0,
                message: Some(format!(
                    "Fill form failed: {}",
                    super::backend_error_text(&self.manager, &e)
                )),
            }),
        }
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
                        ref_id: Some("e1".into()),
                        value: "Alice".into(),
                    },
                    FormField {
                        ref_id: Some("e2".into()),
                        value: "alice@example.com".into(),
                    },
                ],
            })
            .await
            .unwrap();

        // ref_id targeting reaches the backend, which degrades gracefully
        // without a running browser.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(!message.contains("requires 'ref_id'"), "got: {message}");
    }

    /// An empty `fields` batch is refused rather than reported as a completed
    /// fill of nothing — and refused before the approval gate, which an
    /// `Allow`-counting policy proves was never consulted.
    #[tokio::test]
    async fn an_empty_batch_is_refused_before_the_approval_gate() {
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
        let tool = BrowserFillFormTool::new(manager)
            .with_approval_policy(
                Arc::new(CountingAllow(Arc::clone(&asked))) as Arc<dyn ApprovalPolicy>
            );

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![],
            })
            .await
            .unwrap();

        assert!(!result.success, "an empty fill is not a completed fill");
        assert_eq!(result.filled_count, 0);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("at least one field")),
            "got: {:?}",
            result.message
        );
        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "an empty batch must not consume an approval"
        );
    }

    #[tokio::test]
    async fn test_fill_form_unaddressable_field_does_not_consume_approval() {
        use crate::approval::{ConfigApprovalPolicy, DefaultDecision, PolicyConfig};
        use std::collections::HashMap;
        // Deny fills outright: a field with no ref_id must still report the
        // targeting contract, proving validation ran before the gate.
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserFill, DefaultDecision::Deny);
        let policy = Arc::new(ConfigApprovalPolicy::new(PolicyConfig {
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        }));
        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let tool = BrowserFillFormTool::new(manager).with_approval_policy(policy);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![FormField {
                    ref_id: None,
                    value: "Alice".into(),
                }],
            })
            .await
            .unwrap();

        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(message.contains("ref_id"), "got: {message}");
        assert!(!message.contains("denied"), "got: {message}");
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

        // Without a running browser, tools degrade gracefully
        // Empty fields: get_active_tab fails, returns success: false
        assert!(!result.success);
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_fill_form_blocks_secret_value() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserFillFormTool::new(manager);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![
                    FormField {
                        ref_id: Some("e1".into()),
                        value: "Alice".into(),
                    },
                    FormField {
                        ref_id: Some("e2".into()),
                        value: "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789".into(),
                    },
                ],
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.filled_count, 0);
        let message = result.message.unwrap();
        assert!(message.contains("Blocked"), "expected refusal: {message}");
        // The refusal names the rule but never echoes the secret value.
        assert!(!message.contains("sk-ant-api03"));
    }

    #[tokio::test]
    async fn test_fill_form_clean_values_not_blocked() {
        let config = BrowserSystemConfig::default();
        let manager = Arc::new(ProfileManager::new(config));
        let tool = BrowserFillFormTool::new(manager);

        let result = tool
            .call(BrowserFillFormArgs {
                profile: "default".into(),
                fields: vec![FormField {
                    ref_id: Some("e1".into()),
                    value: "alice@example.com".into(),
                }],
            })
            .await
            .unwrap();

        // Clean input passes the secret scan and reaches the backend, which
        // degrades gracefully without a running browser.
        assert!(!result.success);
        let message = result.message.unwrap();
        assert!(
            !message.contains("Blocked"),
            "clean input blocked: {message}"
        );
    }
}

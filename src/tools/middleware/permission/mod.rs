//! PermissionLayer — SmartFilter classification + ApprovalGate confirmation.
//!
//! Phase 2 Task 7: relocate permission evaluation into the middleware layer.
//! Phase 3 Task 2: backfill the placeholder `SmartFilter` with a concrete
//! resolver (`LayeredPermissionResolver`) that consults the merged
//! (global + per-agent) `ToolPermissionsConfig` — see `resolver.rs`.
//!
//! `ApprovalGate` lives at `src/agent_loop/exec_approval/gate.rs` and is not
//! moved — only the call-site relocates here. To keep the layer unit-testable
//! without wiring full approval UI plumbing, the layer talks to a small
//! `Approver` trait. A blanket `Approver` impl is provided for `ApprovalGate`
//! so real wiring is a one-liner.

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;

use crate::sandbox::exec_approval::gate::{ApprovalGate, ApprovalOutcome};
use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub mod agent_filter;
pub mod resolver;

pub use agent_filter::AgentPermissionFilter;
pub use resolver::LayeredPermissionResolver;

/// Classification outcome for a `(name, input)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Allow the call through without prompting.
    Allow,
    /// Prompt the user for confirmation before proceeding. `reason` is
    /// surfaced to the approver.
    Confirm { reason: String },
    /// Reject unconditionally with the given reason.
    Deny { reason: String },
}

/// Classifies a tool invocation into `Allow` / `Confirm` / `Deny`.
///
/// This is the policy hook for `PermissionLayer`. A production classifier
/// consults the merged two-tier permission table; see
/// [`LayeredPermissionResolver`].
pub trait SmartFilter: Send + Sync {
    fn classify(&self, name: &str, input: &Value) -> Classification;
}

/// Thin abstraction over the approval step so the layer can be tested
/// without spinning up real approval transport. The real `ApprovalGate`
/// implements this via the blanket impl below.
#[async_trait]
pub trait Approver: Send + Sync {
    async fn ask(&self, tool_name: &str, reason: &str) -> ApprovalOutcome;
}

#[async_trait]
impl Approver for ApprovalGate {
    async fn ask(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        self.request_approval_for_tool(tool_name, reason).await
    }
}

pub struct PermissionLayer {
    inner: Arc<dyn ToolService>,
    smart_filter: Arc<ArcSwap<Option<Arc<dyn SmartFilter>>>>,
    approver: Arc<ArcSwap<Option<Arc<dyn Approver>>>>,
}

impl PermissionLayer {
    /// Construct a layer with no filter and no approver attached. This is
    /// the `ToolServiceBuilder` default — `Allow` is inferred for every call
    /// and the inner service runs unguarded, preserving the pass-through
    /// behaviour installed in Task 5.
    pub fn new(inner: Arc<dyn ToolService>) -> Self {
        Self {
            inner,
            smart_filter: Arc::new(ArcSwap::from_pointee(None)),
            approver: Arc::new(ArcSwap::from_pointee(None)),
        }
    }

    /// Construct a layer with a concrete filter + approver wired in.
    pub fn with_policy(
        inner: Arc<dyn ToolService>,
        smart_filter: Arc<dyn SmartFilter>,
        approver: Arc<dyn Approver>,
    ) -> Self {
        Self {
            inner,
            smart_filter: Arc::new(ArcSwap::from_pointee(Some(smart_filter))),
            approver: Arc::new(ArcSwap::from_pointee(Some(approver))),
        }
    }

    /// Swap the active `SmartFilter` atomically. Pass `None` to clear.
    pub fn set_smart_filter(&self, filter: Option<Arc<dyn SmartFilter>>) {
        self.smart_filter.store(Arc::new(filter));
    }

    /// Swap the active `Approver` atomically. Pass `None` to clear.
    pub fn set_approver(&self, approver: Option<Arc<dyn Approver>>) {
        self.approver.store(Arc::new(approver));
    }

    fn classify(&self, name: &str, input: &Value) -> Classification {
        let snapshot = self.smart_filter.load();
        match snapshot.as_ref() {
            Some(filter) => filter.classify(name, input),
            None => Classification::Allow,
        }
    }

    async fn confirm(&self, name: &str, reason: &str) -> Result<(), ToolError> {
        let snapshot = self.approver.load();
        let approver = match snapshot.as_ref() {
            Some(a) => a.clone(),
            None => {
                return Err(ToolError::PermissionDenied {
                    name: name.to_string(),
                    reason: "no approver configured to honour confirm".into(),
                });
            }
        };

        match approver.ask(name, reason).await {
            ApprovalOutcome::Approved => Ok(()),
            ApprovalOutcome::Denied => Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: "user denied approval".into(),
            }),
            ApprovalOutcome::Timeout => Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: "approval request timed out".into(),
            }),
        }
    }
}

#[async_trait]
impl ToolService for PermissionLayer {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        // Exhaustive match — never use `_` so adding a Classification variant
        // forces us to think about the layer's behaviour.
        match self.classify(name, &input) {
            Classification::Allow => {}
            Classification::Confirm { reason } => {
                self.confirm(name, &reason).await?;
            }
            Classification::Deny { reason } => {
                return Err(ToolError::PermissionDenied {
                    name: name.to_string(),
                    reason,
                });
            }
        }
        self.inner.execute(name, input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner.list().await
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.inner.describe(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use crate::session::events::{ToolOutput, ToolOutputMetadata};

    /// Inner service that counts execute() calls and records the last one.
    struct RecordingInner {
        calls: AtomicUsize,
        last: Mutex<Option<(String, Value)>>,
    }

    impl RecordingInner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                last: Mutex::new(None),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ToolService for RecordingInner {
        async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last.lock().expect("mutex poisoned") = Some((name.to_string(), input));
            Ok(ToolOutput {
                value: serde_json::json!({"ran": name}),
                metadata: ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<ToolDefinition> {
            Vec::new()
        }

        async fn describe(&self, _name: &str) -> Option<ToolDefinition> {
            None
        }
    }

    /// Filter that always returns a fixed classification.
    struct ConstantFilter(Classification);

    impl SmartFilter for ConstantFilter {
        fn classify(&self, _name: &str, _input: &Value) -> Classification {
            self.0.clone()
        }
    }

    /// Approver that returns a fixed outcome and records how often it was
    /// called.
    struct ScriptedApprover {
        outcome: ApprovalOutcome,
        calls: AtomicUsize,
    }

    impl ScriptedApprover {
        fn new(outcome: ApprovalOutcome) -> Arc<Self> {
            Arc::new(Self {
                outcome,
                calls: AtomicUsize::new(0),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Approver for ScriptedApprover {
        async fn ask(&self, _tool_name: &str, _reason: &str) -> ApprovalOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcome
        }
    }

    fn layer_with(
        inner: Arc<dyn ToolService>,
        classification: Classification,
        approver: Arc<dyn Approver>,
    ) -> PermissionLayer {
        PermissionLayer::with_policy(inner, Arc::new(ConstantFilter(classification)), approver)
    }

    #[tokio::test]
    async fn always_allow_bypasses_approval() {
        let inner = RecordingInner::new();
        let approver = ScriptedApprover::new(ApprovalOutcome::Denied);
        let layer = layer_with(inner.clone(), Classification::Allow, approver.clone());

        let out = layer
            .execute("echo", serde_json::json!({"k": "v"}))
            .await
            .expect("allow should forward");
        assert_eq!(out.value, serde_json::json!({"ran": "echo"}));
        assert_eq!(inner.call_count(), 1, "inner must run");
        assert_eq!(
            approver.call_count(),
            0,
            "Allow must never consult the approver"
        );
    }

    #[tokio::test]
    async fn never_allow_returns_denied() {
        let inner = RecordingInner::new();
        let approver = ScriptedApprover::new(ApprovalOutcome::Approved);
        let layer = layer_with(
            inner.clone(),
            Classification::Deny {
                reason: "blocked by policy".into(),
            },
            approver.clone(),
        );

        let err = layer
            .execute("rm_rf", serde_json::json!({}))
            .await
            .expect_err("deny must short-circuit");
        match err {
            ToolError::PermissionDenied { name, reason } => {
                assert_eq!(name, "rm_rf");
                assert_eq!(reason, "blocked by policy");
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert_eq!(inner.call_count(), 0, "inner must not run on Deny");
        assert_eq!(
            approver.call_count(),
            0,
            "Deny must not consult the approver"
        );
    }

    #[tokio::test]
    async fn require_confirmation_proceeds_when_user_approves() {
        let inner = RecordingInner::new();
        let approver = ScriptedApprover::new(ApprovalOutcome::Approved);
        let layer = layer_with(
            inner.clone(),
            Classification::Confirm {
                reason: "dangerous".into(),
            },
            approver.clone(),
        );

        let out = layer
            .execute("bash_exec", serde_json::json!({"cmd": "ls"}))
            .await
            .expect("approved confirm should forward");
        assert_eq!(out.value, serde_json::json!({"ran": "bash_exec"}));
        assert_eq!(inner.call_count(), 1, "inner must run after approval");
        assert_eq!(
            approver.call_count(),
            1,
            "approver must be consulted exactly once"
        );
    }

    #[tokio::test]
    async fn require_confirmation_denies_when_user_rejects() {
        let inner = RecordingInner::new();
        let approver = ScriptedApprover::new(ApprovalOutcome::Denied);
        let layer = layer_with(
            inner.clone(),
            Classification::Confirm {
                reason: "dangerous".into(),
            },
            approver.clone(),
        );

        let err = layer
            .execute("bash_exec", serde_json::json!({"cmd": "rm -rf /"}))
            .await
            .expect_err("rejected confirm must deny");
        match err {
            ToolError::PermissionDenied { name, reason } => {
                assert_eq!(name, "bash_exec");
                assert_eq!(reason, "user denied approval");
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert_eq!(inner.call_count(), 0, "inner must not run after rejection");
        assert_eq!(approver.call_count(), 1, "approver must still be consulted");
    }

    #[tokio::test]
    async fn require_confirmation_treats_timeout_as_denied() {
        let inner = RecordingInner::new();
        let approver = ScriptedApprover::new(ApprovalOutcome::Timeout);
        let layer = layer_with(
            inner.clone(),
            Classification::Confirm {
                reason: "slow user".into(),
            },
            approver.clone(),
        );

        let err = layer
            .execute("bash_exec", serde_json::json!({}))
            .await
            .expect_err("timeout must deny");
        match err {
            ToolError::PermissionDenied { name, reason } => {
                assert_eq!(name, "bash_exec");
                assert_eq!(reason, "approval request timed out");
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert_eq!(inner.call_count(), 0);
    }

    #[tokio::test]
    async fn no_filter_installed_allows_through() {
        let inner = RecordingInner::new();
        let layer = PermissionLayer::new(inner.clone());
        let out = layer
            .execute("echo", serde_json::json!({}))
            .await
            .expect("no-filter default must allow");
        assert_eq!(out.value, serde_json::json!({"ran": "echo"}));
        assert_eq!(inner.call_count(), 1);
    }

    #[tokio::test]
    async fn confirm_without_approver_denies() {
        let inner = RecordingInner::new();
        let layer = PermissionLayer::new(inner.clone());
        layer.set_smart_filter(Some(Arc::new(ConstantFilter(Classification::Confirm {
            reason: "needs approval".into(),
        }))));

        let err = layer
            .execute("bash_exec", serde_json::json!({}))
            .await
            .expect_err("no approver means deny");
        match err {
            ToolError::PermissionDenied { name, reason } => {
                assert_eq!(name, "bash_exec");
                assert!(reason.contains("no approver"), "reason: {reason}");
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert_eq!(inner.call_count(), 0);
    }
}

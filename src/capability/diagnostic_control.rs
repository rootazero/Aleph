//! Diagnostic control surface for the live [`ProjectionHost`].
//!
//! The diagnostic plane is a NARROW set of operations the host itself owns;
//! it never creates a second authority. The control holds an
//! [`Arc<ProjectionHost>`] and an [`Arc<OwnershipTree>`] supplied by the
//! caller (mount-time authority), and every mutation is delegated to one of
//! those two authorities.
//!
//! # Hold plane
//!
//! At most one hold is active at a time. The hold either gates the source
//! worker's intake of authority changes ([`DiagnosticPlane::SourceIntake`])
//! or the default applier's delivery to the run-loop-readable applied state
//! ([`DiagnosticPlane::Delivery`]). The hold is auto-released by a tokio
//! timer (1..=5000 ms), is bypassed by [`DiagnosticControl::close`], and
//! does NOT change the active queue capacity.
//!
//! # Status
//!
//! [`DiagnosticControl::status`] reads the REAL applied state (post the
//! default applier's drain) plus live queue / telemetry state. It never
//! presents the publisher's initial snapshot as delivered.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::capability::descriptor::CapabilityId;
use crate::capability::facade::Cursor;
use crate::capability::ownership::{OwnerGeneration, OwnershipTree};
use crate::capability::projection_host::{ProjectionHost, ProjectionShutdownOutcome};

/// Which production-path block to hold. Re-exported from [`ProjectionHost`]
/// so the type lives at the host it gates and downstream callers can refer to
/// it through the diagnostic-control module path.
pub use crate::capability::projection_host::DiagnosticPlane;

/// Lifecycle state of the host as observed by the diagnostic control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLifecycle {
    /// Mounted; source open; not closed.
    Active,
    /// Close has been requested; the shared completion boundary is running.
    Closing,
    /// The host has reached its shared completion boundary.
    Closed,
}

/// Read-only snapshot of the host's applied + telemetry state.
///
/// `pending_depth` and `pending_capacity` reflect the DEFAULT consumer's
/// queue. `replacement_count` counts `Invalidated` enqueues on the default
/// (overflow recovery); `lag_count` counts broadcast `Lagged` reports
/// observed by the source worker. `applied_tool_ids` and
/// `applied_owner_generations` come from the REAL applied snapshot, never
/// the publisher's initial state.
#[derive(Debug, Clone)]
pub struct DiagnosticStatus {
    pub lifecycle: DiagnosticLifecycle,
    pub registry_cursor: Cursor,
    pub pending_depth: usize,
    pub pending_capacity: usize,
    pub replacement_count: u64,
    pub lag_count: u64,
    pub applied_tool_ids: Vec<CapabilityId>,
    pub applied_owner_generations: HashMap<CapabilityId, OwnerGeneration>,
}

/// Error returned by diagnostic control operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticError {
    /// Another hold is already active for this host.
    HoldAlreadyActive,
    /// `duration` is outside the 1..=5000 ms window.
    InvalidHoldDuration,
    /// `close()` did not observe the shared completion boundary within 5000 ms.
    /// The host remains close-requested / fail-closed.
    CloseTimeout { elapsed_ms: u64 },
    /// Operation attempted on a host that has reached its completion boundary.
    Closed,
    /// `revoke_tool` could not find a binding for the named tool.
    UnknownTool { name: String },
    /// The supplied owner tree is not (by `Arc` identity) the tree the host
    /// is mounted on; refusing to pair the host with a second authority.
    AuthorityMismatch,
}

/// Narrow diagnostic surface over an existing host + ownership tree pair.
///
/// Holds the SAME authority objects the rest of the host uses. Never
/// constructs a second registry, second tree, or second facade.
pub struct DiagnosticControl {
    host: Arc<ProjectionHost>,
    tree: Arc<OwnershipTree>,
}

impl DiagnosticControl {
    /// Build a diagnostic control over the given host and ownership tree.
    /// Both must be the SAME authority objects the rest of the system uses:
    /// `tree` must be the exact `Arc` the host was mounted on (checked by
    /// pointer identity, never by value). A mismatch is refused fail-closed
    /// with [`DiagnosticError::AuthorityMismatch`].
    pub fn new(
        host: Arc<ProjectionHost>,
        tree: Arc<OwnershipTree>,
    ) -> Result<Self, DiagnosticError> {
        if !Arc::ptr_eq(host.owner_tree(), &tree) {
            return Err(DiagnosticError::AuthorityMismatch);
        }
        Ok(Self { host, tree })
    }

    /// Read the REAL applied state plus queue / telemetry counters.
    pub fn status(&self) -> Result<DiagnosticStatus, DiagnosticError> {
        Ok(self.host.diagnostic_status())
    }

    /// Invoke the existing tree's `bump(Runtime)` and return its generation.
    /// The return value is the authority mutation result; the caller must
    /// observe the new generation in [`DiagnosticControl::status`] before
    /// trusting it as delivered. Refuses to bump on a disposed Runtime
    /// (irreversible once [`DiagnosticControl::dispose_runtime`] has run).
    pub fn bump_runtime(&self) -> Result<OwnerGeneration, DiagnosticError> {
        if self.host.is_closing() {
            return Err(DiagnosticError::Closed);
        }
        if self
            .tree
            .is_disposed(crate::capability::ownership::LifetimeScope::Runtime)
        {
            return Err(DiagnosticError::Closed);
        }
        Ok(self
            .tree
            .bump(crate::capability::ownership::LifetimeScope::Runtime))
    }

    /// Revoke the tool binding for `tool_name` on the SAME tree. The
    /// capability id is derived from `TOOL_NAMESPACE`; arbitrary ids are
    /// NOT accepted.
    pub fn revoke_tool(&self, tool_name: &str) -> Result<(), DiagnosticError> {
        if self.host.is_closing() {
            return Err(DiagnosticError::Closed);
        }
        let id = CapabilityId {
            namespace: crate::capability::backend::TOOL_NAMESPACE.to_string(),
            name: tool_name.to_string(),
        };
        if self.tree.revoke(&id) {
            Ok(())
        } else {
            Err(DiagnosticError::UnknownTool {
                name: tool_name.to_string(),
            })
        }
    }

    /// Dispose the existing Runtime lifetime on the same tree. Irreversible.
    /// Idempotent: a duplicate dispose on an already-tombstoned scope is a
    /// no-op and returns `Ok`. The control never synthesises a new Runtime
    /// binding on its own — the tree's `is_disposed(Runtime)` bit is set
    /// regardless of whether a binding was actually released, so subsequent
    /// `bump_runtime` calls return [`DiagnosticError::Closed`].
    pub fn dispose_runtime(&self) -> Result<(), DiagnosticError> {
        if self.host.is_closing() {
            return Err(DiagnosticError::Closed);
        }
        // `dispose` returns true only when a binding was released. The
        // tree ALSO inserts the scope into its `disposed` set on every call,
        // so we ignore the bool and always succeed when the host is open.
        let _ = self
            .tree
            .dispose(crate::capability::ownership::LifetimeScope::Runtime);
        Ok(())
    }

    /// Acquire the named hold for `duration`. Returns when the hold ends:
    /// either by timer expiry (the durable release), by a successful
    /// [`DiagnosticControl::close`], or by [`DiagnosticControl::release`].
    /// A second concurrent `hold` returns [`DiagnosticError::HoldAlreadyActive`].
    pub async fn hold(
        &self,
        plane: DiagnosticPlane,
        duration: Duration,
    ) -> Result<(), DiagnosticError> {
        self.host.diagnostic_hold(plane, duration).await
    }

    /// Idempotent release of any active hold. No drain, no extra effect.
    pub fn release(&self) -> Result<(), DiagnosticError> {
        self.host.diagnostic_release();
        Ok(())
    }

    /// Close the host: release any active hold, request host-local close,
    /// and await the EXISTING shared source/applier completion boundary for
    /// up to 5000 ms. On timeout the host remains close-requested / fail-closed
    /// and a [`DiagnosticError::CloseTimeout`] is returned. The shared
    /// completion boundary is never detached, reopened, or abandoned.
    pub async fn close(&self) -> Result<ProjectionShutdownOutcome, DiagnosticError> {
        self.host.diagnostic_close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::{
        ReplayPolicy, ToolCapabilityDescriptor, ToolKind, SCHEMA_VERSION,
    };
    use crate::tools::handlers::ToolHandler;
    use crate::tools::registry::ToolHandlerRegistry;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeHandler {
        name: String,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({"tool": self.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata {
                    idempotent: false,
                    ..Default::default()
                },
            }
        }
    }

    fn fake(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler { name: name.into() })
    }

    fn desc(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 0,
            implementation_contract: None,
        }
    }

    async fn await_status<F>(ctrl: &DiagnosticControl, pred: F) -> DiagnosticStatus
    where
        F: Fn(&DiagnosticStatus) -> bool,
    {
        // Async + `tokio::time::sleep` so the test thread does NOT block the
        // current-thread tokio runtime; the default applier (spawned via
        // `tokio::spawn`) shares the same runtime and must remain runnable
        // for `applied` to converge. A blocking `std::thread::sleep` here
        // would deadlock the host.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = ctrl.status() {
                if pred(&s) {
                    return s;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timed out waiting for status predicate");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn empty_control() -> DiagnosticControl {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        DiagnosticControl::new(host, tree).unwrap()
    }

    fn control_with(tools: &[&str]) -> DiagnosticControl {
        let reg = ToolHandlerRegistry::new();
        for name in tools {
            reg.register(desc(name), fake(name)).unwrap();
        }
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        DiagnosticControl::new(host, tree).unwrap()
    }

    /// Fix round — the control must not silently pair a host with a second
    /// owner authority: a tree that is not the host's mounted Arc is rejected
    /// by identity (`Arc::ptr_eq`), even when structurally equivalent.
    #[tokio::test]
    async fn mismatched_owner_tree_is_rejected() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let other = Arc::new(OwnershipTree::new());
        let err = DiagnosticControl::new(Arc::clone(&host), other)
            .err()
            .expect("a second owner authority must be rejected");
        assert_eq!(err, DiagnosticError::AuthorityMismatch);
        // The mounted tree itself still works.
        assert!(DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).is_ok());
    }

    #[tokio::test]
    async fn same_owner_tree_is_accepted_and_shared() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let ctrl = DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).unwrap();
        assert!(Arc::ptr_eq(host.owner_tree(), &tree));
        // The tree nonce is per-instance: a control bump followed by a direct
        // bump on the caller's tree is consecutive only if both hit one tree.
        let via_ctrl = ctrl.bump_runtime().unwrap();
        let direct = tree.bump(crate::capability::ownership::LifetimeScope::Runtime);
        assert_eq!(
            direct.0,
            via_ctrl.0 + 1,
            "bump must land on the caller's tree"
        );
    }

    /// RED — §3.1 status must read post-applier applied state, never the
    /// publisher's initial seed. Before the default applier runs the applied
    /// list is empty even though the publisher already saw the tool.
    #[tokio::test]
    async fn status_distinguishes_queued_from_applied() {
        let ctrl = control_with(&["a"]);
        let s0 = ctrl.status().unwrap();
        assert!(
            s0.applied_tool_ids.is_empty(),
            "applied must not present the publisher's seed as delivered"
        );
        assert_eq!(s0.lifecycle, DiagnosticLifecycle::Active);
        let s1 = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;
        assert_eq!(s1.applied_tool_ids.len(), 1);
        assert_eq!(s1.applied_tool_ids[0].name, "a");
        assert!(!s1.applied_owner_generations.is_empty());
    }

    /// RED — §3.5 delivery hold must let the default consumer's queue
    /// accumulate and overflow with `Invalidated` + replacement semantics.
    /// Once the hold releases the applier drains and the final applied
    /// snapshot contains every tool.
    #[tokio::test]
    async fn delivery_hold_overflow_replaces_stale_pending_state() {
        let reg = ToolHandlerRegistry::new();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount_with_capacity(reg.clone(), Arc::clone(&tree), 2);
        let ctrl = DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).unwrap();

        let ctrl = Arc::new(ctrl);
        let hold_ctrl = Arc::clone(&ctrl);
        let hold_task = tokio::spawn(async move {
            hold_ctrl
                .hold(DiagnosticPlane::Delivery, Duration::from_secs(2))
                .await
        });
        // Yield so the applier actually begins waiting on the gate before the
        // first registration fires.
        tokio::time::sleep(Duration::from_millis(50)).await;
        for i in 0..10 {
            let n = format!("t{i}");
            reg.register(desc(&n), fake(&n)).unwrap();
        }
        hold_task.await.unwrap().unwrap();

        let s = await_status(&ctrl, |s| s.applied_tool_ids.len() == 10).await;
        assert_eq!(
            s.applied_tool_ids.len(),
            10,
            "final snapshot must contain all tools"
        );
        assert!(
            s.replacement_count > 0,
            "overflow must have triggered a replacement"
        );
    }

    /// RED — §3.5 source-intake hold must let the broadcast channel lag, so
    /// the source worker observes `Lagged` on release and the lag is
    /// recovered via a fresh `Invalidated` + replacement Snapshot (NOT by
    /// fabricating deltas).
    #[tokio::test]
    async fn source_hold_lag_recovers_with_snapshot() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("seed"), fake("seed")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg.clone(), Arc::clone(&tree));
        let ctrl = DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).unwrap();

        // Wait for the initial applied state so the rest of the test
        // observes only the new registrations.
        let _ = await_status(&ctrl, |s| s.applied_tool_ids.len() == 1).await;

        let ctrl = Arc::new(ctrl);
        let hold_ctrl = Arc::clone(&ctrl);
        let hold_task = tokio::spawn(async move {
            hold_ctrl
                .hold(DiagnosticPlane::SourceIntake, Duration::from_millis(300))
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        for i in 0..2000 {
            let n = format!("t{i}");
            reg.register(desc(&n), fake(&n)).unwrap();
        }
        hold_task.await.unwrap().unwrap();

        let s = await_status(&ctrl, |s| s.applied_tool_ids.len() == 2001).await;
        assert_eq!(
            s.applied_tool_ids.len(),
            2001,
            "lag recovery must carry the final snapshot"
        );
        assert!(
            s.lag_count > 0,
            "the broadcast must have lagged during the source-intake hold"
        );
    }

    /// RED — §3.7 close must bypass any active hold, await the existing
    /// shared completion boundary within 5000 ms, and leave the host
    /// fail-closed. Subsequent status reads must report `Closed`.
    #[tokio::test]
    async fn close_bypasses_hold_and_awaits_shared_completion() {
        let ctrl = control_with(&["a"]);
        let _ = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;

        let ctrl = Arc::new(ctrl);
        let hold_ctrl = Arc::clone(&ctrl);
        let hold_task = tokio::spawn(async move {
            hold_ctrl
                .hold(DiagnosticPlane::SourceIntake, Duration::from_secs(5))
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let outcome = ctrl.close().await.unwrap();
        assert!(outcome.source_joined || outcome.applier_joined);
        let _ = hold_task.await.unwrap().unwrap();

        let s = ctrl.status().unwrap();
        assert_eq!(s.lifecycle, DiagnosticLifecycle::Closed);
    }

    /// RED — §3.2 bump must return the tree's `bump(Runtime)` generation;
    /// two consecutive bumps produce strictly greater values.
    #[tokio::test]
    async fn bump_returns_generation() {
        let ctrl = empty_control();
        let g1 = ctrl.bump_runtime().unwrap();
        let g2 = ctrl.bump_runtime().unwrap();
        assert!(
            g2.0 > g1.0,
            "consecutive bumps must yield strictly greater generations"
        );
    }

    /// RED — §3.3 revoke must derive the canonical Tool id from
    /// `TOOL_NAMESPACE` and NOT advance the registry cursor (the registry
    /// cursor only moves on registry-side mutations).
    #[tokio::test]
    async fn revoke_tool_uses_canonical_id_and_does_not_advance_cursor() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let ctrl = DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).unwrap();

        let s0 = await_status(&ctrl, |s| !s.applied_tool_ids.is_empty()).await;
        let cursor_before = s0.registry_cursor.0;

        ctrl.revoke_tool("a").unwrap();

        let s1 = await_status(&ctrl, |s| s.applied_tool_ids.is_empty()).await;
        assert!(
            s1.applied_tool_ids.is_empty(),
            "revoked tool must leave applied"
        );
        assert_eq!(
            s1.registry_cursor.0, cursor_before,
            "revoke must NOT advance the registry cursor"
        );
    }

    /// RED — §3.4 dispose is irreversible; a second dispose reports `Closed`.
    #[tokio::test]
    async fn dispose_runtime_irreversible() {
        let ctrl = empty_control();
        ctrl.dispose_runtime().unwrap();
        assert!(
            ctrl.bump_runtime().is_err(),
            "bump on a disposed runtime must fail"
        );
    }

    /// RED — §3.2 after `bump_runtime` the applied owner generation
    /// observed in `status()` must be STRICTLY greater than the generation
    /// observed before the bump, at the same registry cursor.
    #[tokio::test]
    async fn bump_then_status_observes_strictly_greater_owner_generations() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let host = ProjectionHost::mount(reg, Arc::clone(&tree));
        let ctrl = DiagnosticControl::new(Arc::clone(&host), Arc::clone(&tree)).unwrap();

        let s0 = await_status(&ctrl, |s| !s.applied_owner_generations.is_empty()).await;
        let gen_before: OwnerGeneration = *s0.applied_owner_generations.values().next().unwrap();

        ctrl.bump_runtime().unwrap();

        let s1 = await_status(&ctrl, |s| {
            s.applied_owner_generations
                .values()
                .any(|g| g.0 > gen_before.0)
        })
        .await;
        let max_obs = *s1.applied_owner_generations.values().max().unwrap();
        assert!(
            max_obs.0 > gen_before.0,
            "observed generation must be strictly greater"
        );
    }
}

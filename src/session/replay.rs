//! Phase 2B replay execution seam — pure types. NO registry/platform imports.
//!
//! This module is the session-layer half of the two-phase replay seam
//! (prepare → claim → execute → commit). It owns only the data shapes the
//! coordinator and the bridge exchange; the eligibility predicate and the
//! handler invocation live in `orchestrator::harness_bridge::replay_adapter`,
//! and the once-only fence lives in [`crate::session::store`]. Keeping this
//! module free of registry/platform imports (contract C1) is what lets the
//! coordinator hold an `Arc<dyn ReplayPreparer>` without dragging the tool
//! stack into the session layer.

use futures::future::BoxFuture;
use serde_json::Value;

use crate::session::events::{SessionEvent, TurnId};
use crate::tools::descriptor::ToolCallIdentity;

/// One dangling tool call the coordinator is deciding whether to replay.
pub struct ReplayRequest {
    pub call_id: String,
    pub tool_name: String,
    /// The turn the call was made in, carried so the bridge can rebuild a
    /// `SessionEvent::ToolResult`/`ToolError` (both require `turn_id`).
    pub turn_id: TurnId,
    /// Durable effective POST-guardrail input. `None` ⇒ `prepare` MUST return
    /// [`ReplayPrepare::Refused`]. No such field exists on the event today, so
    /// production passes `None` unconditionally and the seam stays inert.
    pub effective_input: Option<Value>,
    pub stored_identity: Option<ToolCallIdentity>,
}

/// Result of preparing a dangling call for replay.
pub enum ReplayPrepare {
    /// Opaque, non-Copy, single-use permit. The session layer moves it only.
    Ready(ReplayPermit),
    /// Deterministic fail-closed. Caller falls back to VerifyOnly; no claim.
    Refused,
}

/// Implemented by the bridge layer; holds the registry snapshot source.
pub trait ReplayPreparer: Send + Sync {
    /// Pure, snapshot-derived, fail-closed, NO store I/O. MUST NOT invoke.
    fn prepare(&self, req: &ReplayRequest) -> ReplayPrepare;
}

/// An opaque, bridge-owned, single-use handle to a prepared replay.
///
/// The field is private and the concrete inner type lives in the bridge, so the
/// `aleph-server` bin crate can name this type but can neither construct nor
/// invoke it. It is deliberately NOT `Copy`/`Clone`: `invoke` consumes `self`.
pub struct ReplayPermit {
    inner: Box<dyn ReplayInvoker>,
}

impl ReplayPermit {
    pub(crate) fn new(inner: Box<dyn ReplayInvoker>) -> Self {
        Self { inner }
    }

    pub(crate) async fn invoke(self, claim_token: String) -> ReplayInvocation {
        self.inner.invoke(claim_token).await
    }
}

/// The bridge-side half of a permit: actually runs the prepared handler.
pub(crate) trait ReplayInvoker: Send {
    fn invoke(self: Box<Self>, claim_token: String) -> BoxFuture<'static, ReplayInvocation>;
}

/// What a prepared replay produced.
pub enum ReplayInvocation {
    /// A real handler ran (Ok → `ToolResult`, Err → `ToolError` are BOTH real
    /// outcomes); commit with the original `call_id` under the claim.
    Executed(Box<SessionEvent>),
    /// No effect ran (permit precondition broke mid-flight); VerifyOnly
    /// fallback. No commit.
    NoEffect,
}

/// Bounded at-least-once: each resume pass may effect a dangling call at most
/// once; across crashes bounded by this cursor budget; exhausted ⇒ VerifyOnly.
pub const REPLAY_MAX_ATTEMPTS_PER_CALL: u32 = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A minimal in-crate invoker that records it ran and returns a real
    /// `Executed(ToolResult)`. Lives here (not in the bridge) so the seam's
    /// single-use/ownership shape can be exercised without the registry.
    struct RecordingInvoker {
        fired: Arc<AtomicBool>,
        call_id: String,
    }

    impl ReplayInvoker for RecordingInvoker {
        fn invoke(self: Box<Self>, _claim_token: String) -> BoxFuture<'static, ReplayInvocation> {
            let RecordingInvoker { fired, call_id } = *self;
            Box::pin(async move {
                fired.store(true, Ordering::SeqCst);
                ReplayInvocation::Executed(Box::new(SessionEvent::ToolResult {
                    turn_id: uuid::Uuid::new_v4(),
                    call_id,
                    output: ToolOutput {
                        value: serde_json::json!({"ok": true}),
                        metadata: Default::default(),
                    },
                    at: 0,
                }))
            })
        }
    }

    /// `ReplayPermit::new` is `pub(crate)`: reachable in-crate (this test) but
    /// invisible to `aleph-server`. `invoke(self, ..)` consumes the permit, so
    /// it is single-use by construction — a second call would be a
    /// use-after-move and will not compile. `ReplayPermit` is likewise not
    /// `Copy`/`Clone` (its only field is a `Box<dyn ReplayInvoker>`, and no
    /// derive is present); that is a structural guarantee, not something a
    /// runtime test can re-prove.
    #[tokio::test]
    async fn permit_is_constructed_in_crate_and_invoke_consumes_self() {
        let fired = Arc::new(AtomicBool::new(false));
        let permit = ReplayPermit::new(Box::new(RecordingInvoker {
            fired: Arc::clone(&fired),
            call_id: "c-1".into(),
        }));

        let invocation = permit.invoke("claim-token".into()).await;

        assert!(fired.load(Ordering::SeqCst), "invoker must have run");
        match invocation {
            ReplayInvocation::Executed(event) => match *event {
                SessionEvent::ToolResult { call_id, .. } => {
                    assert_eq!(call_id, "c-1");
                }
                _ => panic!("expected Executed(ToolResult)"),
            },
            _ => panic!("expected Executed(ToolResult)"),
        }
    }

    /// The `None` refusal is enforced by the bridge's `prepare`, but the seam
    /// type itself must expose `effective_input` as an explicit `Option` so the
    /// gate is a type-level contract, not a remembered check. Pin that shape.
    #[test]
    fn request_effective_input_is_an_explicit_option() {
        let req = ReplayRequest {
            call_id: "c".into(),
            tool_name: "t".into(),
            turn_id: uuid::Uuid::new_v4(),
            effective_input: None,
            stored_identity: None,
        };
        assert!(req.effective_input.is_none());
    }
}

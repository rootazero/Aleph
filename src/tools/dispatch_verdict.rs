//! Read-only, exact-call dispatch authority and sealed Safe replay scopes.

use futures::future::BoxFuture;
use std::future::Future;

use serde_json::Value;

use crate::orchestrator::harness_bridge::replay_adapter::ReplayAdmissionToken;
use crate::tools::descriptor::{ReplayPolicy, ToolCallIdentity};
use crate::tools::scoped::dispatch::GateAdmissionToken;

/// In-memory authority for one exact, gate-judged invocation.
/// Readers can compare or retain a verdict, but cannot mint or publish one.
#[derive(Clone)]
pub(crate) struct DispatchVerdict {
    call_id: String,
    canonical_name: String,
    identity: ToolCallIdentity,
    input: Value,
    actor: Option<String>,
}

impl DispatchVerdict {
    /// The factory requires a token whose private constructor lives at the gate.
    fn from_gate(
        _admission: &GateAdmissionToken,
        call_id: String,
        canonical_name: String,
        identity: ToolCallIdentity,
        input: Value,
        actor: Option<String>,
    ) -> Self {
        Self {
            call_id,
            canonical_name,
            identity,
            input,
            actor,
        }
    }

    /// Match every field without name repair or input rewriting.
    #[must_use]
    pub(crate) fn matches(
        &self,
        call_id: &str,
        canonical_name: &str,
        identity: &ToolCallIdentity,
        input: &Value,
        actor: Option<&str>,
    ) -> bool {
        self.call_id == call_id
            && self.canonical_name == canonical_name
            && self.identity == *identity
            && self.input == *input
            && self.actor.as_deref() == actor
    }
}

tokio::task_local! {
    static DISPATCH_VERDICT: Option<DispatchVerdict>;
    static SAFE_REPLAY_ADMISSION: Option<ReplayAdmissionToken>;
}

/// Outside the invocation scope (including Tokio children), there is no proof.
#[must_use]
pub(crate) fn current_dispatch_verdict() -> Option<DispatchVerdict> {
    DISPATCH_VERDICT.try_with(Clone::clone).ok().flatten()
}

/// A nested dispatch's gates and hooks cannot borrow an outer handler's proof.
pub(super) fn without_admission<'a, T>(future: BoxFuture<'a, T>) -> BoxFuture<'a, T>
where
    T: Send + 'a,
{
    Box::pin(async move {
        DISPATCH_VERDICT
            .scope(None, SAFE_REPLAY_ADMISSION.scope(None, future))
            .await
    })
}

/// Only the gate can supply the sealed token; a readonly verdict cannot scope.
pub(super) async fn with_gate_admission<F: Future + Send>(
    admission: &GateAdmissionToken,
    call_id: String,
    canonical_name: String,
    identity: ToolCallIdentity,
    input: Value,
    actor: Option<String>,
    future: F,
) -> F::Output {
    let verdict =
        DispatchVerdict::from_gate(admission, call_id, canonical_name, identity, input, actor);
    DISPATCH_VERDICT
        .scope(Some(verdict), SAFE_REPLAY_ADMISSION.scope(None, future))
        .await
}

/// Replay eligibility is not dispatch, human or operator authorization.
/// Only ReplayAdapter::prepare can construct this token.
pub(crate) async fn with_replay_admission<F: Future>(
    token: ReplayAdmissionToken,
    future: F,
) -> F::Output {
    DISPATCH_VERDICT
        .scope(None, SAFE_REPLAY_ADMISSION.scope(Some(token), future))
        .await
}

/// Canonical invoke checks both sealed lanes against the entire ambient tuple.
pub(super) fn invocation_admitted(
    call_id: &str,
    name: &str,
    identity: &ToolCallIdentity,
    input: &Value,
    actor: Option<&str>,
) -> bool {
    current_dispatch_verdict().is_some_and(|v| v.matches(call_id, name, identity, input, actor))
        || (identity.replay_policy == ReplayPolicy::Safe
            && SAFE_REPLAY_ADMISSION
                .try_with(|token| {
                    token
                        .as_ref()
                        .is_some_and(|t| t.matches(call_id, name, identity, input, actor))
                })
                .unwrap_or(false))
}

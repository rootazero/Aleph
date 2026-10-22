//! Read-only, exact-call dispatch authority and sealed Safe replay scopes.

use futures::future::BoxFuture;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::sync_primitives::Arc;

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
    caller: AdmissionCaller,
    operator_approval: Option<Arc<AtomicBool>>,
}

/// Agent identity and the originating human/turn are different facts.
#[derive(Clone, PartialEq, Eq)]
struct AdmissionCaller {
    actor: Option<String>,
    turn: Option<TurnCaller>,
}

#[derive(Clone, PartialEq, Eq)]
struct TurnCaller {
    session_key: crate::routing::session_key::SessionKey,
    run_id: String,
    channel_id: String,
    conversation_id: String,
    role: Option<String>,
}

impl AdmissionCaller {
    fn current() -> Self {
        Self {
            actor: crate::gateway::visibility::ambient_actor(),
            turn: crate::tools::turn_context::current_turn_context().map(|ctx| TurnCaller {
                session_key: ctx.session_key,
                run_id: ctx.run_id,
                channel_id: ctx.channel_id,
                conversation_id: ctx.conversation_id,
                role: ctx.caller_role,
            }),
        }
    }
}

impl DispatchVerdict {
    /// The factory requires a token whose private constructor lives at the gate.
    fn from_gate(
        admission: &GateAdmissionToken,
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
            caller: AdmissionCaller::current(),
            operator_approval: admission.operator_approval(),
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

/// Exact canonical invocation proof carried only through the inner plugin route.
/// No public fields, serde, or constructor: replay and transport RPC cannot mint it.
pub struct PluginActivationAuthority {
    verdict: DispatchVerdict,
    plugin_id: String,
    handler: String,
}

impl PluginActivationAuthority {
    pub(crate) fn from_current(
        canonical_name: &str,
        plugin_id: &str,
        handler: &str,
        input: &Value,
    ) -> Option<Self> {
        let verdict = current_dispatch_verdict()?;
        let call_id = crate::approval::current_tool_call_id()?;
        let identity = current_admitted_handler_identity()?;
        let actor = crate::identity::current_actor();
        if !verdict.matches(&call_id, canonical_name, &identity, input, actor.as_deref()) {
            return None;
        }
        Some(Self {
            verdict,
            plugin_id: plugin_id.to_owned(),
            handler: handler.to_owned(),
        })
    }

    /// Revalidate the whole admitted tuple after waiting. No authority crosses
    /// task-local scope exit, nested dispatch, caller change, or replay scope.
    pub(crate) fn validates(&self, plugin_id: &str, handler: &str, input: &Value) -> bool {
        self.plugin_id == plugin_id
            && self.handler == handler
            && Self::from_current(&self.verdict.canonical_name, plugin_id, handler, input)
                .is_some_and(|current| {
                    current.verdict.caller == self.verdict.caller
                        && current.verdict.matches(
                            &self.verdict.call_id,
                            &self.verdict.canonical_name,
                            &self.verdict.identity,
                            &self.verdict.input,
                            self.verdict.actor.as_deref(),
                        )
                })
    }
}

tokio::task_local! {
    static DISPATCH_VERDICT: Option<DispatchVerdict>;
    static SAFE_REPLAY_ADMISSION: Option<ReplayAdmissionToken>;
    static ADMITTED_HANDLER_IDENTITY: Option<ToolCallIdentity>;
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
            .scope(
                None,
                SAFE_REPLAY_ADMISSION.scope(None, ADMITTED_HANDLER_IDENTITY.scope(None, future)),
            )
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

/// Consume only an operator-approved exact call, never generic admission or replay.
/// Mismatches leave the proof untouched; successful consumers share one atomic
/// claim across verdict clones and dispatch retries. No authority crosses spawn.
pub(crate) fn consume_operator_approval(name: &str, input: &Value) -> bool {
    let Some(call_id) = crate::approval::current_tool_call_id() else {
        return false;
    };
    let Some(identity) = current_admitted_handler_identity() else {
        return false;
    };
    let actor = crate::identity::current_actor();
    DISPATCH_VERDICT
        .try_with(|verdict| {
            verdict.as_ref().is_some_and(|verdict| {
                verdict.matches(&call_id, name, &identity, input, actor.as_deref())
                    && verdict.caller == AdmissionCaller::current()
                    && verdict.operator_approval.as_ref().is_some_and(|used| {
                        used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                    })
            })
        })
        .unwrap_or(false)
}

/// A nested dispatch masks the identity as well as both admission lanes.
#[must_use]
pub(super) fn current_admitted_handler_identity() -> Option<ToolCallIdentity> {
    ADMITTED_HANDLER_IDENTITY
        .try_with(|identity| *identity)
        .ok()
        .flatten()
}

/// Publish only the descriptor identity checked by the registry boundary.
pub(super) async fn with_admitted_handler<F: Future>(
    identity: ToolCallIdentity,
    future: F,
) -> F::Output {
    ADMITTED_HANDLER_IDENTITY
        .scope(Some(identity), future)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_authority_cannot_be_derived_outside_gate_scope() {
        let identity = ToolCallIdentity {
            schema_version: 1,
            revision: 0,
            replay_policy: ReplayPolicy::Unsafe,
            replay_contract_fingerprint: None,
        };
        assert!(PluginActivationAuthority::from_current(
            "plugin_tool",
            "plugin-id",
            "tool_handler",
            &serde_json::json!({"x": 1}),
        )
        .is_none());
        assert!(!invocation_admitted(
            "call-1",
            "plugin_tool",
            &identity,
            &serde_json::json!({"x": 1}),
            None,
        ));
    }
}

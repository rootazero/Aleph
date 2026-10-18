use crate::sync_primitives::RwLock;
use std::sync::Arc;

use async_trait::async_trait;

use super::action::ApprovalAction;

/// A transport that puts an approval in front of a human and returns their
/// decision.
///
/// The request carries the whole [`ApprovalAction`], not a tool name: a
/// requester that only knows the name can only render the name, and a human who
/// is shown `bash` has not been shown anything.
#[async_trait]
pub trait ApprovalRequester: Send + Sync {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse;
}

pub use aleph_slot_api::approval::{ApprovalOutcome, ApprovalResponse};

pub struct ApprovalGate {
    /// Swappable so boot can construct the gate before the channel registry
    /// exists, then wire the real requester via `set_requester` once channels
    /// are up. An empty slot denies — never a silent auto-approve.
    requester: RwLock<Option<Arc<dyn ApprovalRequester>>>,
}

impl ApprovalGate {
    #[must_use]
    pub fn new(requester: Option<Arc<dyn ApprovalRequester>>) -> Self {
        Self {
            requester: RwLock::new(requester),
        }
    }

    /// Install (or replace) the approval requester after construction.
    ///
    /// Boot constructs the gate before the channel registry exists; once
    /// channels are up this wires the real `ChannelApprovalBridgeAdapter` so
    /// elevated-capability sandbox escalations can actually reach the user
    /// instead of being auto-denied.
    pub fn set_requester(&self, requester: Arc<dyn ApprovalRequester>) {
        *self.requester.write().unwrap_or_else(|e| e.into_inner()) = Some(requester);
    }

    /// Put `action` in front of a human and wait for their answer.
    ///
    /// # The unattended tax
    ///
    /// An unattended run — a goal / loop continuation, a heartbeat, an A2A
    /// delegation, a cron job with no origin channel — has nobody on any
    /// surface. Awaiting a card there buys nothing: the card expires and the
    /// call fails anyway, having spent the full approval timeout doing it, once
    /// **per escalation**.
    ///
    /// The tool confirm gate has charged this tax since 2026-07-14
    /// (`ScopedToolService::confirm_with_memory`). This gate — the twin that
    /// serves sandbox capability elevation and the failover route escalation —
    /// never did, so a headless run that asked for `allow_network` parked for
    /// the whole timeout and was refused at the end of it. The two gates have
    /// already been found divergent twice (the ledger key, and the approval
    /// that closes the breaker); this is the third, and the tax belongs on the
    /// shared chokepoint rather than at a third call site, so anything wired
    /// here later inherits it.
    ///
    /// Read from the ambient [`TurnContext`], which the tool-dispatch
    /// chokepoint scopes around every tool call and which this gate's own
    /// channel requester already reads for routing. Absent context is treated
    /// as **attended** — a missing signal must widen nothing, and the pre-tax
    /// behaviour (park and ask) is the conservative side here.
    ///
    /// [`TurnContext`]: crate::tools::turn_context::TurnContext
    pub async fn request_approval_for_action(&self, action: &ApprovalAction) -> ApprovalResponse {
        if crate::tools::turn_context::current_turn_context().is_some_and(|t| t.unattended) {
            tracing::warn!(
                tool = %action.tool_name,
                "unattended run: refusing an approval-gated action without asking \
                 (no human is watching this run)"
            );
            return ApprovalOutcome::Unavailable.into();
        }
        // Clone the Arc out of the lock and drop the guard before awaiting —
        // a std `RwLock` guard is not `Send` and must not be held across await.
        let requester = self
            .requester
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let response = match requester {
            Some(requester) => requester.request_approval(action).await,
            None => {
                tracing::warn!(
                    tool = %action.tool_name,
                    "no approval requester configured — failing closed (nobody to ask)"
                );
                ApprovalOutcome::Unavailable.into()
            }
        };
        record_gate_decision(action, &response).await;
        response
    }
}

/// File this gate's decision on the signed operation ledger.
///
/// The tool-dispatch gate writes its own approval rows
/// (`ScopedToolService::record_approval_decision`); this gate — sandbox
/// capability elevation and route escalation — never did, so an approved
/// `allow_network` left no signed trace even though it is the most
/// privilege-widening decision the system makes. Wired here, on the shared
/// chokepoint, for the same reason the unattended tax lives here: every
/// current and future caller inherits it.
///
/// Only real decisions are recorded. `Timeout`/`Unavailable` are the
/// *absence* of a decision (see [`ApprovalOutcome::Unavailable`]); filing
/// them as `ApprovalDenied` would be the same lie that variant exists to
/// prevent.
async fn record_gate_decision(action: &ApprovalAction, response: &ApprovalResponse) {
    use crate::identity::{LedgerAction, LedgerOutcome, NewRecord};

    if crate::identity::global().is_none() {
        return;
    }
    let (ledger_action, outcome, scope_note) = match response.outcome {
        ApprovalOutcome::Approved => (LedgerAction::ApprovalGranted, LedgerOutcome::Ok, ""),
        ApprovalOutcome::ApprovedForSession => (
            LedgerAction::ApprovalGranted,
            LedgerOutcome::Ok,
            " (session)",
        ),
        ApprovalOutcome::ApprovedAlways => (
            LedgerAction::ApprovalGranted,
            LedgerOutcome::Ok,
            " (always)",
        ),
        ApprovalOutcome::Denied => (LedgerAction::ApprovalDenied, LedgerOutcome::Denied, ""),
        ApprovalOutcome::Timeout | ApprovalOutcome::Unavailable => return,
    };
    // Same attribution rule as the dispatch chokepoint
    // (`ScopedToolService::ledger_actor_for`): the scoped sub-agent actor
    // first, then the turn's own agent. No context → no record: an
    // unattributable decision must not be filed under a guessed agent.
    let Some(agent_id) = crate::identity::current_actor().or_else(|| {
        crate::tools::turn_context::current_turn_context()
            .map(|t| t.session_key.agent_id().to_string())
    }) else {
        return;
    };
    // Both halves are already redacted: `summary` by the same masker the
    // approval cards use, `reason` authored by the gate itself. A human's
    // deny reason is free text, so it goes through the masker too.
    let detail = match response.deny_reason.as_deref() {
        Some(r) => format!(
            "{} — denied: {}",
            action.summary,
            crate::sandbox::exec_approval::redact_and_cap(r)
        ),
        None => format!("{} — {}{}", action.summary, action.reason, scope_note),
    };
    crate::identity::record_action(NewRecord {
        agent_id,
        principal: crate::gateway::visibility::ambient_actor(),
        action: ledger_action,
        target: action.tool_name.clone(),
        outcome,
        args_fp: action.grant_key.clone(),
        detail,
    })
    .await;
}

/// The gate is itself an [`ApprovalRequester`], delegating to its own
/// late-bound requester via [`request_approval_for_action`].
///
/// This lets one shared `ApprovalGate` (whose channel requester is wired after
/// channels are up) serve both the sandbox escalation path and the failover
/// route escalation (borrow-cloud) gate, without exposing the inner requester.
///
/// [`request_approval_for_action`]: ApprovalGate::request_approval_for_action
#[async_trait]
impl ApprovalRequester for ApprovalGate {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        self.request_approval_for_action(action).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the gate denies when no requester is wired (the CRITICAL
    /// boot bug), and routes to the requester once `set_requester` installs
    /// one — the post-construction wiring boot now performs.
    #[tokio::test]
    async fn set_requester_makes_escalation_reach_the_requester() {
        struct AlwaysApprove;
        #[async_trait::async_trait]
        impl ApprovalRequester for AlwaysApprove {
            async fn request_approval(&self, _action: &ApprovalAction) -> ApprovalResponse {
                ApprovalOutcome::Approved.into()
            }
        }

        let action = ApprovalAction::bare("code_exec", "allow_network");
        let gate = ApprovalGate::new(None);
        // No requester wired → fails closed (never a silent auto-approve), and
        // as `Unavailable` rather than `Denied`: there was nobody to refuse it.
        assert_eq!(
            gate.request_approval_for_action(&action).await.outcome,
            ApprovalOutcome::Unavailable
        );
        // Once the requester is installed, escalations reach it.
        gate.set_requester(Arc::new(AlwaysApprove));
        assert_eq!(
            gate.request_approval_for_action(&action).await.outcome,
            ApprovalOutcome::Approved
        );
    }

    /// An unattended run never parks on a card. The requester here would say
    /// yes, and it is never consulted — which is the point: the alternative is
    /// the full approval timeout, per escalation, ending in a refusal anyway.
    ///
    /// The tool confirm gate has charged this since 2026-07-14; this gate is
    /// the twin that did not, and the twins have already been found divergent
    /// twice before.
    #[tokio::test]
    async fn an_unattended_run_is_refused_without_asking() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        struct WouldApprove;
        #[async_trait::async_trait]
        impl ApprovalRequester for WouldApprove {
            async fn request_approval(&self, _action: &ApprovalAction) -> ApprovalResponse {
                ApprovalOutcome::Approved.into()
            }
        }

        let action = ApprovalAction::bare("code_exec", "allow_network");
        let gate = ApprovalGate::new(Some(Arc::new(WouldApprove)));

        let ctx = |unattended: bool| TurnContext {
            session_key: SessionKey::main("main"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        };

        // Attended (and outside any turn) still reaches the requester.
        assert_eq!(
            gate.request_approval_for_action(&action).await.outcome,
            ApprovalOutcome::Approved
        );
        assert_eq!(
            TURN_CONTEXT
                .scope(ctx(false), gate.request_approval_for_action(&action))
                .await
                .outcome,
            ApprovalOutcome::Approved
        );

        // Unattended fails closed as `Unavailable`, not `Denied`: nobody
        // refused it, so the denial ledger must not make it sticky.
        assert_eq!(
            TURN_CONTEXT
                .scope(ctx(true), gate.request_approval_for_action(&action))
                .await
                .outcome,
            ApprovalOutcome::Unavailable
        );
    }
}

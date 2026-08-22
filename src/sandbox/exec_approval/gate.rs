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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// Approved for this single invocation only.
    Approved,
    /// Approved for the remainder of the session ("allow session").
    /// Treated as approved everywhere; additionally recorded in the grant store
    /// so the SAME ACTION — same tool, same arguments — is not re-prompted this
    /// session. A different action of the same tool still asks.
    ApprovedForSession,
    /// Approved until revoked, across restarts ("always allow"). Recorded in
    /// the persistent tier of the grant store.
    ///
    /// Only reachable when the approval record's `allowed_decisions` offered
    /// the tier — [`ApprovalDecisionType::to_outcome_within`] cannot produce it
    /// otherwise, which is why every decision→outcome call site has to name the
    /// set it offered.
    ///
    /// [`ApprovalDecisionType::to_outcome_within`]: crate::exec::socket::ApprovalDecisionType::to_outcome_within
    ApprovedAlways,
    Denied,
    Timeout,
    /// Fail-closed because **nobody could be asked** — no approval transport is
    /// wired, the channel refused or failed the delivery, no route resolves, or
    /// the run is unattended.
    ///
    /// # Why this is not `Denied`
    ///
    /// A refusal and the absence of a decision are different facts, and every
    /// consumer downstream of this enum was reading the first when it was handed
    /// the second. [`DenialLedger`] makes a `Denied` **sticky for the action for
    /// the rest of the session** and advances the brute-force breaker; the model
    /// is told "the user already declined this exact action". None of that is
    /// true when a Telegram delivery timed out. Three such hiccups paused the
    /// session, and the sentence the model relayed to the user was a lie about
    /// something the user never did.
    ///
    /// The ledger already draws exactly this line for [`DenialReason::Timeout`]
    /// ("a timeout is not a decision") — this variant is what lets the same rule
    /// reach the refusals that arrive labelled as a person's answer. Security
    /// posture is unchanged: [`Self::is_approved`] is false, so every one of
    /// these still fails closed.
    ///
    /// [`DenialLedger`]: super::denial_ledger::DenialLedger
    /// [`DenialReason::Timeout`]: super::denial_ledger::DenialReason::Timeout
    Unavailable,
}

impl ApprovalOutcome {
    #[must_use]
    pub const fn is_approved(&self) -> bool {
        matches!(
            self,
            Self::Approved | Self::ApprovedForSession | Self::ApprovedAlways
        )
    }

    /// The scope of the standing grant this outcome creates, if any — the one
    /// derivation of "how long does this yes last", so a surface cannot record
    /// a grant at a scope the outcome never carried.
    ///
    /// `None` for a one-shot approval and for every refusal.
    #[must_use]
    pub const fn grant_scope(&self) -> Option<crate::sandbox::exec_approval::GrantScope> {
        use crate::sandbox::exec_approval::GrantScope;
        match self {
            Self::ApprovedForSession => Some(GrantScope::Session),
            Self::ApprovedAlways => Some(GrantScope::Always),
            _ => None,
        }
    }
}

/// A human's answer to an approval request: the outcome, plus any free-text
/// reason attached to a denial (`/deny <reason>` from a channel, the `reason`
/// field on `exec.approval.resolve`).
///
/// The reason is what turns a bare "denied" into an instruction the model can
/// act on — hermes ships the same affordance. Requesters whose transport
/// cannot carry one (buttons, auto-deny policies, test stubs) build the
/// response via `From<ApprovalOutcome>`, which leaves it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResponse {
    pub outcome: ApprovalOutcome,
    /// Set only when `outcome` is [`ApprovalOutcome::Denied`] and the human
    /// supplied a reason.
    pub deny_reason: Option<String>,
}

impl From<ApprovalOutcome> for ApprovalResponse {
    fn from(outcome: ApprovalOutcome) -> Self {
        Self {
            outcome,
            deny_reason: None,
        }
    }
}

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

    #[test]
    fn approval_outcome_is_approved() {
        assert!(ApprovalOutcome::Approved.is_approved());
        assert!(!ApprovalOutcome::Denied.is_approved());
        assert!(!ApprovalOutcome::Timeout.is_approved());
    }

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

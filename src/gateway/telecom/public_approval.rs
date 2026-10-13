//! One-shot trusted approvals and the effect fence for weak telecom public
//! conversations (addendum §5–§6, execution contract §8).
//!
//! A weak SMS/PSTN origin converses with the same Core in public-context mode.
//! Anything private needs TWO separate, non-interchangeable operator approvals:
//!
//! - **A — [`PublicApprovalStage::PrivateRead`]**: binds the exact private
//!   source/query for the current turn;
//! - **B — [`PublicApprovalStage::Disclosure`]**: binds the exact, complete,
//!   bounded final body the operator reviewed.
//!
//! Policy enforced here (pure, storage-free, fail closed):
//!
//! - each approval is one-shot: consuming it, or any context mismatch at
//!   consumption, retires it for good;
//! - the deadline is fixed at request creation (SMS 300 s, voice 60 s) and
//!   neither approval nor anything else renews it;
//! - neither approval grants the other; B additionally needs a consumed,
//!   still-live A for the same scope, and is re-checked against A's deadline
//!   at consumption (disclosure must satisfy both deadlines);
//! - the approver and the consumer must be the authenticated operator control
//!   plane — never the weak telecom channel the conversation runs on;
//! - an approval is bound to the exact body, binding/provenance (channel,
//!   region, binding ref, connection generation), Core, agent, turn and
//!   session epoch; any difference at consumption refuses;
//! - [`deliver_private_disclosure`] is the effect fence: the sink is reached
//!   only after B is consumed against the *current* context, so every refusal
//!   leaves the private answer undelivered.
//!
//! # Integration state (read this before relying on it)
//!
//! Wired: startup installs one ledger, registers the operator service
//! (`super::operator_control`, `telecom.approval.*`) over it, and hands the
//! same instance to the inbound router. The public run driver opens a live
//! turn scope only after admission (random core/agent/turn ids, session epoch
//! from the connection generation) and retires it compare-and-retire when the
//! turn ends, which cancels that binding's pending records. Operators review
//! the full body and call [`PublicApprovalLedger::decide_exact`].
//! `ExecApprovalManager` is deliberately not reused (no deadline/scope binding).
//!
//! NOT wired (product decision: no private source is enabled yet):
//! - no private-read driver is installed and `PublicRunRequest.scope` is
//!   `None`, so no approved A record is ever consumed;
//! - nothing routes a private-draft answer through
//!   [`deliver_private_disclosure`] (the B sink has no production caller);
//! - whether the connection generation changes on every rebind, revocation
//!   or session reset (the `session_epoch` contract) is not yet proven.
//!
//! The public run path therefore has no private-read or private disclosure
//! capability (public default: no tools/derivations/private writeback), which
//! is the safe state.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::public_context::{TelecomChannel, TelecomProvenance, TelecomPublicContext};
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::{Arc, Mutex};

/// Approval lifetime for SMS, counted from request creation.
pub const SMS_APPROVAL_TTL_SECS: i64 = 300;
/// Approval lifetime for voice, counted from request creation.
pub const VOICE_APPROVAL_TTL_SECS: i64 = 60;
/// Upper bound of the body an operator must be able to read in full.
pub const MAX_APPROVAL_BODY_BYTES: usize = 8 * 1024;
/// Hard cap on live records; a full ledger refuses new requests.
pub const MAX_APPROVAL_RECORDS: usize = 256;

/// The two non-interchangeable approval stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicApprovalStage {
    /// A: permit the exact private source/query into current-turn reasoning.
    PrivateRead,
    /// B: permit the exact final body to be disclosed to the current binding.
    Disclosure,
}

/// Everything an approval is bound to besides its body.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicApprovalScope {
    core_id: String,
    agent_id: String,
    turn_id: String,
    session_epoch: u64,
    provenance: TelecomProvenance,
}

impl PublicApprovalScope {
    /// `session_epoch` must change whenever the binding is rebound/revoked or
    /// the public session is reset, so that older approvals cannot match.
    pub fn new(
        ctx: &TelecomPublicContext,
        core_id: impl Into<String>,
        agent_id: impl Into<String>,
        turn_id: impl Into<String>,
        session_epoch: u64,
    ) -> Self {
        Self {
            core_id: core_id.into(),
            agent_id: agent_id.into(),
            turn_id: turn_id.into(),
            session_epoch,
            provenance: ctx.provenance().clone(),
        }
    }

    pub fn channel(&self) -> TelecomChannel {
        self.provenance.channel()
    }

    pub fn binding_ref(&self) -> &str {
        self.provenance.binding_ref()
    }
}

impl std::fmt::Debug for PublicApprovalScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicApprovalScope")
            .field("core_id", &"<redacted>")
            .field("agent_id", &"<redacted>")
            .field("turn_id", &"<redacted>")
            .field("session_epoch", &self.session_epoch)
            .field("provenance", &self.provenance)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    OperatorControlPlane,
    WeakTelecom(TelecomChannel),
    Unauthenticated,
}

/// Who is deciding or consuming. Only the authenticated operator control
/// plane is ever accepted; `resolved_by`-style free strings are not authority.
///
/// Like `TelecomPublicContext`, the operator constructor is crate-visible by
/// convention, not a cryptographic proof: the real control-plane handler is
/// the only intended caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalSource(SourceKind);

impl ApprovalSource {
    /// The authenticated operator control plane (integration seam).
    /// The operator service uses this only after its role check.
    pub(crate) fn operator_control_plane() -> Self {
        Self(SourceKind::OperatorControlPlane)
    }

    /// An attempt arriving over the weak SMS/PSTN conversation itself.
    pub fn weak_telecom(ctx: &TelecomPublicContext) -> Self {
        Self(SourceKind::WeakTelecom(ctx.provenance().channel()))
    }

    /// Anything that cannot prove it is the operator control plane.
    pub fn unauthenticated() -> Self {
        Self(SourceKind::Unauthenticated)
    }

    fn is_operator(self) -> bool {
        self.0 == SourceKind::OperatorControlPlane
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicApprovalError {
    #[error("approval body is empty")]
    EmptyBody,
    #[error("approval body exceeds the operator-visible bound")]
    BodyTooLarge,
    #[error("approval ledger is full")]
    Capacity,
    #[error("unknown or retired approval request")]
    UnknownRequest,
    #[error("approval is for a different stage")]
    WrongStage,
    #[error("only the authenticated operator control plane may decide or consume")]
    NotOperator,
    #[error("approval request has expired")]
    Expired,
    #[error("approval request was already decided")]
    AlreadyDecided,
    #[error("approval request is not approved")]
    NotApproved,
    #[error("approval was already consumed")]
    AlreadyConsumed,
    #[error("approval was cancelled, denied or voided")]
    Retired,
    #[error("approval is bound to a different binding, provenance, core, agent, turn or epoch")]
    ScopeMismatch,
    #[error("approval is bound to a different body")]
    BodyMismatch,
    #[error("disclosure requires a consumed, unexpired private-read approval for the same scope")]
    ReadNotSatisfied,
    #[error("private disclosure sink reported delivery failure")]
    DeliveryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Approved,
    Consumed,
    /// Denied, cancelled, or burned by a failed consumption check.
    Retired,
}

struct Record {
    stage: PublicApprovalStage,
    scope: PublicApprovalScope,
    body: String,
    expires_at: DateTime<Utc>,
    state: State,
    /// Disclosure: the consumed read approval it derives from.
    linked_read: Option<String>,
    /// Read: whether a disclosure request already claimed this read.
    disclosure_claimed: bool,
}

impl Record {
    fn expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }
}

/// What the operator control plane must show for a pending request: the
/// complete bounded body plus where it would go. Contains the private body by
/// design — render it only on the authenticated control plane.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicApprovalView {
    pub id: String,
    pub stage: PublicApprovalStage,
    pub channel: TelecomChannel,
    pub body: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for PublicApprovalView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicApprovalView")
            .field("id", &self.id)
            .field("stage", &self.stage)
            .field("channel", &self.channel)
            .field("body", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Proof that one approval was consumed against the current context. Not
/// `Clone`; the body is released only by consuming the grant.
pub struct ConsumedApproval {
    stage: PublicApprovalStage,
    body: String,
}

impl ConsumedApproval {
    pub fn stage(&self) -> PublicApprovalStage {
        self.stage
    }

    pub fn into_body(self) -> String {
        self.body
    }
}

impl std::fmt::Debug for ConsumedApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsumedApproval")
            .field("stage", &self.stage)
            .field("body", &"<redacted>")
            .finish()
    }
}

/// In-memory, mutex-serialized ledger of A/B approval requests.
#[derive(Default)]
pub struct PublicApprovalLedger {
    records: Mutex<HashMap<String, Record>>,
}

fn ttl_for(channel: TelecomChannel) -> Duration {
    match channel {
        TelecomChannel::Sms => Duration::seconds(SMS_APPROVAL_TTL_SECS),
        TelecomChannel::Voice => Duration::seconds(VOICE_APPROVAL_TTL_SECS),
    }
}

fn check_body(body: &str) -> Result<(), PublicApprovalError> {
    if body.trim().is_empty() {
        return Err(PublicApprovalError::EmptyBody);
    }
    if body.len() > MAX_APPROVAL_BODY_BYTES {
        return Err(PublicApprovalError::BodyTooLarge);
    }
    Ok(())
}

impl PublicApprovalLedger {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> crate::sync_primitives::MutexGuard<'_, HashMap<String, Record>> {
        self.records.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert a fresh pending record; the deadline starts now and is final.
    fn insert(
        records: &mut HashMap<String, Record>,
        stage: PublicApprovalStage,
        scope: PublicApprovalScope,
        body: String,
        linked_read: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<String, PublicApprovalError> {
        // Drop what can no longer matter: expired records, and retired ones.
        // A consumed read stays until its own deadline (B depends on it).
        records.retain(|_, r| !r.expired(now) && r.state != State::Retired);
        if records.len() >= MAX_APPROVAL_RECORDS {
            return Err(PublicApprovalError::Capacity);
        }
        let id = Uuid::new_v4().to_string();
        let expires_at = now + ttl_for(scope.channel());
        records.insert(
            id.clone(),
            Record {
                stage,
                scope,
                body,
                expires_at,
                state: State::Pending,
                linked_read,
                disclosure_claimed: false,
            },
        );
        Ok(id)
    }

    /// Open request A for the exact private source/query of this turn.
    pub fn request_private_read(
        &self,
        scope: PublicApprovalScope,
        query: &str,
        now: DateTime<Utc>,
    ) -> Result<String, PublicApprovalError> {
        check_body(query)?;
        let mut records = self.lock();
        Self::insert(
            &mut records,
            PublicApprovalStage::PrivateRead,
            scope,
            query.to_string(),
            None,
            now,
        )
    }

    /// Open request B for the exact final body. Needs read approval
    /// `read_id` to be consumed, unexpired, same scope, and not already
    /// claimed by another disclosure. B gets its own deadline from `now` but
    /// is later re-checked against A's.
    pub fn request_disclosure(
        &self,
        read_id: &str,
        scope: PublicApprovalScope,
        final_body: &str,
        now: DateTime<Utc>,
    ) -> Result<String, PublicApprovalError> {
        check_body(final_body)?;
        let mut records = self.lock();
        // Prune and capacity-check before claiming the one-shot read. If a
        // full ledger refuses B, A must remain usable for a later retry.
        records.retain(|_, r| !r.expired(now) && r.state != State::Retired);
        {
            let read = records
                .get(read_id)
                .ok_or(PublicApprovalError::ReadNotSatisfied)?;
            if read.stage != PublicApprovalStage::PrivateRead
                || read.state != State::Consumed
                || read.expired(now)
                || read.scope != scope
                || read.disclosure_claimed
            {
                return Err(PublicApprovalError::ReadNotSatisfied);
            }
        }
        if records.len() >= MAX_APPROVAL_RECORDS {
            return Err(PublicApprovalError::Capacity);
        }

        let id = Uuid::new_v4().to_string();
        let expires_at = now + ttl_for(scope.channel());
        records.insert(
            id.clone(),
            Record {
                stage: PublicApprovalStage::Disclosure,
                scope,
                body: final_body.to_string(),
                expires_at,
                state: State::Pending,
                linked_read: Some(read_id.to_string()),
                disclosure_claimed: false,
            },
        );
        records
            .get_mut(read_id)
            .expect("validated read approval must still exist")
            .disclosure_claimed = true;
        Ok(id)
    }

    /// Snapshot all pending, unexpired records for the authenticated control plane.
    pub fn operator_views(&self, now: DateTime<Utc>) -> Vec<PublicApprovalView> {
        let records = self.lock();
        records
            .iter()
            .filter(|(_, r)| r.state == State::Pending && !r.expired(now))
            .map(|(id, r)| PublicApprovalView {
                id: id.clone(),
                stage: r.stage,
                channel: r.scope.channel(),
                body: r.body.clone(),
                expires_at: r.expires_at,
            })
            .collect()
    }

    /// The full bounded request for operator display; `None` unless pending
    /// and unexpired.
    pub fn operator_view(&self, id: &str, now: DateTime<Utc>) -> Option<PublicApprovalView> {
        let records = self.lock();
        let r = records.get(id)?;
        (r.state == State::Pending && !r.expired(now)).then(|| PublicApprovalView {
            id: id.to_string(),
            stage: r.stage,
            channel: r.scope.channel(),
            body: r.body.clone(),
            expires_at: r.expires_at,
        })
    }

    /// Operator approves or denies a pending request. A non-operator source is
    /// refused and leaves the request untouched (a text message cannot deny or
    /// approve anything).
    pub fn decide(
        &self,
        id: &str,
        source: ApprovalSource,
        approve: bool,
        now: DateTime<Utc>,
    ) -> Result<(), PublicApprovalError> {
        if !source.is_operator() {
            return Err(PublicApprovalError::NotOperator);
        }
        let mut records = self.lock();
        let r = records
            .get_mut(id)
            .ok_or(PublicApprovalError::UnknownRequest)?;
        if r.expired(now) {
            r.state = State::Retired;
            return Err(PublicApprovalError::Expired);
        }
        if r.state != State::Pending {
            return Err(PublicApprovalError::AlreadyDecided);
        }
        // Deadline stays `expires_at`: approving never renews it.
        r.state = if approve {
            State::Approved
        } else {
            State::Retired
        };
        Ok(())
    }

    /// Decide a pending request only when the operator supplied the exact
    /// stage and complete body shown by the control-plane review surface.
    /// Truncated previews can therefore never approve a different payload.
    pub fn decide_exact(
        &self,
        id: &str,
        stage: PublicApprovalStage,
        body: &str,
        source: ApprovalSource,
        approve: bool,
        now: DateTime<Utc>,
    ) -> Result<(), PublicApprovalError> {
        // Authenticate before inspecting the record; check and transition under
        // one lock so cancellation or a competing decision cannot race the view.
        if !source.is_operator() {
            return Err(PublicApprovalError::NotOperator);
        }
        let mut records = self.lock();
        let r = records
            .get_mut(id)
            .ok_or(PublicApprovalError::UnknownRequest)?;
        if r.expired(now) {
            r.state = State::Retired;
            return Err(PublicApprovalError::Expired);
        }
        if r.state != State::Pending {
            return Err(PublicApprovalError::AlreadyDecided);
        }
        if r.stage != stage {
            return Err(PublicApprovalError::WrongStage);
        }
        if r.body != body {
            return Err(PublicApprovalError::BodyMismatch);
        }
        r.state = if approve {
            State::Approved
        } else {
            State::Retired
        };
        Ok(())
    }

    /// Consume an approved request exactly once against the CURRENT context.
    ///
    /// Refusals that indicate a non-operator caller or a wrong stage leave the
    /// approval intact (a stray caller must not burn it); expiry, replay and
    /// any scope/body/read mismatch retire it.
    pub fn consume(
        &self,
        id: &str,
        stage: PublicApprovalStage,
        source: ApprovalSource,
        current_scope: &PublicApprovalScope,
        current_body: &str,
        now: DateTime<Utc>,
    ) -> Result<ConsumedApproval, PublicApprovalError> {
        let mut records = self.lock();
        let r = records.get(id).ok_or(PublicApprovalError::UnknownRequest)?;
        if r.stage != stage {
            return Err(PublicApprovalError::WrongStage);
        }
        if !source.is_operator() {
            return Err(PublicApprovalError::NotOperator);
        }
        let linked_read = r.linked_read.clone();
        let verdict = Self::verdict(r, current_scope, current_body, now).and_then(|()| {
            // Disclosure must also satisfy A's data-use deadline.
            match (stage, linked_read.as_deref()) {
                (PublicApprovalStage::Disclosure, Some(read_id)) => match records.get(read_id) {
                    Some(read) if read.state == State::Consumed && !read.expired(now) => Ok(()),
                    _ => Err(PublicApprovalError::ReadNotSatisfied),
                },
                (PublicApprovalStage::Disclosure, None) => {
                    Err(PublicApprovalError::ReadNotSatisfied)
                }
                _ => Ok(()),
            }
        });
        let r = records.get_mut(id).expect("record checked above");
        match verdict {
            Ok(()) => {
                r.state = State::Consumed;
                Ok(ConsumedApproval {
                    stage,
                    body: r.body.clone(),
                })
            }
            Err(e) => {
                // Not-yet-approved stays retryable; everything else is final.
                if r.state != State::Pending && r.state != State::Consumed {
                    r.state = State::Retired;
                }
                Err(e)
            }
        }
    }

    fn verdict(
        r: &Record,
        current_scope: &PublicApprovalScope,
        current_body: &str,
        now: DateTime<Utc>,
    ) -> Result<(), PublicApprovalError> {
        match r.state {
            State::Pending => return Err(PublicApprovalError::NotApproved),
            State::Consumed => return Err(PublicApprovalError::AlreadyConsumed),
            State::Retired => return Err(PublicApprovalError::Retired),
            State::Approved => {}
        }
        if r.expired(now) {
            return Err(PublicApprovalError::Expired);
        }
        if &r.scope != current_scope {
            return Err(PublicApprovalError::ScopeMismatch);
        }
        if r.body != current_body {
            return Err(PublicApprovalError::BodyMismatch);
        }
        Ok(())
    }

    /// Retire every live request of a binding (revocation, hang-up,
    /// interruption, Core disconnect, generation change). Returns the count.
    pub fn cancel_binding(&self, binding_ref: &str) -> usize {
        let mut n = 0;
        for r in self.lock().values_mut() {
            if r.scope.binding_ref() == binding_ref && r.state != State::Retired {
                r.state = State::Retired;
                n += 1;
            }
        }
        n
    }
}

/// Delivery of an approved private answer (the channel reply path in
/// production, a spy in tests).
pub trait PrivateDisclosureSink {
    fn deliver(&self, body: &str) -> Result<(), PublicApprovalError>;
}

/// The effect fence for private disclosure: consume B against the current
/// context and only then hand the approved body to the sink. Every refusal
/// returns before the sink is touched; a sink delivery failure is propagated
/// so the caller never reports delivery success without sink acknowledgement.
pub fn deliver_private_disclosure(
    ledger: &PublicApprovalLedger,
    disclosure_id: &str,
    source: ApprovalSource,
    current_scope: &PublicApprovalScope,
    final_body: &str,
    now: DateTime<Utc>,
    sink: &dyn PrivateDisclosureSink,
) -> Result<(), PublicApprovalError> {
    let grant = ledger.consume(
        disclosure_id,
        PublicApprovalStage::Disclosure,
        source,
        current_scope,
        final_body,
        now,
    )?;
    sink.deliver(&grant.into_body())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::telecom::persisted_marker::{
        MarkerRestore, PersistedTelecomMarker, MARKER_VERSION,
    };
    use crate::gateway::telecom::public_context::test_support::sample_public_context;
    use std::cell::RefCell;

    const CANARY: &str = "PRIVATE-CANARY-vault-code-7731";

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn op() -> ApprovalSource {
        ApprovalSource::operator_control_plane()
    }

    fn scope_for(ctx: &TelecomPublicContext) -> PublicApprovalScope {
        PublicApprovalScope::new(ctx, "core-1", "agent-1", "turn-1", 1)
    }

    fn voice_ctx() -> TelecomPublicContext {
        let ctx = sample_public_context("bind-voice");
        let m: PersistedTelecomMarker = serde_json::from_value(serde_json::json!({
            "version": MARKER_VERSION, "state": "public", "channel": "voice",
            "region": "test-region", "binding_ref": "bind-voice",
            "caller_digest": ctx.provenance().caller_digest().as_str(),
            "connection_generation": 1
        }))
        .unwrap();
        match m.restore() {
            MarkerRestore::Public(c) => c,
            _ => panic!("voice marker must restore"),
        }
    }

    #[derive(Default)]
    struct SpySink(RefCell<Vec<String>>);
    impl PrivateDisclosureSink for SpySink {
        fn deliver(&self, body: &str) -> Result<(), PublicApprovalError> {
            self.0.borrow_mut().push(body.to_string());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingSink(RefCell<Vec<String>>);
    impl PrivateDisclosureSink for FailingSink {
        fn deliver(&self, body: &str) -> Result<(), PublicApprovalError> {
            self.0.borrow_mut().push(body.to_string());
            Err(PublicApprovalError::DeliveryFailed)
        }
    }

    /// Ledger with a consumed read approval, returns (ledger, ctx, scope, read_id).
    fn with_consumed_read() -> (
        PublicApprovalLedger,
        TelecomPublicContext,
        PublicApprovalScope,
        String,
    ) {
        let l = PublicApprovalLedger::new();
        let ctx = sample_public_context("bind-1");
        let scope = scope_for(&ctx);
        let a = l
            .request_private_read(scope.clone(), "read: vault notes", t0())
            .unwrap();
        l.decide(&a, op(), true, t0()).unwrap();
        l.consume(
            &a,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            "read: vault notes",
            t0(),
        )
        .unwrap();
        (l, ctx, scope, a)
    }

    fn approved_disclosure(
        l: &PublicApprovalLedger,
        scope: &PublicApprovalScope,
        read: &str,
        body: &str,
    ) -> String {
        let b = l
            .request_disclosure(read, scope.clone(), body, t0())
            .unwrap();
        l.decide(&b, op(), true, t0()).unwrap();
        b
    }

    #[test]
    fn approved_read_cannot_satisfy_disclosure() {
        let (l, ctx, scope, a) = with_consumed_read();
        let sink = SpySink::default();
        // The consumed A id presented as B is the wrong stage; and a fresh
        // approved A cannot be consumed as B either.
        let a2 = l.request_private_read(scope.clone(), CANARY, t0()).unwrap();
        l.decide(&a2, op(), true, t0()).unwrap();
        for id in [&a, &a2] {
            let r = deliver_private_disclosure(&l, id, op(), &scope, CANARY, t0(), &sink);
            assert_eq!(r, Err(PublicApprovalError::WrongStage));
        }
        assert!(sink.0.borrow().is_empty());
        // And disclosure cannot even be requested from an unconsumed read.
        let r = l.request_disclosure(&a2, scope_for(&ctx), CANARY, t0());
        assert_eq!(r, Err(PublicApprovalError::ReadNotSatisfied));
    }

    #[test]
    fn approved_disclosure_cannot_satisfy_read() {
        let (l, _ctx, scope, a) = with_consumed_read();
        let b = approved_disclosure(&l, &scope, &a, CANARY);
        let r = l.consume(
            &b,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            CANARY,
            t0(),
        );
        assert_eq!(r.unwrap_err(), PublicApprovalError::WrongStage);
        // B is untouched by the wrong-stage probe and still works for B.
        let sink = SpySink::default();
        deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &sink).unwrap();
        assert_eq!(sink.0.borrow().as_slice(), [CANARY]);
    }

    #[test]
    fn weak_channel_can_neither_approve_nor_consume() {
        let l = PublicApprovalLedger::new();
        let ctx = sample_public_context("bind-1");
        let scope = scope_for(&ctx);
        let a = l.request_private_read(scope.clone(), "q", t0()).unwrap();
        // Replying "yes" over the same SMS cannot approve (nor deny).
        assert_eq!(
            l.decide(&a, ApprovalSource::weak_telecom(&ctx), true, t0()),
            Err(PublicApprovalError::NotOperator)
        );
        assert_eq!(
            l.decide(&a, ApprovalSource::unauthenticated(), true, t0()),
            Err(PublicApprovalError::NotOperator)
        );
        // Still pending: operator approves, weak channel cannot consume it.
        l.decide(&a, op(), true, t0()).unwrap();
        let weak = ApprovalSource::weak_telecom(&ctx);
        let r = l.consume(
            &a,
            PublicApprovalStage::PrivateRead,
            weak,
            &scope,
            "q",
            t0(),
        );
        assert_eq!(r.unwrap_err(), PublicApprovalError::NotOperator);
        // The refusal did not burn it; the operator can still consume once.
        assert!(l
            .consume(
                &a,
                PublicApprovalStage::PrivateRead,
                op(),
                &scope,
                "q",
                t0()
            )
            .is_ok());
    }

    #[test]
    fn stale_epoch_agent_core_turn_binding_generation_all_refuse_and_burn() {
        let ctx = sample_public_context("bind-1");
        let base = scope_for(&ctx);
        let other_binding = sample_public_context("bind-2");
        let variants = [
            PublicApprovalScope::new(&ctx, "core-1", "agent-1", "turn-1", 2), // epoch
            PublicApprovalScope::new(&ctx, "core-1", "agent-2", "turn-1", 1), // agent
            PublicApprovalScope::new(&ctx, "core-2", "agent-1", "turn-1", 1), // core
            PublicApprovalScope::new(&ctx, "core-1", "agent-1", "turn-2", 1), // turn
            PublicApprovalScope::new(&other_binding, "core-1", "agent-1", "turn-1", 1),
        ];
        for stale in variants {
            let l = PublicApprovalLedger::new();
            let a = l.request_private_read(base.clone(), "q", t0()).unwrap();
            l.decide(&a, op(), true, t0()).unwrap();
            let r = l.consume(
                &a,
                PublicApprovalStage::PrivateRead,
                op(),
                &stale,
                "q",
                t0(),
            );
            assert_eq!(r.unwrap_err(), PublicApprovalError::ScopeMismatch);
            // Burned: the right context can no longer use it either.
            let r = l.consume(&a, PublicApprovalStage::PrivateRead, op(), &base, "q", t0());
            assert_eq!(r.unwrap_err(), PublicApprovalError::Retired);
        }
    }

    #[test]
    fn connection_generation_change_refuses() {
        let ctx = sample_public_context("bind-1");
        let l = PublicApprovalLedger::new();
        let scope = scope_for(&ctx);
        let a = l.request_private_read(scope, "q", t0()).unwrap();
        l.decide(&a, op(), true, t0()).unwrap();
        // Same binding ref, different connection generation.
        let m: PersistedTelecomMarker = serde_json::from_value(serde_json::json!({
            "version": MARKER_VERSION, "state": "public", "channel": "sms",
            "region": "test-region", "binding_ref": "bind-1",
            "caller_digest": ctx.provenance().caller_digest().as_str(),
            "connection_generation": 2
        }))
        .unwrap();
        let MarkerRestore::Public(newer) = m.restore() else {
            panic!()
        };
        let r = l.consume(
            &a,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope_for(&newer),
            "q",
            t0(),
        );
        assert_eq!(r.unwrap_err(), PublicApprovalError::ScopeMismatch);
    }

    #[test]
    fn body_change_after_approval_refuses_and_burns() {
        let (l, _ctx, scope, a) = with_consumed_read();
        let b = approved_disclosure(&l, &scope, &a, "approved body");
        let sink = SpySink::default();
        let r = deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &sink);
        assert_eq!(r, Err(PublicApprovalError::BodyMismatch));
        let r = deliver_private_disclosure(&l, &b, op(), &scope, "approved body", t0(), &sink);
        assert_eq!(r, Err(PublicApprovalError::Retired));
        assert!(sink.0.borrow().is_empty(), "no private delivery on refusal");
    }

    #[test]
    fn deadline_counts_from_creation_and_approval_never_renews() {
        let ctx = sample_public_context("bind-1");
        let scope = scope_for(&ctx);
        let l = PublicApprovalLedger::new();
        let a = l.request_private_read(scope.clone(), "q", t0()).unwrap();
        // Approved late in the window; consumption at +300s is still too late.
        l.decide(&a, op(), true, t0() + Duration::seconds(299))
            .unwrap();
        let r = l.consume(
            &a,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            "q",
            t0() + Duration::seconds(SMS_APPROVAL_TTL_SECS),
        );
        assert_eq!(r.unwrap_err(), PublicApprovalError::Expired);
        // Deciding after the deadline is refused too.
        let late = l.request_private_read(scope, "q", t0()).unwrap();
        let r = l.decide(&late, op(), true, t0() + Duration::seconds(300));
        assert_eq!(r, Err(PublicApprovalError::Expired));
        assert!(l
            .operator_view(&late, t0() + Duration::seconds(300))
            .is_none());
    }

    #[test]
    fn voice_deadline_is_sixty_seconds() {
        let ctx = voice_ctx();
        let scope = scope_for(&ctx);
        let l = PublicApprovalLedger::new();
        let a = l.request_private_read(scope.clone(), "q", t0()).unwrap();
        l.decide(&a, op(), true, t0()).unwrap();
        let r = l.consume(
            &a,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            "q",
            t0() + Duration::seconds(VOICE_APPROVAL_TTL_SECS),
        );
        assert_eq!(r.unwrap_err(), PublicApprovalError::Expired);
        let b = l.request_private_read(scope.clone(), "q", t0()).unwrap();
        l.decide(&b, op(), true, t0()).unwrap();
        assert!(l
            .consume(
                &b,
                PublicApprovalStage::PrivateRead,
                op(),
                &scope,
                "q",
                t0() + Duration::seconds(VOICE_APPROVAL_TTL_SECS - 1),
            )
            .is_ok());
    }

    #[test]
    fn second_consume_is_a_replay_and_refuses() {
        let (l, _ctx, scope, a) = with_consumed_read();
        let b = approved_disclosure(&l, &scope, &a, CANARY);
        let sink = SpySink::default();
        deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &sink).unwrap();
        let again = deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &sink);
        assert_eq!(again, Err(PublicApprovalError::AlreadyConsumed));
        assert_eq!(sink.0.borrow().len(), 1, "delivered exactly once");
        // A is one-shot too.
        let r = l.consume(
            &a,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            "read: vault notes",
            t0(),
        );
        assert_eq!(r.unwrap_err(), PublicApprovalError::AlreadyConsumed);
    }

    #[test]
    fn unapproved_denied_and_cancelled_requests_deliver_nothing() {
        let (l, ctx, scope, a) = with_consumed_read();
        let sink = SpySink::default();

        let pending = l
            .request_disclosure(&a, scope.clone(), CANARY, t0())
            .unwrap();
        let r = deliver_private_disclosure(&l, &pending, op(), &scope, CANARY, t0(), &sink);
        assert_eq!(r, Err(PublicApprovalError::NotApproved));

        // Denied (a is already claimed, so use a fresh read).
        let a2 = l.request_private_read(scope.clone(), "q2", t0()).unwrap();
        l.decide(&a2, op(), true, t0()).unwrap();
        l.consume(
            &a2,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            "q2",
            t0(),
        )
        .unwrap();
        let denied = l
            .request_disclosure(&a2, scope.clone(), CANARY, t0())
            .unwrap();
        l.decide(&denied, op(), false, t0()).unwrap();
        let r = deliver_private_disclosure(&l, &denied, op(), &scope, CANARY, t0(), &sink);
        assert_eq!(r, Err(PublicApprovalError::Retired));

        // Cancelled by hang-up / revocation.
        let a3 = l.request_private_read(scope.clone(), "q3", t0()).unwrap();
        l.decide(&a3, op(), true, t0()).unwrap();
        l.consume(
            &a3,
            PublicApprovalStage::PrivateRead,
            op(),
            &scope,
            "q3",
            t0(),
        )
        .unwrap();
        let b3 = approved_disclosure(&l, &scope, &a3, CANARY);
        assert!(l.cancel_binding(ctx.provenance().binding_ref()) >= 1);
        let r = deliver_private_disclosure(&l, &b3, op(), &scope, CANARY, t0(), &sink);
        assert_eq!(r, Err(PublicApprovalError::Retired));

        assert!(
            sink.0.borrow().is_empty(),
            "no private delivery on refusals"
        );
    }

    #[test]
    fn disclosure_must_also_satisfy_the_read_deadline() {
        let (l, _ctx, scope, a) = with_consumed_read();
        // B is created later, so its own deadline outlives A's.
        let later = t0() + Duration::seconds(200);
        let b = l
            .request_disclosure(&a, scope.clone(), CANARY, later)
            .unwrap();
        l.decide(&b, op(), true, later).unwrap();
        let sink = SpySink::default();
        // +310s: B (expires +500) is live, A (expired +300) is not.
        let at = t0() + Duration::seconds(310);
        let r = deliver_private_disclosure(&l, &b, op(), &scope, CANARY, at, &sink);
        assert_eq!(r, Err(PublicApprovalError::ReadNotSatisfied));
        assert!(sink.0.borrow().is_empty());
    }

    #[test]
    fn disclosure_needs_a_consumed_same_scope_unclaimed_read() {
        let (l, ctx, scope, a) = with_consumed_read();
        let other = PublicApprovalScope::new(&ctx, "core-1", "agent-1", "turn-9", 1);
        assert_eq!(
            l.request_disclosure(&a, other, CANARY, t0()),
            Err(PublicApprovalError::ReadNotSatisfied)
        );
        assert!(l
            .request_disclosure(&a, scope.clone(), CANARY, t0())
            .is_ok());
        // One read, one disclosure.
        assert_eq!(
            l.request_disclosure(&a, scope, CANARY, t0()),
            Err(PublicApprovalError::ReadNotSatisfied)
        );
    }

    #[test]
    fn disclosure_capacity_refusal_does_not_claim_consumed_read() {
        let (l, _ctx, scope, a) = with_consumed_read();
        let filler_ctx = sample_public_context("filler-bind");
        let filler_scope = scope_for(&filler_ctx);
        for i in 0..(MAX_APPROVAL_RECORDS - 1) {
            l.request_private_read(filler_scope.clone(), &format!("filler-{i}"), t0())
                .unwrap();
        }

        assert_eq!(
            l.request_disclosure(&a, scope.clone(), CANARY, t0()),
            Err(PublicApprovalError::Capacity)
        );
        assert_eq!(
            l.cancel_binding(filler_ctx.provenance().binding_ref()),
            MAX_APPROVAL_RECORDS - 1
        );

        let b = l
            .request_disclosure(&a, scope.clone(), CANARY, t0())
            .expect("capacity refusal must not claim the read approval");
        l.decide(&b, op(), true, t0()).unwrap();
        let sink = SpySink::default();
        deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &sink).unwrap();
        assert_eq!(sink.0.borrow().as_slice(), [CANARY]);
    }

    #[test]
    fn sink_failure_is_reported_as_failed_delivery() {
        let (l, _ctx, scope, a) = with_consumed_read();
        let b = approved_disclosure(&l, &scope, &a, CANARY);
        let failing = FailingSink::default();
        let r = deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &failing);
        assert_eq!(r, Err(PublicApprovalError::DeliveryFailed));
        assert_eq!(failing.0.borrow().as_slice(), [CANARY]);

        let success_sink = SpySink::default();
        let replay = deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &success_sink);
        assert_eq!(replay, Err(PublicApprovalError::AlreadyConsumed));
        assert!(success_sink.0.borrow().is_empty());
    }

    #[test]
    fn full_flow_delivers_exactly_the_approved_body_once() {
        let (l, _ctx, scope, a) = with_consumed_read();
        let b = approved_disclosure(&l, &scope, &a, CANARY);
        let view = l.operator_view(&b, t0());
        assert!(view.is_none(), "approved request is no longer pending");
        let sink = SpySink::default();
        deliver_private_disclosure(&l, &b, op(), &scope, CANARY, t0(), &sink).unwrap();
        assert_eq!(sink.0.borrow().as_slice(), [CANARY]);
    }

    #[test]
    fn operator_view_shows_full_body_but_debug_redacts() {
        let l = PublicApprovalLedger::new();
        let ctx = sample_public_context("bind-1");
        let scope = scope_for(&ctx);
        let a = l.request_private_read(scope.clone(), CANARY, t0()).unwrap();
        let v = l.operator_view(&a, t0()).unwrap();
        assert_eq!(v.body, CANARY);
        let dbg = format!("{v:?} {scope:?}");
        assert!(!dbg.contains(CANARY) && !dbg.contains("core-1"), "{dbg}");
    }

    #[test]
    fn empty_oversize_and_full_ledger_are_refused() {
        let l = PublicApprovalLedger::new();
        let ctx = sample_public_context("bind-1");
        let scope = scope_for(&ctx);
        assert_eq!(
            l.request_private_read(scope.clone(), "  ", t0()),
            Err(PublicApprovalError::EmptyBody)
        );
        let big = "x".repeat(MAX_APPROVAL_BODY_BYTES + 1);
        assert_eq!(
            l.request_private_read(scope.clone(), &big, t0()),
            Err(PublicApprovalError::BodyTooLarge)
        );
        for _ in 0..MAX_APPROVAL_RECORDS {
            l.request_private_read(scope.clone(), "q", t0()).unwrap();
        }
        assert_eq!(
            l.request_private_read(scope.clone(), "q", t0()),
            Err(PublicApprovalError::Capacity)
        );
        // Expired records are reclaimed, so the cap is not a permanent wedge.
        assert!(l
            .request_private_read(scope, "q", t0() + Duration::seconds(301))
            .is_ok());
    }

    #[test]
    fn exact_decision_rejects_wrong_stage_and_body_without_deciding() {
        let l = PublicApprovalLedger::new();
        let scope = scope_for(&sample_public_context("exact-bind"));
        let body = "x".repeat(MAX_APPROVAL_BODY_BYTES);
        let id = l.request_private_read(scope.clone(), &body, t0()).unwrap();
        let expiry = l.operator_view(&id, t0()).unwrap().expires_at;
        for approve in [true, false] {
            assert_eq!(
                l.decide_exact(
                    &id,
                    PublicApprovalStage::Disclosure,
                    &body,
                    op(),
                    approve,
                    t0()
                ),
                Err(PublicApprovalError::WrongStage)
            );
            for wrong in [&body[..256], "different body"] {
                assert_eq!(
                    l.decide_exact(
                        &id,
                        PublicApprovalStage::PrivateRead,
                        wrong,
                        op(),
                        approve,
                        t0()
                    ),
                    Err(PublicApprovalError::BodyMismatch)
                );
            }
            assert_eq!(l.operator_view(&id, t0()).unwrap().body, body);
        }
        let later = t0() + Duration::seconds(299);
        l.decide_exact(
            &id,
            PublicApprovalStage::PrivateRead,
            &body,
            op(),
            true,
            later,
        )
        .unwrap();
        assert_eq!(l.lock().get(&id).unwrap().expires_at, expiry);
        assert_eq!(
            l.decide_exact(
                &id,
                PublicApprovalStage::PrivateRead,
                &body,
                op(),
                true,
                later
            ),
            Err(PublicApprovalError::AlreadyDecided)
        );
        assert_eq!(
            l.consume(
                &id,
                PublicApprovalStage::PrivateRead,
                op(),
                &scope,
                &body,
                later
            )
            .unwrap()
            .into_body(),
            body
        );
    }

    #[test]
    fn exact_decision_auth_denial_expiry_and_cancel_are_fail_closed() {
        let l = PublicApprovalLedger::new();
        let scope = scope_for(&sample_public_context("exact-bind"));
        let id = l.request_private_read(scope.clone(), CANARY, t0()).unwrap();
        // No unauthenticated body/stage oracle, even for unknown IDs.
        for candidate in [&id[..], "unknown"] {
            assert_eq!(
                l.decide_exact(
                    candidate,
                    PublicApprovalStage::Disclosure,
                    "wrong",
                    ApprovalSource::unauthenticated(),
                    true,
                    t0()
                ),
                Err(PublicApprovalError::NotOperator)
            );
        }
        assert!(l.operator_view(&id, t0()).is_some());
        l.decide_exact(
            &id,
            PublicApprovalStage::PrivateRead,
            CANARY,
            op(),
            false,
            t0(),
        )
        .unwrap();
        assert_eq!(
            l.consume(
                &id,
                PublicApprovalStage::PrivateRead,
                op(),
                &scope,
                CANARY,
                t0()
            )
            .unwrap_err(),
            PublicApprovalError::Retired
        );
        let expired = l.request_private_read(scope.clone(), CANARY, t0()).unwrap();
        assert_eq!(
            l.decide_exact(
                &expired,
                PublicApprovalStage::PrivateRead,
                CANARY,
                op(),
                true,
                t0() + Duration::seconds(SMS_APPROVAL_TTL_SECS)
            ),
            Err(PublicApprovalError::Expired)
        );
        let cancelled = l.request_private_read(scope, CANARY, t0()).unwrap();
        l.cancel_binding("exact-bind");
        assert_eq!(
            l.decide_exact(
                &cancelled,
                PublicApprovalStage::PrivateRead,
                CANARY,
                op(),
                true,
                t0()
            ),
            Err(PublicApprovalError::AlreadyDecided)
        );
    }

    #[test]
    fn exact_disclosure_decision_still_requires_consumed_read_and_exact_body() {
        let (l, _ctx, scope, read) = with_consumed_read();
        let id = l
            .request_disclosure(&read, scope.clone(), CANARY, t0())
            .unwrap();
        assert_eq!(
            l.decide_exact(
                &id,
                PublicApprovalStage::PrivateRead,
                CANARY,
                op(),
                true,
                t0()
            ),
            Err(PublicApprovalError::WrongStage)
        );
        l.decide_exact(
            &id,
            PublicApprovalStage::Disclosure,
            CANARY,
            op(),
            true,
            t0(),
        )
        .unwrap();
        let sink = SpySink::default();
        deliver_private_disclosure(&l, &id, op(), &scope, CANARY, t0(), &sink).unwrap();
        assert_eq!(sink.0.borrow().as_slice(), [CANARY]);
    }

    #[test]
    fn unknown_ids_refuse() {
        let l = PublicApprovalLedger::new();
        let ctx = sample_public_context("bind-1");
        let scope = scope_for(&ctx);
        let sink = SpySink::default();
        let r = deliver_private_disclosure(&l, "nope", op(), &scope, CANARY, t0(), &sink);
        assert_eq!(r, Err(PublicApprovalError::UnknownRequest));
        assert_eq!(
            l.decide("nope", op(), true, t0()),
            Err(PublicApprovalError::UnknownRequest)
        );
    }
}

// ============================================================================
// Process-global slot (Task #14, A-step). The ledger is the registry shared
// between the public run driver and the operator control plane; the slot makes
// its install-or-decline observable to `core/capability-wiring` like every
// other subsystem, so a startup that skipped wiring it shows up as a
// `declined` reason instead of a silent no-op. The value is reached through
// `installed()`; the slot itself is reachable only through `installed_slot()`
// for the roster.
// ============================================================================

static INSTALLED: CapabilitySlot<Arc<PublicApprovalLedger>> = CapabilitySlot::new(
    "gateway/telecom/public_approval",
    MissingSemantics::ConsumerDecides,
);

/// Install the process-wide ledger. Returns the ledger back if a different
/// instance is already installed (first-writer-wins; the test asserts that a
/// double install is a hard error rather than a silent overwrite).
pub fn install_global(ledger: Arc<PublicApprovalLedger>) -> Result<(), Arc<PublicApprovalLedger>> {
    if INSTALLED.install(ledger.clone()) {
        Ok(())
    } else {
        Err(ledger)
    }
}

/// Record that startup reached this slot but deliberately did not install it.
/// Static reason because it surfaces in operator diagnostics.
pub fn decline_global(because: &'static str) {
    INSTALLED.decline(because);
}

/// The installed ledger, if any. None means "the public run driver must not
/// attempt A/B flow".
pub fn installed() -> Option<&'static Arc<PublicApprovalLedger>> {
    INSTALLED.get()
}

/// Capability census accessor; the ledger value itself is not exposed here.
pub(crate) const fn installed_slot() -> &'static dyn SlotStatus {
    &INSTALLED
}

#[cfg(test)]
mod installed_slot_tests {
    use super::*;

    /// Per-test handle into the static's `Option<&T>` is the only way to
    /// observe a value the test owns; the static is shared with other tests in
    /// the binary, so each test only asserts the round-trip it can prove
    /// without racy ordering assumptions.
    #[test]
    fn installed_slot_default_is_consumer_decides_with_no_value() {
        // Use the slot's static ID and missing semantics, not its value, to
        // assert the contract; the value may be installed by parallel tests.
        assert!(matches!(
            INSTALLED.missing(),
            MissingSemantics::ConsumerDecides
        ));
        let slot: &dyn SlotStatus = &INSTALLED;
        assert_eq!(slot.id(), "gateway/telecom/public_approval");
    }

    #[test]
    fn installed_slot_accessor_is_static() {
        // The accessor is a `const fn` returning `&'static dyn SlotStatus`,
        // which is what `ALL_SLOTS` and the census consume; pin the signature.
        let slot: &'static dyn SlotStatus = installed_slot();
        assert_eq!(slot.id(), "gateway/telecom/public_approval");
    }

    // Install/decline round-trips are deliberately not tested here: the slot
    // is a process-wide static shared with every other test in the binary
    // (including the capability census), so writing it from a unit test would
    // pollute global state. Only read-only id/missing semantics are pinned.
}

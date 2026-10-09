//! Pure transport state machine for telecom relay events.
//!
//! No I/O, no clock reads (callers pass `now`), no persistence: the
//! [`DedupLedger`] is an in-memory model of what the durable store must
//! implement, and every transition is a plain function so it can be unit
//! tested exhaustively.
//!
//! Invariants enforced here (spec §6/§8):
//! - Dedup identity comes from provider identifiers, **never** from message
//!   body: two identical texts with different provider ids are two events.
//! - A receipt is keyed by its own `provider_event_id` (plus outcome), so the
//!   submitted/delivered/failed receipts of one `provider_message_id` are all
//!   kept.
//! - Events from a connection generation other than the active lease are
//!   fenced out before they touch any state.
//! - Outcomes only move forward; `Unknown` ≠ `Failed`; `OutcomeUnknownExpired`
//!   is a locally derived terminal state that nothing may auto-retry.
//! - An expired or non-committed event never admits an agent turn.

use std::collections::HashMap;

use aleph_protocol::telecom_relay::{EnvelopeError, TelecomEnvelope, TelecomKind, TelecomOutcome};
use chrono::{DateTime, Duration, Utc};

/// Why an event was refused outright (nothing recorded).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelecomError {
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    /// The envelope (or a binding repeated in its payload) names a binding
    /// other than the authenticated connection's.
    #[error("binding mismatch: connection is bound to {authenticated}, event claims {claimed}")]
    BindingMismatch {
        authenticated: String,
        claimed: String,
    },
    /// Event from an older connection lease; must not submit sends/playback.
    #[error("stale connection generation {got} (active {active})")]
    StaleGeneration { active: u64, got: u64 },
    /// Event claims a lease newer than the one we hold — also untrusted.
    #[error("future connection generation {got} (active {active})")]
    FutureGeneration { active: u64, got: u64 },
    /// `OutcomeUnknownExpired` is derived locally by [`expire_unknown`]; a
    /// peer cannot report it.
    #[error("outcome_unknown_expired is derived locally and cannot be reported")]
    ExpiryNotReportable,
}

/// Opaque, collision-free dedup identity (parts are compared as a vector, so
/// no delimiter can be spoofed by a provider id containing one).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DedupKey(Vec<String>);

/// Derive the dedup key of an event from provider identity only.
///
/// Per spec: provider account + event type + provider event id. For inbound
/// SMS the provider message id *is* the event id; for receipts it is the
/// receipt's own `provider_event_id` (plus the outcome it reports), never the
/// message id it refers to. The envelope's authenticated binding scopes it.
pub fn dedup_key(env: &TelecomEnvelope) -> DedupKey {
    let s = |v: &str| v.to_owned();
    let mut parts = vec![s(&env.binding_id)];
    match &env.kind {
        TelecomKind::SmsInbound(m) => {
            parts.extend([
                s(&m.provider_account_id),
                s("sms_inbound"),
                s(&m.provider_message_id),
            ]);
        }
        TelecomKind::SmsCommand(c) => {
            parts.extend([s("-"), s("sms_command"), s(&c.operation_id)]);
        }
        TelecomKind::CallTurn(t) => {
            parts.extend([
                s(&t.provider_account_id),
                s("call_turn"),
                s(&t.phone_call_id),
                s(&t.turn_id),
                format!("{:?}", t.role),
                format!("{:?}", t.transcript_state),
                t.output_epoch.to_string(),
                t.output_seq.to_string(),
            ]);
        }
        TelecomKind::PlaybackEvent(p) => {
            parts.extend([
                s(&p.provider_account_id),
                s("playback"),
                s(&p.phone_call_id),
                s(&p.turn_id),
                p.output_epoch.to_string(),
                p.output_seq.to_string(),
                format!("{:?}", p.state),
            ]);
        }
        TelecomKind::Outcome(o) => {
            parts.extend([
                s(&o.provider_account_id),
                s("outcome"),
                s(&o.provider_event_id),
                s(o.outcome.as_wire()),
            ]);
        }
    }
    DedupKey(parts)
}

/// In-memory model of the Relay/Core dedup horizon. Keys are retained for
/// `retention` (must cover the provider's maximum retry window).
#[derive(Debug, Clone)]
pub struct DedupLedger {
    retention: Duration,
    seen: HashMap<DedupKey, DateTime<Utc>>,
}

impl DedupLedger {
    pub fn new(retention: Duration) -> Self {
        Self {
            retention,
            seen: HashMap::new(),
        }
    }

    pub fn retention(&self) -> Duration {
        self.retention
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    /// Forget keys recorded at or before `now - retention`.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let horizon = self.retention;
        self.seen
            .retain(|_, first_seen| now - *first_seen < horizon);
    }
}

/// Facts about the connection an event arrived on. The connection — not the
/// payload — is authoritative for identity.
#[derive(Debug, Clone, Copy)]
pub struct AcceptContext<'a> {
    pub authenticated_binding: &'a str,
    pub active_generation: u64,
    pub now: DateTime<Utc>,
}

/// Verdict for an event that passed identity and fencing checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    /// First sighting, within retention and not expired. Recorded.
    Fresh(DedupKey),
    /// Already recorded; do not reprocess.
    Duplicate(DedupKey),
    /// Older than the dedup horizon: log as stale/unmatched, **not** recorded,
    /// must not change any closed result.
    Stale(DedupKey),
    /// Past `expires_at` (inbound/command kinds). Recorded so a redelivery is
    /// a duplicate, but it must not produce a semantic reply or a send.
    Expired(DedupKey),
}

impl Acceptance {
    /// May this event start an agent turn? Only a fresh inbound SMS or a fresh
    /// *committed* caller transcript — never duplicates, stale/expired events,
    /// interim transcripts, receipts, or playback feedback.
    pub fn admits_agent_turn(&self, env: &TelecomEnvelope) -> bool {
        if !matches!(self, Self::Fresh(_)) {
            return false;
        }
        match &env.kind {
            TelecomKind::SmsInbound(_) => true,
            TelecomKind::CallTurn(t) => t.starts_agent_turn(),
            TelecomKind::SmsCommand(_)
            | TelecomKind::PlaybackEvent(_)
            | TelecomKind::Outcome(_) => false,
        }
    }
}

/// Fence a frame's connection generation against the active lease.
pub fn fence_generation(active: u64, got: u64) -> Result<(), TelecomError> {
    match got.cmp(&active) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Less => Err(TelecomError::StaleGeneration { active, got }),
        std::cmp::Ordering::Greater => Err(TelecomError::FutureGeneration { active, got }),
    }
}

/// Admit one event: version → authenticated binding → payload binding →
/// generation fence → dedup/horizon → expiry. A refused event records nothing.
pub fn accept_event(
    ledger: &mut DedupLedger,
    ctx: &AcceptContext<'_>,
    env: &TelecomEnvelope,
) -> Result<Acceptance, TelecomError> {
    env.check_version()?;

    let mismatch = |claimed: &str| TelecomError::BindingMismatch {
        authenticated: ctx.authenticated_binding.to_owned(),
        claimed: claimed.to_owned(),
    };
    if env.binding_id != ctx.authenticated_binding {
        return Err(mismatch(&env.binding_id));
    }
    if let TelecomKind::SmsCommand(c) = &env.kind {
        if c.recipient_binding_id != ctx.authenticated_binding {
            return Err(mismatch(&c.recipient_binding_id));
        }
    }

    fence_generation(ctx.active_generation, env.connection_generation)?;

    let key = dedup_key(env);
    if ledger.seen.contains_key(&key) {
        return Ok(Acceptance::Duplicate(key));
    }
    if ctx.now - env.created_at > ledger.retention {
        return Ok(Acceptance::Stale(key));
    }
    ledger.seen.insert(key.clone(), ctx.now);

    // Receipts and playback feedback describe something that already
    // happened; their value does not decay with the frame's expiry. Inbound
    // text, call turns and send commands do.
    let decays = matches!(
        env.kind,
        TelecomKind::SmsInbound(_) | TelecomKind::SmsCommand(_) | TelecomKind::CallTurn(_)
    );
    if decays && ctx.now >= env.expires_at {
        return Ok(Acceptance::Expired(key));
    }
    Ok(Acceptance::Fresh(key))
}

/// Why an incoming outcome did not change the recorded one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    /// Out-of-order report of an earlier stage (e.g. `Submitted` after `Delivered`).
    Regressed,
    /// A different terminal fact than the recorded terminal one; surfaced for
    /// audit instead of silently overwriting.
    Contradiction,
    /// The result is closed (`OutcomeUnknownExpired`); late receipts are
    /// non-mutating.
    Closed,
}

/// Result of [`advance_outcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeTransition {
    Applied {
        from: Option<TelecomOutcome>,
        to: TelecomOutcome,
    },
    /// Same outcome reported again.
    Unchanged,
    Ignored(IgnoreReason),
}

fn is_terminal(o: TelecomOutcome) -> bool {
    use TelecomOutcome as O;
    matches!(
        o,
        O::Delivered | O::Played | O::Failed | O::Cancelled | O::OutcomeUnknownExpired
    )
}

fn stage_rank(o: TelecomOutcome) -> u8 {
    use TelecomOutcome as O;
    match o {
        O::Accepted => 0,
        O::Submitted => 1,
        O::Connected => 2,
        // Not part of the linear progression; handled explicitly.
        _ => u8::MAX,
    }
}

/// Monotonic outcome transition for a reported `incoming` outcome.
///
/// `Unknown` is a holding state, not a failure: any real fact may resolve it.
/// `OutcomeUnknownExpired` cannot be *reported* — see [`expire_unknown`].
pub fn advance_outcome(
    current: Option<TelecomOutcome>,
    incoming: TelecomOutcome,
) -> Result<OutcomeTransition, TelecomError> {
    use TelecomOutcome as O;
    if incoming == O::OutcomeUnknownExpired {
        return Err(TelecomError::ExpiryNotReportable);
    }
    let applied = |to| Ok(OutcomeTransition::Applied { from: current, to });
    let ignored = |r| Ok(OutcomeTransition::Ignored(r));

    let Some(cur) = current else {
        return applied(incoming);
    };
    if cur == incoming {
        return Ok(OutcomeTransition::Unchanged);
    }
    if cur == O::OutcomeUnknownExpired {
        return ignored(IgnoreReason::Closed);
    }
    if is_terminal(cur) {
        return if is_terminal(incoming) {
            ignored(IgnoreReason::Contradiction)
        } else {
            ignored(IgnoreReason::Regressed)
        };
    }
    if cur == O::Unknown {
        return if incoming == O::Accepted {
            ignored(IgnoreReason::Regressed)
        } else {
            applied(incoming)
        };
    }
    // cur is Accepted | Submitted | Connected.
    if is_terminal(incoming) {
        return applied(incoming);
    }
    if incoming == O::Unknown {
        return if cur == O::Connected {
            ignored(IgnoreReason::Regressed)
        } else {
            applied(incoming)
        };
    }
    if stage_rank(incoming) > stage_rank(cur) {
        applied(incoming)
    } else {
        ignored(IgnoreReason::Regressed)
    }
}

/// A recorded outcome and when it was entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutcomeRecord {
    pub outcome: TelecomOutcome,
    pub entered_at: DateTime<Utc>,
}

/// Result of [`expire_unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownExpiry {
    /// The record is not `Unknown`; nothing to expire.
    NotUnknown,
    /// Still within retention; keep waiting for a receipt / status query.
    Pending { expires_at: DateTime<Utc> },
    /// Retention elapsed: the new terminal record (entered at the deadline).
    Expired(OutcomeRecord),
}

/// Turn a bounded `Unknown` into the explicit terminal
/// `OutcomeUnknownExpired` once `max_unknown` has elapsed. Never schedules a
/// resend; see [`follow_up`].
pub fn expire_unknown(
    rec: &OutcomeRecord,
    now: DateTime<Utc>,
    max_unknown: Duration,
) -> UnknownExpiry {
    if rec.outcome != TelecomOutcome::Unknown {
        return UnknownExpiry::NotUnknown;
    }
    let deadline = rec.entered_at + max_unknown;
    if now >= deadline {
        UnknownExpiry::Expired(OutcomeRecord {
            outcome: TelecomOutcome::OutcomeUnknownExpired,
            entered_at: deadline,
        })
    } else {
        UnknownExpiry::Pending {
            expires_at: deadline,
        }
    }
}

/// What the system may do next for an operation in a given outcome. There is
/// deliberately no "retry" variant: a resend is a *new* operation (new
/// `operation_id`) that must pass approval again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    /// Wait for a receipt / query status. Do not resend.
    AwaitReceipt,
    /// Delivered/played — nothing further.
    Done,
    /// Closed without confirmed effect; only a brand-new approved operation
    /// may follow.
    NewApprovedOperationOnly,
}

pub fn follow_up(outcome: TelecomOutcome) -> FollowUp {
    use TelecomOutcome as O;
    match outcome {
        O::Accepted | O::Submitted | O::Connected | O::Unknown => FollowUp::AwaitReceipt,
        O::Delivered | O::Played => FollowUp::Done,
        O::Failed | O::Cancelled | O::OutcomeUnknownExpired => FollowUp::NewApprovedOperationOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::telecom_relay::{SmsEncoding, SmsInbound, TELECOM_PROTOCOL_VERSION};
    use chrono::TimeZone;

    fn env(msg_id: &str, body: &str) -> TelecomEnvelope {
        let t = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        TelecomEnvelope {
            protocol_version: TELECOM_PROTOCOL_VERSION,
            binding_id: "b".into(),
            connection_generation: 1,
            event_id: "e".into(),
            kind: TelecomKind::SmsInbound(SmsInbound {
                provider_account_id: "a".into(),
                provider_message_id: msg_id.into(),
                service_address: "+1".into(),
                peer_address: "+2".into(),
                body: body.into(),
                encoding: SmsEncoding::Gsm7,
                segment_count: 1,
            }),
            created_at: t,
            expires_at: t + Duration::hours(1),
        }
    }

    #[test]
    fn dedup_key_parts_cannot_collide_through_delimiters() {
        // Parts are a vector, so ids that look like a joined key still differ.
        assert_ne!(dedup_key(&env("m|1", "x")), dedup_key(&env("m", "x")));
    }

    #[test]
    fn dedup_key_ignores_body() {
        assert_eq!(dedup_key(&env("m1", "a")), dedup_key(&env("m1", "b")));
    }

    #[test]
    fn fence_orders_correctly() {
        assert!(fence_generation(3, 3).is_ok());
        assert!(matches!(
            fence_generation(3, 2),
            Err(TelecomError::StaleGeneration { .. })
        ));
        assert!(matches!(
            fence_generation(3, 4),
            Err(TelecomError::FutureGeneration { .. })
        ));
    }
}

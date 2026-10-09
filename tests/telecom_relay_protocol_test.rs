//! Contract tests for the Core-side telecom transport guards
//! (`alephcore::gateway::telecom`): dedup, generation fencing, outcome
//! monotonicity, unknown expiry and event expiry.
//!
//! Spec: `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md` §6, §8.

use aleph_protocol::telecom_relay::{
    CallTurn, CallTurnRole, OutcomeReport, SmsCommand, SmsEncoding, SmsInbound, TelecomEnvelope,
    TelecomKind, TelecomOutcome, TranscriptState, TELECOM_PROTOCOL_VERSION,
};
use alephcore::gateway::telecom::{
    accept_event, advance_outcome, dedup_key, expire_unknown, fence_generation, follow_up,
    AcceptContext, Acceptance, DedupLedger, FollowUp, IgnoreReason, OutcomeRecord,
    OutcomeTransition, TelecomError, UnknownExpiry,
};
use chrono::{DateTime, Duration, TimeZone, Utc};

const BINDING: &str = "bind-1";
const GEN: u64 = 5;

fn t0() -> DateTime<Utc> {
    Utc.timestamp_opt(1_800_000_000, 0).unwrap()
}

fn envelope(event_id: &str, kind: TelecomKind) -> TelecomEnvelope {
    TelecomEnvelope {
        protocol_version: TELECOM_PROTOCOL_VERSION,
        binding_id: BINDING.into(),
        connection_generation: GEN,
        event_id: event_id.into(),
        kind,
        created_at: t0(),
        expires_at: t0() + Duration::hours(24),
    }
}

fn sms(message_id: &str, body: &str) -> TelecomKind {
    TelecomKind::SmsInbound(SmsInbound {
        provider_account_id: "acct".into(),
        provider_message_id: message_id.into(),
        service_address: "+8613800000000".into(),
        peer_address: "+8613900000000".into(),
        body: body.into(),
        encoding: SmsEncoding::Gsm7,
        segment_count: 1,
    })
}

fn receipt(event_id: &str, message_id: &str, outcome: TelecomOutcome) -> TelecomKind {
    TelecomKind::Outcome(OutcomeReport {
        provider_account_id: "acct".into(),
        provider_event_id: event_id.into(),
        operation_id: "op-1".into(),
        provider_message_id: Some(message_id.into()),
        provider_call_id: None,
        outcome,
        error_code: None,
    })
}

fn ctx(now: DateTime<Utc>) -> AcceptContext<'static> {
    AcceptContext {
        authenticated_binding: BINDING,
        active_generation: GEN,
        now,
    }
}

fn ledger() -> DedupLedger {
    DedupLedger::new(Duration::days(7))
}

// ---- dedup ---------------------------------------------------------------

#[test]
fn duplicate_webhook_is_reported_duplicate_and_not_reprocessed() {
    let mut l = ledger();
    let env = envelope("e1", sms("m1", "hello"));
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &env).unwrap(),
        Acceptance::Fresh(_)
    ));
    // Provider retry: same provider message id, even with a new relay frame id.
    let retry = envelope("e1-retry", sms("m1", "hello"));
    let again = accept_event(&mut l, &ctx(t0()), &retry).unwrap();
    assert!(matches!(again, Acceptance::Duplicate(_)));
    assert!(!again.admits_agent_turn(&retry));
}

#[test]
fn identical_body_with_different_message_id_is_not_deduplicated() {
    let mut l = ledger();
    let a = envelope("e1", sms("m1", "yes"));
    let b = envelope("e2", sms("m2", "yes"));
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &a).unwrap(),
        Acceptance::Fresh(_)
    ));
    let second = accept_event(&mut l, &ctx(t0()), &b).unwrap();
    assert!(
        matches!(second, Acceptance::Fresh(_)),
        "same text twice is two events"
    );
    assert!(second.admits_agent_turn(&b));
    assert_ne!(dedup_key(&a), dedup_key(&b));
}

#[test]
fn dedup_key_ignores_body_and_is_scoped_by_provider_account() {
    let a = envelope("e1", sms("m1", "first body"));
    let b = envelope("e2", sms("m1", "completely different body"));
    assert_eq!(dedup_key(&a), dedup_key(&b));

    let mut other_acct = sms("m1", "x");
    if let TelecomKind::SmsInbound(s) = &mut other_acct {
        s.provider_account_id = "acct-2".into();
    }
    assert_ne!(dedup_key(&a), dedup_key(&envelope("e3", other_acct)));
}

#[test]
fn receipts_for_one_provider_message_are_all_kept() {
    let mut l = ledger();
    for (ev, outcome) in [
        ("r1", TelecomOutcome::Submitted),
        ("r2", TelecomOutcome::Delivered),
    ] {
        let env = envelope(ev, receipt(ev, "m1", outcome));
        assert!(
            matches!(
                accept_event(&mut l, &ctx(t0()), &env).unwrap(),
                Acceptance::Fresh(_)
            ),
            "{ev} must not be swallowed by the shared provider_message_id"
        );
    }
    // Same provider message id AND same receipt event id again => duplicate.
    let dup = envelope("r2-again", receipt("r2", "m1", TelecomOutcome::Delivered));
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &dup).unwrap(),
        Acceptance::Duplicate(_)
    ));
}

#[test]
fn same_receipt_event_id_with_different_outcome_is_a_different_event() {
    let mut l = ledger();
    let a = envelope("x", receipt("same", "m1", TelecomOutcome::Submitted));
    let b = envelope("y", receipt("same", "m1", TelecomOutcome::Delivered));
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &a).unwrap(),
        Acceptance::Fresh(_)
    ));
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &b).unwrap(),
        Acceptance::Fresh(_)
    ));
}

#[test]
fn late_callback_beyond_dedup_horizon_is_stale_and_non_mutating() {
    let mut l = DedupLedger::new(Duration::days(7));
    let env = envelope("e1", sms("m1", "late"));
    let now = t0() + Duration::days(8);
    let verdict = accept_event(&mut l, &ctx(now), &env).unwrap();
    assert!(matches!(verdict, Acceptance::Stale(_)));
    assert!(!verdict.admits_agent_turn(&env));
    assert_eq!(l.len(), 0, "stale callbacks must not mutate the ledger");
}

#[test]
fn prune_forgets_keys_only_past_retention() {
    let mut l = DedupLedger::new(Duration::days(7));
    let env = envelope("e1", sms("m1", "x"));
    accept_event(&mut l, &ctx(t0()), &env).unwrap();
    l.prune(t0() + Duration::days(6));
    assert_eq!(l.len(), 1);
    l.prune(t0() + Duration::days(8));
    assert_eq!(l.len(), 0);
}

// ---- generation / identity -----------------------------------------------

#[test]
fn stale_generation_is_rejected() {
    let mut l = ledger();
    let mut env = envelope("e1", sms("m1", "x"));
    env.connection_generation = GEN - 1;
    assert_eq!(
        accept_event(&mut l, &ctx(t0()), &env),
        Err(TelecomError::StaleGeneration {
            active: GEN,
            got: GEN - 1
        })
    );
    assert_eq!(l.len(), 0, "a fenced event leaves no trace");
}

#[test]
fn future_generation_is_also_rejected() {
    assert_eq!(
        fence_generation(GEN, GEN + 1),
        Err(TelecomError::FutureGeneration {
            active: GEN,
            got: GEN + 1
        })
    );
    assert_eq!(fence_generation(GEN, GEN), Ok(()));
}

#[test]
fn payload_binding_must_match_authenticated_connection() {
    let mut l = ledger();
    let mut env = envelope("e1", sms("m1", "x"));
    env.binding_id = "someone-else".into();
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &env),
        Err(TelecomError::BindingMismatch { .. })
    ));

    let cmd = |recipient: &str| {
        envelope(
            "e2",
            TelecomKind::SmsCommand(SmsCommand {
                operation_id: "op-1".into(),
                approval_reference: None,
                recipient_binding_id: recipient.into(),
                expires_at: t0() + Duration::minutes(5),
                body: "hi".into(),
                encoding: SmsEncoding::Gsm7,
                segment_count: 1,
            }),
        )
    };
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &cmd("other")),
        Err(TelecomError::BindingMismatch { .. })
    ));
    assert!(accept_event(&mut l, &ctx(t0()), &cmd(BINDING)).is_ok());
}

#[test]
fn unsupported_version_is_rejected_by_accept_event() {
    let mut l = ledger();
    let mut env = envelope("e1", sms("m1", "x"));
    env.protocol_version = 99;
    assert!(matches!(
        accept_event(&mut l, &ctx(t0()), &env),
        Err(TelecomError::Envelope(_))
    ));
}

// ---- expiry --------------------------------------------------------------

#[test]
fn expired_event_is_recorded_but_never_triggers_a_reply() {
    let mut l = ledger();
    let env = envelope("e1", sms("m1", "queued too long"));
    let now = t0() + Duration::hours(25);
    let verdict = accept_event(&mut l, &ctx(now), &env).unwrap();
    assert!(matches!(verdict, Acceptance::Expired(_)));
    assert!(!verdict.admits_agent_turn(&env));
    // Redelivery stays non-fresh.
    assert!(matches!(
        accept_event(&mut l, &ctx(now), &env).unwrap(),
        Acceptance::Duplicate(_)
    ));
}

#[test]
fn late_delivery_receipt_is_not_dropped_for_age() {
    let mut l = ledger();
    let env = envelope("r1", receipt("r1", "m1", TelecomOutcome::Delivered));
    let now = t0() + Duration::hours(30);
    assert!(matches!(
        accept_event(&mut l, &ctx(now), &env).unwrap(),
        Acceptance::Fresh(_)
    ));
}

#[test]
fn only_committed_user_transcript_admits_an_agent_turn() {
    let mut l = ledger();
    let turn = |ev: &str, state, seq| {
        envelope(
            ev,
            TelecomKind::CallTurn(CallTurn {
                provider_account_id: "acct".into(),
                phone_call_id: "call-1".into(),
                provider_call_id: "pc-1".into(),
                turn_id: "t-1".into(),
                role: CallTurnRole::UserTranscript,
                transcript_state: state,
                output_epoch: 1,
                output_seq: seq,
                text: "hi".into(),
            }),
        )
    };
    let interim = turn("e1", TranscriptState::Interim, 0);
    let committed = turn("e2", TranscriptState::Committed, 1);
    let v1 = accept_event(&mut l, &ctx(t0()), &interim).unwrap();
    let v2 = accept_event(&mut l, &ctx(t0()), &committed).unwrap();
    assert!(matches!(v1, Acceptance::Fresh(_)));
    assert!(!v1.admits_agent_turn(&interim));
    assert!(v2.admits_agent_turn(&committed));
}

// ---- outcomes ------------------------------------------------------------

use TelecomOutcome as O;

fn applied(from: Option<O>, to: O) -> OutcomeTransition {
    OutcomeTransition::Applied { from, to }
}

#[test]
fn outcome_progresses_forward_and_stays_idempotent() {
    assert_eq!(
        advance_outcome(None, O::Accepted),
        Ok(applied(None, O::Accepted))
    );
    assert_eq!(
        advance_outcome(Some(O::Accepted), O::Submitted),
        Ok(applied(Some(O::Accepted), O::Submitted))
    );
    assert_eq!(
        advance_outcome(Some(O::Submitted), O::Delivered),
        Ok(applied(Some(O::Submitted), O::Delivered))
    );
    assert_eq!(
        advance_outcome(Some(O::Delivered), O::Delivered),
        Ok(OutcomeTransition::Unchanged)
    );
    assert_eq!(
        advance_outcome(Some(O::Connected), O::Played),
        Ok(applied(Some(O::Connected), O::Played))
    );
}

#[test]
fn out_of_order_receipt_does_not_regress_state() {
    assert_eq!(
        advance_outcome(Some(O::Delivered), O::Submitted),
        Ok(OutcomeTransition::Ignored(IgnoreReason::Regressed))
    );
    assert_eq!(
        advance_outcome(Some(O::Submitted), O::Accepted),
        Ok(OutcomeTransition::Ignored(IgnoreReason::Regressed))
    );
}

#[test]
fn contradictory_terminal_outcomes_are_surfaced_not_overwritten() {
    assert_eq!(
        advance_outcome(Some(O::Delivered), O::Failed),
        Ok(OutcomeTransition::Ignored(IgnoreReason::Contradiction))
    );
    assert_eq!(
        advance_outcome(Some(O::Failed), O::Delivered),
        Ok(OutcomeTransition::Ignored(IgnoreReason::Contradiction))
    );
}

#[test]
fn unknown_is_not_failure_and_can_be_resolved_by_a_real_receipt() {
    assert_eq!(
        advance_outcome(Some(O::Submitted), O::Unknown),
        Ok(applied(Some(O::Submitted), O::Unknown))
    );
    assert_eq!(
        advance_outcome(Some(O::Unknown), O::Delivered),
        Ok(applied(Some(O::Unknown), O::Delivered))
    );
    assert_eq!(
        advance_outcome(Some(O::Unknown), O::Failed),
        Ok(applied(Some(O::Unknown), O::Failed))
    );
}

#[test]
fn expiry_is_a_local_decision_never_a_reported_outcome() {
    assert_eq!(
        advance_outcome(Some(O::Unknown), O::OutcomeUnknownExpired),
        Err(TelecomError::ExpiryNotReportable)
    );
    assert_eq!(
        advance_outcome(None, O::OutcomeUnknownExpired),
        Err(TelecomError::ExpiryNotReportable)
    );
}

#[test]
fn unknown_expires_after_retention_and_is_never_resent() {
    let rec = OutcomeRecord {
        outcome: O::Unknown,
        entered_at: t0(),
    };
    let max = Duration::hours(6);

    match expire_unknown(&rec, t0() + Duration::hours(1), max) {
        UnknownExpiry::Pending { expires_at } => assert_eq!(expires_at, t0() + max),
        other => panic!("expected Pending, got {other:?}"),
    }
    let expired = match expire_unknown(&rec, t0() + max, max) {
        UnknownExpiry::Expired(r) => r,
        other => panic!("expected Expired, got {other:?}"),
    };
    assert_eq!(expired.outcome, O::OutcomeUnknownExpired);
    assert_eq!(expired.entered_at, t0() + max);

    // Not a failure to blindly retry: only a brand-new approved operation may follow.
    assert_eq!(follow_up(O::Unknown), FollowUp::AwaitReceipt);
    assert_eq!(
        follow_up(O::OutcomeUnknownExpired),
        FollowUp::NewApprovedOperationOnly
    );

    // A late receipt cannot reopen or rewrite the closed result.
    assert_eq!(
        advance_outcome(Some(O::OutcomeUnknownExpired), O::Delivered),
        Ok(OutcomeTransition::Ignored(IgnoreReason::Closed))
    );
}

#[test]
fn expire_unknown_only_touches_unknown_records() {
    let rec = OutcomeRecord {
        outcome: O::Submitted,
        entered_at: t0(),
    };
    assert_eq!(
        expire_unknown(&rec, t0() + Duration::days(30), Duration::hours(1)),
        UnknownExpiry::NotUnknown
    );
}

#[test]
fn follow_up_distinguishes_every_outcome() {
    for (o, want) in [
        (O::Accepted, FollowUp::AwaitReceipt),
        (O::Submitted, FollowUp::AwaitReceipt),
        (O::Unknown, FollowUp::AwaitReceipt),
        (O::Connected, FollowUp::AwaitReceipt),
        (O::Delivered, FollowUp::Done),
        (O::Played, FollowUp::Done),
        (O::Failed, FollowUp::NewApprovedOperationOnly),
        (O::Cancelled, FollowUp::NewApprovedOperationOnly),
        (O::OutcomeUnknownExpired, FollowUp::NewApprovedOperationOnly),
    ] {
        assert_eq!(follow_up(o), want, "{o:?}");
    }
}

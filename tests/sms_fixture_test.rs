//! SMS inbound fixtures: Relay envelope -> InboundMessage, provider event
//! identity, and duplicate-safe ingestion. Frames are built in-process; no
//! provider or network is involved.

use aleph_protocol::telecom_relay::{
    OutcomeReport, SmsEncoding, SmsInbound, TelecomEnvelope, TelecomKind, TelecomOutcome,
    TELECOM_PROTOCOL_VERSION,
};
use alephcore::gateway::channel::{Channel, MessageMeta};
use alephcore::gateway::interfaces::sms::message_ops::{
    convert_inbound, scoped_message_id, SmsError, SmsInboundMeta, SmsOutcomeMeta,
};
use alephcore::gateway::interfaces::sms::{SmsChannel, SmsConfig, SmsIngest};
use chrono::{DateTime, Duration, Utc};

const BINDING: &str = "core-binding-1";
const SERVICE: &str = "+8613800000000";
const PEER: &str = "+14155550100";

fn config() -> SmsConfig {
    serde_json::from_value(serde_json::json!({
        "region": "CN",
        "service_address": SERVICE,
        "binding_ref": BINDING,
    }))
    .unwrap()
}

fn inbound(provider_message_id: &str, body: &str, now: DateTime<Utc>) -> TelecomEnvelope {
    TelecomEnvelope {
        protocol_version: TELECOM_PROTOCOL_VERSION,
        binding_id: BINDING.into(),
        connection_generation: 7,
        event_id: format!("evt-{provider_message_id}"),
        kind: TelecomKind::SmsInbound(SmsInbound {
            provider_account_id: "acct-1".into(),
            provider_message_id: provider_message_id.into(),
            service_address: SERVICE.into(),
            peer_address: PEER.into(),
            body: body.into(),
            encoding: SmsEncoding::Gsm7,
            segment_count: 1,
        }),
        created_at: now - Duration::seconds(5),
        expires_at: now + Duration::seconds(300),
    }
}

#[test]
fn convert_inbound_preserves_metadata_and_provider_identity() {
    let now = Utc::now();
    let env = inbound("pm-1", "hello", now);
    let channel_id = alephcore::gateway::channel::ChannelId::new("sms");
    let msg = convert_inbound(&config(), &channel_id, &env).unwrap();

    assert_eq!(
        msg.id.as_str(),
        "6:acct-1:pm-1",
        "id is scoped by provider account"
    );
    assert_eq!(msg.id.as_str(), scoped_message_id("acct-1", "pm-1"));
    assert!(msg.raw.is_none(), "runtime metadata is not in raw");
    assert_eq!(msg.conversation_id.as_str(), PEER);
    assert_eq!(msg.sender_id.as_str(), PEER);
    assert_eq!(msg.text, "hello");
    assert!(msg.attachments.is_empty());
    assert!(!msg.is_group);
    assert_eq!(
        msg.timestamp, env.created_at,
        "original event time, not delivery time"
    );

    let meta = SmsInboundMeta::from_message(&msg).expect("typed sms meta");
    assert_eq!(meta.region, "CN");
    assert_eq!(meta.binding_ref, BINDING);
    assert_eq!(meta.provider_account_id, "acct-1");
    assert_eq!(meta.provider_message_id, "pm-1");
    assert_eq!(meta.envelope_event_id, "evt-pm-1");
    assert_eq!(meta.connection_generation, 7);
    assert_eq!(meta.service_address, SERVICE);
    assert_eq!(meta.peer_address, PEER);
    assert_eq!(meta.encoding, SmsEncoding::Gsm7);
    assert_eq!(meta.segment_count, 1);
    assert_eq!(meta.expires_at, env.expires_at);
}

#[test]
fn convert_inbound_preserves_ucs2_multipart_metadata() {
    let now = Utc::now();
    let mut env = inbound("pm-2", "你好", now);
    if let TelecomKind::SmsInbound(s) = &mut env.kind {
        s.encoding = SmsEncoding::Ucs2;
        s.segment_count = 3;
    }
    let msg = convert_inbound(
        &config(),
        &alephcore::gateway::channel::ChannelId::new("sms"),
        &env,
    )
    .unwrap();
    let meta = SmsInboundMeta::from_message(&msg).unwrap();
    assert_eq!(meta.encoding, SmsEncoding::Ucs2);
    assert_eq!(meta.segment_count, 3);
}

#[test]
fn convert_inbound_rejects_bad_frames() {
    let now = Utc::now();
    let cfg = config();
    let id = alephcore::gateway::channel::ChannelId::new("sms");

    // Wrong service line.
    let mut env = inbound("pm-3", "x", now);
    if let TelecomKind::SmsInbound(s) = &mut env.kind {
        s.service_address = "+14155559999".into();
    }
    assert!(matches!(
        convert_inbound(&cfg, &id, &env),
        Err(SmsError::ServiceAddressMismatch)
    ));

    // Non-E.164 peer: never guessed.
    let mut env = inbound("pm-4", "x", now);
    if let TelecomKind::SmsInbound(s) = &mut env.kind {
        s.peer_address = "4155550100".into();
    }
    assert!(matches!(
        convert_inbound(&cfg, &id, &env),
        Err(SmsError::InvalidAddress(_))
    ));

    // Zero segments / empty body.
    let mut env = inbound("pm-5", "x", now);
    if let TelecomKind::SmsInbound(s) = &mut env.kind {
        s.segment_count = 0;
    }
    assert!(matches!(
        convert_inbound(&cfg, &id, &env),
        Err(SmsError::InvalidSegmentCount)
    ));
    let env = inbound("pm-6", "", now);
    assert!(matches!(
        convert_inbound(&cfg, &id, &env),
        Err(SmsError::EmptyBody)
    ));
}

#[tokio::test]
async fn ingest_publishes_once_and_dedups_by_provider_identity() {
    let now = Utc::now();
    let channel = SmsChannel::new("sms", config());
    let mut rx = channel.state().inbound_subscribe();

    let first = channel
        .ingest_envelope(&inbound("pm-10", "hello", now), 7, now)
        .unwrap();
    assert!(matches!(first, SmsIngest::Committed(_)));
    let got = rx.try_recv().expect("published");
    assert_eq!(got.text, "hello");

    // Redelivery with a different Relay event_id and (hostile) different body:
    // same provider message id => duplicate, nothing published.
    let mut redelivery = inbound("pm-10", "tampered", now);
    redelivery.event_id = "evt-other".into();
    assert_eq!(
        channel.ingest_envelope(&redelivery, 7, now).unwrap(),
        SmsIngest::Duplicate
    );
    assert!(rx.try_recv().is_err(), "duplicate must not reach the agent");

    // A different provider message id is a new message.
    assert!(matches!(
        channel
            .ingest_envelope(&inbound("pm-11", "again", now), 7, now)
            .unwrap(),
        SmsIngest::Committed(_)
    ));
    assert_eq!(rx.try_recv().unwrap().id.as_str(), "6:acct-1:pm-11");
}

#[tokio::test]
async fn ingest_without_subscriber_is_not_recorded_so_redelivery_is_fresh() {
    let now = Utc::now();
    let channel = SmsChannel::new("sms", config());
    let env = inbound("pm-20", "hello", now);

    // Nobody is listening: the message must not be silently swallowed.
    let err = channel.ingest_envelope(&env, 7, now).unwrap_err();
    assert!(matches!(err, SmsError::NoConsumer), "{err:?}");

    // Once a consumer exists, the Relay's redelivery is committed, not a dup.
    let mut rx = channel.state().inbound_subscribe();
    assert!(matches!(
        channel.ingest_envelope(&env, 7, now).unwrap(),
        SmsIngest::Committed(_)
    ));
    assert_eq!(rx.try_recv().unwrap().id.as_str(), "6:acct-1:pm-20");
}

#[tokio::test]
async fn ingest_refuses_wrong_binding_and_generation_without_recording() {
    let now = Utc::now();
    let channel = SmsChannel::new("sms", config());
    let mut rx = channel.state().inbound_subscribe();

    let mut env = inbound("pm-30", "x", now);
    env.binding_id = "someone-else".into();
    assert!(channel.ingest_envelope(&env, 7, now).is_err());

    let env = inbound("pm-31", "x", now);
    assert!(
        channel.ingest_envelope(&env, 8, now).is_err(),
        "stale generation"
    );
    assert!(
        channel.ingest_envelope(&env, 6, now).is_err(),
        "future generation"
    );
    assert!(rx.try_recv().is_err());

    // The refused frame left no trace: with the right context it is fresh.
    assert!(matches!(
        channel.ingest_envelope(&env, 7, now).unwrap(),
        SmsIngest::Committed(_)
    ));
}

#[tokio::test]
async fn ingest_expired_and_stale_are_not_published() {
    let now = Utc::now();
    let channel = SmsChannel::new("sms", config());
    let mut rx = channel.state().inbound_subscribe();

    let expired = inbound("pm-40", "late", now - Duration::seconds(1000));
    assert_eq!(
        channel.ingest_envelope(&expired, 7, now).unwrap(),
        SmsIngest::Expired
    );
    // Recorded: its redelivery is a duplicate, not a second chance.
    assert_eq!(
        channel.ingest_envelope(&expired, 7, now).unwrap(),
        SmsIngest::Duplicate
    );

    let mut old = inbound("pm-41", "ancient", now);
    old.created_at = now - Duration::days(8);
    old.expires_at = now + Duration::days(1);
    assert_eq!(
        channel.ingest_envelope(&old, 7, now).unwrap(),
        SmsIngest::Stale
    );
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn ingest_respects_inbound_disabled_and_non_inbound_frames() {
    let now = Utc::now();
    let mut cfg = config();
    cfg.inbound_enabled = false;
    let channel = SmsChannel::new("sms", cfg);
    assert!(matches!(
        channel.ingest_envelope(&inbound("pm-50", "x", now), 7, now),
        Err(SmsError::InboundDisabled)
    ));

    let channel = SmsChannel::new("sms", config());
    let mut receipt = inbound("pm-51", "x", now);
    receipt.kind = TelecomKind::Outcome(OutcomeReport {
        provider_account_id: "acct-1".into(),
        provider_event_id: "pe-1".into(),
        operation_id: "op-1".into(),
        provider_message_id: None,
        provider_call_id: None,
        outcome: TelecomOutcome::Delivered,
        error_code: None,
    });
    assert_eq!(
        channel.ingest_envelope(&receipt, 7, now).unwrap(),
        SmsIngest::NotInbound
    );
}

#[test]
fn only_delivered_counts_as_delivered() {
    for (outcome, delivered) in [
        (TelecomOutcome::Accepted, false),
        (TelecomOutcome::Submitted, false),
        (TelecomOutcome::Unknown, false),
        (TelecomOutcome::Failed, false),
        (TelecomOutcome::OutcomeUnknownExpired, false),
        (TelecomOutcome::Delivered, true),
    ] {
        let report = OutcomeReport {
            provider_account_id: "acct-1".into(),
            provider_event_id: "pe-1".into(),
            operation_id: "op-1".into(),
            provider_message_id: Some("pm".into()),
            provider_call_id: None,
            outcome,
            error_code: None,
        };
        let meta = SmsOutcomeMeta::from_report(&report);
        assert_eq!(meta.operation_id, "op-1");
        assert_eq!(meta.delivered(), delivered, "{outcome:?}");
    }
}

// ---- typed metadata, scoped ids ---------------------------------------------

#[test]
fn typed_metadata_survives_without_raw_and_through_serde() {
    let now = Utc::now();
    let env = inbound("pm-50", "hello", now);
    let mut msg = convert_inbound(
        &config(),
        &alephcore::gateway::channel::ChannelId::new("sms"),
        &env,
    )
    .unwrap();
    let before = SmsInboundMeta::from_message(&msg).unwrap();

    // Strip debugging payload: required runtime metadata must still be there.
    msg.raw = None;
    assert_eq!(SmsInboundMeta::from_message(&msg).unwrap(), before);

    // MessageMeta serde roundtrip.
    let meta = msg
        .metadata
        .iter()
        .find(|m| matches!(m, MessageMeta::Sms(_)))
        .unwrap();
    let json = serde_json::to_string(meta).unwrap();
    let back: MessageMeta = serde_json::from_str(&json).unwrap();
    match back {
        MessageMeta::Sms(m) => assert_eq!(*m, before),
        other => panic!("{other:?}"),
    }
    assert!(!json.contains("hello"), "metadata carries no body");

    // Whole InboundMessage roundtrip keeps it.
    let json = serde_json::to_string(&msg).unwrap();
    let back: alephcore::gateway::channel::InboundMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(SmsInboundMeta::from_message(&back).unwrap(), before);
    // Original (unscoped) provider id is preserved in typed metadata.
    assert_eq!(before.provider_message_id, "pm-50");
}

#[test]
fn metadata_debug_redacts_phone_numbers() {
    let now = Utc::now();
    let msg = convert_inbound(
        &config(),
        &alephcore::gateway::channel::ChannelId::new("sms"),
        &inbound("pm-51", "secret body", now),
    )
    .unwrap();
    let dbg = format!("{:?}", msg.metadata);
    assert!(!dbg.contains(PEER) && !dbg.contains(SERVICE), "{dbg}");
    assert!(!dbg.contains("secret body"), "{dbg}");
}

#[test]
fn same_provider_message_id_on_two_accounts_gets_distinct_ids() {
    let now = Utc::now();
    let id = |acct: &str, msg: &str| {
        let mut env = inbound(msg, "x", now);
        if let TelecomKind::SmsInbound(s) = &mut env.kind {
            s.provider_account_id = acct.into();
        }
        convert_inbound(
            &config(),
            &alephcore::gateway::channel::ChannelId::new("sms"),
            &env,
        )
        .unwrap()
        .id
    };
    assert_ne!(id("acct-A", "pm-1"), id("acct-B", "pm-1"));
    // Delimiter games cannot make two (account, message) pairs collide.
    assert_ne!(id("a", "b:c"), id("a:b", "c"));
    assert_eq!(id("acct-A", "pm-1"), id("acct-A", "pm-1"));
}

// ---- dedup ledger bounds / rollback scope -----------------------------------

#[tokio::test]
async fn ingest_prunes_keys_past_retention() {
    let now = Utc::now();
    let channel = SmsChannel::new("sms", config());
    let _rx = channel.state().inbound_subscribe();

    // Expired frames are recorded (so a redelivery is a duplicate).
    for i in 0..5 {
        let env = inbound(&format!("old-{i}"), "late", now - Duration::seconds(1000));
        assert_eq!(
            channel.ingest_envelope(&env, 7, now).unwrap(),
            SmsIngest::Expired
        );
    }
    assert_eq!(channel.dedup_len(), 5);

    // Eight days later the old keys are gone; only the new one remains.
    let later = now + Duration::days(8);
    assert!(matches!(
        channel
            .ingest_envelope(&inbound("new-1", "hi", later), 7, later)
            .unwrap(),
        SmsIngest::Committed(_)
    ));
    assert_eq!(channel.dedup_len(), 1, "expired keys pruned");
}

#[tokio::test]
async fn no_consumer_rollback_keeps_other_keys() {
    let now = Utc::now();
    let channel = SmsChannel::new("sms", config());

    // One committed (with a subscriber), one expired, both recorded.
    let rx = channel.state().inbound_subscribe();
    let committed = inbound("keep-1", "hi", now);
    let expired = inbound("keep-2", "late", now - Duration::seconds(1000));
    assert!(matches!(
        channel.ingest_envelope(&committed, 7, now).unwrap(),
        SmsIngest::Committed(_)
    ));
    assert_eq!(
        channel.ingest_envelope(&expired, 7, now).unwrap(),
        SmsIngest::Expired
    );
    assert_eq!(channel.dedup_len(), 2);

    // Subscriber goes away; a fresh message hits NoConsumer.
    drop(rx);
    let fresh = inbound("fresh-1", "hello", now);
    let err = channel.ingest_envelope(&fresh, 7, now).unwrap_err();
    assert!(matches!(err, SmsError::NoConsumer), "{err:?}");

    // Only the fresh key was undone; the earlier keys still dedup.
    assert_eq!(channel.dedup_len(), 2);
    assert_eq!(
        channel.ingest_envelope(&committed, 7, now).unwrap(),
        SmsIngest::Duplicate
    );
    assert_eq!(
        channel.ingest_envelope(&expired, 7, now).unwrap(),
        SmsIngest::Duplicate
    );
    // And the rolled-back one is fresh again once someone listens.
    let mut rx = channel.state().inbound_subscribe();
    assert!(matches!(
        channel.ingest_envelope(&fresh, 7, now).unwrap(),
        SmsIngest::Committed(_)
    ));
    assert_eq!(rx.try_recv().unwrap().text, "hello");
}

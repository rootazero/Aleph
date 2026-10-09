//! SMS channel contract: factory, capabilities, strict addressing, segment
//! metadata, and the "unavailable is never delivered" rule. No network and no
//! provider: transports here are in-process fakes.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aleph_protocol::telecom_relay::{SmsEncoding, TelecomOutcome};
use alephcore::gateway::channel::{
    Attachment, Channel, ChannelError, ChannelFactory, ChannelStatus, InlineKeyboard,
    OutboundMessage,
};
use alephcore::gateway::interfaces::sms::message_ops::{
    build_pending_operation, estimate_segments, max_chars_for_segments, normalize_e164,
};
use alephcore::gateway::interfaces::sms::{
    PendingSmsOperation, SmsChannel, SmsChannelFactory, SmsConfig, SmsSubmission, SmsTransport,
};
use async_trait::async_trait;
use chrono::Utc;
use common::channel_contract::{test_channel_contract, test_channel_properties};

fn config() -> SmsConfig {
    serde_json::from_value(serde_json::json!({
        "region": "CN",
        "service_address": "+8613800000000",
        "binding_ref": "core-binding-1",
    }))
    .expect("valid sms config")
}

/// Fake Relay transport: reports a fixed outcome and counts submissions.
struct FakeTransport {
    connected: bool,
    outcome: TelecomOutcome,
    submits: AtomicUsize,
    echo_wrong_id: bool,
}

impl FakeTransport {
    fn new(connected: bool, outcome: TelecomOutcome) -> Arc<Self> {
        Arc::new(Self {
            connected,
            outcome,
            submits: AtomicUsize::new(0),
            echo_wrong_id: false,
        })
    }
}

#[async_trait]
impl SmsTransport for FakeTransport {
    async fn is_connected(&self) -> bool {
        self.connected
    }

    async fn submit(
        &self,
        operation: &PendingSmsOperation,
    ) -> alephcore::gateway::channel::ChannelResult<SmsSubmission> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        Ok(SmsSubmission {
            operation_id: if self.echo_wrong_id {
                "someone-else".to_string()
            } else {
                operation.operation_id().to_string()
            },
            outcome: self.outcome,
        })
    }
}

// ---- factory & properties -------------------------------------------------

#[tokio::test]
async fn factory_creates_sms_channel() {
    let factory = SmsChannelFactory;
    assert_eq!(factory.channel_type(), "sms");
    let channel = factory
        .create(serde_json::to_value(config()).unwrap())
        .await
        .expect("create");
    assert_eq!(channel.channel_type(), "sms");
    assert_eq!(channel.info().id.as_str(), "sms");
    let named = factory
        .create_with_id("sms-main", serde_json::to_value(config()).unwrap())
        .await
        .unwrap();
    assert_eq!(named.info().id.as_str(), "sms-main");
}

#[tokio::test]
async fn factory_rejects_missing_or_guessed_region() {
    let factory = SmsChannelFactory;
    // No region: there is deliberately no serde default.
    let missing = serde_json::json!({
        "service_address": "+8613800000000",
        "binding_ref": "core-binding-1",
    });
    assert!(factory.create(missing).await.is_err());
    // Lowercase / wrong-shape regions are errors, not normalised guesses.
    for bad in ["cn", "CHN", "", "C1"] {
        let v = serde_json::json!({
            "region": bad,
            "service_address": "+8613800000000",
            "binding_ref": "core-binding-1",
        });
        assert!(factory.create(v).await.is_err(), "region {bad:?} accepted");
    }
}

#[tokio::test]
async fn factory_rejects_bad_service_address_and_binding() {
    let factory = SmsChannelFactory;
    for (addr, binding) in [("13800000000", "b"), ("+8613800000000", ""), ("+12", "b")] {
        let v = serde_json::json!({
            "region": "CN",
            "service_address": addr,
            "binding_ref": binding,
        });
        assert!(
            factory.create(v).await.is_err(),
            "accepted addr={addr:?} binding={binding:?}"
        );
    }
}

#[test]
fn channel_properties_hold() {
    let channel = SmsChannel::new("sms-test", config());
    test_channel_properties(&channel);
    assert_eq!(channel.channel_type(), "sms");
}

#[test]
fn capabilities_declare_text_only() {
    let channel = SmsChannel::new("sms-test", config());
    let caps = channel.capabilities();
    assert!(!caps.attachments);
    assert!(!caps.images);
    assert!(!caps.audio);
    assert!(!caps.video);
    assert!(!caps.reactions);
    assert!(!caps.replies);
    assert!(!caps.editing);
    assert!(!caps.deletion);
    assert!(!caps.typing_indicator);
    assert!(!caps.read_receipts);
    assert!(!caps.rich_text);
    assert!(!caps.polls);
    assert_eq!(caps.max_attachment_size, 0);
    assert_eq!(caps.max_message_length, max_chars_for_segments(3));
}

#[tokio::test]
async fn full_contract_with_no_transport() {
    // start() fails (nothing to connect to) and the contract requires the
    // status to say so; stop() lands on Disconnected.
    test_channel_contract(SmsChannel::new("sms-test", config())).await;
}

// ---- unavailable never reports delivered ------------------------------------

#[tokio::test]
async fn start_without_transport_is_unavailable() {
    let mut channel = SmsChannel::new("sms-test", config());
    let err = channel.start().await.expect_err("must not start");
    assert!(matches!(err, ChannelError::NotConnected(_)), "{err:?}");
    assert_eq!(channel.status(), ChannelStatus::Error);
}

#[tokio::test]
async fn send_without_transport_is_not_connected() {
    let channel = SmsChannel::new("sms-test", config());
    let err = channel
        .send(OutboundMessage::text("+14155550100", "hi"))
        .await
        .expect_err("nothing carried this");
    assert!(matches!(err, ChannelError::NotConnected(_)), "{err:?}");
}

#[tokio::test]
async fn send_before_start_with_transport_is_not_connected() {
    let t = FakeTransport::new(true, TelecomOutcome::Accepted);
    let channel = SmsChannel::new("sms-test", config()).with_transport(t.clone());
    let err = channel
        .send(OutboundMessage::text("+14155550100", "hi"))
        .await
        .expect_err("channel not started");
    assert!(matches!(err, ChannelError::NotConnected(_)), "{err:?}");
    assert_eq!(t.submits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn start_with_disconnected_transport_is_unavailable() {
    let t = FakeTransport::new(false, TelecomOutcome::Accepted);
    let mut channel = SmsChannel::new("sms-test", config()).with_transport(t);
    assert!(channel.start().await.is_err());
    assert_eq!(channel.status(), ChannelStatus::Error);
}

#[tokio::test]
async fn accepted_send_is_ok_with_operation_id_not_delivery() {
    let t = FakeTransport::new(true, TelecomOutcome::Accepted);
    let mut channel = SmsChannel::new("sms-test", config()).with_transport(t.clone());
    channel.start().await.unwrap();
    assert_eq!(channel.status(), ChannelStatus::Connected);
    let result = channel
        .send(OutboundMessage::text("+14155550100", "hello"))
        .await
        .expect("accepted");
    assert!(!result.message_id.as_str().is_empty());
    assert_eq!(t.submits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn non_success_outcomes_are_errors() {
    for outcome in [
        TelecomOutcome::Failed,
        TelecomOutcome::Cancelled,
        TelecomOutcome::Unknown,
        TelecomOutcome::OutcomeUnknownExpired,
    ] {
        let t = FakeTransport::new(true, outcome);
        let mut channel = SmsChannel::new("sms-test", config()).with_transport(t);
        channel.start().await.unwrap();
        let err = channel
            .send(OutboundMessage::text("+14155550100", "hello"))
            .await
            .expect_err("must not claim success");
        assert!(
            matches!(err, ChannelError::SendFailed(_)),
            "{outcome:?}: {err:?}"
        );
    }
}

#[tokio::test]
async fn unknown_outcome_says_unknown() {
    let t = FakeTransport::new(true, TelecomOutcome::Unknown);
    let mut channel = SmsChannel::new("sms-test", config()).with_transport(t);
    channel.start().await.unwrap();
    let err = channel
        .send(OutboundMessage::text("+14155550100", "hello"))
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("unknown") && text.contains("not retried"),
        "{text}"
    );
}

#[tokio::test]
async fn mismatched_operation_id_is_rejected() {
    let t = Arc::new(FakeTransport {
        connected: true,
        outcome: TelecomOutcome::Accepted,
        submits: AtomicUsize::new(0),
        echo_wrong_id: true,
    });
    let mut channel = SmsChannel::new("sms-test", config()).with_transport(t);
    channel.start().await.unwrap();
    assert!(channel
        .send(OutboundMessage::text("+14155550100", "hello"))
        .await
        .is_err());
}

// ---- disabled features / strict validation -----------------------------------

#[tokio::test]
async fn attachments_and_buttons_are_refused_without_submitting() {
    let t = FakeTransport::new(true, TelecomOutcome::Accepted);
    let mut channel = SmsChannel::new("sms-test", config()).with_transport(t.clone());
    channel.start().await.unwrap();

    let attachment = Attachment {
        id: "a".into(),
        mime_type: "image/png".into(),
        filename: None,
        size: None,
        url: None,
        path: None,
        data: Some(vec![1]),
        is_voice_note: false,
        is_video_note: false,
    };
    let with_att = OutboundMessage::text("+14155550100", "pic").with_attachment(attachment);
    assert!(matches!(
        channel.send(with_att).await,
        Err(ChannelError::UnsupportedFeature(_))
    ));

    let mut with_kb = OutboundMessage::text("+14155550100", "pick");
    with_kb.inline_keyboard = Some(InlineKeyboard::new().button("Yes", "yes"));
    assert!(matches!(
        channel.send(with_kb).await,
        Err(ChannelError::UnsupportedFeature(_))
    ));
    assert_eq!(t.submits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsupported_actions_return_unsupported() {
    let channel = SmsChannel::new("sms-test", config());
    let conv = alephcore::gateway::channel::ConversationId::new("+14155550100");
    let mid = alephcore::gateway::channel::MessageId::new("m");
    assert!(matches!(
        channel.send_typing(&conv).await,
        Err(ChannelError::UnsupportedFeature(_))
    ));
    assert!(matches!(
        channel.mark_read(&mid).await,
        Err(ChannelError::UnsupportedFeature(_))
    ));
    assert!(matches!(
        channel.react(&conv, &mid, "x").await,
        Err(ChannelError::UnsupportedFeature(_))
    ));
}

#[test]
fn e164_is_strict_and_never_guesses_a_region() {
    assert_eq!(normalize_e164("+14155550100").unwrap(), "+14155550100");
    for bad in [
        "4155550100", // no '+': would need a guessed region
        "14155550100",
        "004155550100",    // international prefix is region-specific
        "+0141555501",     // country code cannot start with 0
        "+1 415 555 0100", // formatting is not normalised away
        " +14155550100",
        "+1-415-555-0100",
        "+1415555010a",
        "+1234567",          // too short (7 digits)
        "+1234567890123456", // too long (16 digits)
        "",
        "+",
    ] {
        assert!(normalize_e164(bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn outbound_peer_must_be_e164() {
    let cfg = config();
    let err = build_pending_operation(
        &cfg,
        &OutboundMessage::text("415-555-0100", "hi"),
        Utc::now(),
    )
    .unwrap_err();
    assert!(matches!(err, ChannelError::SendFailed(_)), "{err:?}");
}

#[test]
fn empty_body_is_refused() {
    let cfg = config();
    assert!(
        build_pending_operation(&cfg, &OutboundMessage::text("+14155550100", ""), Utc::now())
            .is_err()
    );
}

// ---- segmentation metadata ---------------------------------------------------

#[test]
fn gsm7_segment_boundaries() {
    let (enc, n) = estimate_segments(&"a".repeat(160));
    assert_eq!((enc, n), (SmsEncoding::Gsm7, 1));
    let (enc, n) = estimate_segments(&"a".repeat(161));
    assert_eq!((enc, n), (SmsEncoding::Gsm7, 2));
    let (_, n) = estimate_segments(&"a".repeat(306));
    assert_eq!(n, 2);
    let (_, n) = estimate_segments(&"a".repeat(307));
    assert_eq!(n, 3);
}

#[test]
fn gsm7_extension_chars_cost_two() {
    // '€' is a GSM extension character: width 2.
    let (enc, n) = estimate_segments(&"€".repeat(80));
    assert_eq!((enc, n), (SmsEncoding::Gsm7, 1));
    let (_, n) = estimate_segments(&"€".repeat(81));
    assert_eq!(n, 2);
}

#[test]
fn ucs2_segment_boundaries_and_surrogates() {
    let (enc, n) = estimate_segments(&"你".repeat(70));
    assert_eq!((enc, n), (SmsEncoding::Ucs2, 1));
    let (_, n) = estimate_segments(&"你".repeat(71));
    assert_eq!(n, 2);
    let (_, n) = estimate_segments(&"你".repeat(134));
    assert_eq!(n, 2);
    let (_, n) = estimate_segments(&"你".repeat(135));
    assert_eq!(n, 3);
    // An emoji is a surrogate pair (2 units) and forces UCS-2.
    let (enc, _) = estimate_segments("hi 😀");
    assert_eq!(enc, SmsEncoding::Ucs2);
}

#[test]
fn pending_operation_carries_encoding_and_segments() {
    let cfg = config();
    let now = Utc::now();
    let op = build_pending_operation(
        &cfg,
        &OutboundMessage::text("+14155550100", "你好".repeat(40)),
        now,
    )
    .unwrap();
    assert_eq!(op.command.encoding, SmsEncoding::Ucs2);
    assert_eq!(op.command.segment_count, 2);
    assert_eq!(op.command.recipient_binding_id, "core-binding-1");
    assert_eq!(op.peer_address, "+14155550100");
    assert!(op.command.expires_at > now);
    assert!(op.command.expires_at <= now + chrono::Duration::seconds(300));
    assert!(!op.command.operation_id.is_empty());
}

#[test]
fn over_budget_text_is_refused_not_truncated() {
    let cfg = config(); // max_segments = 3
    let body = "a".repeat(460); // 3 multipart segments hold 459 -> 4
    let err = build_pending_operation(
        &cfg,
        &OutboundMessage::text("+14155550100", body),
        Utc::now(),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            ChannelError::MessageTooLarge {
                size: 4,
                max_size: 3
            }
        ),
        "{err:?}"
    );
}

#[test]
fn approval_reference_flows_from_metadata_only() {
    let cfg = config();
    let mut msg = OutboundMessage::text("+14155550100", "hi");
    let op = build_pending_operation(&cfg, &msg, Utc::now()).unwrap();
    assert_eq!(op.command.approval_reference, None);
    msg.metadata
        .insert("approval_reference".into(), "appr-1".into());
    let op = build_pending_operation(&cfg, &msg, Utc::now()).unwrap();
    assert_eq!(op.command.approval_reference.as_deref(), Some("appr-1"));
}

#[test]
fn outbound_disabled_builds_nothing() {
    let mut cfg = config();
    cfg.outbound_enabled = false;
    assert!(build_pending_operation(
        &cfg,
        &OutboundMessage::text("+14155550100", "hi"),
        Utc::now()
    )
    .is_err());
}

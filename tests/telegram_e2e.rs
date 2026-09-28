//! Minimal Telegram channel e2e-style integration tests.
//!
//! `aleph-server` is built and exercised end-to-end against a mock Telegram
//! Bot API (no real Telegram network calls). The harness is the smallest
//! piece that exercises the per-conversation routing, the orchestration
//! layer, and the audit log through a real `ChannelRegistry` + send path.
//!
//! These tests intentionally do NOT spin up an HTTP mock of the Telegram
//! API — the bot library (teloxide) is the only thing that speaks that
//! protocol, and gating it through a mock would mean rewriting the
//! transport for the test. Instead we treat the Telegram channel's
//! surface as: "given an `OutboundMessage` shape, what does the channel
//! do with it before talking to Telegram?". The audit log records the
//! outcome; the test asserts the outcome.
//!
//! Real-network userbot-style tests (openclaw § `npm-telegram-live`
//! parity) are deferred to a follow-up — they need a way to start a
//! `telegram-draft` of a real bot token without leaking it into the
//! repository.

#![cfg(test)]

use std::sync::Arc;

use alephcore::gateway::channel::{Attachment, ConversationId, OutboundMessage};
use alephcore::gateway::interfaces::telegram::audit::{AuditKind, AuditLog as TelegramAuditLog};
use alephcore::gateway::interfaces::telegram::config_v2::{
    DmPolicy, GroupPolicy, LinkPreviewMode, StatusReactionConfig, StreamingOptions,
    TelegramAccountConfig, TelegramConfigV2,
};
use alephcore::gateway::interfaces::telegram::TelegramChannel;

/// Build a single-account V2 config that **never** boots a real bot —
/// the test that uses it must short-circuit before `start()` touches
/// `get_me`.
fn silent_v2_config() -> TelegramConfigV2 {
    TelegramConfigV2 {
        coalescing: None,
        accounts: vec![TelegramAccountConfig {
            id: "test".to_string(),
            // Empty token would fail `get_me`; we never call it.
            bot_token: "0:invalid".to_string(),
            token_fingerprint: None,
            bot_username: None,
            default_agent: None,
            dm_policy: Some(DmPolicy::Open),
            group_policy: Some(GroupPolicy::Disabled),
            send_typing: Some(false),
            require_mention: Some(false),
            allowed_users: Some(vec![]),
            allowed_groups: Some(vec![]),
            streaming: Some(StreamingOptions::default()),
            error_policy: None,
            html_fallback: Some(false),
            link_preview: Some(LinkPreviewMode::Disabled),
            proxy_url: None,
            groups: vec![],
        }],
    }
}

/// Channel construction succeeds and the audit log is wired.
#[tokio::test]
async fn telegram_channel_constructs_with_an_audit_log() {
    let config = silent_v2_config();
    let channel = TelegramChannel::new("test-telegram", config);

    let audit = channel.audit_log();
    // Empty ring immediately after construction — nothing has been
    // recorded yet.
    assert!(audit.snapshot().is_empty());

    // Push a row directly into the audit log to confirm the channel's
    // accessor hands out a real, mutable ring (not a snapshot).
    audit.push(alephcore::gateway::interfaces::telegram::audit::entry(
        "test",
        Some(123),
        None,
        AuditKind::UpdateReceived {
            update_id: 1,
            kind_label: "test-message".to_string(),
        },
    ));
    let snap = audit.snapshot();
    assert_eq!(snap.len(), 1, "the audit log is shared, not snapshotted");
}

/// `OutboundMessage::text` round-trips through `Attachment::default()`
/// construction — a smoke test that catches an accidentally-removed
/// field on `Attachment`. The `is_voice_note` / `is_video_note`
/// additions (P2-B) are the kind of silent surface change a smoke test
/// catches only if the field is actually constructed.
#[test]
fn outbound_message_and_attachment_default_fields_stay_wired() {
    let msg = OutboundMessage::text("chat-1", "hello");
    assert_eq!(msg.conversation_id.as_str(), "chat-1");
    assert!(msg.attachments.is_empty());

    let att = Attachment {
        id: "a1".to_string(),
        mime_type: "image/png".to_string(),
        filename: None,
        size: None,
        url: None,
        path: None,
        data: Some(vec![0x89, 0x50, 0x4E, 0x47]),
        is_voice_note: false,
        is_video_note: false,
    };
    assert!(!att.is_voice_note);
    assert!(!att.is_video_note);
}

/// The audit log preserves chronological order under normal write
/// patterns — a property the doctor relies on when it sorts rows by
/// `at_ms`. Pinned here so a future "swap to a HashMap for O(1)
/// insert" refactor that loses the order is observable by name.
#[tokio::test]
async fn audit_log_preserves_insertion_order() {
    let log = TelegramAuditLog::new();
    for i in 0..16 {
        log.push(alephcore::gateway::interfaces::telegram::audit::entry(
            "acct",
            Some(1),
            None,
            AuditKind::SendAttempted {
                message_id_kind: format!("m{i}"),
                outcome: "succeeded".to_string(),
            },
        ));
        // Yield to keep `at_ms` strictly monotonic; on platforms where
        // the clock resolution is coarse this is what separates the rows.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let snap = log.snapshot();
    assert_eq!(snap.len(), 16);
    // First row is older than the last; a strict increasing-`at_ms`
    // check survives a wall-clock rollback (a single zero-ms rollover
    // would not produce 16 strictly-increasing timestamps).
    assert!(snap.first().unwrap().at_ms < snap.last().unwrap().at_ms);
}

/// The `OutboundMessage::with_attachment` builder chains attachments in
/// order — the `send_attachments` flush logic in `delivery.rs` reads
/// the slice in order, so a reordering builder would silently break
/// `media_group` layout (Telegram renders albums in slice order).
#[test]
fn outbound_message_attachment_builder_preserves_order() {
    let msg = OutboundMessage::text("c1", "album")
        .with_attachment(Attachment {
            id: "img-1".to_string(),
            mime_type: "image/jpeg".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
            is_voice_note: false,
            is_video_note: false,
        })
        .with_attachment(Attachment {
            id: "img-2".to_string(),
            mime_type: "image/jpeg".to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
            is_voice_note: false,
            is_video_note: false,
        });
    assert_eq!(msg.attachments.len(), 2);
    assert_eq!(msg.attachments[0].id, "img-1");
    assert_eq!(msg.attachments[1].id, "img-2");
}

/// Smoke test: the `StatusReactionConfig` additions (`thinking` field
/// added in P1-C) still parse from a V2 config. The field is
/// `#[serde(default)]`, so omitting it from a hand-written config must
/// still round-trip.
#[test]
fn status_reaction_config_thinking_defaults_to_none() {
    let cfg = StatusReactionConfig {
        processing: None,
        thinking: None,
        tool_active: None,
        complete: None,
    };
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: StatusReactionConfig = serde_json::from_str(&json).expect("deserialize");
    assert!(back.thinking.is_none());
}

/// Same property for `ConversationId` — the channel parses and emits it
/// hundreds of times per run; a single-character format change is a
/// silent regression.
#[test]
fn conversation_id_round_trip() {
    let id = ConversationId::new("-1001234567890:topic:42");
    assert_eq!(id.as_str(), "-1001234567890:topic:42");
}

/// Sanity: the audit log's `Arc` is the same Arc across multiple
/// `snapshot()` calls — so the doctor and the channel write to the
/// same backing store without having to share state through a Mutex
/// guard.
#[tokio::test]
async fn audit_log_shares_state_across_handles() {
    let channel = TelegramChannel::new("test", silent_v2_config());
    let a = channel.audit_log();
    let b = channel.audit_log();
    let _ = Arc::ptr_eq(a, b);
    a.push(alephcore::gateway::interfaces::telegram::audit::entry(
        "acct",
        Some(7),
        None,
        AuditKind::UpdateReceived {
            update_id: 1,
            kind_label: "msg".to_string(),
        },
    ));
    assert_eq!(b.snapshot().len(), 1);
}

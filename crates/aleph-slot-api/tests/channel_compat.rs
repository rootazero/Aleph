//! Task 4 compatibility contract — the channel / approval / cancellation /
//! webhook surface that moved from `alephcore::gateway::{channel,
//! cancellation, channel_approval, webhook_receiver}`, `alephcore::exec::
//! {socket, allowed_decisions, approval::types}` and
//! `alephcore::sandbox::exec_approval::{gate, grants}` into this crate.
//!
//! Every assertion pins a behaviour the old Core definitions already had, so
//! the move cannot silently change a default, a fallback, an ordering, or a
//! wire shape. The type-identity half of the contract (old Core path and new
//! API path naming the SAME type) lives in `alephcore`'s
//! `tests/modularization_contracts.rs`.

use aleph_slot_api::approval::{
    full_set, ApprovalAction, ApprovalDecisionType, ApprovalOutcome, ApprovalRequest,
    ApprovalResponse, AuthorizationResult, ChannelApprovalCapability, ChannelApprovalExt,
    CommandApprovalRequest, GrantScope, PendingApproval, RenderedApproval,
};
use aleph_slot_api::cancellation::CancellationToken;
use aleph_slot_api::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage, InlineKeyboard,
    MessageId, OutboundMessage, SendResult, UserId, CB_MESSAGE_ID_PREFIX,
};
use aleph_slot_api::webhook::{WebhookHandler, WEBHOOK_ROUTE_PREFIX};
use chrono::Utc;

fn test_inbound(text: &str) -> InboundMessage {
    InboundMessage {
        id: MessageId::new("m-1"),
        channel_id: ChannelId::new("test"),
        conversation_id: ConversationId::new("conv-1"),
        sender_id: UserId::new("user-1"),
        sender_name: None,
        text: text.to_string(),
        attachments: vec![],
        timestamp: Utc::now(),
        reply_to: None,
        is_group: false,
        raw: None,
        metadata: vec![],
    }
}

fn test_request() -> ApprovalRequest {
    ApprovalRequest::Command(CommandApprovalRequest {
        command: "test_tool".to_string(),
        cwd: None,
        reason: None,
        allowed_decisions: vec![ApprovalDecisionType::AllowOnce],
    })
}

// ---------------------------------------------------------------------------
// Channel optional-method honesty (declared vs undeclared diagnostics)
// ---------------------------------------------------------------------------

struct StubChannel {
    info: ChannelInfo,
    state: ChannelState,
}

fn stub_channel(capabilities: ChannelCapabilities) -> StubChannel {
    StubChannel {
        info: ChannelInfo {
            id: ChannelId::new("stub"),
            name: "Stub".to_string(),
            channel_type: "stub".to_string(),
            status: ChannelStatus::Connected,
            capabilities,
        },
        state: ChannelState::new(8),
    }
}

#[async_trait::async_trait]
impl Channel for StubChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }
    fn state(&self) -> &ChannelState {
        &self.state
    }
    async fn start(&mut self) -> ChannelResult<()> {
        Ok(())
    }
    async fn stop(&mut self) -> ChannelResult<()> {
        Ok(())
    }
    async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
        unreachable!("these tests never send")
    }
}

/// A capability flag is a promise: declaring it without overriding the method
/// must fail loudly and name both the adapter and the broken promise.
#[tokio::test]
async fn declared_but_unimplemented_optional_methods_fail_loudly() {
    let ch = stub_channel(ChannelCapabilities {
        reactions: true,
        editing: true,
        deletion: true,
        typing_indicator: true,
        read_receipts: true,
        polls: true,
        group_icons: true,
        ..ChannelCapabilities::default()
    });
    let conv = ConversationId::new("c1");
    let msg = MessageId::new("m1");

    for err in [
        ch.react(&conv, &msg, "👍").await.unwrap_err(),
        ch.edit(&conv, &msg, "new").await.unwrap_err(),
        ch.delete(&conv, &msg).await.unwrap_err(),
        ch.send_typing(&conv).await.unwrap_err(),
        ch.mark_read(&msg).await.unwrap_err(),
        ch.create_poll(&conv, "q?", &["a".to_string()], false)
            .await
            .unwrap_err(),
        ch.set_group_icon(&conv, "data:image/png;base64,AAAA")
            .await
            .unwrap_err(),
    ] {
        match err {
            ChannelError::UnsupportedFeature(m) => {
                assert!(m.contains("stub"), "{m}");
                assert!(m.contains("does not implement it"), "{m}");
            }
            other => panic!("expected UnsupportedFeature, got {other:?}"),
        }
    }
}

/// The honest case keeps the plain wording — callers match on the feature
/// word, and the message must not claim a broken promise that never existed.
#[tokio::test]
async fn undeclared_optional_methods_report_plain_unsupported() {
    let ch = stub_channel(ChannelCapabilities::default());

    let err = ch
        .send_typing(&ConversationId::new("c1"))
        .await
        .unwrap_err();
    match err {
        ChannelError::UnsupportedFeature(m) => {
            assert_eq!(m, "typing indicator (not supported by stub)");
        }
        other => panic!("expected UnsupportedFeature, got {other:?}"),
    }

    let err = ch.list_conversations("", 10).await.unwrap_err();
    match err {
        ChannelError::UnsupportedFeature(m) => {
            assert_eq!(m, "conversation directory (not supported by stub)");
        }
        other => panic!("expected UnsupportedFeature, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Broadcast / receiver semantics
// ---------------------------------------------------------------------------

/// `broadcast` semantics: every subscriber gets every message.
#[tokio::test]
async fn broadcast_two_subscribers_both_receive() {
    let state = ChannelState::new(8);
    let mut a = state.inbound_subscribe();
    let mut b = state.take_receiver().unwrap();

    assert!(state.send_inbound(test_inbound("hello")));

    assert_eq!(a.recv().await.unwrap().text, "hello");
    assert_eq!(b.recv().await.unwrap().text, "hello");
}

/// `take_receiver` is NOT `mpsc::Receiver::take()` — every call subscribes a
/// fresh receiver and the return value is always `Some`.
#[tokio::test]
async fn take_receiver_returns_some_on_every_call() {
    let state = ChannelState::new(8);
    assert!(state.take_receiver().is_some());
    assert!(state.take_receiver().is_some());
    assert!(state.take_receiver().is_some());
}

/// A send with zero listeners returns the ORIGINAL message, unchanged — the
/// caller, not the channel, decides what dropping it means.
#[tokio::test]
async fn send_with_no_listener_returns_the_original_message() {
    let state = ChannelState::new(8);
    let sender = state.sender();
    let err = sender.send(test_inbound("payload-x")).unwrap_err();
    assert_eq!(err.text, "payload-x");
    assert_eq!(err.id.as_str(), "m-1");
    assert_eq!(err.conversation_id.as_str(), "conv-1");
    // ChannelState::send_inbound reports the same fact as `false`.
    assert!(!state.send_inbound(test_inbound("payload-y")));
}

/// `status()` is a non-blocking `try_read`; while a writer holds the lock the
/// fallback is the most pessimistic non-restart status — `Disconnected`.
#[tokio::test]
async fn status_falls_back_to_disconnected_while_write_locked() {
    let state = ChannelState::new(8);
    state.set_status(ChannelStatus::Connected).await;
    assert_eq!(state.status(), ChannelStatus::Connected);

    let handle = state.status_handle();
    let _guard = handle.write().await;
    assert_eq!(state.status(), ChannelStatus::Disconnected);
}

// ---------------------------------------------------------------------------
// CancellationToken
// ---------------------------------------------------------------------------

/// The lost-wakeup closure: subscribing AFTER `cancel()` must still observe
/// the cancellation (the token re-emits for fresh subscribers).
#[tokio::test]
async fn cancel_before_subscribe_still_notifies() {
    let token = CancellationToken::new();
    token.cancel();
    assert!(token.is_cancelled());

    let mut rx = token.subscribe();
    rx.recv()
        .await
        .expect("cancelled token must re-emit to late subscribers");

    // Cancellation is idempotent and shared across clones.
    let clone = token.clone();
    assert!(clone.is_cancelled());
    let mut rx2 = clone.subscribe();
    rx2.recv()
        .await
        .expect("clone observes the same cancelled state");
}

// ---------------------------------------------------------------------------
// ChannelFactory::create_with_id default
// ---------------------------------------------------------------------------

struct StubFactory;

#[async_trait::async_trait]
impl ChannelFactory for StubFactory {
    fn channel_type(&self) -> &str {
        "stub"
    }
    async fn create(&self, _config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        Ok(Box::new(stub_channel(ChannelCapabilities::default())))
    }
}

/// The default `create_with_id` IGNORES the configured id and delegates to
/// `create` — the historical behaviour. Factories that need the real id
/// override it.
#[tokio::test]
async fn create_with_id_default_ignores_the_id() {
    let factory = StubFactory;
    let channel = factory
        .create_with_id("configured-id-that-must-not-arrive", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(channel.id().as_str(), "stub");
    assert_eq!(channel.channel_type(), "stub");
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn wire_prefix_constants_are_stable() {
    assert_eq!(CB_MESSAGE_ID_PREFIX, "cb_");
    assert_eq!(WEBHOOK_ROUTE_PREFIX, "/webhook");
}

// ---------------------------------------------------------------------------
// Approval wire shapes
// ---------------------------------------------------------------------------

/// A payload serialized before `reason` / `allowed_decisions` existed must
/// still deserialize, with the historical unconstrained decision set in the
/// historical order (Once / Always / Deny — never Session).
#[test]
fn legacy_command_approval_payload_backfills_defaults() {
    let request: ApprovalRequest =
        serde_json::from_value(serde_json::json!({"type": "command", "command": "ls"}))
            .expect("legacy payload must deserialize");
    let ApprovalRequest::Command(cmd) = request;
    assert_eq!(cmd.command, "ls");
    assert_eq!(cmd.cwd, None);
    assert_eq!(cmd.reason, None);
    assert_eq!(
        cmd.allowed_decisions,
        vec![
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowAlways,
            ApprovalDecisionType::Deny,
        ],
        "full_set order is the wire contract"
    );
    assert_eq!(full_set().len(), 3);
    assert!(!full_set().contains(&ApprovalDecisionType::AllowSession));
}

/// The tagged-enum wire shape and the kebab-case decision vocabulary.
#[test]
fn approval_wire_casing_is_stable() {
    let request = test_request();
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["type"], "command");
    // `reason` is skip_serializing_if = None — absent, not null.
    assert!(value.get("reason").is_none());
    assert_eq!(
        value["allowed_decisions"],
        serde_json::json!(["allow-once"])
    );

    for (decision, wire) in [
        (ApprovalDecisionType::AllowOnce, "allow-once"),
        (ApprovalDecisionType::AllowSession, "allow-session"),
        (ApprovalDecisionType::AllowAlways, "allow-always"),
        (ApprovalDecisionType::Deny, "deny"),
    ] {
        assert_eq!(serde_json::to_value(decision).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<ApprovalDecisionType>(serde_json::json!(wire)).unwrap(),
            decision
        );
    }
}

/// `clamped_for` walks DOWN the ladder one rung at a time; `AllowOnce` and
/// `Deny` are the floor (a human's answer is never escalated and never turned
/// into a refusal).
#[test]
fn clamped_for_walks_down_one_rung_at_a_time() {
    use ApprovalDecisionType::{AllowAlways, AllowOnce, AllowSession, Deny};

    let session_max = [AllowOnce, AllowSession, Deny];
    assert_eq!(AllowAlways.clamped_for(&session_max), AllowSession);

    let once_only = [AllowOnce, Deny];
    // Two narrowing steps, not a jump past the missing tier.
    assert_eq!(AllowAlways.clamped_for(&once_only), AllowOnce);
    assert_eq!(AllowSession.clamped_for(&once_only), AllowOnce);

    // The legacy backfill set skips the session rung: an unoffered session
    // decision narrows to the widest tier the card DID offer.
    assert_eq!(AllowSession.clamped_for(&full_set()), AllowOnce);

    // The floor: never escalated, never refused here.
    assert_eq!(AllowOnce.clamped_for(&[Deny]), AllowOnce);
    assert_eq!(Deny.clamped_for(&[]), Deny);

    // Offered decisions pass through untouched, and narrowing is idempotent.
    for decision in [AllowOnce, AllowSession, Deny] {
        assert_eq!(decision.clamped_for(&session_max), decision);
    }
    assert_eq!(
        AllowAlways
            .clamped_for(&session_max)
            .clamped_for(&session_max),
        AllowSession
    );
}

/// The single decision → outcome mapping every approval surface shares.
#[test]
fn to_outcome_within_maps_through_the_clamp() {
    use ApprovalDecisionType::{AllowAlways, AllowOnce, AllowSession, Deny};

    let session_max = [AllowOnce, AllowSession, Deny];
    assert_eq!(
        AllowOnce.to_outcome_within(&session_max),
        ApprovalOutcome::Approved
    );
    assert_eq!(
        AllowSession.to_outcome_within(&session_max),
        ApprovalOutcome::ApprovedForSession
    );
    // "Allow always" on a card that did not offer it degrades to session.
    assert_eq!(
        AllowAlways.to_outcome_within(&session_max),
        ApprovalOutcome::ApprovedForSession
    );
    let with_persistent = [AllowOnce, AllowSession, AllowAlways, Deny];
    assert_eq!(
        AllowAlways.to_outcome_within(&with_persistent),
        ApprovalOutcome::ApprovedAlways
    );
    assert_eq!(
        Deny.to_outcome_within(&session_max),
        ApprovalOutcome::Denied
    );
}

/// `ApprovalOutcome`: which outcomes approve, and the standing-grant scope
/// each carries.
#[test]
fn approval_outcome_approval_and_grant_scope() {
    assert!(ApprovalOutcome::Approved.is_approved());
    assert!(ApprovalOutcome::ApprovedForSession.is_approved());
    assert!(ApprovalOutcome::ApprovedAlways.is_approved());
    assert!(!ApprovalOutcome::Denied.is_approved());
    assert!(!ApprovalOutcome::Timeout.is_approved());
    assert!(!ApprovalOutcome::Unavailable.is_approved());

    assert_eq!(ApprovalOutcome::Approved.grant_scope(), None);
    assert_eq!(
        ApprovalOutcome::ApprovedForSession.grant_scope(),
        Some(GrantScope::Session)
    );
    assert_eq!(
        ApprovalOutcome::ApprovedAlways.grant_scope(),
        Some(GrantScope::Always)
    );
    assert_eq!(ApprovalOutcome::Denied.grant_scope(), None);
    assert_eq!(ApprovalOutcome::Timeout.grant_scope(), None);
    assert_eq!(ApprovalOutcome::Unavailable.grant_scope(), None);

    // The From conversion leaves no deny reason.
    let response: ApprovalResponse = ApprovalOutcome::Denied.into();
    assert_eq!(response.outcome, ApprovalOutcome::Denied);
    assert_eq!(response.deny_reason, None);
}

/// `GrantScope` wire tokens: snake_case, round-trip, unknown rejected.
#[test]
fn grant_scope_tokens_round_trip_and_reject_unknown() {
    assert_eq!(GrantScope::Session.as_str(), "session");
    assert_eq!(GrantScope::Always.as_str(), "always");
    assert_eq!(GrantScope::parse("session"), Some(GrantScope::Session));
    assert_eq!(GrantScope::parse("always"), Some(GrantScope::Always));
    assert_eq!(GrantScope::parse("forever"), None);
    assert_eq!(GrantScope::parse(""), None);
    assert_eq!(
        serde_json::to_value(GrantScope::Session).unwrap(),
        "session"
    );
    assert_eq!(serde_json::to_value(GrantScope::Always).unwrap(), "always");
}

// ---------------------------------------------------------------------------
// ChannelApprovalCapability::render_approval_for_actor default
// ---------------------------------------------------------------------------

struct StubApprovalCapability {
    auth: AuthorizationResult,
}

#[async_trait::async_trait]
impl ChannelApprovalCapability for StubApprovalCapability {
    async fn deliver_approval(
        &self,
        _conversation_id: &ConversationId,
        _request: &ApprovalRequest,
        _approval_id: &str,
    ) -> ChannelResult<PendingApproval> {
        unreachable!("these tests only render")
    }

    async fn authorize_actor(
        &self,
        _actor_user_id: &UserId,
        _action: ApprovalAction,
    ) -> AuthorizationResult {
        self.auth
    }

    async fn render_approval(
        &self,
        conversation_id: &ConversationId,
        _request: &ApprovalRequest,
    ) -> ChannelResult<RenderedApproval> {
        let mut message = OutboundMessage::text(conversation_id.as_str(), "approve?");
        message.inline_keyboard = Some(InlineKeyboard::new().button("Approve", "cb_x:approve"));
        Ok(RenderedApproval {
            message,
            callback_prefix: CB_MESSAGE_ID_PREFIX.to_string(),
        })
    }

    async fn resolve_approval(
        &self,
        _pending: &PendingApproval,
        _action: ApprovalAction,
    ) -> ChannelResult<()> {
        unreachable!("these tests only render")
    }
}

/// An actor who may not decide must not be shown the buttons: `Denied` and
/// `NotAuthenticated` both strip the inline keyboard (and keep the callback
/// prefix); `Authorized` keeps the full render.
#[tokio::test]
async fn render_approval_for_actor_removes_keyboard_when_unauthorized() {
    let conv = ConversationId::new("c1");
    let actor = UserId::new("u1");
    let request = test_request();

    for auth in [
        AuthorizationResult::Denied,
        AuthorizationResult::NotAuthenticated,
    ] {
        let cap = StubApprovalCapability { auth };
        let rendered = cap
            .render_approval_for_actor(&conv, &request, &actor, "approval-1")
            .await
            .unwrap();
        assert!(
            rendered.message.inline_keyboard.is_none(),
            "{auth:?} must strip the keyboard"
        );
        assert_eq!(rendered.callback_prefix, CB_MESSAGE_ID_PREFIX);
    }

    let cap = StubApprovalCapability {
        auth: AuthorizationResult::Authorized,
    };
    let rendered = cap
        .render_approval_for_actor(&conv, &request, &actor, "approval-1")
        .await
        .unwrap();
    assert!(rendered.message.inline_keyboard.is_some());
}

/// `ChannelApprovalExt` default: a channel exposes no approval capability.
#[test]
fn channel_approval_ext_defaults_to_none() {
    struct Plain;
    impl ChannelApprovalExt for Plain {}
    assert!(Plain.approval_capability().is_none());
}

// ---------------------------------------------------------------------------
// WebhookHandler
// ---------------------------------------------------------------------------

struct StubWebhook;

#[async_trait::async_trait]
impl WebhookHandler for StubWebhook {
    fn verify(&self, _headers: &http::HeaderMap, _body: &[u8]) -> bool {
        true
    }

    async fn handle(
        &self,
        _headers: &http::HeaderMap,
        _body: bytes::Bytes,
    ) -> ChannelResult<Vec<InboundMessage>> {
        Ok(vec![test_inbound("from-webhook")])
    }

    fn path(&self) -> &str {
        "/webhook/stub"
    }
}

/// The handler trait is object-safe, its `HeaderMap` / `Bytes` are the
/// workspace's `http` 1.x / `bytes` 1.x types, and paths live under the
/// shared prefix.
#[tokio::test]
async fn webhook_handler_trait_surface() {
    let handler: std::sync::Arc<dyn WebhookHandler> = std::sync::Arc::new(StubWebhook);
    assert!(handler.path().starts_with(WEBHOOK_ROUTE_PREFIX));

    let headers = http::HeaderMap::new();
    assert!(handler.verify(&headers, b"{}"));

    let messages = handler
        .handle(&headers, bytes::Bytes::from_static(b"{}"))
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "from-webhook");
}

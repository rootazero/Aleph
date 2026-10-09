//! Integration tests for `handle_component` D1 rewrite.
//!
//! These tests drive the typed dispatch half of the Discord component
//! interaction handler (`dispatch_component_click` in
//! `src/gateway/interfaces/discord/commands.rs`). The pure function is
//! the testable seam: it parses the wire-format `custom_id`, runs
//! `CommandRegistry::dispatch`, and returns the `ComponentAction` the
//! serenity handler should take. Constructing a full
//! `serenity::ComponentInteraction` for testing is brittle (the struct
//! has many private fields), so the handler is wired to delegate to
//! this function and these tests verify the dispatch logic directly.
//!
//! The test fixture mirrors what the old `handle_component` produced:
//! an `InboundMessage` with a `cb_` prefix on the `id` (so the inbound
//! router's `cb_` intercept picks it up) and the `text` field carrying
//! the callback_data the approval sink expects (`approve:<id>:<decision>`).

use alephcore::gateway::channel::ConversationId;
use alephcore::gateway::interfaces::discord::commands::{
    dispatch_component_click, CommandAction, CommandRegistry,
};

fn ctx() -> (CommandRegistry, ConversationId) {
    (
        CommandRegistry::with_defaults(),
        ConversationId::new("channel-1"),
    )
}

/// Approve button (typed ComponentId wire format `approve:<id>`)
/// must route through to inbound with a callback_data the approval
/// sink can parse. Reconstructed as `approve:<id>:once` so
/// `ApprovalBridge::parse_callback` matches.
#[test]
fn approval_button_routes_to_inbound_with_parseable_callback() {
    let (registry, conv) = ctx();
    let action = dispatch_component_click(
        &registry,
        "approve:rec-1",
        "user-1",
        Some("Alice"),
        42,
        conv,
        false,
    );
    match action {
        CommandAction::Forward(inbound) => {
            assert!(
                inbound.id.as_str().starts_with("cb_"),
                "inbound id must carry the cb_ prefix so the router intercepts: got {:?}",
                inbound.id.as_str()
            );
            assert_eq!(
                inbound.text, "approve:rec-1:once",
                "Approve button must reconstruct to the sink's parse_callback format"
            );
        }
        other => panic!("Expected Forward, got {other:?}"),
    }
}

/// Deny button (typed ComponentId wire format `deny:<id>`) must route
/// through to inbound as `approve:<id>:deny` (the sink expects
/// `approve:` prefix for both allow and deny clicks).
#[test]
fn deny_button_routes_to_inbound_with_parseable_callback() {
    let (registry, conv) = ctx();
    let action = dispatch_component_click(
        &registry,
        "deny:rec-1",
        "user-1",
        Some("Alice"),
        42,
        conv,
        false,
    );
    match action {
        CommandAction::Forward(inbound) => {
            assert_eq!(
                inbound.text, "approve:rec-1:deny",
                "Deny button must reconstruct to approve:<id>:deny"
            );
        }
        other => panic!("Expected Forward, got {other:?}"),
    }
}

/// Pre-D4 multi-tier approval buttons emit
/// `approve:<id>:<decision>` directly. ComponentId parses the payload
/// as `<id>:<decision>`; reconstruction must round-trip via `to_wire`
/// so the decision tier isn't doubled up.
#[test]
fn legacy_multitier_approval_payload_is_preserved() {
    let (registry, conv) = ctx();
    let action = dispatch_component_click(
        &registry,
        "approve:rec-1:session",
        "user-1",
        Some("Alice"),
        42,
        conv,
        false,
    );
    match action {
        CommandAction::Forward(inbound) => {
            assert_eq!(
                inbound.text, "approve:rec-1:session",
                "Legacy multi-tier payload must round-trip via to_wire, not get a second decision tier appended"
            );
        }
        other => panic!("Expected Forward, got {other:?}"),
    }
}

/// `with_defaults()` registers the `Callback` handler which returns
/// `AckNoReply`. Unknown kinds (`mystery:x`) fall through to the
/// callback handler — they should NOT reach the inbound router as
/// text (Review Focus #1: unknown-kind swallowing must not silently
/// leak raw text to the agent loop).
#[test]
fn unknown_kind_with_callback_handler_acks_only_no_inbound() {
    let (registry, conv) = ctx();
    let action = dispatch_component_click(&registry, "mystery:x", "user-1", None, 42, conv, false);
    assert!(
        matches!(action, CommandAction::AckOnly),
        "with_defaults() registers Callback handler → AckNoReply, not Forward, got {action:?}"
    );
}

/// An empty registry (no Callback handler) must NOT silently swallow
/// unknown kinds — they fall back to the legacy inbound path so an
/// operator can still see the click.
#[test]
fn unknown_kind_without_callback_handler_falls_back_to_legacy_inbound() {
    let registry = CommandRegistry::new(); // empty, no Callback fallback
    let conv = ConversationId::new("channel-1");
    let action = dispatch_component_click(&registry, "mystery:x", "user-1", None, 42, conv, false);
    match action {
        CommandAction::Forward(inbound) => {
            assert_eq!(
                inbound.text, "mystery:x",
                "Empty registry: unknown kind must reach inbound unchanged"
            );
        }
        other => panic!("Expected Forward legacy fallback, got {other:?}"),
    }
}

/// Legacy bots still emit `cb_<message_id>` style strings (no colon).
/// `ComponentId::parse` returns Err for the no-colon case if the
/// string contains a colon-shaped kind... actually, a no-colon string
/// yields `Unknown(<whole string>)` with empty payload. The right
/// behavior: forward the raw custom_id to inbound unchanged (Review
/// Focus #3: legacy cb_<id> bots must keep working).
#[test]
fn legacy_cb_id_no_colon_forwards_raw_to_inbound() {
    let (registry, conv) = ctx();
    let action =
        dispatch_component_click(&registry, "cb_legacy_id", "user-1", None, 42, conv, false);
    match action {
        CommandAction::Forward(inbound) => {
            assert_eq!(
                inbound.text, "cb_legacy_id",
                "Legacy cb_<id> format must reach inbound_tx unchanged"
            );
        }
        other => panic!("Expected Forward, got {other:?}"),
    }
}

/// Empty payload (`approve:`) is a parse error → legacy fallback. The
/// raw custom_id is forwarded so the inbound router sees the malformed
/// shape and the operator can diagnose it.
#[test]
fn empty_payload_rejected_as_malformed_legacy_fallback() {
    let (registry, conv) = ctx();
    let action = dispatch_component_click(&registry, "approve:", "user-1", None, 42, conv, false);
    match action {
        CommandAction::Forward(inbound) => {
            assert_eq!(
                inbound.text, "approve:",
                "Empty payload must reach inbound unchanged for diagnostic visibility"
            );
        }
        other => panic!("Expected Forward, got {other:?}"),
    }
}

/// Sender metadata (id + name) must propagate into the forwarded
/// inbound so the inbound router can run the originator gate and the
/// audit trail has a human-readable actor.
#[test]
fn forwarded_inbound_carries_sender_metadata() {
    let (registry, conv) = ctx();
    let action = dispatch_component_click(
        &registry,
        "approve:rec-1",
        "user-42",
        Some("Bob"),
        7,
        conv,
        true,
    );
    match action {
        CommandAction::Forward(inbound) => {
            assert_eq!(inbound.sender_id.as_str(), "user-42");
            assert_eq!(inbound.sender_name.as_deref(), Some("Bob"));
            assert_eq!(inbound.conversation_id.as_str(), "channel-1");
            assert!(inbound.is_group, "DM/guild flag must propagate");
        }
        other => panic!("Expected Forward, got {other:?}"),
    }
}

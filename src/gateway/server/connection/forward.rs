//! Forward phase — event bus → per-client buffer → wire projection.
//!
//! Owns the per-event envelope parsing, the four-term receive filter
//! (login wall + scope + audience + owner-scope), the payload projection
//! (the running-set narrowing), and the TopicEvent → JSON-RPC wrap on
//! the wire. All pure helpers are `pub(crate)` so the test module in
//! `handler.rs` (`use super::*;`) can still reach them via the
//! `connection::*` re-exports in `handler.rs`.
//!
//! Source-level pin: the production half of THIS file is what the
//! `the_delivery_loop_parses_each_event_once_and_projects_it` guard test
//! reads with `include_str!("connection/forward.rs")` and counts
//! needles in. The two needles are:
//!
//! - `serde_json::from_str::<serde_json::Value>(&event_json)` (the
//!   forward loop's single parse — a second occurrence would re-introduce
//!   the double-parse regression), and
//! - `project_for(` (the payload-projection call site, so removing the
//!   loop integration is caught rather than passing unit tests on
//!   `EventVisibilityIndex` alone).
//!
//! `forward.rs` deliberately holds no `#[cfg(test)]` block, so the
//! guard test's `split(&format!("#[cfg{}]", "(test)")).next()` returns
//! the entire file.

use tokio::sync::broadcast;

use crate::gateway::server::per_client_buffer::PerClientBuffer;

/// Build the JSON-RPC `connection.warning` frame announcing dropped events.
///
/// Shared by the per-client drain path and the global-bus overflow watchdog so
/// both surface identical `events_overflow` diagnostics (with `advice:reconnect`)
/// before the connection is closed with WS code 1008.
pub fn overflow_warning_frame(dropped: u64, total_overflow: u64) -> String {
    serde_json::to_string(&aleph_protocol::JsonRpcRequest::notification(
        aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD,
        Some(serde_json::json!({
            "topic": "connection.warning",
            "data": {
                "reason": "events_overflow",
                "dropped": dropped,
                "total_overflow": total_overflow,
                "advice": "reconnect"
            }
        })),
    ))
    .unwrap_or_else(|_| String::new())
}

/// Drain the global event bus into a single client's per-client buffer.
///
/// A `tokio::broadcast` receiver that falls behind yields `RecvError::Lagged(n)`
/// but **remains valid** — subsequent `recv()` calls keep working, having skipped
/// `n` messages. We mirror the per-client drain policy here: account the dropped
/// events on the buffer's shared overflow metric and keep forwarding. The
/// connection's idle/ping watchdog observes that metric and closes the socket
/// with code 1008 so the client reconnects and re-syncs from the hello snapshot.
///
/// The forwarder terminates on either of two terminal conditions: the global bus
/// closes (`RecvError::Closed`, i.e. process shutdown) **or** the per-client
/// receiver is dropped (connection closed). The latter makes `try_send` fail —
/// the bounded broadcast errors only when it has zero receivers, and
/// `handle_connection` holds the sole receiver for the connection's lifetime —
/// so a failed send unambiguously means "socket gone", and we stop instead of
/// looping forever against a dead client. (Previously the send error was
/// discarded with `let _ =`, so this task — and its live global-bus receiver —
/// leaked for the whole process lifetime on *every* WS disconnect, cloning every
/// published event to a dead receiver: O(dead_connections) fan-out growth on a
/// long-running daemon. Separately, an earlier `while let Ok(..)` loop treated
/// `Lagged` as fatal, silently killing the task and event-starving a live socket.)
pub async fn forward_bus_to_client(mut rx: broadcast::Receiver<String>, buffer: PerClientBuffer) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                // A failed send == the per-client receiver was dropped == the
                // connection closed. Reap this task instead of leaking it.
                if buffer.try_send(event).is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                buffer.metrics().add_overflow(n);
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Wire `topic` under which the shared-token rotation event is published (see
/// [`crate::gateway::events::GatewayEventFrame::TokenRotated`]). A drift-guard
/// test keeps this equal to `GatewayEventFrame::TokenRotated.topic_name()`.
pub const TOKEN_ROTATED_TOPIC: &str = "gateway.token.rotated";

/// Whether the given serialized event frame is a `token_rotated` notification.
///
/// `GatewayEvents::publish_frame` wraps every non-stream event as the TopicEvent
/// wire form `{"topic": "<name>", "data": <frame>}`, so the discriminant is the
/// **top-level `topic`**, not a top-level `type`. (The inner `data` still carries
/// the serde tag `{"type":"token_rotated"}`, but the forward loop only ever sees
/// the wrapped form.) Reading `type` here silently never matched, which left the
/// rotation kick — the documented "revoke all remotes" hammer — inert: open
/// remote Panels kept operator authority until idle timeout. Parses the JSON once
/// and matches `topic == TOKEN_ROTATED_TOPIC`. Pure, host-testable.
pub fn is_token_rotated_frame(event_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(event_json)
        .ok()
        .and_then(|v| {
            v.get("topic")
                .and_then(|t| t.as_str())
                .map(|s| s == TOKEN_ROTATED_TOPIC)
        })
        .unwrap_or(false)
}

/// Whether this connection must be torn down because the shared token was
/// rotated. True only for a `token_rotated` event on a *remote* (non-loopback)
/// connection — loopback is always operator and never token-gated, so it is
/// unaffected. Pure for host testing.
pub fn rotated_should_close_remote(event_json: &str, is_loopback: bool) -> bool {
    !is_loopback && is_token_rotated_frame(event_json)
}

/// Wire `topic` under which a single paired-device revocation is published (see
/// [`crate::gateway::events::GatewayEventFrame::DeviceRevoked`]). A drift-guard
/// test keeps this equal to `DeviceRevoked{..}.topic_name()`.
pub const DEVICE_REVOKED_TOPIC: &str = "gateway.device.revoked";

/// The `device_id` carried by a `device_revoked` event frame, or `None` for any
/// other event.
///
/// Same wire shape as [`is_token_rotated_frame`]: `publish_frame` wraps every
/// non-stream event as `{"topic": …, "data": <frame>}`, so the discriminant is
/// the **top-level `topic`** and the payload lives under `data`. Reading a
/// top-level `type` here is the mistake that once turned the rotation kick into
/// a dud — do not reintroduce it.
pub fn device_revoked_id(event_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(event_json).ok()?;
    if v.get("topic").and_then(|t| t.as_str()) != Some(DEVICE_REVOKED_TOPIC) {
        return None;
    }
    v.get("data")
        .and_then(|d| d.get("device_id"))
        .and_then(|d| d.as_str())
        .map(String::from)
}

/// Whether this connection must be torn down because *its own* paired device was
/// revoked. True only when the event names exactly the device this session
/// authenticated as — an unbound session (loopback, legacy shared token, or a
/// still-walled connection) has no `device_id` and is never matched, so a
/// per-device revoke can never collaterally kick the operator's own local App.
/// Pure for host testing.
pub fn device_revoked_should_close(event_json: &str, session_device_id: Option<&str>) -> bool {
    match (device_revoked_id(event_json), session_device_id) {
        (Some(revoked), Some(mine)) => revoked == mine,
        _ => false,
    }
}

/// Extract `(topic, data)` from an already-parsed event envelope — the
/// single chokepoint every per-connection filter term reads from
/// (`EventScopeGuard`, `audience_allows`, `SubscriptionManager`,
/// `EventVisibilityIndex`).
///
/// Handles every wire shape an event can arrive in on `ctx.event_bus`:
/// - `TopicEvent` form (non-stream `GatewayEventFrame` variants, published
///   by `publish_frame`): `{"topic": "...", "data": {...}}`.
/// - `stream.*` JSON-RPC notification form (streaming `GatewayEventFrame`
///   variants): `{"method": "stream.X", "params": <frame body>}` — the
///   frame's own fields live directly under `params`, not nested under a
///   `.data` (see `event_bus.rs::publish_frame`'s doc). `data` reads `None`
///   for this shape — unchanged from before this function existed; no
///   `stream.*` frame has ever had a nested `.data` to find.
/// - The double-wrapped `TopicEvent::to_notification()` form, used by
///   producers that build a raw string and call `GatewayEventBus::publish`
///   directly rather than going through `publish_frame` (e.g.
///   `subagent_tree_relay.rs`'s `run.subagent_tree`):
///   `{"jsonrpc":"2.0","method":"event","params":{"topic":"...",
///   "data":{...},"timestamp":...}}`. **Missing this branch reads `topic`
///   as the literal string `"event"` for every producer using this shape**
///   — `EventScopeGuard` happens to default-allow an unrecognized topic
///   anyway, but `session_identity_of` ALSO defaults an unrecognized topic
///   to `Global`, so a session-scoped event published this way silently
///   skipped owner-scoping entirely (found in review, fix round 1 — see
///   `run.subagent_tree`'s entry in `event_visibility::session_identity_of`).
pub fn extract_topic_and_data(event_obj: &serde_json::Value) -> (&str, Option<&serde_json::Value>) {
    if event_obj.get("method").and_then(serde_json::Value::as_str)
        == Some(aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD)
    {
        if let Some(params) = event_obj.get("params") {
            let topic = params
                .get("topic")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            return (topic, params.get("data"));
        }
    }
    let topic = event_obj
        .get("topic")
        .and_then(serde_json::Value::as_str)
        .or_else(|| event_obj.get("method").and_then(serde_json::Value::as_str))
        .unwrap_or("");
    let data = event_obj
        .get("data")
        .or_else(|| event_obj.get("params").and_then(|p| p.get("data")));
    (topic, data)
}

/// The exact bytes a connection receives for one already-admitted event.
///
/// Two jobs, in this order:
///
/// 1. **Apply the per-connection payload projection**, if
///    [`EventVisibilityIndex::project_for`](crate::gateway::event_visibility::EventVisibilityIndex::project_for)
///    produced one. A projected payload replaces `.params`, because every frame
///    that method has an arm for is published through `publish_frame`'s STREAM
///    branch, which puts the frame body exactly there — the same place
///    [`extract_topic_and_data`]'s caller read it from to make the decision.
/// 2. **Wrap the bare `TopicEvent` form** (`{topic, data}`, no `method`) into a
///    JSON-RPC notification so the Panel can dispatch it via `method == "event"`.
///
/// A frame that is neither projected nor wrapped is forwarded as the ORIGINAL
/// string, not a re-serialization of the parse — byte-identical output and no
/// serialization cost for the overwhelming majority of frames. That is what
/// pays for the projection: this used to re-parse `original` from scratch just
/// to answer question 2.
pub fn event_wire_form(
    mut event_obj: serde_json::Value,
    projected_payload: Option<serde_json::Value>,
    original: String,
) -> String {
    let rewritten = projected_payload.is_some();
    if let (Some(payload), Some(obj)) = (projected_payload, event_obj.as_object_mut()) {
        obj.insert("params".to_string(), payload);
    }
    if event_obj.get("topic").is_some() && event_obj.get("method").is_none() {
        // The shared constructor, so this envelope carries `"jsonrpc": "2.0"`
        // like every other notification on the wire — see
        // `event_bus.rs::publish_frame` for what a hand-built one cost.
        serde_json::to_string(&aleph_protocol::JsonRpcRequest::notification(
            aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD,
            Some(event_obj),
        ))
        .unwrap_or_else(|_| String::new())
    } else if rewritten {
        event_obj.to_string()
    } else {
        original
    }
}
//! Cleanup phase — post-disconnect teardown.
//!
//! The original `handle_connection` inlined its cleanup after the main
//! `tokio::select!` loop fell through (token-rotated close, device-revoked
//! close, idle-timeout close, normal socket end, …). Everything that must
//! run exactly once per connection — deregistering the node, removing
//! presence, releasing subscriptions, freeing the PTY viewport lock,
//! cancelling in-flight reverse-RPC calls — lives in [`run_cleanup`].
//!
//! Called from the orchestrator (`connection/mod.rs::handle_connection`)
//! via a single call site after the loop breaks.
//!
//! All items are `pub(super)` so only the orchestrator in
//! `connection/mod.rs` can call into this module; the test module in
//! `handler.rs` does not exercise cleanup directly — its
//! `node_shape_connect_registers_and_disconnect_deregisters` covers the
//! registry half at the `NodeRegistry` seam.

use std::sync::Arc;
use tracing::{debug, info, warn};

use super::ConnectionContext;

/// Disconnect-side cleanup shared by every termination path inside
/// `handle_connection`.
///
/// Order matters here:
///
/// 1. Remove the connection from the shared map so any new traffic on this
///    socket sees `caller_role: "guest"` (fail-closed) and stops being
///    delivered to a `caller_role` this process no longer recognises.
/// 2. Cancel in-flight reverse-RPC calls so node_invoke / node_file /
///    approval callers do not block on their per-call timeout (≤130s)
///    against a node that is already gone — see openclaw
///    `node-registry.unregister()`.
/// 3. Deregister this socket from the cluster node registry AND emit
///    `node.disconnected` if the registration was ours. (The
///    operator-deregister path emits the event itself, so the guard
///    here skips the second publish; see
///    `cluster::enrollment::tests::deregister_publishes_node_disconnected_on_live_eviction`.)
/// 4. Remove presence + bump state-version + emit `presence.left`.
/// 5. Release subscriptions owned by this connection.
/// 6. Release any PTY viewport constraints (`caller_identity::CALLER_CONN_ID`
///    / `PtyManager::note_viewport`) so a crashed tab does not pin a shared
///    terminal's size.
pub(super) async fn run_cleanup(
    conn_id: &str,
    ctx: &ConnectionContext,
    rpc_pending: &Arc<crate::cluster::reverse_rpc::PendingInvokes>,
) {
    // 1. Shared connection map
    {
        let mut conns = ctx.connections.write().await;
        conns.remove(conn_id);
    }

    // 2. Fail-fast every in-flight reverse-RPC call bound to this socket.
    let cancelled = rpc_pending.cancel_all();
    if cancelled > 0 {
        debug!(
            "Connection {} closed: cancelled {} in-flight reverse-RPC call(s)",
            conn_id, cancelled
        );
    }

    // 3. Cluster Phase 0b: drop this connection's node session if it was a
    // node, and emit a `node.disconnected` lifecycle event.
    let node_ident = ctx.node_registry.node_identity_by_conn(conn_id);
    if ctx.node_registry.deregister(conn_id) {
        if let Some((node_id, name)) = node_ident {
            if let Some(store) = ctx.security_store.as_ref() {
                if let Err(e) = store.touch_device(&node_id) {
                    debug!(
                        "failed to stamp node last_seen on disconnect for {}: {}",
                        node_id, e
                    );
                }
            }
            if let Err(e) = ctx.event_bus.publish_json(&crate::gateway::event_bus::TopicEvent::new(
                "node.disconnected",
                serde_json::json!({"node_id": node_id, "name": name, "conn_id": conn_id}),
            )) {
                warn!(
                    error = %e,
                    node_id = %node_id,
                    conn_id = %conn_id,
                    "failed to publish node.disconnected event"
                );
            }
        }
    }

    // 4. Presence + lifecycle event
    if let Some(_entry) = ctx.presence.remove(conn_id) {
        ctx.state_versions.bump_presence();
        if let Err(e) = ctx.event_bus.publish_json(
            &crate::gateway::event_bus::TopicEvent::new(
                "presence.left",
                serde_json::json!({"conn_id": conn_id}),
            )
            .with_state_version(ctx.state_versions.snapshot()),
        ) {
            warn!(
                error = %e,
                conn_id = %conn_id,
                "failed to publish presence.left event"
            );
        }
    }

    // 5. Subscriptions owned by this connection
    ctx.subscription_manager.remove_connection(conn_id).await;

    // 6. PTY viewport lock
    crate::gateway::pty::manager().release_conn(conn_id);

    info!("Connection closed: {}", conn_id);
}
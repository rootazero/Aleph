//! Gateway bootstrap ticket handler.
//!
//! `gateway.ticket.create` generates a short-lived, single-use bootstrap ticket
//! that a remote Panel can exchange for a per-device token during the
//! WebSocket `connect` handshake. This keeps the long-lived shared Gateway token
//! out of URLs and QR codes.
//!
//! Authorization: the caller must already be authorized (operator role) or the
//! connection must be loopback. The WS login wall enforces this because the
//! method is unreachable to unauthorized callers.

use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::security::{DeviceTokenError, DeviceTokenManager};
use crate::sync_primitives::Arc;

/// Context required by the ticket handler.
#[derive(Clone)]
pub struct TicketHandlerContext {
    pub device_token_mgr: Arc<DeviceTokenManager>,
}

/// `gateway.ticket.create` — generate a short-lived bootstrap ticket.
///
/// Request params (all optional):
/// - `ttl_seconds`: ticket lifetime in seconds (default 300, min 60)
///
/// Response:
/// - `ticket`: the bootstrap ticket string (`aleph-bt-<uuid>`)
/// - `expires_at`: Unix timestamp in milliseconds
pub async fn handle_ticket_create(
    request: JsonRpcRequest,
    ctx: Arc<TicketHandlerContext>,
) -> JsonRpcResponse {
    let ttl_seconds: Option<u64> = request
        .params
        .as_ref()
        .and_then(|p| p.get("ttl_seconds").and_then(serde_json::Value::as_u64));

    // Clamp caller-supplied ttl to a sane bounded range before the *1000 (raw
    // `s as i64 * 1000` overflows i64 for huge values) — and pass the SAME clamped
    // value used for expires_at into the manager so the reported expiry matches
    // what is stored (the manager applies a 60s floor of its own).
    let ttl_ms = ttl_seconds.map(|s| s.clamp(60, 86_400) as i64 * 1000);

    // Opportunistic hygiene — clear expired tickets / tokens on this chokepoint
    // so there is no dedicated prune daemon. Best-effort, never fails the call.
    if let Err(e) = ctx.device_token_mgr.prune_now() {
        tracing::debug!("ticket.create: prune failed: {e}");
    }

    match ctx.device_token_mgr.create_bootstrap_ticket(ttl_ms) {
        Ok(ticket) => {
            // Expiration is 5 minutes from now by default; compute client-facing value.
            let ttl_ms = ttl_ms.unwrap_or(5 * 60 * 1000);
            let expires_at = current_timestamp_ms() + ttl_ms;
            JsonRpcResponse::success(
                request.id,
                json!({
                    "ticket": ticket,
                    "expires_at": expires_at,
                }),
            )
        }
        Err(DeviceTokenError::Storage(e)) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ticket creation failed: {e}"),
        ),
        Err(DeviceTokenError::InvalidBootstrapTicket) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "invalid bootstrap ticket configuration".to_string(),
        ),
        Err(DeviceTokenError::InvalidDeviceToken) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "invalid device token configuration".to_string(),
        ),
    }
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

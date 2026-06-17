//! Panel device tier RPC handlers (G2/G3).
//!
//! List paired remote Panel devices and grant/revoke their Config tier. All
//! three are operator-only — gated both at the WS dispatch RPC gate and in
//! `method_authz::OPERATOR_RPC_METHODS`. Shape mirrors `handlers::pairing`.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::inbound_router::ChannelPermissionLevel;
use crate::gateway::panel_devices::{tier_label, PanelDeviceStore};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// `devices.list` — all known Panel devices, most-recently-seen first. Each
/// entry carries `tier` (`"chat"` / `"config"`) plus seen/grant timestamps.
pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<dyn PanelDeviceStore>,
) -> JsonRpcResponse {
    match store.list().await {
        Ok(devices) => JsonRpcResponse::success(request.id, json!({ "devices": devices })),
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("devices.list failed: {e}"))
        }
    }
}

/// `devices.set_level` — set a device's tier. Params: `device_id` (string),
/// `level` (`"chat"` | `"config"`). Upserts, so an operator may pre-authorize a
/// device id before it first connects.
pub async fn handle_set_level(
    request: JsonRpcRequest,
    store: Arc<dyn PanelDeviceStore>,
) -> JsonRpcResponse {
    let params = request.params.clone().unwrap_or_else(|| json!({}));
    let device_id = params
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let level = params.get("level").and_then(|v| v.as_str()).unwrap_or("");
    if device_id.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "device_id is required");
    }
    let tier = match level {
        "config" | "operator" => ChannelPermissionLevel::Config,
        "chat" | "guest" => ChannelPermissionLevel::Chat,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "level must be 'chat' or 'config'",
            )
        }
    };
    match store.set_tier(device_id, tier, None).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({ "ok": true, "device_id": device_id, "level": tier_label(tier) }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("devices.set_level failed: {e}"),
        ),
    }
}

/// `devices.revoke` — forget a device. It re-registers at the Chat default on
/// its next handshake. Params: `device_id` (string).
pub async fn handle_revoke(
    request: JsonRpcRequest,
    store: Arc<dyn PanelDeviceStore>,
) -> JsonRpcResponse {
    let params = request.params.clone().unwrap_or_else(|| json!({}));
    let device_id = params
        .get("device_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if device_id.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "device_id is required");
    }
    match store.remove(device_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("devices.revoke failed: {e}"),
        ),
    }
}

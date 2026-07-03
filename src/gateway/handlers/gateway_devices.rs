//! Paired-device management handlers.
//!
//! `gateway.devices.list` returns the remote Panel devices that have paired via
//! the bootstrap-ticket flow; `gateway.devices.revoke` kicks one of them by
//! invalidating its device record and tokens. Together they give an operator a
//! visible, revocable inventory of remote devices — the per-device counterpart
//! to `gateway.token.rotate`'s "revoke all remotes" hammer.
//!
//! Scope guard: both operate only on `device_type = "panel"` rows, never on
//! cluster nodes (`role = "node"`), which are managed through `cluster.rs`.
//!
//! Authorization: reachable only by an authorized (operator / loopback)
//! connection — the WS login wall refuses every non-`connect` method to an
//! unauthorized caller, so no extra per-method gate is needed.
//!
//! Known limitation: revocation takes effect at the next `connect` handshake
//! (device tokens are validated there). An already-open socket is not force
//! closed; `gateway.token.rotate` remains the immediate kick-everyone path.

use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::gateway::security::DeviceTokenManager;
use crate::sync_primitives::Arc;

/// Context required by the device-management handlers.
#[derive(Clone)]
pub struct DevicesHandlerContext {
    pub device_token_mgr: Arc<DeviceTokenManager>,
}

/// `gateway.devices.list` — list paired remote Panel devices.
///
/// Response: `{ "devices": [{ device_id, device_name, created_at,
/// last_seen_at }] }`. Also opportunistically prunes expired bootstrap tickets
/// / device tokens (no dedicated daemon task).
pub async fn handle_devices_list(
    request: JsonRpcRequest,
    ctx: Arc<DevicesHandlerContext>,
) -> JsonRpcResponse {
    // Opportunistic hygiene — cheap, best-effort, never fails the request.
    if let Err(e) = ctx.device_token_mgr.prune_now() {
        tracing::debug!("devices.list: prune failed: {e}");
    }

    let devices = match ctx.device_token_mgr.list_panel_devices() {
        Ok(rows) => rows,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                crate::gateway::protocol::INTERNAL_ERROR,
                format!("failed to list devices: {e}"),
            );
        }
    };

    let list: Vec<_> = devices
        .into_iter()
        .map(|d| {
            json!({
                "device_id": d.device_id,
                "device_name": d.device_name,
                "created_at": d.created_at,
                "last_seen_at": d.last_seen_at,
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "devices": list }))
}

/// `gateway.devices.revoke` — revoke one paired Panel device by id.
///
/// Request params: `{ "device_id": "device-…" }` (required).
/// Response: `{ "revoked": bool }` — `false` when the id is unknown, already
/// revoked, or is not a Panel device (e.g. a cluster node).
pub async fn handle_devices_revoke(
    request: JsonRpcRequest,
    ctx: Arc<DevicesHandlerContext>,
) -> JsonRpcResponse {
    let device_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("device_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let Some(device_id) = device_id else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "missing required param: device_id".to_string(),
        );
    };

    match ctx.device_token_mgr.revoke_panel_device(device_id) {
        Ok(revoked) => JsonRpcResponse::success(request.id, json!({ "revoked": revoked })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            crate::gateway::protocol::INTERNAL_ERROR,
            format!("failed to revoke device: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::SecurityStore;

    fn ctx() -> Arc<DevicesHandlerContext> {
        Arc::new(DevicesHandlerContext {
            device_token_mgr: Arc::new(DeviceTokenManager::new(Arc::new(
                SecurityStore::in_memory().unwrap(),
            ))),
        })
    }

    #[tokio::test]
    async fn list_returns_only_paired_panels() {
        let ctx = ctx();
        let ticket = ctx.device_token_mgr.create_bootstrap_ticket(None).unwrap();
        ctx.device_token_mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();

        let req = JsonRpcRequest::with_id("gateway.devices.list", None, json!(1));
        let resp = handle_devices_list(req, ctx).await;
        assert!(resp.is_success(), "{resp:?}");
        let devices = resp.result.unwrap();
        let arr = devices.get("devices").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].get("device_id").unwrap(), "panel-1");
    }

    #[tokio::test]
    async fn revoke_requires_device_id() {
        let req = JsonRpcRequest::with_id("gateway.devices.revoke", Some(json!({})), json!(1));
        let resp = handle_devices_revoke(req, ctx()).await;
        assert!(resp.is_error());
    }

    #[tokio::test]
    async fn revoke_invalidates_the_device_token() {
        let ctx = ctx();
        let ticket = ctx.device_token_mgr.create_bootstrap_ticket(None).unwrap();
        let paired = ctx
            .device_token_mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();

        let req = JsonRpcRequest::with_id(
            "gateway.devices.revoke",
            Some(json!({ "device_id": "panel-1" })),
            json!(1),
        );
        let resp = handle_devices_revoke(req, ctx.clone()).await;
        assert!(resp.is_success(), "{resp:?}");
        assert_eq!(resp.result.unwrap().get("revoked").unwrap(), &json!(true));

        assert!(ctx
            .device_token_mgr
            .validate_device_token(&paired.device_token)
            .unwrap()
            .is_none());
    }
}

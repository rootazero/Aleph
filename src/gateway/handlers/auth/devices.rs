//! Device management handlers
//!
//! Handles device listing and revocation.

use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

use super::AuthContext;

/// Handle "devices.list" request
pub async fn handle_devices_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let devices = ctx.device_store.list_devices();

    let items: Vec<serde_json::Value> = devices
        .into_iter()
        .map(|d| {
            json!({
                "device_id": d.device_id,
                "device_name": d.device_name,
                "device_type": d.device_type,
                "approved_at": d.approved_at,
                "last_seen_at": d.last_seen_at,
                "permissions": d.permissions,
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({"devices": items}))
}

/// Handle "devices.revoke" request
pub async fn handle_devices_revoke(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct RevokeParams {
        device_id: String,
    }

    let params: RevokeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Revoke device from store
    match ctx.device_store.revoke_device(&params.device_id) {
        Ok(true) => {
            // Also revoke any tokens for this device
            if let Err(e) = ctx.token_manager.revoke_device_tokens(&params.device_id) {
                warn!(error = %e, "Failed to revoke device tokens");
            }

            info!(device_id = %params.device_id, "Device revoked");
            JsonRpcResponse::success(request.id, json!({"revoked": true}))
        }
        Ok(false) => JsonRpcResponse::error(request.id, -32004, "Device not found"),
        Err(e) => JsonRpcResponse::error(request.id, -32603, format!("Failed to revoke: {e}")),
    }
}

/// Handle "devices.set_level" request — permanently change an approved
/// device's permission tier (chat <-> config) and refresh any live
/// connection(s) of that device in place so the change takes effect on the
/// device's next request (no reconnect required). Operator-only (gated in
/// method_authz). device_store.permissions is the SSOT; security_store is
/// deliberately not touched (its device role/scopes are not consulted by the
/// live authz gate).
pub async fn handle_devices_set_level(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct SetLevelParams {
        device_id: String,
        /// "config" => operator/config tier, "chat" => chat tier.
        level: String,
    }

    let params: SetLevelParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Reject any level that isn't exactly chat/config (case-insensitive).
    // We do NOT fall back to Tier::from_level's silent default-to-Chat here:
    // a typo must not silently downgrade an operator device.
    let level_lc = params.level.to_ascii_lowercase();
    if level_lc != "chat" && level_lc != "config" {
        return JsonRpcResponse::error(
            request.id,
            -32602,
            format!(
                "invalid level '{}': expected 'chat' or 'config'",
                params.level
            ),
        );
    }

    // Device must already be paired.
    if ctx.device_store.get_device(&params.device_id).is_none() {
        return JsonRpcResponse::error(request.id, -32004, "Device not found");
    }

    let tier = super::tier::Tier::from_level(Some(level_lc.as_str()));
    let permissions = tier.permissions();

    // Persist the new tier. device_store.permissions is what the connect path
    // re-derives the connection role from, so this alone fixes future reconnects.
    match ctx
        .device_store
        .update_permissions(&params.device_id, &permissions)
    {
        Ok(true) => {}
        Ok(false) => return JsonRpcResponse::error(request.id, -32004, "Device not found"),
        Err(e) => {
            warn!(error = %e, "Failed to update device permissions");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to update permissions: {e}"),
            );
        }
    }

    // Refresh any LIVE connection(s) for this device in place. The method-authz
    // gate and Phase-2 caller_role both read the live ConnectionState per
    // request, so a downgrade bites on the next request without reconnect.
    let new_role = super::tier::role_for_permissions(&permissions).to_string();
    {
        let mut conns = ctx.connections.write().await;
        for state in conns.values_mut() {
            if state.device_id.as_deref() == Some(params.device_id.as_str()) {
                state.role = Some(new_role.clone());
                state.permissions = permissions.clone();
            }
        }
    }

    info!(device_id = %params.device_id, level = %level_lc, "Device level set");
    JsonRpcResponse::success(
        request.id,
        json!({
            "device_id": params.device_id,
            "level": level_lc,
            "permissions": permissions,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::device_store::ApprovedDevice;

    #[tokio::test]
    async fn test_devices_list() {
        let ctx = super::super::tests::create_test_context();

        // Add a device
        ctx.device_store
            .approve_device(&ApprovedDevice::new(
                "test-device".to_string(),
                "Test Device".to_string(),
                Some("macos".to_string()),
            ))
            .unwrap();

        let request = JsonRpcRequest::with_id("devices.list", None, json!(1));
        let response = handle_devices_list(request, ctx).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let devices = result.get("devices").unwrap().as_array().unwrap();
        assert_eq!(devices.len(), 1);
    }

    #[tokio::test]
    async fn set_level_upgrade_persists_and_refreshes_live_connection() {
        use crate::gateway::server::ConnectionState;
        let ctx = super::super::tests::create_test_context();

        let mut dev = ApprovedDevice::new("dev-up".to_string(), "Up".to_string(), None);
        dev.permissions = vec!["chat".to_string(), "read".to_string()];
        ctx.device_store.approve_device(&dev).unwrap();

        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        cs.authenticate(
            "dev-up".to_string(),
            vec!["chat".to_string(), "read".to_string()],
            Some("guest".to_string()),
        );
        ctx.connections.write().await.insert("c-up".to_string(), cs);

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-up", "level": "config"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(resp.is_success(), "{:?}", resp);

        let stored = ctx.device_store.get_device("dev-up").unwrap();
        assert_eq!(stored.permissions, vec!["*".to_string()]);

        let conns = ctx.connections.read().await;
        let live = conns.get("c-up").unwrap();
        assert_eq!(live.role.as_deref(), Some("operator"));
        assert_eq!(live.permissions, vec!["*".to_string()]);
    }

    #[tokio::test]
    async fn set_level_downgrade_persists_and_refreshes_live_connection() {
        use crate::gateway::server::ConnectionState;
        let ctx = super::super::tests::create_test_context();

        let mut dev = ApprovedDevice::new("dev-dn".to_string(), "Dn".to_string(), None);
        dev.permissions = vec!["*".to_string()];
        ctx.device_store.approve_device(&dev).unwrap();

        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        cs.authenticate(
            "dev-dn".to_string(),
            vec!["*".to_string()],
            Some("operator".to_string()),
        );
        ctx.connections.write().await.insert("c-dn".to_string(), cs);

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-dn", "level": "chat"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(resp.is_success(), "{:?}", resp);

        let stored = ctx.device_store.get_device("dev-dn").unwrap();
        assert_eq!(
            stored.permissions,
            vec!["chat".to_string(), "read".to_string()]
        );

        let conns = ctx.connections.read().await;
        let live = conns.get("c-dn").unwrap();
        assert_eq!(live.role.as_deref(), Some("guest"));
        assert_eq!(
            live.permissions,
            vec!["chat".to_string(), "read".to_string()]
        );
    }

    #[tokio::test]
    async fn set_level_unknown_device_errors() {
        let ctx = super::super::tests::create_test_context();
        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "nope", "level": "config"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx).await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, -32004);
    }

    #[tokio::test]
    async fn set_level_invalid_level_errors_without_mutating() {
        let ctx = super::super::tests::create_test_context();
        let mut dev = ApprovedDevice::new("dev-bad".to_string(), "Bad".to_string(), None);
        dev.permissions = vec!["*".to_string()];
        ctx.device_store.approve_device(&dev).unwrap();

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-bad", "level": "admin"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.as_ref().unwrap().code, -32602);

        let stored = ctx.device_store.get_device("dev-bad").unwrap();
        assert_eq!(stored.permissions, vec!["*".to_string()]);
    }

    #[tokio::test]
    async fn set_level_leaves_other_devices_connections_untouched() {
        use crate::gateway::server::ConnectionState;
        let ctx = super::super::tests::create_test_context();

        let mut target = ApprovedDevice::new("dev-t".to_string(), "T".to_string(), None);
        target.permissions = vec!["*".to_string()];
        ctx.device_store.approve_device(&target).unwrap();

        let mut other = ConnectionState::new("127.0.0.1".parse().unwrap());
        other.authenticate(
            "dev-other".to_string(),
            vec!["*".to_string()],
            Some("operator".to_string()),
        );
        ctx.connections
            .write()
            .await
            .insert("c-other".to_string(), other);

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-t", "level": "chat"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(resp.is_success(), "{:?}", resp);

        let conns = ctx.connections.read().await;
        let other = conns.get("c-other").unwrap();
        assert_eq!(other.role.as_deref(), Some("operator"));
        assert_eq!(other.permissions, vec!["*".to_string()]);
    }
}

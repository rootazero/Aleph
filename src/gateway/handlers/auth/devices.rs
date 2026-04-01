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
        Err(e) => JsonRpcResponse::error(request.id, -32603, format!("Failed to revoke: {}", e)),
    }
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
}

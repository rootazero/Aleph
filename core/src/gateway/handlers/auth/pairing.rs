//! Pairing handlers
//!
//! Handles device pairing approval, rejection, and listing.

use serde::Deserialize;
use serde_json::{json, Value};
use crate::sync_primitives::Arc;
use tracing::{info, warn};

use crate::gateway::device_store::ApprovedDevice;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_FAILED};
use crate::gateway::handlers::parse_params;
use crate::gateway::security::{DeviceRole, DeviceType, PairingRequest};
use crate::gateway::security::store::DeviceUpsertData;

use super::AuthContext;

/// Handle "pairing.approve" request (from authorized client or CLI)
pub async fn handle_pairing_approve(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct ApproveParams {
        code: String,
    }

    let params: ApproveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Confirm pairing (removes from pending and returns the request)
    let pairing_request = match ctx.pairing_manager.confirm_pairing(&params.code) {
        Ok(req) => req,
        Err(e) => {
            warn!(code = %params.code, error = %e, "Invalid or expired pairing code");
            return JsonRpcResponse::error(request.id, AUTH_FAILED, "Invalid or expired pairing code");
        }
    };

    // Extract device info from pairing request
    let (device_name, device_type): (String, Option<String>) = match &pairing_request {
        PairingRequest::Device { device_name, device_type, .. } => {
            (device_name.clone(), device_type.map(|t: DeviceType| t.as_str().to_string()))
        }
        PairingRequest::Channel { channel, sender_id, .. } => {
            // Channel pairing approved - approve the sender via SecurityStore
            if let Err(e) = ctx.security_store.approve_sender(channel, sender_id) {
                warn!(error = %e, "Failed to approve sender");
                return JsonRpcResponse::error(request.id, -32603, format!("Failed to approve sender: {}", e));
            }
            info!(channel = %channel, sender_id = %sender_id, "Channel sender approved");
            return JsonRpcResponse::success(request.id, json!({
                "channel": channel,
                "sender_id": sender_id,
                "approved": true,
            }));
        }
    };

    // Generate device ID and create approved device
    let device_id = uuid::Uuid::new_v4().to_string();
    let device = ApprovedDevice::new(
        device_id.clone(),
        device_name.clone(),
        device_type,
    );

    // Store in device store
    if let Err(e) = ctx.device_store.approve_device(&device) {
        warn!(error = %e, "Failed to store approved device");
        return JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to store device: {}", e),
        );
    }

    // Register device in SecurityStore for token FK constraint
    let device_fingerprint: String = device_id.chars().take(16).collect();
    if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
        device_id: &device_id,
        device_name: &device_name,
        device_type: None,
        public_key: &[0u8; 32], // placeholder public key
        fingerprint: &device_fingerprint, // use device_id prefix as fingerprint
        role: "operator",
        scopes: &["*".to_string()],
    }) {
        warn!(error = %e, "Failed to register device in security store");
        return JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to register device: {}", e),
        );
    }

    // Generate token for the new device
    let signed_token = match ctx
        .token_manager
        .issue_token(&device_id, DeviceRole::Operator, vec!["*".to_string()])
    {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "Failed to issue token");
            return JsonRpcResponse::error(request.id, -32603, format!("Failed to issue token: {}", e));
        }
    };

    info!(
        device_id = %device_id,
        device_name = %device_name,
        "Device pairing approved"
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "device_id": device_id,
            "device_name": device_name,
            "token": format!("{}:{}", signed_token.token, signed_token.signature),
            "approved_at": device.approved_at,
        }),
    )
}

/// Handle "pairing.reject" request
pub async fn handle_pairing_reject(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct RejectParams {
        code: String,
    }

    let params: RejectParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match ctx.pairing_manager.cancel_pairing(&params.code) {
        Ok(true) => {
            info!(code = %params.code, "Pairing rejected");
            JsonRpcResponse::success(request.id, json!({"rejected": true}))
        }
        Ok(false) => {
            JsonRpcResponse::error(request.id, AUTH_FAILED, "Invalid or expired pairing code")
        }
        Err(e) => {
            warn!(error = %e, "Failed to cancel pairing");
            JsonRpcResponse::error(request.id, -32603, format!("Failed to cancel pairing: {}", e))
        }
    }
}

/// Handle "pairing.list" request
pub async fn handle_pairing_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let pending = match ctx.pairing_manager.list_pending() {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to list pending pairings");
            return JsonRpcResponse::error(request.id, -32603, format!("Failed to list pending pairings: {}", e));
        }
    };

    let items: Vec<Value> = pending
        .into_iter()
        .map(|req| {
            match req {
                PairingRequest::Device { code, device_name, device_type, expires_at, created_at, .. } => {
                    let remaining = if expires_at > created_at {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                            .as_millis() as i64;
                        if expires_at > now {
                            ((expires_at - now) / 1000) as u64
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    json!({
                        "type": "device",
                        "code": code,
                        "device_name": device_name,
                        "device_type": device_type.map(|t: DeviceType| t.as_str()),
                        "expires_in": remaining,
                    })
                }
                PairingRequest::Channel { code, channel, sender_id, expires_at, created_at, .. } => {
                    let remaining = if expires_at > created_at {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                            .as_millis() as i64;
                        if expires_at > now {
                            ((expires_at - now) / 1000) as u64
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    json!({
                        "type": "channel",
                        "code": code,
                        "channel": channel,
                        "sender_id": sender_id,
                        "expires_in": remaining,
                    })
                }
            }
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({"pending": items}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::connect::handle_connect;

    #[tokio::test]
    async fn test_pairing_flow() {
        let ctx = super::super::tests::create_test_context();

        // Step 1: Try to connect (should get pairing code)
        let connect_req = JsonRpcRequest::new(
            "connect",
            Some(json!({"device_name": "Test Device"})),
            Some(json!(1)),
        );
        let connect_resp = handle_connect(connect_req, ctx.clone()).await;
        let code = connect_resp
            .error
            .unwrap()
            .data
            .unwrap()
            .get("code")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        // Step 2: Approve pairing
        let approve_req = JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({"code": code})),
            Some(json!(2)),
        );
        let approve_resp = handle_pairing_approve(approve_req, ctx.clone()).await;
        assert!(approve_resp.is_success());

        let result = approve_resp.result.unwrap();
        let device_id = result.get("device_id").unwrap().as_str().unwrap();

        // Step 3: Connect with approved device_id
        let reconnect_req = JsonRpcRequest::new(
            "connect",
            Some(json!({"device_id": device_id})),
            Some(json!(3)),
        );
        let reconnect_resp = handle_connect(reconnect_req, ctx).await;
        assert!(reconnect_resp.is_success());
    }
}

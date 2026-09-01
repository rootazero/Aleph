//! `gateway.token.{current,rotate}` — read / rotate the shared Gateway token.
//!
//! `current` returns the plaintext token so an authorized operator can display
//! it / build a QR or LAN URL to authorize other devices. `rotate` generates a
//! fresh token (re-encrypting the secret vault) **and revokes every paired
//! remote Panel device**, so a stolen long-lived device token cannot outlive a
//! rotation — this is the "revoke all remotes" path. Cluster nodes keep their
//! own enrollment (`cluster.rs`).
//!
//! Both are reachable only by an authorized connection: the WS login wall
//! refuses every non-`connect` method to an unauthorized caller, consistent
//! with the model where an authorized Panel has full local-equivalent authority.

use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::security::{DeviceTokenManager, SharedTokenManager};
use crate::sync_primitives::Arc;

/// `gateway.token.current` — the shared Gateway token plaintext (or `null` if
/// none is loaded in this process).
pub async fn handle_token_current(request: JsonRpcRequest) -> JsonRpcResponse {
    let token = SharedTokenManager::global().and_then(|m| m.get_current_token());
    JsonRpcResponse::success(request.id, json!({ "token": token }))
}

/// `gateway.token.rotate` — generate a fresh shared token and revoke every
/// paired remote Panel device. Returns the new token (so the operator can
/// re-share / re-display it) plus `revoked_devices` (how many paired devices
/// were kicked). `device_token_mgr` is `None` only when the device-token
/// subsystem is unwired (legacy fallback), in which case device revocation is
/// skipped.
pub async fn handle_token_rotate(
    request: JsonRpcRequest,
    device_token_mgr: Option<Arc<DeviceTokenManager>>,
) -> JsonRpcResponse {
    let Some(mgr) = SharedTokenManager::global() else {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "shared token manager unavailable".to_string(),
        );
    };
    let token = match mgr.reset_token() {
        Ok(token) => token,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("token rotation failed: {e}"),
            );
        }
    };

    // Rotating the shared token is the "revoke all remotes" action: also revoke
    // every paired Panel device so a leaked device token cannot survive a
    // rotation. Best-effort — the shared token has already rotated, so a
    // device-revocation failure is logged, not surfaced as an error.
    let mut revoked_devices = 0usize;
    if let Some(dt) = device_token_mgr {
        match dt.revoke_all_panel_devices() {
            Ok(n) => revoked_devices = n,
            Err(e) => tracing::warn!("token rotate: revoking paired devices failed: {e}"),
        }
    }

    JsonRpcResponse::success(request.id, {
        // Token rotation is the "revoke all remotes" action — the most
        // consequential single call on the admin surface. The audit entry
        // records the rotation and how many paired devices were revoked
        // alongside it, never the new token itself.
        if let Some(log) = crate::security::audit::global() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!(
                        "gateway.token.rotate: rotated shared token; revoked {revoked_devices} paired device(s)"
                    ),
                ));
        }
        json!({ "token": token, "revoked_devices": revoked_devices })
    })
}

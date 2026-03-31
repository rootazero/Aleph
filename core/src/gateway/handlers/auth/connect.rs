//! Connect handler
//!
//! Handles the main "connect" authentication endpoint.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_FAILED, INVALID_PARAMS};
use crate::gateway::security::store::DeviceUpsertData;
use crate::gateway::security::DeviceRole;

use super::{AuthContext, ConnectParams, ConnectResult, PairingRequiredParams};

/// Handle "connect" request
///
/// This is the main authentication endpoint. Clients must call this
/// before any other methods when auth is required.
pub async fn handle_connect(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse {
    // Parse parameters
    let params: ConnectParams = match &request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map.clone())) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid connect params: {}", e),
                );
            }
        },
        _ => ConnectParams {
            token: None,
            shared_token: None,
            invitation_token: None,
            device_name: None,
            device_type: None,
            device_id: None,
        },
    };

    // Check for guest invitation token FIRST
    // Guest invitations should work regardless of auth_mode setting
    if let Some(invitation_token) = &params.invitation_token {
        debug!(
            "Processing guest invitation token: {}...",
            invitation_token.get(..8).unwrap_or("***")
        );
        match ctx.invitation_manager.activate_invitation(invitation_token) {
            Ok(guest_token) => {
                debug!(
                    "Guest invitation activated successfully for guest_id: {}",
                    guest_token.guest_id
                );

                // Generate a unique session ID
                let session_id = uuid::Uuid::new_v4().to_string();
                let connection_id = "pending".to_string(); // Will be updated by server

                // Get guest name from scope or use default
                let guest_name = guest_token
                    .scope
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "Guest".to_string());

                // Register guest session
                let session = ctx.guest_session_manager.register_session(
                    session_id.clone(),
                    guest_token.guest_id.clone(),
                    guest_name.clone(),
                    connection_id,
                    guest_token.scope.clone(),
                );

                // Emit session connected event
                let event = crate::gateway::event_bus::TopicEvent {
                    topic: "guest.session.connected".to_string(),
                    data: serde_json::json!({
                        "session_id": session.session_id,
                        "guest_id": session.guest_id,
                        "guest_name": session.guest_name,
                        "connected_at": session.connected_at
                    }),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                };
                let _ = ctx.event_bus.publish_json(&event);

                info!(
                    guest_id = %guest_token.guest_id,
                    session_id = %session_id,
                    "Guest session created via invitation"
                );

                // Return session info (no persistent token for guests)
                return JsonRpcResponse::success(
                    request.id,
                    json!(ConnectResult {
                        token: format!("guest:{}:{}", session_id, guest_token.token),
                        device_id: format!("guest-{}", guest_token.guest_id),
                        permissions: guest_token
                            .scope
                            .allowed_tools
                            .iter()
                            .map(|t| format!("tool:{}", t))
                            .collect(),
                        expires_at: guest_token
                            .scope
                            .expires_at
                            .and_then(chrono::DateTime::from_timestamp_millis)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| {
                                (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()
                            }),
                    }),
                );
            }
            Err(e) => {
                debug!(error = %e, "Invalid invitation token");
                return JsonRpcResponse::error(
                    request.id,
                    -32001,
                    format!("Invalid invitation: {}", e),
                );
            }
        }
    }

    // Case 0: Shared token authentication (before device token check)
    if let Some(ref shared_token) = params.shared_token {
        debug!("Processing shared token authentication");
        match ctx.shared_token_mgr.validate(shared_token) {
            Ok(true) => {
                let device_id = params
                    .device_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let device_name = params.device_name.as_deref().unwrap_or("Web Panel");

                // Register device in SecurityStore (required for FK constraint on tokens)
                let device_fingerprint: String = device_id.chars().take(16).collect();
                if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
                    device_id: &device_id,
                    device_name,
                    device_type: None,
                    public_key: &[0u8; 32],
                    fingerprint: &device_fingerprint,
                    role: DeviceRole::Operator.as_str(),
                    scopes: &["*".to_string()],
                }) {
                    warn!(error = %e, "Failed to register device via shared token");
                    return JsonRpcResponse::error(
                        request.id,
                        -32603,
                        format!("Failed to register device: {}", e),
                    );
                }

                let signed_token = match ctx.token_manager.issue_token(
                    &device_id,
                    DeviceRole::Operator,
                    vec!["*".to_string()],
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = %e, "Failed to issue token");
                        return JsonRpcResponse::error(
                            request.id,
                            -32603,
                            format!("Failed to issue token: {}", e),
                        );
                    }
                };

                info!(device_id = %device_id, "Connection authenticated via shared token");

                return JsonRpcResponse::success(
                    request.id,
                    json!(ConnectResult {
                        token: format!("{}:{}", signed_token.token, signed_token.signature),
                        device_id,
                        permissions: vec!["*".to_string()],
                        expires_at: chrono::DateTime::from_timestamp_millis(
                            signed_token.expires_at
                        )
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(
                            || (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()
                        ),
                    }),
                );
            }
            Ok(false) => {
                debug!("Invalid shared token provided");
                return JsonRpcResponse::error(request.id, AUTH_FAILED, "Invalid shared token");
            }
            Err(e) => {
                warn!(error = %e, "Shared token validation error");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Token validation error: {}", e),
                );
            }
        }
    }

    // If authentication is not required, allow any connection
    if !ctx.auth_mode.is_auth_required() {
        let device_id = params
            .device_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Register device in SecurityStore first (required for FK constraint on tokens)
        let device_name = params.device_name.as_deref().unwrap_or("Auto-Device");
        let device_fingerprint: String = device_id.chars().take(16).collect();
        if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
            device_id: &device_id,
            device_name,
            device_type: None,
            public_key: &[0u8; 32],           // Placeholder public key
            fingerprint: &device_fingerprint, // Use prefix as fingerprint
            role: DeviceRole::Operator.as_str(),
            scopes: &["*".to_string()],
        }) {
            warn!(error = %e, "Failed to register device");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to register device: {}", e),
            );
        }

        let signed_token = match ctx.token_manager.issue_token(
            &device_id,
            DeviceRole::Operator,
            vec!["*".to_string()],
        ) {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "Failed to issue token");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to issue token: {}", e),
                );
            }
        };

        info!(device_id = %device_id, "Connection accepted (auth not required)");

        return JsonRpcResponse::success(
            request.id,
            json!(ConnectResult {
                token: format!("{}:{}", signed_token.token, signed_token.signature),
                device_id,
                permissions: vec!["*".to_string()],
                expires_at: chrono::DateTime::from_timestamp_millis(signed_token.expires_at)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(
                        || (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()
                    ),
            }),
        );
    }

    debug!(
        "Processing connect request: device_id={:?}, has_token={}, has_invitation_token={}",
        params.device_id,
        params.token.is_some(),
        params.invitation_token.is_some()
    );

    // Case 1: Client has a token - validate it
    if let Some(token_str) = &params.token {
        debug!("Case 1: Validating existing token");
        // Token format: "{token}:{signature}"
        if let Some((token, signature)) = token_str.split_once(':') {
            match ctx.token_manager.validate_token(token, signature) {
                Ok(validation) => {
                    // Security: If client provides a device_id, it must match the token's
                    // device_id to prevent identity spoofing (using a valid token from
                    // device-A to impersonate device-B).
                    if let Some(ref claimed_id) = params.device_id {
                        if claimed_id != &validation.device_id {
                            return JsonRpcResponse::error(
                                request.id,
                                -32001,
                                "device_id mismatch: token was issued for a different device",
                            );
                        }
                    }
                    let device_id = validation.device_id.clone();

                    // Update last seen time if device is in store
                    let _ = ctx.device_store.update_last_seen(&device_id);

                    info!(device_id = %device_id, "Connection authenticated via token");

                    return JsonRpcResponse::success(
                        request.id,
                        json!(ConnectResult {
                            token: token_str.clone(),
                            device_id,
                            permissions: validation.scopes,
                            expires_at: chrono::DateTime::from_timestamp_millis(
                                chrono::Utc::now().timestamp_millis() + validation.remaining_ms
                            )
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(
                                || (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()
                            ),
                        }),
                    );
                }
                Err(e) => {
                    debug!(error = %e, "Invalid token provided");
                    // Token invalid, fall through to pairing
                }
            }
        } else {
            debug!("Invalid token format (expected token:signature)");
            // Token invalid, fall through to pairing
        }
    }

    // Case 2: Check if device_id is already approved
    if let Some(device_id) = &params.device_id {
        debug!("Case 2: Checking if device is approved: {}", device_id);
        if ctx.device_store.is_approved(device_id) {
            debug!("Device is approved, issuing token");
            // Device is approved, generate new token
            let device = ctx.device_store.get_device(device_id);
            let permissions = device
                .as_ref()
                .map(|d| d.permissions.clone())
                .unwrap_or_else(|| vec!["*".to_string()]);

            let signed_token = match ctx.token_manager.issue_token(
                device_id,
                DeviceRole::Operator,
                permissions.clone(),
            ) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Failed to issue token");
                    return JsonRpcResponse::error(
                        request.id,
                        -32603,
                        format!("Failed to issue token: {}", e),
                    );
                }
            };

            // Update last seen
            let _ = ctx.device_store.update_last_seen(device_id);

            info!(device_id = %device_id, "Connection authenticated via approved device");

            return JsonRpcResponse::success(
                request.id,
                json!(ConnectResult {
                    token: format!("{}:{}", signed_token.token, signed_token.signature),
                    device_id: device_id.clone(),
                    permissions,
                    expires_at: chrono::DateTime::from_timestamp_millis(signed_token.expires_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(
                            || (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339()
                        ),
                }),
            );
        }
    }

    // Case 3: New device - initiate pairing
    debug!("Case 3: Initiating new device pairing");
    let device_name = params
        .device_name
        .unwrap_or_else(|| "Unknown Device".to_string());
    let _device_type = params.device_type;
    let _device_id = params
        .device_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Check if there's already a pending pairing for this device
    // (to prevent spamming pairing requests)
    let pending = match ctx.pairing_manager.list_pending() {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to list pending pairings");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to list pending pairings: {}", e),
            );
        }
    };

    // Find existing device pairing by name
    if let Some(existing) = pending.iter().find(|req| {
        matches!(req, PairingRequest::Device { device_name: name, .. } if name == &device_name)
    }) {
        let code = existing.code();
        let remaining = existing.remaining_secs();
        info!(
            device_name = %device_name,
            code = %code,
            "Returning existing pairing code"
        );

        return JsonRpcResponse::error_with_data(
            request.id,
            AUTH_FAILED,
            "pairing_required",
            json!(PairingRequiredParams {
                code: code.to_string(),
                expires_in: remaining,
                message: format!(
                    "Enter code {} to approve this device, or run: aleph-gateway pairing approve {}",
                    code, code
                ),
            }),
        );
    }

    // Initiate new pairing (device pairing without public key for now - legacy compatibility)
    // In a full Ed25519 implementation, the client would provide a public key
    let pairing_request = match ctx.pairing_manager.request_device_pairing(
        device_name.clone(),
        None,          // device_type parsed as DeviceType
        vec![0u8; 32], // placeholder public key for legacy API
        None,          // remote_addr
    ) {
        Ok(req) => req,
        Err(e) => {
            warn!(error = %e, "Failed to initiate pairing");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to initiate pairing: {}", e),
            );
        }
    };

    let code = pairing_request.code().to_string();
    let expires_in = pairing_request.remaining_secs();

    info!(
        device_name = %device_name,
        code = %code,
        "Pairing initiated"
    );

    JsonRpcResponse::error_with_data(
        request.id,
        AUTH_FAILED,
        "pairing_required",
        json!(PairingRequiredParams {
            code: code.clone(),
            expires_in,
            message: format!(
                "Enter code {} to approve this device, or run: aleph-gateway pairing approve {}",
                code, code
            ),
        }),
    )
}

use crate::gateway::security::PairingRequest;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::config::AuthMode;
    use crate::gateway::device_store::DeviceStore;
    use crate::gateway::security::store::DeviceUpsertData;
    use crate::gateway::security::{
        PairingManager, SecurityStore, SharedTokenManager, TokenManager,
    };

    #[tokio::test]
    async fn test_connect_no_auth_required() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "test-dev",
                device_name: "Test",
                device_type: None,
                public_key: &[1u8; 32],
                fingerprint: "fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        let invitation_manager = Arc::new(crate::gateway::security::InvitationManager::new());
        let guest_session_manager = Arc::new(crate::gateway::security::GuestSessionManager::new());
        let event_bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let shared_token_mgr = Arc::new(SharedTokenManager::new(
            store.clone(),
            "/tmp/aleph_test.vault",
        ));

        let ctx = Arc::new(AuthContext {
            token_manager: Arc::new(TokenManager::new(store.clone())),
            pairing_manager: Arc::new(PairingManager::new(store.clone())),
            device_store: Arc::new(DeviceStore::in_memory().unwrap()),
            security_store: store,
            invitation_manager,
            guest_session_manager,
            event_bus,
            auth_mode: AuthMode::None, // Auth not required
            shared_token_mgr,
        });

        let request = JsonRpcRequest::new(
            "connect",
            Some(json!({"device_name": "Test Device"})),
            Some(json!(1)),
        );

        let response = handle_connect(request, ctx).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result.get("token").is_some());
        assert!(result.get("device_id").is_some());
    }

    #[tokio::test]
    async fn test_connect_requires_pairing() {
        let ctx = super::super::tests::create_test_context();

        let request = JsonRpcRequest::new(
            "connect",
            Some(json!({
                "device_name": "New Device",
                "device_type": "macos"
            })),
            Some(json!(1)),
        );

        let response = handle_connect(request, ctx).await;
        assert!(response.is_error());

        let error = response.error.unwrap();
        assert_eq!(error.message, "pairing_required");
        assert!(error.data.is_some());

        let data = error.data.unwrap();
        assert!(data.get("code").is_some());
    }

    #[tokio::test]
    async fn test_connect_with_shared_token() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "test-dev",
                device_name: "Test",
                device_type: None,
                public_key: &[1u8; 32],
                fingerprint: "fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        let shared_token_mgr = Arc::new(SharedTokenManager::new(
            store.clone(),
            "/tmp/aleph_test.vault",
        ));
        let token = shared_token_mgr.generate_token().unwrap();

        let invitation_manager = Arc::new(crate::gateway::security::InvitationManager::new());
        let guest_session_manager = Arc::new(crate::gateway::security::GuestSessionManager::new());
        let event_bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());

        let ctx = Arc::new(AuthContext {
            token_manager: Arc::new(TokenManager::new(store.clone())),
            pairing_manager: Arc::new(PairingManager::new(store.clone())),
            device_store: Arc::new(DeviceStore::in_memory().unwrap()),
            security_store: store,
            shared_token_mgr,
            invitation_manager,
            guest_session_manager,
            event_bus,
            auth_mode: AuthMode::Token,
        });

        let request = JsonRpcRequest::new(
            "connect",
            Some(json!({"shared_token": token, "device_name": "Test Panel"})),
            Some(json!(1)),
        );

        let response = handle_connect(request, ctx).await;
        assert!(
            response.is_success(),
            "Expected success but got: {:?}",
            response
        );

        let result = response.result.unwrap();
        assert!(result.get("token").is_some());
        assert!(result.get("device_id").is_some());
    }
}

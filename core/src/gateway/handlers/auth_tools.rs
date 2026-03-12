//! Auth management tools exposed as RPC handlers.
//! Follows R9 (Everything is a Tool) — auth config via natural language.

use serde_json::json;
use crate::sync_primitives::Arc;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::auth::AuthContext;

/// Handle "auth.show_token" — display current shared token
pub async fn handle_auth_show_token(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    match ctx.shared_token_mgr.get_current_token() {
        Some(token) => JsonRpcResponse::success(request.id, json!({
            "token": token,
            "message": "This is your current access token"
        })),
        None => JsonRpcResponse::success(request.id, json!({
            "token": null,
            "message": "Token not in memory. Check ~/.aleph/data/.shared_token"
        })),
    }
}

/// Handle "auth.reset_token" — regenerate shared token
pub async fn handle_auth_reset_token(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    match ctx.shared_token_mgr.generate_token() {
        Ok(token) => {
            // Update file
            if let Some(home) = dirs::home_dir() {
                let path = home.join(".aleph/data/.shared_token");
                if let Err(e) = std::fs::write(&path, &token) {
                    tracing::warn!("Failed to write token file: {}", e);
                } else {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            &path,
                            std::fs::Permissions::from_mode(0o600),
                        );
                    }
                }
            }
            JsonRpcResponse::success(request.id, json!({
                "token": token,
                "message": "Token regenerated. All existing sessions are now invalid."
            }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to generate token: {}", e),
        ),
    }
}

/// Handle "auth.list_sessions" — list active HTTP sessions
pub async fn handle_auth_list_sessions(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    match ctx.security_store.list_active_sessions() {
        Ok(sessions) => {
            let items: Vec<_> = sessions
                .iter()
                .map(|(session_id, created_at, expires_at, last_used_at)| {
                    json!({
                        "session_id": session_id,
                        "created_at": created_at,
                        "expires_at": expires_at,
                        "last_used_at": last_used_at,
                    })
                })
                .collect();
            let count = items.len();
            JsonRpcResponse::success(request.id, json!({
                "sessions": items,
                "count": count,
            }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to list sessions: {}", e),
        ),
    }
}

/// Handle "auth.revoke_session" — revoke a specific HTTP session
pub async fn handle_auth_revoke_session(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let session_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("session_id"))
        .and_then(|v| v.as_str());

    match session_id {
        Some(id) => match ctx.security_store.delete_session(id) {
            Ok(()) => JsonRpcResponse::success(request.id, json!({
                "revoked": true,
                "session_id": id,
            })),
            Err(e) => JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to revoke session: {}", e),
            ),
        },
        None => JsonRpcResponse::error(
            request.id,
            -32602,
            "Missing required parameter: session_id",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::gateway::security::{
        SecurityStore, SharedTokenManager, TokenManager, PairingManager,
        InvitationManager, GuestSessionManager,
    };
    use crate::gateway::security::store::DeviceUpsertData;
    use crate::gateway::device_store::DeviceStore;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::config::AuthMode;
    use crate::gateway::protocol::JsonRpcRequest;

    fn create_test_context() -> Arc<AuthContext> {
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

        Arc::new(AuthContext {
            token_manager: Arc::new(TokenManager::new(store.clone())),
            pairing_manager: Arc::new(PairingManager::new(store.clone())),
            device_store: Arc::new(DeviceStore::in_memory().unwrap()),
            security_store: store.clone(),
            invitation_manager: Arc::new(InvitationManager::new()),
            guest_session_manager: Arc::new(GuestSessionManager::new()),
            event_bus: Arc::new(GatewayEventBus::new()),
            auth_mode: AuthMode::Token,
            shared_token_mgr: Arc::new(SharedTokenManager::new(store)),
        })
    }

    #[tokio::test]
    async fn test_show_token_none() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id("auth.show_token", None, json!(1));
        let resp = handle_auth_show_token(req, ctx).await;
        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert!(result.get("token").unwrap().is_null());
    }

    #[tokio::test]
    async fn test_show_token_after_generate() {
        let ctx = create_test_context();
        // Generate a token first
        let _token = ctx.shared_token_mgr.generate_token().unwrap();

        let req = JsonRpcRequest::with_id("auth.show_token", None, json!(1));
        let resp = handle_auth_show_token(req, ctx).await;
        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert!(result.get("token").unwrap().is_string());
    }

    #[tokio::test]
    async fn test_reset_token() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id("auth.reset_token", None, json!(1));
        let resp = handle_auth_reset_token(req, ctx).await;
        assert!(resp.is_success());
        let result = resp.result.unwrap();
        let token = result.get("token").unwrap().as_str().unwrap();
        assert!(token.starts_with("aleph-"));
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id("auth.list_sessions", None, json!(1));
        let resp = handle_auth_list_sessions(req, ctx).await;
        assert!(resp.is_success());
        let result = resp.result.unwrap();
        assert_eq!(result.get("count").unwrap().as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_revoke_session_missing_param() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id("auth.revoke_session", None, json!(1));
        let resp = handle_auth_revoke_session(req, ctx).await;
        assert!(resp.is_error());
    }

    #[tokio::test]
    async fn test_revoke_session() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id(
            "auth.revoke_session",
            Some(json!({"session_id": "nonexistent"})),
            json!(1),
        );
        let resp = handle_auth_revoke_session(req, ctx).await;
        // Should succeed even if session doesn't exist (DELETE is idempotent)
        assert!(resp.is_success());
    }
}

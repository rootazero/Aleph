//! `gateway.token.{current,rotate}` — read / rotate the shared Gateway token.
//!
//! `current` returns the plaintext token so an authorized operator can display
//! it / build a QR or LAN URL to authorize other devices. `rotate` generates a
//! fresh token (re-encrypting the secret vault), invalidating every previously
//! authorized remote — the single-tier revocation path.
//!
//! Both are reachable only by an authorized connection: the WS login wall
//! refuses every non-`connect` method to an unauthorized caller, consistent
//! with the model where an authorized Panel has full local-equivalent authority.

use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::security::SharedTokenManager;

/// `gateway.token.current` — the shared Gateway token plaintext (or `null` if
/// none is loaded in this process).
pub async fn handle_token_current(request: JsonRpcRequest) -> JsonRpcResponse {
    let token = SharedTokenManager::global().and_then(|m| m.get_current_token());
    JsonRpcResponse::success(request.id, json!({ "token": token }))
}

/// `gateway.token.rotate` — generate a fresh token (revokes all prior ones).
/// Returns the new token so the operator can re-share / re-display it.
pub async fn handle_token_rotate(request: JsonRpcRequest) -> JsonRpcResponse {
    match SharedTokenManager::global() {
        Some(mgr) => match mgr.reset_token() {
            Ok(token) => JsonRpcResponse::success(request.id, json!({ "token": token })),
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("token rotation failed: {e}"),
            ),
        },
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "shared token manager unavailable".to_string(),
        ),
    }
}

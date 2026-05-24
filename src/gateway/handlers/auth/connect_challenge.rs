//! `connect.challenge` handler — issues a fresh nonce for the
//! challenge-response hardening of `connect` (T4 follow-up).
//!
//! Flow (client side):
//! 1. `connect.challenge` → returns `{nonce, timestamp, server_id}`.
//! 2. Client computes `HMAC-SHA256(token, nonce + timestamp + device_id)`
//!    and echoes `{nonce, signature}` as `connect`'s `challenge` field.
//! 3. Server verifies via [`ChallengeManager::verify`] before accepting
//!    the token. Nonces are single-use (replay-guarded) and time-windowed
//!    (`TIMESTAMP_WINDOW_SECS = 30`).
//!
//! This handler is *always available* (cheap to issue nonces). Whether
//! the server *requires* a signed response is governed by
//! `AuthContext::require_challenge`; default `false` keeps existing
//! clients working.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

use super::AuthContext;

/// Handle `connect.challenge`. Always returns a freshly-generated nonce.
pub async fn handle_connect_challenge(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let challenge = ctx.challenge_manager.generate();
    JsonRpcResponse::success(
        request.id,
        json!({
            "nonce": challenge.nonce,
            "timestamp": challenge.timestamp,
            "server_id": challenge.server_id,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::challenge::ChallengeManager;
    use serde_json::json;

    #[tokio::test]
    async fn connect_challenge_returns_fresh_nonce() {
        let ctx = super::super::tests::create_test_context();
        let req = JsonRpcRequest::with_id("connect.challenge", None, json!(1));
        let resp = handle_connect_challenge(req, ctx.clone()).await;

        assert!(resp.is_success(), "connect.challenge must succeed: {resp:?}");
        let result = resp.result.expect("success has result");
        let nonce = result["nonce"].as_str().expect("nonce must be string");
        assert_eq!(nonce.len(), 64, "nonce should be 64 hex chars");
        assert!(result["timestamp"].as_u64().is_some(), "timestamp present");
        assert!(
            !result["server_id"].as_str().unwrap_or("").is_empty(),
            "server_id non-empty"
        );

        // Second call returns a different nonce (single-use).
        let req2 = JsonRpcRequest::with_id("connect.challenge", None, json!(2));
        let resp2 = handle_connect_challenge(req2, ctx).await;
        let nonce2 = resp2.result.unwrap()["nonce"].as_str().unwrap().to_string();
        assert_ne!(nonce, nonce2, "each call must mint a fresh nonce");
    }

    #[tokio::test]
    async fn issued_nonce_can_be_verified_with_correct_signature() {
        use crate::gateway::challenge::compute_signature;
        let manager = Arc::new(ChallengeManager::with_server_id("srv-t4".to_string()));
        let challenge = manager.generate();

        let token = "device-bearer-token";
        let device_id = "device-42";
        let sig = compute_signature(token, &challenge.nonce, challenge.timestamp, device_id);

        let result = manager.verify(&challenge.nonce, device_id, &sig, token);
        assert!(result.is_ok(), "verify should succeed: {result:?}");
    }
}

//! Pairing Handlers
//!
//! RPC handlers for pairing operations: list, approve, reject.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::sync_primitives::Arc;
use tracing::debug;

use crate::gateway::pairing_store::{PairingRequest, PairingStore};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

/// Pairing request response format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequestResponse {
    pub channel: String,
    pub sender_id: String,
    pub code: String,
    pub created_at: String,
}

impl From<PairingRequest> for PairingRequestResponse {
    fn from(req: PairingRequest) -> Self {
        Self {
            channel: req.channel,
            sender_id: req.sender_id,
            code: req.code,
            created_at: req.created_at.to_rfc3339(),
        }
    }
}

/// Handle pairing.list RPC request
///
/// Lists pending pairing requests, optionally filtered by channel.
pub async fn handle_list(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let channel = request
        .params
        .as_ref()
        .and_then(|p| p.get("channel"))
        .and_then(|v| v.as_str());

    debug!("Handling pairing.list for channel: {:?}", channel);

    match store.list_pending(channel).await {
        Ok(requests) => {
            let responses: Vec<PairingRequestResponse> =
                requests.into_iter().map(|r| r.into()).collect();

            JsonRpcResponse::success(
                request.id,
                json!({
                    "requests": responses,
                    "count": responses.len(),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list pairing requests: {}", e),
        ),
    }
}

/// Handle pairing.approve RPC request
///
/// Approves a pairing request by code, adding the sender to the approved list.
pub async fn handle_approve(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel = match params.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    let code = match params.get("code").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'code' field");
        }
    };

    debug!("Handling pairing.approve for {}:{}", channel, code);

    match store.approve(channel, code).await {
        Ok(req) => {
            let response: PairingRequestResponse = req.into();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "approved": true,
                    "request": response,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to approve pairing: {}", e),
        ),
    }
}

/// Handle pairing.reject RPC request
///
/// Rejects a pairing request by code.
pub async fn handle_reject(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel = match params.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    let code = match params.get("code").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'code' field");
        }
    };

    debug!("Handling pairing.reject for {}:{}", channel, code);

    match store.reject(channel, code).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "rejected": true,
                "channel": channel,
                "code": code,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to reject pairing: {}", e),
        ),
    }
}

/// Handle pairing.approved RPC request
///
/// Lists approved senders for a channel.
pub async fn handle_approved_list(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let channel = match request
        .params
        .as_ref()
        .and_then(|p| p.get("channel"))
        .and_then(|v| v.as_str())
    {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    debug!("Handling pairing.approved for channel: {}", channel);

    match store.list_approved(channel).await {
        Ok(senders) => JsonRpcResponse::success(
            request.id,
            json!({
                "channel": channel,
                "approved": senders,
                "count": senders.len(),
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list approved senders: {}", e),
        ),
    }
}

/// Handle pairing.revoke RPC request
///
/// Revokes approval for a sender.
pub async fn handle_revoke(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let channel = match params.get("channel").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'channel' field");
        }
    };

    let sender_id = match params.get("sender_id").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing 'sender_id' field");
        }
    };

    debug!("Handling pairing.revoke for {}:{}", channel, sender_id);

    match store.revoke(channel, sender_id).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "revoked": true,
                "channel": channel,
                "sender_id": sender_id,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to revoke approval: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::pairing_store::SqlitePairingStore;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_handle_list_empty() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let request = JsonRpcRequest::with_id("pairing.list", None, json!(1));

        let response = handle_list(request, store).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["count"], 0);
    }

    #[tokio::test]
    async fn test_handle_approve() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());

        // Create a pairing request first
        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        let request = JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({
                "channel": "imessage",
                "code": code,
            })),
            Some(json!(1)),
        );

        let response = handle_approve(request, store.clone()).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["approved"], true);

        // Verify approved
        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());
    }

    #[tokio::test]
    async fn test_handle_reject() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        let request = JsonRpcRequest::new(
            "pairing.reject",
            Some(json!({
                "channel": "imessage",
                "code": code,
            })),
            Some(json!(1)),
        );

        let response = handle_reject(request, store.clone()).await;
        assert!(response.is_success());

        // Verify NOT approved
        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }
}

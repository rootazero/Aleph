//! Pairing Handlers
//!
//! RPC handlers for pairing operations: list, approve, reject.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
pub async fn handle_list(request: JsonRpcRequest, store: Arc<dyn PairingStore>) -> JsonRpcResponse {
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
            format!("Failed to list pairing requests: {e}"),
        ),
    }
}

/// Handle pairing.approve RPC request
///
/// Approves a pairing request by code, adding the sender to the approved list —
/// and, optionally, binds that sender to an Aleph principal.
///
/// # The `user_id` parameter is the other half of P0's identity link
///
/// SECURITY.md describes two independent binding paths feeding the same users
/// table: a DEVICE binds through pairing tickets (`aleph-server pair --user`),
/// and a CHANNEL SENDER binds here. The device half got its client in round 2;
/// this half was written with the store column, the SQL, and the live consumer
/// (`inbound_router::executor` stamps `ScopeAttribution::personal` from
/// `sender_user`) — and no producer. `handle_approve` hard-coded `None`, so
/// every approved sender on every channel resolved to the single-machine owner.
///
/// The consequence is not cosmetic: a member's Telegram turns were stamped
/// `personal:u-owner`, so their session rows were filed under the operator,
/// their messages ingested into `main__u-owner`, and the OWNER's curated
/// MEMORY.md / USER.md injected into their prompts.
///
/// `None` is still accepted and still means "the owner" — that is the
/// zero-config single-user path and it is byte-identical to before.
///
/// # Why the id is validated here
///
/// Same reason `pair --user` validates before minting: a binding to a dangling
/// id is worse than no binding. `sender_user` would return an id the users
/// table cannot resolve, and every predicate downstream compares against it —
/// so the sender gets an identity nobody can grant, revoke, or list. Fail at
/// the point of the mistake, where the operator is still looking.
pub async fn handle_approve(
    request: JsonRpcRequest,
    store: Arc<dyn PairingStore>,
    users: Arc<crate::gateway::security::store::SecurityStore>,
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

    // Optional. Absent → the store's `COALESCE` writes `OWNER_USER_ID`, which
    // is the single-user path and unchanged.
    let user_id = match params.get("user_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "'user_id' must be a non-empty string naming an existing user, or be omitted",
            );
        }
    };

    if let Some(ref uid) = user_id {
        match users.get_user(uid) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "no such user: {uid} — approving a sender onto a dangling id gives them \
                         an identity nobody can grant, revoke or list"
                    ),
                );
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to resolve user {uid}: {e}"),
                );
            }
        }
    }

    debug!(
        "Handling pairing.approve for {}:{} (user: {:?})",
        channel, code, user_id
    );

    match store.approve(channel, code, user_id.as_deref()).await {
        Ok(req) => {
            let response: PairingRequestResponse = req.into();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "approved": true,
                    "request": response,
                    // Echoed so the approving surface can SAY who this sender
                    // now is. An owner-bound approval and a member-bound one
                    // are indistinguishable on the wire otherwise, and they
                    // differ in which person's memory the sender's turns will
                    // read and write — the same reason `pair --user` prints its
                    // binding target.
                    "user_id": user_id,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to approve pairing: {e}"),
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
            format!("Failed to reject pairing: {e}"),
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
            format!("Failed to list approved senders: {e}"),
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
            format!("Failed to revoke approval: {e}"),
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

        let response = handle_approve(request, store.clone(), users()).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["approved"], true);
        assert!(
            result["user_id"].is_null(),
            "an unbound approval must say so — it is the owner-adopting path"
        );

        // Verify approved
        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());
        // ...and adopted by the owner, unchanged from before the parameter
        // existed.
        assert_eq!(
            store
                .sender_user("imessage", "+15551234567")
                .await
                .as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
    }

    /// A freshly migrated store already contains the owner; `create_user` mints
    /// anybody else.
    fn users() -> Arc<crate::gateway::security::store::SecurityStore> {
        Arc::new(crate::gateway::security::store::SecurityStore::in_memory().unwrap())
    }

    /// The producer this parameter exists to be: an approval that names a
    /// principal binds the sender to them, which is what
    /// `inbound_router::executor` reads to scope every inbound turn.
    #[tokio::test]
    async fn an_approval_can_name_the_principal_the_sender_speaks_as() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let users = users();
        let alice = "u-alice";
        users
            .create_user(
                alice,
                "Alice",
                crate::gateway::security::store::UserRole::Member,
            )
            .unwrap();

        let (code, _) = store
            .upsert("telegram", "tg-42", HashMap::new())
            .await
            .unwrap();

        let request = JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({
                "channel": "telegram",
                "code": code,
                "user_id": alice,
            })),
            Some(json!(1)),
        );
        let response = handle_approve(request, store.clone(), users).await;
        assert!(response.is_success(), "{:?}", response.error);
        assert_eq!(response.result.unwrap()["user_id"], json!(alice));

        assert_eq!(
            store.sender_user("telegram", "tg-42").await.as_deref(),
            Some(alice),
            "without this binding the sender's turns are filed under the operator"
        );
    }

    /// Fail where the operator is still looking. A sender bound to a dangling
    /// id has an identity nobody can grant, revoke or list, and the symptom
    /// surfaces much later as "the wrong person's memory".
    #[tokio::test]
    async fn approving_onto_a_dangling_user_id_is_refused() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let (code, _) = store
            .upsert("telegram", "tg-43", HashMap::new())
            .await
            .unwrap();

        let request = JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({
                "channel": "telegram",
                "code": code,
                "user_id": "u-nobody",
            })),
            Some(json!(1)),
        );
        let response = handle_approve(request, store.clone(), users()).await;
        assert!(!response.is_success());
        assert!(response.error.unwrap().message.contains("no such user"));

        // And the request is untouched — a refused approval must not half-apply.
        assert!(!store.is_approved("telegram", "tg-43").await.unwrap());
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

//! Pairing Handlers
//!
//! RPC handlers for pairing operations: list, approve, reject.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::debug;

use crate::gateway::pairing_store::{PairingRequest, PairingStore};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::store::UserStatus;
use aleph_protocol::channel_pairing::{ApprovedSenderList, ApprovedSenderRow};

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
            // Active only. A deactivated principal is walled everywhere else
            // — `connect` fails their devices closed to `("guest", None)`,
            // `users.update` revokes those devices and freezes their goals and
            // loops — so minting them a *fresh* channel identity here would
            // hand back, through a different door, exactly the authority the
            // deactivation withdrew. The sibling id-binding producer already
            // asks this question in this exact shape
            // (`handlers/projects.rs::require_known_user`); this one asked
            // only whether the row existed.
            Ok(Some(u)) if u.status == UserStatus::Active => {}
            Ok(Some(_)) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "user {uid} is deactivated — approving a sender onto a walled principal \
                         would restore on the channel axis the identity deactivation withdrew \
                         everywhere else"
                    ),
                );
            }
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
            // Authority-change audit (round-5 ⑦): approving a sender binds a
            // channel credential to a principal — one of the two independent
            // credential axes. The sender id comes from the approved request;
            // the code itself is credential material and is never logged.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!(
                        "channel.pairing.approve: {}:{} bound to {}",
                        channel,
                        req.sender_id,
                        user_id.as_deref().unwrap_or("(owner default)")
                    ),
                ));
            }
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
        Ok(senders) => {
            // Built FROM the shared contract type, not hand-assembled next to
            // it: `senders` is the key the Panel has always walked, while this
            // response only ever carried `approved` (a bare string array under
            // a different key), so that list rendered empty on every channel
            // from the day it shipped. Constructing the contract makes the
            // field names one fact instead of two, and makes over-sending a
            // compile impossibility rather than an untested hope.
            let rows = senders
                .into_iter()
                .map(|s| ApprovedSenderRow {
                    // Resolved through the same directory projection the room
                    // bubbles use, so an operator deciding which sender to
                    // revoke reads a name, not a `u-` id.
                    display_name: s
                        .user_id
                        .as_deref()
                        .and_then(crate::scope::directory::display_name),
                    sender_id: s.sender_id,
                    user_id: s.user_id,
                    approved_at: s.approved_at,
                })
                .collect();
            match serde_json::to_value(ApprovedSenderList::new(channel, rows)) {
                Ok(v) => JsonRpcResponse::success(request.id, v),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to encode approved senders: {e}"),
                ),
            }
        }
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
        Ok(()) => {
            // Authority-change audit (round-5 ⑦): withdrawing a channel
            // credential — SECURITY.md names this verb as the way to cut a
            // person off a channel, and it left no record of itself.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!("channel.pairing.revoke: {}:{}", channel, sender_id),
                ));
            }
            JsonRpcResponse::success(
                request.id,
                json!({
                    "revoked": true,
                    "channel": channel,
                    "sender_id": sender_id,
                }),
            )
        }
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

    /// A deactivated principal is walled on every other axis — their devices
    /// are revoked, their live connections closed, their goals and loops
    /// paused, and (since this round) their existing channel senders withdrawn.
    /// Binding a *fresh* sender onto them would restore, through a different
    /// door, exactly the authority deactivation withdrew.
    ///
    /// The sibling id-binding producer, `projects.member.add`, has always asked
    /// the full question; this one asked only whether the row existed.
    #[tokio::test]
    async fn an_approval_refuses_to_bind_a_sender_onto_a_deactivated_principal() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let users = users();
        users
            .create_user(
                "u-bob",
                "Bob",
                crate::gateway::security::store::UserRole::Member,
            )
            .unwrap();
        users
            .update_user("u-bob", None, None, Some(UserStatus::Deactivated))
            .unwrap();

        let (code, _) = store
            .upsert("telegram", "tg-bob", HashMap::new())
            .await
            .unwrap();
        let response = handle_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({"channel": "telegram", "code": code, "user_id": "u-bob"})),
                Some(json!(1)),
            ),
            store.clone(),
            users,
        )
        .await;

        assert!(
            response.is_error(),
            "a walled principal must not be bindable"
        );
        assert!(
            response.error.unwrap().message.contains("deactivated"),
            "the refusal must name why, not just say no"
        );
        assert!(
            store.list_approved("telegram").await.unwrap().is_empty(),
            "a refused approval must not have consumed the code or approved the sender"
        );
    }

    /// The response has to carry the principal each sender speaks as, because
    /// `channel.pairing.revoke` is keyed on `sender_id`: SECURITY.md names it
    /// as the way to cut someone off a channel, which is impossible if no
    /// surface says which sender is theirs.
    ///
    /// The `senders` key is also the one the Panel has always read while this
    /// handler emitted only `approved` — so this assertion is the one that
    /// would have failed on the day the two shapes diverged.
    #[tokio::test]
    async fn the_approved_list_names_the_principal_behind_each_sender() {
        let store: Arc<dyn PairingStore> = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let users = users();
        users
            .create_user(
                "u-alice",
                "Alice",
                crate::gateway::security::store::UserRole::Member,
            )
            .unwrap();
        let (code, _) = store
            .upsert("telegram", "tg-42", HashMap::new())
            .await
            .unwrap();
        handle_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({"channel": "telegram", "code": code, "user_id": "u-alice"})),
                Some(json!(1)),
            ),
            store.clone(),
            users.clone(),
        )
        .await;

        let response = handle_approved_list(
            JsonRpcRequest::new(
                "pairing.approved",
                Some(json!({"channel": "telegram"})),
                Some(json!(1)),
            ),
            store,
        )
        .await;
        let v = response.result.expect("success");
        // Decoded through the shared contract, not by walking keys: the whole
        // point is that both sides read one definition of the shape.
        let list: ApprovedSenderList =
            serde_json::from_value(v).expect("the response must be the contract");
        assert_eq!(list.senders.len(), 1);
        assert_eq!(list.senders[0].sender_id, "tg-42");
        assert_eq!(list.senders[0].user_id.as_deref(), Some("u-alice"));
        assert_eq!(
            list.approved,
            vec!["tg-42".to_string()],
            "the legacy projection must still describe the same rows"
        );
        assert_eq!(list.count, 1);
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

    /// The startup banner claims these methods answer. This asserts they do.
    ///
    /// # Why this reads source instead of booting a server
    ///
    /// What ships is a *runtime* check: `start/mod.rs` asks the live registry
    /// `has_method` for every advertised name and logs `tracing::error!` on a
    /// miss, under `--daemon` too. That is the right shape for an operator — a
    /// wiring bug reports itself at boot instead of at somebody's first call —
    /// and it is exactly the wrong shape for CI, which never boots the server.
    /// The only test that could observe it would have to start one, and the
    /// integration harness that starts one leaks a server process per run.
    ///
    /// So the guard compares the two *representations* directly. The defect it
    /// exists to stop (2026-08-09) was precisely a drift between them: `list`
    /// and `reject` sat in the banner — and in the webhook design doc's
    /// copy-pasteable `aleph-server gateway call` invocation — while never
    /// being passed to `register`, so the dispatcher answered
    /// `METHOD_NOT_FOUND` to everyone who followed the advertisement.
    ///
    /// # Why it is bidirectional
    ///
    /// Advertised-but-unregistered is the bug that happened. Registered-but-
    /// unadvertised is the same fact drifting the other way, and it is not
    /// harmless here: the pairing code is delivered to the *stranger*, so the
    /// banner is one of the few places an operator can discover that a method
    /// exists at all.
    ///
    /// # The third leg is a runtime call, not a third scrape
    ///
    /// Whether these names are admin-gated is asked of the real predicate
    /// rather than of `method_admin.rs`'s source, because a source scrape would
    /// only prove a literal is written down somewhere — not that the gate
    /// reaches it. `list` in particular is an enumeration face: it returns
    /// every pending sender id across every channel.
    #[test]
    fn the_pairing_banner_advertises_exactly_what_start_registers() {
        use std::collections::BTreeSet;

        /// Every `"channel.pairing.*"` string literal that follows `prefix`.
        ///
        /// Prose mentions do not match: the doc comments above the table talk
        /// about `list` and `reject` in backticks, never in wire form.
        fn names_after(haystack: &str, prefix: &str) -> BTreeSet<String> {
            let needle = format!("{prefix}\"channel.pairing.");
            let mut out = BTreeSet::new();
            let mut cursor = 0usize;
            while let Some(at) = haystack[cursor..].find(&needle) {
                let start = cursor + at + needle.len();
                let Some(end) = haystack[start..].find('"') else {
                    break;
                };
                out.insert(format!("channel.pairing.{}", &haystack[start..start + end]));
                cursor = start + end;
            }
            out
        }

        // The Windows checkout is CRLF (git autocrlf). Nothing below anchors on
        // a line ending, but normalising once up front is the shape that stays
        // correct if someone later adds a separator that does — see CLAUDE.md
        // §10, where a `\n`-anchored split silently matched nothing on Windows
        // and turned a guard into a no-op that still reported green.
        let src = include_str!("../../bin/aleph-server/commands/start/mod.rs").replace('\r', "");

        let table_at = src.find("const CHANNEL_PAIRING_METHODS").expect(
            "the banner table is gone or renamed; this guard is now protecting nothing. \
             Point it at the new name rather than deleting it.",
        );
        let table_end = table_at
            + src[table_at..]
                .find("];")
                .expect("CHANNEL_PAIRING_METHODS is unterminated");

        let advertised = names_after(&src[table_at..table_end], "");
        let registered = names_after(&src, ".register(");

        assert!(
            !advertised.is_empty(),
            "found the banner table but no method names in it — the extractor drifted from \
             the table's shape, which would make every assertion below vacuous"
        );
        assert_eq!(
            advertised, registered,
            "the startup banner and the `register` calls in start/mod.rs disagree. \
             Names only in the banner answer METHOD_NOT_FOUND; names only in the \
             registrations exist but cannot be discovered."
        );

        for method in &advertised {
            assert!(
                crate::gateway::method_admin::method_requires_admin(method),
                "`{method}` answers but is not admin-gated. `channel.pairing.*` decides who \
                 may DM the bot at all, and `.list` enumerates every pending sender id \
                 across every channel."
            );
        }
    }
}

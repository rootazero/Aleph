//! Cluster hub-side RPC: `cluster.enroll` (**pre**-register a node device record),
//! `cluster.deregister` (operator deregisters a node: evict online session + revoke
//! device record), `environments.list` (enumerate online + offline nodes).
//!
//! Under the LAN-trust model nodes do not hold tokens — connection identity is declared
//! by the connect parameter shape (`commands` + `tags`), and **registration itself is
//! also completed during `connect`** (`cluster::admit_node`). This file's
//! `cluster.enroll` is therefore NOT the mandatory entry point for a node to join;
//! it is just the operator's pre-reservation entry. It shares the same device-record
//! source of truth as self-service connect registration, so a same-name enroll cannot
//! mint a duplicate row.

use crate::sync_primitives::Arc;

use serde::Deserialize;
use tracing::warn;

use crate::cluster::{mint_node_device, normalize_node_key, ResolveError};
use crate::gateway::handlers::auth::AuthContext;
use crate::gateway::handlers::{parse_params, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Error code when no node matches (same as devices.*'s -32004 not-found).
const NODE_NOT_FOUND: i32 = -32004;

#[derive(Deserialize)]
struct EnrollParams {
    node_name: String,
}

/// **Pre-register** a role=node device record (operator clicks Enroll in the Panel).
///
/// LAN-trust: no token minted. A node does **not** need to go through here first —
/// it self-registers on `connect` (`cluster::admit_node`). The value of this RPC is
/// letting the operator reserve a slot ahead of time: the node appears as
/// `status:"offline"` in the fleet view before it even dials in, and when it later
/// connects with the **same name**, [`crate::cluster::admit_node`] merges it into
/// this record instead of minting a second UUID ghost row. The device-record write
/// shares the same source of truth as connect's self-service registration
/// ([`mint_node_device`]).
pub async fn handle_cluster_enroll(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: EnrollParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match mint_node_device(&ctx.security_store, &params.node_name) {
        Ok(node_id) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "node_id": node_id,
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

#[derive(Deserialize)]
struct DeregisterParams {
    /// Target node: name or id (multi-tier match, same addressing as `node_invoke`).
    node: String,
}

/// Offline-fallback addressing: resolve from the `security_store`'s registered node
/// devices (role=node, not revoked) by ① exact `device_id` ② unique normalized
/// `device_name`. Still deliberately rejects fuzzy prefix/substring (conservative —
/// the operator should address precisely for offline deregistration); but name
/// equality uses the same [`normalize_node_key`] as the online `NodeRegistry::match_id`,
/// eliminating the addressing drift of "online names are case-insensitive, offline
/// names are sensitive" — the same "GPU Box" can be deregistered as "gpu-box" whether
/// online or offline.
fn resolve_enrolled_node(ctx: &AuthContext, q: &str) -> Option<String> {
    let devices = ctx.security_store.list_devices().ok()?;
    let nodes: Vec<_> = devices.into_iter().filter(|d| d.role == "node").collect();
    if let Some(d) = nodes.iter().find(|d| d.device_id == q) {
        return Some(d.device_id.clone());
    }
    let nq = normalize_node_key(q);
    if nq.is_empty() {
        return None;
    }
    let by_name: Vec<_> = nodes
        .iter()
        .filter(|d| normalize_node_key(&d.device_name) == nq)
        .collect();
    match by_name.as_slice() {
        [d] => Some(d.device_id.clone()),
        _ => None,
    }
}

/// operator-gated: deregister a node. Two-phase takedown —
/// ① `forget` instantly evicts the online session (immediately gone from
///     `environments.list` and no longer addressable by `node_invoke`/`node_file`);
/// ② `revoke_device` revokes the device record (`revoked_at` is set).
///
/// **Deregistration is sticky**: ② is not just bookkeeping. The node's next `connect`
/// hits this `revoked_at` via `cluster::admit_node`, receives
/// `NodeAdmission::Deregistered`, and is rejected — the node receives
/// `{"node":{"status":"deregistered"}}` and exits on its own. Previously `connect`
/// never checked the device table, so a deregistered node would self-resurrect on
/// its next backoff-reconnect cycle — deregistration was a dead letter.
///
/// This call still does NOT forcibly close the node's current socket — the transport
/// layer will disconnect it at its next ping/idle-watchdog expiry, or the node will
/// be rejected on its own reconnect.
///
/// Addressing first tries online `NodeRegistry` multi-tier matching; if not online,
/// falls back to the `security_store`'s registered node devices (exact id / unique
/// normalized name) — offline nodes visible in environments.list must be equally
/// deregisterable (in which case `evicted:false`).
pub async fn handle_cluster_deregister(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: DeregisterParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let node_id = match ctx.node_registry.resolve_id(&params.node) {
        Ok(id) => id,
        // Not online → fall back to the device store so an enrolled-but-offline
        // node (now visible in environments.list) can still be deregistered.
        Err(ResolveError::NotFound) => match resolve_enrolled_node(&ctx, &params.node) {
            Some(id) => id,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    NODE_NOT_FOUND,
                    format!("no online or enrolled node matches '{}'", params.node),
                )
            }
        },
        Err(e @ (ResolveError::Ambiguous(_) | ResolveError::NodeNotFound { .. })) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("node '{}' {e}", params.node),
            )
        }
    };

    // ① Instantly evict online session.
    let evicted = ctx.node_registry.forget(&node_id);
    // ② Remove device record (the symmetric undo of enroll).
    let device_removed = ctx
        .security_store
        .revoke_device(&node_id)
        .unwrap_or_else(|e| {
            warn!(node_id = %node_id, error = %e, "failed to revoke node device on deregister");
            false
        });

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "node_id": node_id,
            "evicted": evicted,
            "device_removed": device_removed,
        }),
    )
}

/// read: enumerate cluster nodes (thin rendering contract, no credentials). Online
/// sessions come from `NodeRegistry`; then merged with `security_store` registered
/// (role=node, not revoked) but currently-offline devices, as `status:"offline"` +
/// `last_seen_at` (Unix seconds; `null` = enrolled but never connected). Mirrors
/// openclaw `nodes status` "paired-state + connected-state merged" view — offline
/// nodes no longer vanish from the UI. Gracefully degrades to online-only view on
/// store read failure (P7).
pub async fn handle_environments_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let mut envs = ctx.node_registry.list_environments();
    match ctx.security_store.list_devices() {
        Ok(devices) => {
            let online: std::collections::HashSet<String> =
                envs.iter().map(|e| e.id.clone()).collect();
            envs.extend(
                devices
                    .into_iter()
                    .filter(|d| d.role == "node" && !online.contains(&d.device_id))
                    .map(|d| crate::cluster::Environment {
                        id: d.device_id,
                        name: d.device_name,
                        status: "offline",
                        commands: Vec::new(),
                        tags: Vec::new(),
                        connected_at: 0,
                        // The device store stamps milliseconds; Environment speaks
                        // Unix seconds (same unit as connected_at).
                        last_seen_at: d.last_seen_at.map(|ms| ms / 1000),
                    }),
            );
        }
        Err(e) => warn!(
            error = %e,
            "environments.list: failed to read enrolled node devices; returning online-only view"
        ),
    }
    // Deterministic merged ordering: online first, then by name, then id. Online
    // nodes come from a HashMap and offline from the store (created_at DESC); a
    // stable order keeps the Panel fleet list from jittering on every refresh.
    envs.sort_by(|a, b| {
        let rank = |status: &str| u8::from(status != "online");
        rank(a.status)
            .cmp(&rank(b.status))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    JsonRpcResponse::success(request.id, serde_json::json!({ "environments": envs }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::tests::create_test_context;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn enroll_registers_node_device_without_token() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id(
            "cluster.enroll",
            Some(serde_json::json!({"node_name": "worker-1"})),
            serde_json::json!(1),
        );
        let resp = handle_cluster_enroll(req, ctx.clone()).await;
        assert!(resp.is_success(), "enroll should succeed: {:?}", resp.error);
        let result = resp.result.unwrap();
        let node_id = result["node_id"].as_str().unwrap().to_string();
        // LAN-trust: no token material leaves enroll.
        assert!(result.get("token").is_none());
        assert!(result.get("signature").is_none());
        // The device record lands in the store with role=node.
        let devices = ctx.security_store.list_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, node_id);
        assert_eq!(devices[0].role, "node");
    }

    #[tokio::test]
    async fn deregister_evicts_session_and_revokes_device() {
        let ctx = create_test_context();
        // Enroll mints a node device + token in security_store.
        let enroll = handle_cluster_enroll(
            JsonRpcRequest::with_id(
                "cluster.enroll",
                Some(serde_json::json!({"node_name": "worker-1"})),
                serde_json::json!(1),
            ),
            ctx.clone(),
        )
        .await;
        let node_id = enroll.result.unwrap()["node_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Register a live session for that id (mirrors the connect seam).
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let ch = crate::cluster::ReverseRpcChannel::new(tx);
        crate::cluster::maybe_register_node(
            &ctx.node_registry,
            Some("node"),
            &node_id,
            "conn-1",
            Some(
                &serde_json::json!({"device_name": "worker-1", "commands": [{"name":"bash","schema":{}}]}),
            ),
            &ch,
        );
        assert_eq!(ctx.node_registry.list_environments().len(), 1);

        // Deregister by NAME (exercises multi-tier resolve_id).
        let resp = handle_cluster_deregister(
            JsonRpcRequest::with_id(
                "cluster.deregister",
                Some(serde_json::json!({"node": "worker-1"})),
                serde_json::json!(2),
            ),
            ctx.clone(),
        )
        .await;
        assert!(resp.is_success(), "deregister failed: {:?}", resp.error);
        let r = resp.result.unwrap();
        assert_eq!(r["node_id"], node_id);
        assert_eq!(r["evicted"], true);
        assert_eq!(r["device_removed"], true);
        // The node is gone from the live registry → no longer node_invoke-reachable.
        assert!(ctx.node_registry.list_environments().is_empty());
    }

    #[tokio::test]
    async fn deregister_unknown_node_is_not_found() {
        let ctx = create_test_context();
        let resp = handle_cluster_deregister(
            JsonRpcRequest::with_id(
                "cluster.deregister",
                Some(serde_json::json!({"node": "ghost"})),
                serde_json::json!(1),
            ),
            ctx,
        )
        .await;
        assert!(!resp.is_success());
        assert_eq!(resp.error.unwrap().code, -32004);
    }

    #[tokio::test]
    async fn environments_list_projects_registered_nodes() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id("environments.list", None, serde_json::json!(1));
        let resp = handle_environments_list(req, ctx.clone()).await;
        assert!(resp.is_success());
        assert_eq!(
            resp.result.unwrap()["environments"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let ch = crate::cluster::ReverseRpcChannel::new(tx);
        crate::cluster::maybe_register_node(
            &ctx.node_registry,
            Some("node"),
            "node-a",
            "conn-1",
            Some(
                &serde_json::json!({"device_name": "worker-1", "commands": [{"name":"bash","schema":{}}]}),
            ),
            &ch,
        );
        let req = JsonRpcRequest::with_id("environments.list", None, serde_json::json!(2));
        let resp = handle_environments_list(req, ctx.clone()).await;
        let envs = resp.result.unwrap();
        let arr = envs["environments"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "node-a");
        assert_eq!(arr[0]["status"], "online");
        assert_eq!(arr[0]["commands"][0]["name"], "bash");
        assert!(arr[0].get("token").is_none());
    }

    #[tokio::test]
    async fn environments_list_merges_enrolled_offline_nodes() {
        let ctx = create_test_context();
        // Enroll mints a node device (role=node) with no live session yet.
        let enroll = handle_cluster_enroll(
            JsonRpcRequest::with_id(
                "cluster.enroll",
                Some(serde_json::json!({"node_name": "worker-off"})),
                serde_json::json!(1),
            ),
            ctx.clone(),
        )
        .await;
        let node_id = enroll.result.unwrap()["node_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Offline: the enrolled device surfaces with status "offline" and a
        // null last_seen_at (it never connected).
        let resp = handle_environments_list(
            JsonRpcRequest::with_id("environments.list", None, serde_json::json!(2)),
            ctx.clone(),
        )
        .await;
        let v = resp.result.unwrap();
        let arr = v["environments"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "{arr:?}");
        assert_eq!(arr[0]["id"], node_id);
        assert_eq!(arr[0]["status"], "offline");
        assert!(arr[0]["last_seen_at"].is_null());
        assert!(arr[0].get("token").is_none());

        // Once a live session registers, the same node shows online exactly once.
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let ch = crate::cluster::ReverseRpcChannel::new(tx);
        crate::cluster::maybe_register_node(
            &ctx.node_registry,
            Some("node"),
            &node_id,
            "conn-1",
            Some(&serde_json::json!({"device_name": "worker-off", "commands": []})),
            &ch,
        );
        let resp = handle_environments_list(
            JsonRpcRequest::with_id("environments.list", None, serde_json::json!(3)),
            ctx.clone(),
        )
        .await;
        let v = resp.result.unwrap();
        let arr = v["environments"].as_array().unwrap();
        assert_eq!(
            arr.len(),
            1,
            "online session must not duplicate the enrolled device: {arr:?}"
        );
        assert_eq!(arr[0]["status"], "online");

        // Deregister revokes the device → it leaves the fleet view entirely.
        let _ = handle_cluster_deregister(
            JsonRpcRequest::with_id(
                "cluster.deregister",
                Some(serde_json::json!({"node": "worker-off"})),
                serde_json::json!(4),
            ),
            ctx.clone(),
        )
        .await;
        let resp = handle_environments_list(
            JsonRpcRequest::with_id("environments.list", None, serde_json::json!(5)),
            ctx,
        )
        .await;
        assert!(resp.result.unwrap()["environments"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn environments_list_orders_online_before_offline_and_by_name() {
        let ctx = create_test_context();
        // One enrolled-but-offline node ("a-off" — would sort first by name).
        handle_cluster_enroll(
            JsonRpcRequest::with_id(
                "cluster.enroll",
                Some(serde_json::json!({"node_name": "a-off"})),
                serde_json::json!(1),
            ),
            ctx.clone(),
        )
        .await;
        // Two live sessions registered out of name order.
        for (id, name, conn) in [("id-z", "z-online", "c-z"), ("id-m", "m-online", "c-m")] {
            let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
            let ch = crate::cluster::ReverseRpcChannel::new(tx);
            crate::cluster::maybe_register_node(
                &ctx.node_registry,
                Some("node"),
                id,
                conn,
                Some(&serde_json::json!({"device_name": name, "commands": []})),
                &ch,
            );
        }
        let resp = handle_environments_list(
            JsonRpcRequest::with_id("environments.list", None, serde_json::json!(2)),
            ctx,
        )
        .await;
        let v = resp.result.unwrap();
        let arr = v["environments"].as_array().unwrap();
        let order: Vec<&str> = arr.iter().map(|e| e["name"].as_str().unwrap()).collect();
        // Online first (by name: m, z), then the offline node despite its
        // name sorting first alphabetically.
        assert_eq!(order, vec!["m-online", "z-online", "a-off"], "{arr:?}");
        assert_eq!(arr[0]["status"], "online");
        assert_eq!(arr[2]["status"], "offline");
    }

    #[tokio::test]
    async fn deregister_reaches_enrolled_offline_node_by_name() {
        let ctx = create_test_context();
        let enroll = handle_cluster_enroll(
            JsonRpcRequest::with_id(
                "cluster.enroll",
                Some(serde_json::json!({"node_name": "worker-cold"})),
                serde_json::json!(1),
            ),
            ctx.clone(),
        )
        .await;
        let node_id = enroll.result.unwrap()["node_id"]
            .as_str()
            .unwrap()
            .to_string();

        // No live session: resolve falls back to the enrolled device store.
        let resp = handle_cluster_deregister(
            JsonRpcRequest::with_id(
                "cluster.deregister",
                Some(serde_json::json!({"node": "worker-cold"})),
                serde_json::json!(2),
            ),
            ctx.clone(),
        )
        .await;
        assert!(resp.is_success(), "offline deregister: {:?}", resp.error);
        let r = resp.result.unwrap();
        assert_eq!(r["node_id"], node_id);
        assert_eq!(r["evicted"], false, "nothing online to evict");
        assert_eq!(r["device_removed"], true);

        // And it is gone from the merged fleet view.
        let resp = handle_environments_list(
            JsonRpcRequest::with_id("environments.list", None, serde_json::json!(3)),
            ctx,
        )
        .await;
        assert!(resp.result.unwrap()["environments"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn deregister_offline_node_by_normalized_name() {
        let ctx = create_test_context();
        // Enrolled with a spaced, mixed-case name.
        let enroll = handle_cluster_enroll(
            JsonRpcRequest::with_id(
                "cluster.enroll",
                Some(serde_json::json!({"node_name": "GPU Box"})),
                serde_json::json!(1),
            ),
            ctx.clone(),
        )
        .await;
        let node_id = enroll.result.unwrap()["node_id"]
            .as_str()
            .unwrap()
            .to_string();

        // Offline fallback resolves it by a dash-spelled lowercase variant —
        // same normalize_node_key the online path uses (no online/offline drift).
        let resp = handle_cluster_deregister(
            JsonRpcRequest::with_id(
                "cluster.deregister",
                Some(serde_json::json!({"node": "gpu-box"})),
                serde_json::json!(2),
            ),
            ctx,
        )
        .await;
        assert!(
            resp.is_success(),
            "normalized offline deregister: {:?}",
            resp.error
        );
        let r = resp.result.unwrap();
        assert_eq!(r["node_id"], node_id);
        assert_eq!(r["evicted"], false);
        assert_eq!(r["device_removed"], true);
    }
}

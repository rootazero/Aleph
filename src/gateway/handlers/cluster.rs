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

use crate::cluster::{deregister_node, enroll_node_device, DeregisterError};
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
/// ([`enroll_node_device`]).
///
/// **Idempotent**: re-enrolling an existing name returns that node's id
/// unchanged (`reused: true`) rather than minting a second row. It previously
/// minted unconditionally, so a double-clicked "+ Enroll" left two same-name
/// rows — which then made the node's own first boot mint a *third* (the
/// by-name merge refuses to guess) and made `cluster.deregister`'s offline
/// fallback, which needs a unique name, unable to remove any of them. See
/// [`enroll_node_device`].
pub async fn handle_cluster_enroll(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: EnrollParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match enroll_node_device(&ctx.security_store, &params.node_name) {
        Ok((node_id, minted)) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "node_id": node_id,
                // Backward-compatible superset: older Panels ignore it.
                "reused": !minted,
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

/// operator-gated: deregister a node. Pure I/O over
/// [`crate::cluster::deregister_node`] (R4) — the two-phase takedown, the
/// online-then-offline addressing, and the stickiness guarantee all live in the
/// cluster module, shared verbatim with the `node_manage` tool so the Panel and
/// the model cannot drift apart on what "deregister" means.
pub async fn handle_cluster_deregister(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: DeregisterParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match deregister_node(&ctx.node_registry, &ctx.security_store, &params.node) {
        Ok(outcome) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "node_id": outcome.node_id,
                "evicted": outcome.evicted,
                "device_removed": outcome.device_removed,
            }),
        ),
        Err(DeregisterError::NotFound) => JsonRpcResponse::error(
            request.id,
            NODE_NOT_FOUND,
            format!("no online or enrolled node matches '{}'", params.node),
        ),
        Err(DeregisterError::Ambiguous(detail)) => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("node '{}' {detail}", params.node),
        ),
    }
}

/// operator-gated read: enumerate cluster nodes (thin rendering contract, no
/// credentials). The gate is the `environments.` prefix in
/// [`crate::gateway::method_admin`], not an in-handler check — this response is
/// the same for every operator, so there is nothing here to scope per caller;
/// it is simply not a member surface. Until 2026-08-07 the family was absent
/// from `ADMIN_PREFIXES` (its `cluster.` siblings were gated, this read was
/// not) and the only thing withholding fleet topology from a member was the
/// Panel's own `is_operator()` check.
///
/// Online
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
                        // Deliberately never remembered for offline nodes: a
                        // stored version is a claim about a machine we cannot
                        // currently see, and it would go stale exactly when the
                        // operator upgrades the fleet.
                        version: None,
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
    async fn enroll_twice_returns_the_same_node_id() {
        let ctx = create_test_context();
        let enroll = |name: &'static str, id: i32| {
            let ctx = ctx.clone();
            async move {
                handle_cluster_enroll(
                    JsonRpcRequest::with_id(
                        "cluster.enroll",
                        Some(serde_json::json!({ "node_name": name })),
                        serde_json::json!(id),
                    ),
                    ctx,
                )
                .await
                .result
                .unwrap()
            }
        };
        let first = enroll("GPU Box", 1).await;
        // A double-clicked "+ Enroll" (or a retried RPC) must not mint a second
        // row: the duplicate would make the node's own by-name merge ambiguous
        // and strand BOTH rows in the fleet view, un-deregisterable by name.
        let second = enroll("gpu-box", 2).await;
        assert_eq!(first["node_id"], second["node_id"]);
        assert_eq!(first["reused"], false);
        assert_eq!(second["reused"], true);
        assert_eq!(ctx.security_store.list_devices().unwrap().len(), 1);
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

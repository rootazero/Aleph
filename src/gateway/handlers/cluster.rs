//! 集群中心侧 RPC：`cluster.enroll`（**预**登记 node 设备记录）、
//! `cluster.deregister`（operator 注销节点：驱逐在线会话 + 吊销设备记录）、
//! `environments.list`（枚举在线 + 离线节点）。
//!
//! LAN-trust 模型下节点不持 token——连接身份由 connect 参数形状（`commands` +
//! `tags`）声明，**登记本身也在 `connect` 里完成**（`cluster::admit_node`）。本文件
//! 的 `cluster.enroll` 因此不是节点入网的必经之路，只是 operator 的预占位入口；
//! 它与 connect 自助登记共用同一设备记录真源，故同名不会铸出两行。

use crate::sync_primitives::Arc;

use serde::Deserialize;
use tracing::warn;

use crate::cluster::{mint_node_device, normalize_node_key, ResolveError};
use crate::gateway::handlers::auth::AuthContext;
use crate::gateway::handlers::{parse_params, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// 没有任何匹配节点时的错误码（同 devices.* 的 -32004 not-found）。
const NODE_NOT_FOUND: i32 = -32004;

#[derive(Deserialize)]
struct EnrollParams {
    node_name: String,
}

/// **预登记**一个 role=node 的设备记录（operator 在 Panel 点 Enroll）。
///
/// LAN-trust：不铸 token。节点**并不需要**先走这里——它 `connect` 时会自助登记
/// （`cluster::admit_node`）。本 RPC 的价值是让 operator 先占位：节点还没拨入前
/// 就以 `status:"offline"` 出现在舰队视图里，且节点随后按**同名**拨入时会被
/// [`crate::cluster::admit_node`] 归并到这条记录，不会再各铸一个 UUID 留下幽灵行。
/// 设备记录的写入与 `connect` 自助登记共用同一真源 [`mint_node_device`]。
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
    /// 目标节点：name 或 id（多级匹配，同 `node_invoke` 寻址）。
    node: String,
}

/// 离线回退寻址：在 `security_store` 的已登记节点设备（role=node、未吊销）里按
/// ① 精确 `device_id` ② 唯一归一化 `device_name` 解析。仍刻意不做模糊前缀/子串
/// （保守——离线注销 operator 应精确指名）；但名字等值经 [`normalize_node_key`]
/// 与在线 `NodeRegistry::match_id` 同源，消除「在线名大小写不敏感、离线敏感」的
/// 寻址漂移——同一个 "GPU Box" 在线/离线都能用 "gpu-box" 注销。
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

/// operator-gated：注销一个节点。两步下线——
/// ① `forget` 即时驱逐在线会话（立刻从 `environments.list` 消失，且不再
///    被 `node_invoke`/`node_file` 寻址到）；
/// ② `revoke_device` 吊销设备记录（`revoked_at` 置位）。
///
/// **注销是粘的**：②不只是记账。节点下一次 `connect` 会经 `cluster::admit_node`
/// 撞上这条 `revoked_at`，被判 `NodeAdmission::Deregistered` 而拒绝登记，节点收到
/// `{"node":{"status":"deregistered"}}` 后自行退出。此前 connect 从不查设备表，
/// 被注销的节点在下一轮退避重连里就自己复活了——注销形同虚设。
///
/// 本调用仍不强制 close 节点当前 socket——它会在下一次 ping/idle-watchdog 到期时
/// 由传输层断开，或在节点自己重连时被拒。
///
/// 寻址先走在线 `NodeRegistry` 多级匹配；不在线则回退 `security_store` 的已登记
/// 节点设备（精确 id / 唯一归一化 name）——environments.list 里可见的离线节点
/// 必须同样可注销（此时 `evicted:false`）。
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

    // ① 即时驱逐在线会话。
    let evicted = ctx.node_registry.forget(&node_id);
    // ② 抹除设备记录（enroll 的对称撤除）。
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

/// read：枚举集群节点（薄渲染契约，不含凭证）。在线会话来自 `NodeRegistry`；
/// 再合并 `security_store` 里已登记（role=node、未吊销）但当前不在线的设备，
/// `status:"offline"` + `last_seen_at`（Unix 秒；`null` = 登记后从未连入）。
/// 镜像 openclaw `nodes status` 的"配对态 + 连接态合并"视图——离线节点不再
/// 凭空消失。store 读失败时优雅降级为在线视图（P7）。
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

//! 集群中心侧 RPC：`cluster.enroll`（operator 铸 node token）+
//! `environments.list`（read，枚举在线节点）。形态为 gateway RPC 而非 builtin
//! 工具——凭证操作的既有模式（同 devices.*/pairing.*）。LLM-callable 工具面随
//! 0c 的 node_invoke 一起落地。

use std::sync::Arc;

use serde::Deserialize;

use crate::gateway::handlers::auth::AuthContext;
use crate::gateway::handlers::{parse_params, INTERNAL_ERROR};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::security::device::DeviceRole;
use crate::gateway::security::store::DeviceUpsertData;

#[derive(Deserialize)]
struct EnrollParams {
    node_name: String,
}

/// operator-gated：铸一个 DeviceRole::Node 设备 + token，返回给操作员转交节点机。
pub async fn handle_cluster_enroll(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: EnrollParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let device_id = uuid::Uuid::new_v4().to_string();
    let fingerprint: String = device_id.chars().take(16).collect();

    // Placeholder identity material: server-provisioned nodes have no hardware key (mirrors connect.rs).
    if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
        device_id: &device_id,
        device_name: &params.node_name,
        device_type: None,
        public_key: &[0u8; 32],
        fingerprint: &fingerprint,
        role: DeviceRole::Node.as_str(),
        scopes: &["node".to_string()],
    }) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to register node device: {e}"),
        );
    }

    let signed =
        match ctx
            .token_manager
            .issue_token(&device_id, DeviceRole::Node, vec!["node".to_string()])
        {
            Ok(t) => t,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("failed to issue node token: {e}"),
                )
            }
        };

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "node_id": device_id,
            "token": signed.token,
            "signature": signed.signature,
        }),
    )
}

/// read：枚举当前在线节点（薄渲染契约，不含凭证）。
pub async fn handle_environments_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let envs = ctx.node_registry.list_environments();
    JsonRpcResponse::success(request.id, serde_json::json!({ "environments": envs }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::tests::create_test_context;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn enroll_mints_node_token_that_validates_as_node() {
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
        let token = result["token"].as_str().unwrap().to_string();
        let signature = result["signature"].as_str().unwrap().to_string();
        assert!(!token.is_empty());
        let v = ctx
            .token_manager
            .validate_token(&token, &signature)
            .unwrap();
        assert_eq!(v.device_id, node_id);
        assert_eq!(v.role, crate::gateway::security::DeviceRole::Node);
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
}

//! 集群 API client。`environments.list`(已认证读)、`cluster.enroll`
//! (operator-only 铸 token)、`cluster.deregister`(operator-only 注销节点)。
//! 节点命令下发由 LLM 经对话驱动(`node_invoke`/`node_file` 工具,R8),
//! 不在 Panel 暴露手动入口。

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub name: String,
    #[serde(default)]
    pub schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
    #[serde(default)]
    pub connected_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResult {
    pub node_id: String,
    pub token: String,
    #[serde(default)]
    pub signature: String,
}

pub struct ClusterApi;

impl ClusterApi {
    /// 列出已连接节点(每 node = 一个 environment)。RPC `environments.list`。
    pub async fn list_environments(state: &DashboardState) -> Result<Vec<Environment>, String> {
        let result = state.rpc_call("environments.list", Value::Null).await?;
        result
            .get("environments")
            .ok_or_else(|| "Invalid response: missing environments".to_string())
            .and_then(|envs| {
                serde_json::from_value(envs.clone())
                    .map_err(|e| format!("Failed to parse environments: {e}"))
            })
    }

    /// 铸造 node 登记 token。RPC `cluster.enroll`(operator-only)。
    pub async fn enroll_node(
        state: &DashboardState,
        node_name: String,
    ) -> Result<EnrollResult, String> {
        let params = serde_json::json!({ "node_name": node_name });
        let result = state.rpc_call("cluster.enroll", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse enroll result: {e}"))
    }

    /// 注销一个节点(name 或 id):驱逐在线会话 + 撤 token/设备。
    /// RPC `cluster.deregister`(operator-only)。
    pub async fn deregister_node(state: &DashboardState, node: String) -> Result<(), String> {
        let params = serde_json::json!({ "node": node });
        state
            .rpc_call("cluster.deregister", params)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_list() {
        let payload = serde_json::json!({
            "environments": [
                {"id":"n1","name":"node-a","status":"online",
                 "commands":[{"name":"bash","schema":{}}],"connected_at":1234}
            ]
        });
        let envs: Vec<Environment> =
            serde_json::from_value(payload.get("environments").unwrap().clone()).unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].name, "node-a");
        assert_eq!(envs[0].commands[0].name, "bash");
    }

    #[test]
    fn parses_enroll_result() {
        let payload = serde_json::json!({"node_id":"n1","token":"tok","signature":"sig"});
        let r: EnrollResult = serde_json::from_value(payload).unwrap();
        assert_eq!(r.token, "tok");
        assert_eq!(r.node_id, "n1");
    }
}

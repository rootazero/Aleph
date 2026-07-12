//! 集群 API client。`environments.list`(在线 + 离线合并视图)、`cluster.enroll`
//! (operator-only 预登记一个节点名)、`cluster.deregister`(operator-only 注销节点)。
//! 节点命令下发由 LLM 经对话驱动(`node_invoke`/`node_file` 工具,R8),
//! 不在 Panel 暴露手动入口。
//!
//! LAN-trust:enroll **不铸 token**。节点凭 `connect` 帧的参数形状声明身份,
//! 登记本身也在 connect 里完成——所以 `cluster.enroll` 只是 operator 的**预占位**
//! (让节点在真正拨入前就以 offline 出现在舰队里,并让同名的节点归并到这一行)。

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
    /// `"online"` | `"offline"`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
    /// Operator-assigned labels. These are what `node_invoke_many` selects on,
    /// so the fleet list must show them — the backend has always sent them, the
    /// Panel just dropped them on the floor.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub connected_at: i64,
    /// Unix seconds. Only meaningful for an offline node; `None` + offline means
    /// "enrolled but has never connected".
    #[serde(default)]
    pub last_seen_at: Option<i64>,
}

impl Environment {
    pub fn is_online(&self) -> bool {
        self.status == "online"
    }
}

/// `cluster.enroll` 的回包。**没有 token** —— LAN-trust 下 enroll 只返 `node_id`。
/// 此前这里是 `token: String`(必填)+ `signature`,而服务端早已不再返回它们,
/// 于是 `serde_json::from_value` 恒以 "missing field `token`" 失败:Panel 的
/// 「+ Enroll」按钮**从来就没成功过**。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResult {
    pub node_id: String,
}

pub struct ClusterApi;

impl ClusterApi {
    /// 列出集群节点(在线会话 + 已登记但离线的设备)。RPC `environments.list`。
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

    /// 预登记一个节点名,拿回它的 `node_id`。RPC `cluster.enroll`(operator-only)。
    pub async fn enroll_node(
        state: &DashboardState,
        node_name: String,
    ) -> Result<EnrollResult, String> {
        let params = serde_json::json!({ "node_name": node_name });
        let result = state.rpc_call("cluster.enroll", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse enroll result: {e}"))
    }

    /// 注销一个节点(name 或 id):驱逐在线会话 + 吊销设备记录。注销是**粘的**——
    /// 该节点下次 connect 会被中心拒绝,不会自己重连回来。
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
    fn parses_environment_list_with_tags_and_last_seen() {
        let payload = serde_json::json!({
            "environments": [
                {"id":"n1","name":"node-a","status":"online",
                 "commands":[{"name":"bash","schema":{}}],
                 "tags":["gpu","region=us"],"connected_at":1234,"last_seen_at":null},
                {"id":"n2","name":"node-b","status":"offline",
                 "commands":[],"tags":[],"connected_at":0,"last_seen_at":1700000000}
            ]
        });
        let envs: Vec<Environment> =
            serde_json::from_value(payload.get("environments").unwrap().clone()).unwrap();
        assert_eq!(envs.len(), 2);
        assert!(envs[0].is_online());
        assert_eq!(envs[0].commands[0].name, "bash");
        assert_eq!(envs[0].tags, vec!["gpu", "region=us"]);
        // The offline half of the merged view.
        assert!(!envs[1].is_online());
        assert_eq!(envs[1].last_seen_at, Some(1_700_000_000));
    }

    #[test]
    fn parses_enroll_result_without_a_token() {
        // The server returns `{node_id}` only. Requiring a `token` field here is
        // exactly what broke the Enroll button.
        let payload = serde_json::json!({"node_id": "n1"});
        let r: EnrollResult = serde_json::from_value(payload).unwrap();
        assert_eq!(r.node_id, "n1");
    }
}

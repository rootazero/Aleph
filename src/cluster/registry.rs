//! 集群节点登记表（中心侧）。
//!
//! 追踪「哪些已连 WS 连接是已登记节点」，并把它们投影成只读「环境」视图供
//! `environments.list` 渲染。消费 Phase 0a 的 [`ReverseRpcChannel`]——每个
//! `NodeSession` 持一份 channel clone，0c 的 `node_invoke` 经它向节点下发。
//!
//! 红线：纯数据结构，无 LLM 推理（R7），不进 `src/harness/`（R10）。

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cluster::ReverseRpcChannel;

/// 节点声明的一个 command（名字 + 自描述 schema）。0b 不解析 schema，原样透传。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommandDescriptor {
    pub name: String,
    pub schema: Value,
}

/// 一个已连入的节点会话（中心侧视图）。
pub struct NodeSession {
    /// = device_id，直接当环境 id。
    pub node_id: String,
    /// 对应 0a reverse_rpc 表的键，断线清理对账用。
    pub conn_id: String,
    /// 人类可读名（来自 connect 帧）。
    pub device_name: String,
    /// 0a 通道的 clone —— 0c 的 node_invoke 经它下发。
    pub channel: ReverseRpcChannel,
    /// 节点自声明的 command 目录，0b 只存只显。
    pub declared_commands: Vec<CommandDescriptor>,
    /// 登记时刻（Unix 秒）。
    pub connected_at: i64,
}

/// `environments.list` 的对外序列化视图（薄渲染契约，R4）。绝不含凭证。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub status: &'static str,
    pub commands: Vec<CommandDescriptor>,
    pub connected_at: i64,
}

#[derive(Default)]
struct RegistryInner {
    /// node_id → session（权威）。
    nodes_by_id: HashMap<String, NodeSession>,
    /// conn_id → node_id（断线反查）。
    nodes_by_conn: HashMap<String, String>,
}

/// 节点注册表。线程安全；锁中毒按 P7（`unwrap_or_else(|e| e.into_inner())`）。
#[derive(Default)]
pub struct NodeRegistry {
    inner: RwLock<RegistryInner>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个节点会话。同 node_id 重连 → 覆盖旧会话，并清掉旧 conn 映射。
    pub fn register(&self, session: NodeSession) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let node_id = session.node_id.clone();
        let conn_id = session.conn_id.clone();
        // Drop any stale conn→node mapping the previous session for this node_id held,
        // so an old connection's later cleanup can't evict the new session.
        if let Some(prev) = inner.nodes_by_id.get(&node_id) {
            let prev_conn = prev.conn_id.clone();
            inner.nodes_by_conn.remove(&prev_conn);
        }
        inner.nodes_by_conn.insert(conn_id, node_id.clone());
        inner.nodes_by_id.insert(node_id, session);
    }

    /// 注销一个连接的节点会话。仅当该 node_id 当前会话确属此 conn_id 时才移除
    /// （重连安全：旧连接 cleanup 不会误删新会话）。返回是否移除了会话。
    pub fn deregister(&self, conn_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let Some(node_id) = inner.nodes_by_conn.remove(conn_id) else {
            return false;
        };
        match inner.nodes_by_id.get(&node_id) {
            Some(s) if s.conn_id == conn_id => {
                inner.nodes_by_id.remove(&node_id);
                true
            }
            _ => false,
        }
    }

    /// 在线节点的只读投影快照。
    pub fn list_environments(&self) -> Vec<Environment> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .nodes_by_id
            .values()
            .map(|s| Environment {
                id: s.node_id.clone(),
                name: s.device_name.clone(),
                status: "online",
                commands: s.declared_commands.clone(),
                connected_at: s.connected_at,
            })
            .collect()
    }

    /// 取某节点的反向 RPC 通道 clone（0c 的 node_invoke 用；0b 建好接口不调）。
    pub fn get(&self, node_id: &str) -> Option<ReverseRpcChannel> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.nodes_by_id.get(node_id).map(|s| s.channel.clone())
    }
}

/// connect→register 接缝：仅当 `role == Some("node")` 时把这条连接登记进
/// NodeRegistry。`params` 是 connect 帧的 params（取 device_name + commands）。
/// 返回是否登记。抽成纯函数以便单测，且让 `handler.rs` 保持薄。
pub fn maybe_register_node(
    registry: &NodeRegistry,
    role: Option<&str>,
    device_id: &str,
    conn_id: &str,
    params: Option<&Value>,
    channel: &ReverseRpcChannel,
) -> bool {
    if role != Some("node") {
        return false;
    }
    let device_name = params
        .and_then(|p| p.get("device_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let declared_commands = params
        .and_then(|p| p.get("commands"))
        .and_then(|v| serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()).ok())
        .unwrap_or_default();
    registry.register(NodeSession {
        node_id: device_id.to_string(),
        conn_id: conn_id.to_string(),
        device_name,
        channel: channel.clone(),
        declared_commands,
        connected_at: now_unix(),
    });
    true
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn test_channel() -> ReverseRpcChannel {
        let (tx, _rx) = mpsc::channel::<String>(8);
        ReverseRpcChannel::new(tx)
    }

    fn session(node_id: &str, conn_id: &str) -> NodeSession {
        NodeSession {
            node_id: node_id.to_string(),
            conn_id: conn_id.to_string(),
            device_name: format!("dev-{node_id}"),
            channel: test_channel(),
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: json!({"type": "object"}),
            }],
            connected_at: 1,
        }
    }

    #[test]
    fn register_then_list_projects_environment() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "node-a");
        assert_eq!(envs[0].name, "dev-node-a");
        assert_eq!(envs[0].status, "online");
        assert_eq!(envs[0].commands.len(), 1);
        assert_eq!(envs[0].commands[0].name, "bash");
    }

    #[test]
    fn deregister_removes_from_both_maps() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.deregister("conn-1"));
        assert!(reg.list_environments().is_empty());
        assert!(reg.get("node-a").is_none());
        assert!(!reg.deregister("conn-x"));
    }

    #[test]
    fn reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        reg.register(session("node-a", "conn-2"));
        assert_eq!(reg.list_environments().len(), 1);
        assert!(!reg.deregister("conn-1"));
        assert_eq!(reg.list_environments().len(), 1);
        assert!(reg.deregister("conn-2"));
        assert!(reg.list_environments().is_empty());
    }

    #[test]
    fn get_returns_channel_for_known_node() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.get("node-a").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn maybe_register_node_registers_only_for_node_role() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({"device_name": "worker", "commands": [{"name": "bash", "schema": {}}]});
        assert!(!maybe_register_node(&reg, Some("operator"), "d1", "c1", Some(&params), &ch));
        assert!(reg.list_environments().is_empty());
        assert!(!maybe_register_node(&reg, None, "d0", "c0", Some(&params), &ch));
        assert!(reg.list_environments().is_empty());
        assert!(maybe_register_node(&reg, Some("node"), "d2", "c2", Some(&params), &ch));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "d2");
        assert_eq!(envs[0].commands[0].name, "bash");
    }
}

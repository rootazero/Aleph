//! `node_invoke`: center-side LLM tool that dispatches commands to a connected
//! node (via 0a reverse RPC).
//!
//! Addressing is by name or id; fail-fast validation of declared commands before
//! dispatch (the node side remains authoritative).
//! Redline: pure I/O translation (R4), no reasoning (R7); command selection by LLM.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cluster::{NodeRegistry, ResolveError};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeInvokeArgs {
    /// Target node: its name (e.g. "worker-1") or id. Use the `node_list` tool
    /// to see online nodes and the commands each declares.
    pub node: String,
    /// Command to run on the node (e.g. "bash"). Must be one the node declares.
    pub command: String,
    /// JSON arguments for the command, passed through to the node verbatim
    /// (for "bash", e.g. {"cmd": "ls -la"}).
    #[serde(default)]
    pub args: Value,
    /// Reverse-RPC timeout in ms (default 120000). Must exceed the node-side
    /// command's own runtime or the channel times out while it still runs.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub struct NodeInvokeTool {
    node_registry: Arc<NodeRegistry>,
}

impl NodeInvokeTool {
    pub const fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }
}

#[async_trait]
impl AlephTool for NodeInvokeTool {
    const NAME: &'static str = "node_invoke";
    const DESCRIPTION: &'static str = r#"Run a command on a connected cluster node (a remote execution arm).

Address the node by its name or id (call `node_list` for online nodes and
the commands each declares). `command` must be one the node permits (e.g. "bash");
`args` is that command's JSON payload, passed through verbatim — for bash:
{"node": "worker-1", "command": "bash", "args": {"cmd": "uname -a"}}.

The node runs it in ITS OWN sandboxed workspace and returns the result. Set
`timeout_ms` (default 120000) above the expected runtime for long commands. If the
node is offline or the command isn't permitted, you get a clear error."#;

    type Args = NodeInvokeArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let (channel, declared) = match self.node_registry.resolve(&args.node) {
            Ok(v) => v,
            Err(ResolveError::NotFound) => {
                return Err(AlephError::tool(format!("node '{}' not online", args.node)))
            }
            Err(e @ (ResolveError::Ambiguous(_) | ResolveError::NodeNotFound { .. })) => {
                return Err(AlephError::tool(format!("node '{}' {e}", args.node)))
            }
        };
        // Center-side fail-fast: only reject when the node declared a non-empty
        // catalog that excludes this command. Empty catalog → defer to node authority.
        if !declared.is_empty() && !declared.iter().any(|c| c.name == args.command) {
            return Err(AlephError::tool(format!(
                "command '{}' not declared by node '{}'",
                args.command, args.node
            )));
        }
        let timeout = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let params = json!({ "tool": args.command, "args": args.args });
        match channel.call("tool.call", params, timeout).await {
            Ok(resp) if resp.is_success() => Ok(resp.result.unwrap_or(Value::Null)),
            Ok(resp) => Err(AlephError::tool(format!(
                "node '{}' returned error: {}",
                args.node,
                resp.error
                    .map_or_else(|| "unknown".to_string(), |e| e.message)
            ))),
            Err(e) => Err(AlephError::tool(format!(
                "node '{}' reverse-rpc failed: {e}",
                args.node
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeRegistry, NodeSession, ReverseRpcChannel};
    use crate::gateway::protocol::JsonRpcResponse;
    use tokio::sync::mpsc;

    /// Set up a registered node session and return the center-readable channel +
    /// the background "node responder"'s outbound receiver (acting as the node:
    /// on receiving a tool.call frame, resolve a success response).
    fn registry_with_node(
        node_id: &str,
        name: &str,
        commands: Vec<&str>,
    ) -> (Arc<NodeRegistry>, mpsc::Receiver<String>, ReverseRpcChannel) {
        let (tx, rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(tx);
        let reg = Arc::new(NodeRegistry::new());
        reg.register(NodeSession {
            node_id: node_id.to_string(),
            conn_id: "conn-1".to_string(),
            device_name: name.to_string(),
            channel: channel.clone(),
            declared_commands: commands
                .into_iter()
                .map(|c| CommandDescriptor {
                    name: c.to_string(),
                    schema: json!({}),
                })
                .collect(),
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        (reg, rx, channel)
    }

    /// Background node actor: read one frame request → respond success (echo the tool).
    fn spawn_node_responder(mut rx: mpsc::Receiver<String>, channel: ReverseRpcChannel) {
        let pending = channel.pending();
        tokio::spawn(async move {
            if let Some(frame) = rx.recv().await {
                let req: Value = serde_json::from_str(&frame).unwrap();
                let id = req["id"].clone();
                let resp = JsonRpcResponse::success(
                    Some(id.clone()),
                    json!({"ran": req["params"]["tool"]}),
                );
                pending.resolve(&id, resp);
            }
        });
    }

    #[tokio::test]
    async fn invokes_node_by_name_and_returns_result() {
        let (reg, rx, ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        spawn_node_responder(rx, ch);
        let tool = NodeInvokeTool::new(reg);
        let out = tool
            .call(NodeInvokeArgs {
                node: "worker-1".to_string(),
                command: "bash".to_string(),
                args: json!({"cmd": "echo hi"}),
                timeout_ms: Some(2_000),
            })
            .await
            .expect("invoke resolves");
        assert_eq!(out["ran"], "bash");
    }

    #[tokio::test]
    async fn invokes_node_by_id() {
        let (reg, rx, ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        spawn_node_responder(rx, ch);
        let tool = NodeInvokeTool::new(reg);
        let out = tool
            .call(NodeInvokeArgs {
                node: "n-1".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(2_000),
            })
            .await
            .expect("invoke by id resolves");
        assert_eq!(out["ran"], "bash");
    }

    #[tokio::test]
    async fn offline_node_is_clear_error() {
        let reg = Arc::new(NodeRegistry::new());
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "ghost".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("offline node errors");
        assert!(err.to_string().contains("not online"), "{err}");
    }

    #[tokio::test]
    async fn ambiguous_node_surfaces_candidates() {
        let (reg, _rx, _ch) = registry_with_node("id-one", "worker-1", vec!["bash"]);
        // Second node whose name shares the "worker" substring.
        let (tx2, _rx2) = mpsc::channel::<String>(8);
        let ch2 = ReverseRpcChannel::new(tx2);
        reg.register(NodeSession {
            node_id: "id-two".to_string(),
            conn_id: "conn-2".to_string(),
            device_name: "worker-2".to_string(),
            channel: ch2,
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "worker".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("ambiguous node errors");
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "{msg}");
        assert!(
            msg.contains("worker-1") && msg.contains("worker-2"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn fail_fast_rejects_undeclared_command() {
        let (reg, _rx, _ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "worker-1".to_string(),
                command: "python".to_string(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("undeclared command fails fast");
        assert!(err.to_string().contains("not declared"), "{err}");
    }

    #[tokio::test]
    async fn timeout_is_surfaced() {
        let (reg, _rx, _ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "worker-1".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(50),
            })
            .await
            .expect_err("times out");
        assert!(err.to_string().contains("reverse-rpc failed"), "{err}");
    }
}

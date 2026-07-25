//! `node_invoke_many`: center-side LLM tool that fans out one command
//! concurrently to a set of nodes selected by tags.
//!
//! Semantics are explicitly separated from `node_invoke` (resolve→single node,
//! ambiguity=error): this tool matches a set of online nodes by tag AND
//! intersection, dispatches concurrently via `tokio::task::JoinSet`, tolerates
//! partial failure, and returns an aggregate result. Zero matches error with an
//! available-tags hint.
//! Redline: pure I/O translation (R4), no reasoning (R7); tag selection by LLM,
//! tags are not an authorization layer.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cluster::{NodeMatch, NodeRegistry};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeInvokeManyArgs {
    /// Tags an online node must ALL carry to be selected (AND match). Empty or
    /// omitted = every online node (broadcast). Tags are verbatim labels like
    /// "gpu" or "region=us"; the `node_list` tool shows each node's tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Command to run on each matched node (e.g. "bash"). Each node must
    /// declare it, or that node's result is an error (others still run).
    pub command: String,
    /// JSON arguments for the command, passed through to each node verbatim.
    #[serde(default)]
    pub args: Value,
    /// Per-node reverse-RPC timeout in ms (default 120000). Applied to every
    /// node independently; one slow node does not extend the others.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub struct NodeInvokeManyTool {
    node_registry: Arc<NodeRegistry>,
}

impl NodeInvokeManyTool {
    pub const fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }
}

/// Invoke `command` on one matched node; never returns Err — every outcome is
/// encoded as a per-node result object so a failure does not abort the fan-out.
async fn invoke_one(m: NodeMatch, command: String, args: Value, timeout_ms: u64) -> Value {
    // Per-node fail-fast, mirroring node_invoke: reject only when the node
    // declared a non-empty catalog that excludes this command.
    if !m.declared_commands.is_empty() && !m.declared_commands.iter().any(|c| c.name == command) {
        return json!({
            "node": m.name, "node_id": m.node_id, "ok": false,
            "error": format!("command '{command}' not declared by node '{}'", m.name)
        });
    }
    let params = json!({ "tool": command, "args": args });
    match m.channel.call("tool.call", params, timeout_ms).await {
        Ok(resp) if resp.is_success() => json!({
            "node": m.name, "node_id": m.node_id, "ok": true,
            "result": resp.result.unwrap_or(Value::Null)
        }),
        Ok(resp) => json!({
            "node": m.name, "node_id": m.node_id, "ok": false,
            "error": resp.error.map_or_else(|| "unknown".to_string(), |e| e.message)
        }),
        Err(e) => json!({
            "node": m.name, "node_id": m.node_id, "ok": false,
            "error": format!("reverse-rpc failed: {e}")
        }),
    }
}

#[async_trait]
impl AlephTool for NodeInvokeManyTool {
    const NAME: &'static str = "node_invoke_many";
    const DESCRIPTION: &'static str = r#"Run one command CONCURRENTLY on every connected cluster node that carries ALL of the given tags (a scatter-gather fan-out).

Select nodes by `tags` (AND match) — e.g. {"tags": ["gpu"], "command": "bash", "args": {"cmd": "nvidia-smi -L"}}. An empty/omitted `tags` targets every online node. Call `node_list` (with the same tags) to preview which nodes will be hit. `command` must be one each node declares (a node that doesn't declare it returns a per-node error; others still run).

Each node runs in its own sandbox with an independent `timeout_ms` (default 120000). Partial failure is tolerated: you always get back {"invoked", "succeeded", "failed", "results":[{"node","node_id","ok",("result"|"error")}]}. If no online node matches the tags you get a clear error listing the available tags."#;

    type Args = NodeInvokeManyArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let matches = self.node_registry.resolve_all_by_tags(&args.tags);
        if matches.is_empty() {
            // Zero-match fail-fast (mirrors resolve's NotFound style): tell the
            // LLM exactly what tags ARE available so it can correct itself.
            let online = self.node_registry.resolve_all_by_tags(&[]);
            let hint = if online.is_empty() {
                "no nodes are online".to_string()
            } else {
                let mut tags: Vec<String> = online.iter().flat_map(|m| m.tags.clone()).collect();
                tags.sort();
                tags.dedup();
                if tags.is_empty() {
                    format!("{} online node(s) declare no tags", online.len())
                } else {
                    format!("available tags: {}", tags.join(", "))
                }
            };
            return Err(AlephError::tool(format!(
                "no online node matches tags {:?} — {hint}",
                args.tags
            )));
        }
        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let mut set = tokio::task::JoinSet::new();
        for m in matches {
            let command = args.command.clone();
            let call_args = args.args.clone();
            set.spawn(async move { invoke_one(m, command, call_args, timeout_ms).await });
        }
        let mut results: Vec<Value> = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(v) => results.push(v),
                // `invoke_one` is panic-free, so this JoinError arm is currently
                // unreachable; kept as a defensive catch-all (no node id available here).
                Err(e) => {
                    results.push(json!({"ok": false, "error": format!("task join error: {e}")}))
                }
            }
        }
        // JoinSet yields in COMPLETION order, so the same fan-out over the same
        // fleet would hand the model a differently-ordered array every run (fast
        // nodes first). Sort by (node, node_id) to match the deterministic
        // ordering `node_list` / `environments.list` already guarantee — the
        // model compares these side by side.
        results.sort_by(|a, b| {
            let key = |v: &Value| {
                (
                    v["node"].as_str().unwrap_or_default().to_string(),
                    v["node_id"].as_str().unwrap_or_default().to_string(),
                )
            };
            key(a).cmp(&key(b))
        });
        let invoked = results.len();
        let succeeded = results.iter().filter(|r| r["ok"] == json!(true)).count();
        Ok(json!({
            "invoked": invoked,
            "succeeded": succeeded,
            "failed": invoked - succeeded,
            "results": results,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeRegistry, NodeSession, ReverseRpcChannel};
    use crate::gateway::protocol::JsonRpcResponse;
    use tokio::sync::mpsc;

    /// Register a node with the given tags + declared commands. Returns the
    /// node's outbound receiver so the test can choose to service it (success)
    /// or drop it (timeout).
    fn add_node(
        reg: &Arc<NodeRegistry>,
        node_id: &str,
        name: &str,
        tags: &[&str],
        commands: &[&str],
    ) -> (mpsc::Receiver<String>, ReverseRpcChannel) {
        let (tx, rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(tx);
        reg.register(NodeSession {
            node_id: node_id.to_string(),
            conn_id: format!("conn-{node_id}"),
            device_name: name.to_string(),
            channel: channel.clone(),
            declared_commands: commands
                .iter()
                .map(|c| CommandDescriptor {
                    name: c.to_string(),
                    schema: json!({}),
                })
                .collect(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            version: None,
            connected_at: 1,
        });
        (rx, channel)
    }

    /// Background "node": read one tool.call frame → resolve a success response.
    fn spawn_responder(mut rx: mpsc::Receiver<String>, channel: ReverseRpcChannel) {
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
    async fn fans_out_to_all_matching_nodes_concurrently() {
        let reg = Arc::new(NodeRegistry::new());
        let (rx1, ch1) = add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]);
        let (rx2, ch2) = add_node(&reg, "n2", "gpu-2", &["gpu"], &["bash"]);
        add_node(&reg, "n3", "cpu-1", &["cpu"], &["bash"]); // not matched
        spawn_responder(rx1, ch1);
        spawn_responder(rx2, ch2);
        let tool = NodeInvokeManyTool::new(reg);
        let out = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["gpu".into()],
                command: "bash".into(),
                args: json!({"cmd": "echo hi"}),
                timeout_ms: Some(2_000),
            })
            .await
            .expect("fan-out resolves");
        assert_eq!(out["invoked"], 2);
        assert_eq!(out["succeeded"], 2);
        assert_eq!(out["failed"], 0);
        assert_eq!(out["results"].as_array().unwrap().len(), 2);
        // Both matched nodes must appear individually (guards against a dedup regression).
        let names: Vec<&str> = out["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["node"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"gpu-1") && names.contains(&"gpu-2"),
            "{names:?}"
        );
    }

    // start_paused: tokio auto-advances to the next pending timer when the
    // runtime is otherwise idle, so n1's in-process responder resolves and
    // n2's missing responder deterministically hits the timeout — no wall-clock
    // race. (n1 success + n2 timeout asserted below.)
    #[tokio::test(start_paused = true)]
    async fn tolerates_partial_failure() {
        let reg = Arc::new(NodeRegistry::new());
        let (rx1, ch1) = add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]);
        // n2 has no responder → its call times out.
        let (_rx2, _ch2) = add_node(&reg, "n2", "gpu-2", &["gpu"], &["bash"]);
        spawn_responder(rx1, ch1);
        let tool = NodeInvokeManyTool::new(reg);
        let out = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["gpu".into()],
                command: "bash".into(),
                args: json!({}),
                timeout_ms: Some(80),
            })
            .await
            .expect("fan-out resolves even with a failing node");
        assert_eq!(out["invoked"], 2);
        assert_eq!(out["succeeded"], 1);
        assert_eq!(out["failed"], 1);
    }

    #[tokio::test]
    async fn per_node_fail_fast_on_undeclared_command() {
        let reg = Arc::new(NodeRegistry::new());
        add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]); // declares only bash
        let tool = NodeInvokeManyTool::new(reg);
        let out = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["gpu".into()],
                command: "python".into(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect("resolves with a per-node error");
        assert_eq!(out["invoked"], 1);
        assert_eq!(out["succeeded"], 0);
        assert_eq!(out["failed"], 1);
        let err = out["results"][0]["error"].as_str().unwrap();
        assert!(err.contains("not declared"), "{err}");
    }

    #[tokio::test]
    async fn zero_match_errors_with_available_tags_hint() {
        let reg = Arc::new(NodeRegistry::new());
        add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]);
        let tool = NodeInvokeManyTool::new(reg);
        let err = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["fpga".into()],
                command: "bash".into(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("zero match errors");
        let msg = err.to_string();
        assert!(msg.contains("no online node matches"), "{msg}");
        assert!(msg.contains("available tags: gpu"), "{msg}");
    }
}

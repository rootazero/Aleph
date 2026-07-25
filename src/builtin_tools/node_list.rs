//! `node_list`: center-side LLM tool, lists online cluster nodes (read-only projection).
//!
//! The discovery half (R8) of `node_invoke` / `node_invoke_many` / `node_file`:
//! the model first inspects which nodes exist, what commands each declares, and
//! what tags they carry, then decides which to drive.
//! Redline: pure table-scan rendering (R7), no reasoning; never contains credentials (R4).

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cluster::NodeRegistry;
use crate::error::Result;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct NodeListArgs {
    /// Optional tag filter (AND match): only nodes carrying EVERY listed tag.
    /// Omit (or pass []) to list all online nodes. Use this to preview which
    /// nodes a `node_invoke_many` fan-out with the same tags would hit.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct NodeListTool {
    node_registry: Arc<NodeRegistry>,
}

impl NodeListTool {
    pub const fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }
}

#[async_trait]
impl AlephTool for NodeListTool {
    const NAME: &'static str = "node_list";
    const DESCRIPTION: &'static str = r#"List the online cluster nodes (remote execution arms the center can drive).

Each entry shows the node's id, name, declared commands (what node_invoke may run
there), free-text tags (what node_invoke_many selects on), connect time, and the
aleph-server version it runs (null on older nodes; a version differing from the
center's is allowed but worth mentioning if that node misbehaves). Pass
`tags` (AND match) to preview exactly which nodes a node_invoke_many fan-out with
those tags would hit; omit it to see every online node. An enrolled node that is
currently offline does NOT appear here — it cannot be invoked until it reconnects."#;

    type Args = NodeListArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let filter = args.tags.unwrap_or_default();
        let nodes: Vec<Value> = self
            .node_registry
            .list_environments()
            .into_iter()
            .filter(|e| filter.iter().all(|t| e.tags.contains(t)))
            .map(|e| {
                json!({
                    "id": e.id,
                    "name": e.name,
                    "status": e.status,
                    "commands": e.commands.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
                    "tags": e.tags,
                    "connected_at": e.connected_at,
                    // The node's aleph-server build; null on nodes older than
                    // the version handshake. Surfaced so a "this node behaves
                    // differently from the others" report has something to
                    // correlate against — the center never refuses on skew.
                    "version": e.version,
                })
            })
            .collect();
        Ok(json!({ "count": nodes.len(), "nodes": nodes }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeSession, ReverseRpcChannel};
    use tokio::sync::mpsc;

    fn register(reg: &NodeRegistry, id: &str, name: &str, tags: Vec<&str>, commands: Vec<&str>) {
        let (tx, _rx) = mpsc::channel::<String>(8);
        reg.register(NodeSession {
            node_id: id.to_string(),
            conn_id: format!("conn-{id}"),
            device_name: name.to_string(),
            channel: ReverseRpcChannel::new(tx),
            declared_commands: commands
                .into_iter()
                .map(|c| CommandDescriptor {
                    name: c.to_string(),
                    schema: json!({}),
                })
                .collect(),
            tags: tags.into_iter().map(String::from).collect(),
            version: None,
            connected_at: 1,
        });
    }

    #[tokio::test]
    async fn lists_online_nodes_with_commands_and_tags() {
        let reg = Arc::new(NodeRegistry::new());
        register(
            &reg,
            "n-1",
            "worker-1",
            vec!["gpu"],
            vec!["bash", "file.read"],
        );
        let out = NodeListTool::new(reg)
            .call(NodeListArgs::default())
            .await
            .expect("lists");
        assert_eq!(out["count"], 1);
        let node = &out["nodes"][0];
        assert_eq!(node["id"], "n-1");
        assert_eq!(node["name"], "worker-1");
        assert_eq!(node["status"], "online");
        assert_eq!(node["tags"][0], "gpu");
        let cmds: Vec<&str> = node["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(cmds.contains(&"bash") && cmds.contains(&"file.read"));
    }

    #[tokio::test]
    async fn tag_filter_is_and_match() {
        let reg = Arc::new(NodeRegistry::new());
        register(&reg, "a", "node-a", vec!["gpu", "us"], vec!["bash"]);
        register(&reg, "b", "node-b", vec!["gpu"], vec!["bash"]);
        let tool = NodeListTool::new(reg);

        let both = tool
            .call(NodeListArgs {
                tags: Some(vec!["gpu".into(), "us".into()]),
            })
            .await
            .unwrap();
        assert_eq!(both["count"], 1);
        assert_eq!(both["nodes"][0]["id"], "a");

        let gpu = tool
            .call(NodeListArgs {
                tags: Some(vec!["gpu".into()]),
            })
            .await
            .unwrap();
        assert_eq!(gpu["count"], 2);

        let none = tool
            .call(NodeListArgs {
                tags: Some(vec!["fpga".into()]),
            })
            .await
            .unwrap();
        assert_eq!(none["count"], 0);
    }

    #[tokio::test]
    async fn empty_registry_is_empty_list_not_error() {
        let reg = Arc::new(NodeRegistry::new());
        let out = NodeListTool::new(reg)
            .call(NodeListArgs::default())
            .await
            .expect("empty ok");
        assert_eq!(out["count"], 0);
        assert!(out["nodes"].as_array().unwrap().is_empty());
    }
}

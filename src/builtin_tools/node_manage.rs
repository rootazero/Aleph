//! `node_manage`: center-side LLM tool for cluster **membership** —
//! the write half of the fleet that `node_list` only reads.
//!
//! R8 ("everything is a tool"): enrolling and deregistering nodes existed only
//! as Panel RPCs, so "kick worker-3 out of the cluster" was the one cluster
//! operation an operator could not do by talking. This tool exposes both over
//! the SAME single sources of truth the RPCs use
//! ([`crate::cluster::enroll_node_device`] / [`crate::cluster::deregister_node`]),
//! so the Panel and the model cannot end up with different semantics.
//!
//! Redline: pure I/O translation (R4), no reasoning (R7) — the action and the
//! target are chosen by the model. Operator-gated (`OPERATOR_TOOLS`): changing
//! who is in the fleet is not a chat-tier capability.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cluster::{deregister_node, enroll_node_device, DeregisterError, NodeRegistry};
use crate::error::{AlephError, Result};
use crate::gateway::security::SecurityStore;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeManageArgs {
    /// "enroll" to reserve a fleet slot for a node name, "deregister" to remove
    /// a node from the cluster.
    pub action: String,
    /// For "enroll": the name the node will dial in with (`--name`). For
    /// "deregister": the node's name or id, as shown by `node_list`.
    pub node: String,
}

#[derive(Clone)]
pub struct NodeManageTool {
    node_registry: Arc<NodeRegistry>,
    security_store: Arc<SecurityStore>,
}

impl NodeManageTool {
    pub const fn new(node_registry: Arc<NodeRegistry>, security_store: Arc<SecurityStore>) -> Self {
        Self {
            node_registry,
            security_store,
        }
    }
}

#[async_trait]
impl AlephTool for NodeManageTool {
    const NAME: &'static str = "node_manage";
    const DESCRIPTION: &'static str = r#"Change which machines belong to the cluster: enroll a node slot, or deregister a node. (`node_list` shows the current fleet; `node_invoke` runs commands on it.)

- {"action": "enroll", "node": "worker-3"} reserves the name and returns its node_id. The machine joins once someone runs `aleph-server node --center ws://<this-host>:18790 --name worker-3` there — enrolling does NOT install or start anything remotely, and no token is issued (nodes are trusted by network boundary). Enrolling a name that already exists returns the same node_id, unchanged.
- {"action": "deregister", "node": "worker-3"} removes it: the live session is evicted immediately, and the removal STICKS — the node is refused if it reconnects, and exits on its own. Works on offline nodes too. Reversible only by enrolling again.

Returns {action, node_id, ...}. Deregistering is destructive to fleet membership — say which node you are removing before you do it."#;

    type Args = NodeManageArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action.as_str() {
            "enroll" => {
                let name = args.node.trim();
                if name.is_empty() {
                    return Err(AlephError::tool("enroll: `node` must be a non-empty name"));
                }
                let (node_id, minted) = enroll_node_device(&self.security_store, name)
                    .map_err(|e| AlephError::tool(format!("enroll '{name}': {e}")))?;
                Ok(json!({
                    "action": "enroll",
                    "node": name,
                    "node_id": node_id,
                    // false = this name was already enrolled and was returned
                    // unchanged (the call is idempotent).
                    "created": minted,
                }))
            }
            "deregister" => {
                match deregister_node(&self.node_registry, &self.security_store, &args.node) {
                    Ok(o) => Ok(json!({
                        "action": "deregister",
                        "node": args.node,
                        "node_id": o.node_id,
                        // true = a live session was evicted; false = it was
                        // already offline (the removal still sticks).
                        "evicted": o.evicted,
                        "device_removed": o.device_removed,
                    })),
                    Err(DeregisterError::NotFound) => Err(AlephError::tool(format!(
                        "no online or enrolled node matches '{}'",
                        args.node
                    ))),
                    Err(DeregisterError::Ambiguous(detail)) => {
                        Err(AlephError::tool(format!("node '{}' {detail}", args.node)))
                    }
                }
            }
            other => Err(AlephError::tool(format!(
                "action must be 'enroll' or 'deregister', got '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeSession, ReverseRpcChannel};
    use tokio::sync::mpsc;

    fn tool() -> (NodeManageTool, Arc<NodeRegistry>, Arc<SecurityStore>) {
        let reg = Arc::new(NodeRegistry::new());
        let store = Arc::new(SecurityStore::in_memory().expect("in-memory store"));
        (NodeManageTool::new(reg.clone(), store.clone()), reg, store)
    }

    fn register(reg: &NodeRegistry, id: &str, name: &str) {
        let (tx, _rx) = mpsc::channel::<String>(8);
        reg.register(NodeSession {
            node_id: id.to_string(),
            conn_id: format!("conn-{id}"),
            device_name: name.to_string(),
            channel: ReverseRpcChannel::new(tx),
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: json!({}),
            }],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
    }

    #[tokio::test]
    async fn enroll_is_idempotent_by_name() {
        let (t, _reg, store) = tool();
        let first = t
            .call(NodeManageArgs {
                action: "enroll".into(),
                node: "worker-3".into(),
            })
            .await
            .expect("enroll");
        assert_eq!(first["created"], true);
        let id = first["node_id"].as_str().unwrap().to_string();

        // Re-enrolling the same name (a retried tool call, or the operator
        // asking twice) must return the SAME id — a second row would make the
        // node's own by-name merge ambiguous and strand both rows.
        let again = t
            .call(NodeManageArgs {
                action: "enroll".into(),
                node: "Worker 3".into(), // normalized spelling variant
            })
            .await
            .expect("re-enroll");
        assert_eq!(again["node_id"], json!(id));
        assert_eq!(again["created"], false);
        assert_eq!(store.list_devices().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deregister_evicts_online_node_and_revokes_it() {
        let (t, reg, store) = tool();
        let enroll = t
            .call(NodeManageArgs {
                action: "enroll".into(),
                node: "worker-3".into(),
            })
            .await
            .unwrap();
        let id = enroll["node_id"].as_str().unwrap().to_string();
        register(&reg, &id, "worker-3");

        let out = t
            .call(NodeManageArgs {
                action: "deregister".into(),
                node: "worker-3".into(),
            })
            .await
            .expect("deregister");
        assert_eq!(out["node_id"], json!(id));
        assert_eq!(out["evicted"], true);
        assert_eq!(out["device_removed"], true);
        assert!(reg.list_environments().is_empty());
        // Revoked → gone from the enrolled roster, so admission will refuse it.
        assert!(store.list_devices().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deregister_reaches_an_offline_node() {
        let (t, _reg, _store) = tool();
        t.call(NodeManageArgs {
            action: "enroll".into(),
            node: "cold-box".into(),
        })
        .await
        .unwrap();
        let out = t
            .call(NodeManageArgs {
                action: "deregister".into(),
                node: "cold-box".into(),
            })
            .await
            .expect("offline deregister");
        assert_eq!(out["evicted"], false, "nothing online to evict");
        assert_eq!(out["device_removed"], true);
    }

    #[tokio::test]
    async fn unknown_node_and_unknown_action_are_clear_errors() {
        let (t, _reg, _store) = tool();
        let miss = t
            .call(NodeManageArgs {
                action: "deregister".into(),
                node: "ghost".into(),
            })
            .await
            .expect_err("no such node");
        assert!(miss.to_string().contains("no online or enrolled node"));

        let bad = t
            .call(NodeManageArgs {
                action: "restart".into(),
                node: "worker-3".into(),
            })
            .await
            .expect_err("unknown action");
        assert!(bad.to_string().contains("must be 'enroll' or 'deregister'"));
    }
}

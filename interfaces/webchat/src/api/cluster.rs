//! Cluster API client. `environments.list` (online + offline merged view), `cluster.enroll`
//! (operator-only pre-register a node name), `cluster.deregister` (operator-only deregister a node).
//! Node command dispatch is LLM-driven through conversation (`node_invoke`/`node_file` tools, R8),
//! no manual entry point exposed in the Panel.
//!
//! LAN-trust: enroll does **not mint a token**. Nodes declare identity via the parameter shape of the `connect` frame;
//! registration itself is also done inside connect — so `cluster.enroll` is merely an operator **pre-reservation**
//! (so the node appears in the fleet as offline before it actually dials in, and identically-named nodes merge into this row).

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
    /// The `aleph-server` build this node runs. Only ever populated for ONLINE
    /// nodes (the device store keeps no version column, and a remembered
    /// version is a stale claim about a machine we can't see). `None` on an
    /// online node = it predates the version handshake.
    #[serde(default)]
    pub version: Option<String>,
}

impl Environment {
    pub fn is_online(&self) -> bool {
        self.status == "online"
    }
}

/// Response payload of `cluster.enroll`. **No token** — under LAN-trust, enroll only returns `node_id`.
/// Previously this carried `token: String` (required) + `signature`, but the server stopped returning them long ago,
/// so `serde_json::from_value` always failed with "missing field `token`": the Panel's
/// "+ Enroll" button **had never succeeded**.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResult {
    pub node_id: String,
    /// `true` = this name was already enrolled and its id was returned
    /// unchanged. Enroll is idempotent, so a second click is harmless — it used
    /// to mint a duplicate row that then made the node's own by-name merge
    /// ambiguous and stranded both rows in this very list.
    #[serde(default)]
    pub reused: bool,
}

pub struct ClusterApi;

impl ClusterApi {
    /// List cluster nodes (online sessions + registered but offline devices). RPC `environments.list`.
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

    /// Pre-register a node name, get back its `node_id`. RPC `cluster.enroll` (operator-only).
    pub async fn enroll_node(
        state: &DashboardState,
        node_name: String,
    ) -> Result<EnrollResult, String> {
        let params = serde_json::json!({ "node_name": node_name });
        let result = state.rpc_call("cluster.enroll", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse enroll result: {e}"))
    }

    /// Deregister a node (name or id): evict online sessions + revoke device record. Deregistration is **sticky** —
    /// the node's next connect will be rejected by the center; it cannot reconnect on its own.
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
        // Version is optional on the wire in both directions: absent from this
        // (older-center) payload, and never sent for offline rows.
        assert!(envs[0].version.is_none());
    }

    #[test]
    fn parses_the_online_nodes_version() {
        let payload = serde_json::json!([
            {"id":"n1","name":"node-a","status":"online","commands":[],"tags":[],
             "connected_at":1,"last_seen_at":null,"version":"26.7.25"}
        ]);
        let envs: Vec<Environment> = serde_json::from_value(payload).unwrap();
        assert_eq!(envs[0].version.as_deref(), Some("26.7.25"));
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

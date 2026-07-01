//! Subagent tree handler — read-only snapshot of the background sub-agent tree.
//!
//! `subagent.tree` returns the **flat** background sub-agent nodes from the
//! process-global `BackgroundAgentTracker`, optionally filtered to one root
//! session. The panel rebuilds the hierarchy with `aleph_protocol::build_tree`
//! — the same shared reconstruction it runs on each live `run.subagent_tree`
//! delta, so cold-start and live paths are byte-identical (one Rust tree
//! builder, compiled to WASM; no Python+TS-style double implementation).
//! Pure I/O (R4/R10) — no reasoning, no mutation.
//!
//! ## Request
//! ```json
//! { "root_session": "agent:session-key" }   // optional; omitted = whole process
//! ```
//!
//! ## Response (success)
//! ```json
//! { "nodes": [ ...SubagentNode... ], "count": 3 }
//! ```

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::agents::background_tracker::BackgroundAgentTracker;

/// `subagent.tree` — snapshot the flat background sub-agent nodes for the panel.
pub async fn handle_tree(request: JsonRpcRequest) -> JsonRpcResponse {
    let root_session = request
        .params
        .as_ref()
        .and_then(|p| p.get("root_session"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let flat = BackgroundAgentTracker::global().flat_nodes(root_session.as_deref());
    let count = flat.len();

    match serde_json::to_value(&flat) {
        Ok(nodes) => {
            JsonRpcResponse::success(request.id, json!({ "nodes": nodes, "count": count }))
        }
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("subagent.tree serialize failed: {err}"),
        ),
    }
}

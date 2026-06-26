//! Subagent tree handler — read-only snapshot of the background sub-agent tree.
//!
//! `subagent.tree` reconstructs the forest (via `aleph_protocol`'s shared
//! `build_tree`) over the process-global `BackgroundAgentTracker`, optionally
//! filtered to one root session. Backs the panel's cold-start; live updates then
//! arrive incrementally via the `run.subagent_tree` relay. Pure I/O (R4/R10) —
//! no reasoning, no mutation.
//!
//! ## Request
//! ```json
//! { "root_session": "agent:session-key" }   // optional; omitted = whole process
//! ```
//!
//! ## Response (success)
//! ```json
//! { "roots": [ ...TreeNode... ], "count": 3 }
//! ```

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::agents::background_tracker::BackgroundAgentTracker;

/// `subagent.tree` — snapshot the background sub-agent tree for the panel.
pub async fn handle_tree(request: JsonRpcRequest) -> JsonRpcResponse {
    let root_session = request
        .params
        .as_ref()
        .and_then(|p| p.get("root_session"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let tracker = BackgroundAgentTracker::global();
    let flat = tracker.flat_nodes(root_session.as_deref());
    let count = flat.len();
    let tree = aleph_protocol::subagent_tree::build_tree(&flat);

    match serde_json::to_value(&tree) {
        Ok(roots) => JsonRpcResponse::success(request.id, json!({ "roots": roots, "count": count })),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("subagent.tree serialize failed: {err}"),
        ),
    }
}

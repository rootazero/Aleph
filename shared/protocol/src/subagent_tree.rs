//! Background sub-agent tree — shared types + reconstruction.
//!
//! Single source of truth for the "Background sub-agent tree diagram": the flat node shape
//! ([`SubagentNode`]), the live wire events ([`SubagentTreeEvent`]), and the
//! pure reconstruction ([`build_tree`] + [`Rollup`]). Compiled into BOTH the
//! native `aleph-server` (the `subagent.tree` RPC snapshot) AND the WASM panel
//! (live incremental rebuild) — one algorithm, two ends, no Python+TS-style
//! double implementation.
//!
//! The reconstruction is intentionally arbitrary-depth-capable even though the
//! current runtime spawns a structurally 2-level tree (session root → depth-1
//! background subagents; deeper nesting is blocked by the SubAgent-mode
//! recursion guard). Populating `parent_id` for true nesting is then a
//! data-only change — the tree code already handles it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Typed node lifecycle — replaces hermes's stringly-typed status. Illegal
/// states are unrepresentable; transitions are monotonic (Running → terminal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLifecycle {
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl NodeLifecycle {
    /// True for a terminal state that did not succeed. Drives the panel's
    /// `Failed` filter.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled | Self::TimedOut)
    }
}

/// One background sub-agent, flattened. Shared by the live wire events, the
/// RPC snapshot, and the client-side node map — so a stateless client rebuilds
/// the tree from events alone (hermes "identity threading" pattern).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentNode {
    /// Stable node id = the tracker's `request_id`.
    pub node_id: String,
    /// Immediate parent node id. `None` = attaches directly under the session
    /// root (the common depth-1 case).
    pub parent_id: Option<String>,
    /// Nesting depth (`ChainContext.depth`): 1 = direct child of the root.
    pub depth: u32,
    /// Owning top-level session key — the tree this node belongs to. Panels
    /// group roots by this so the global tracker's multiple sessions render as
    /// separate trees.
    pub root_session: String,
    /// Task description the sub-agent was spawned with.
    pub task: String,
    /// Resolved model id, when known.
    pub model: Option<String>,
    pub lifecycle: NodeLifecycle,
    /// Wall-clock spawn time (unix ms) — used for stable start-order sorting.
    pub started_at_ms: u64,
    /// Elapsed (running) or total (terminal) wall-clock, in ms.
    pub elapsed_ms: u64,
    /// Tool calls this node has made so far.
    pub tool_count: u32,
    /// Most recent tool name, when one has been called.
    pub last_tool: Option<String>,
    /// Most recent activity signal ("tool_called" / "tool_returned" /
    /// "llm_thinking" / "cancelled").
    pub last_activity: Option<String>,
}

/// Live wire event — every variant carries enough identity (`node_id` +
/// `root_session`) for a stateless client to update its flat map then rebuild.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubagentTreeEvent {
    /// A new background sub-agent registered.
    Spawned { node: SubagentNode },
    /// Incremental activity on a running node.
    Progress {
        node_id: String,
        root_session: String,
        step: usize,
        activity: String,
        tool_name: Option<String>,
        tool_count: u32,
    },
    /// A node reached a terminal state. Merges completed/failed/cancelled/
    /// timed-out — distinguished by `lifecycle` (one fewer event type).
    Settled {
        node_id: String,
        root_session: String,
        lifecycle: NodeLifecycle,
        duration_ms: u64,
        iterations: usize,
        tool_calls_made: usize,
        total_tokens: usize,
    },
}

/// Recursive subtree rollup — drives the panel's heatmap, sparkline, and
/// summary line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rollup {
    /// Nodes in this subtree, including the node itself.
    pub descendant_count: u32,
    /// Nodes in this subtree still `Running`.
    pub active_count: u32,
    /// Tool calls summed across the subtree.
    pub total_tools: u32,
    /// Wall-clock summed across the subtree, in ms.
    pub total_duration_ms: u64,
    /// Deepest level reached from this node (0 = leaf).
    pub max_depth_from_here: u32,
    /// Activity proxy = tools per second across the subtree. Drives heatmap
    /// coloring so the eye finds the hot branch.
    pub hotness: f32,
}

/// A reconstructed tree node: the flat node, its children, and its rollup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub node: SubagentNode,
    pub children: Vec<TreeNode>,
    pub rollup: Rollup,
}

/// Reconstruct a forest from flat nodes. Pure, deterministic, O(n).
///
/// - Children are grouped by `parent_id`.
/// - A node whose `parent_id` is `None` **or** points to an id not present in
///   `flat` becomes a root (hermes orphan-tolerance: never drop a node).
/// - Siblings sort by `(started_at_ms, node_id)` for a stable spawn order.
/// - Rollups are computed bottom-up in the same pass.
/// - A `parent_id` cycle cannot starve the build: each node is placed at most
///   once via a `visited` guard.
#[must_use]
pub fn build_tree(flat: &[SubagentNode]) -> Vec<TreeNode> {
    let present: std::collections::HashSet<&str> =
        flat.iter().map(|n| n.node_id.as_str()).collect();

    // parent_id -> child node_ids. `None`/dangling parents bucket under "".
    let mut children_of: HashMap<&str, Vec<&SubagentNode>> = HashMap::new();
    for n in flat {
        let key = match &n.parent_id {
            Some(p) if present.contains(p.as_str()) => p.as_str(),
            _ => "", // root bucket
        };
        children_of.entry(key).or_default().push(n);
    }
    for bucket in children_of.values_mut() {
        bucket.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
    }

    let mut visited = std::collections::HashSet::new();
    let roots: Vec<&SubagentNode> = children_of.get("").cloned().unwrap_or_default();
    roots
        .into_iter()
        .map(|n| assemble(n, &children_of, &mut visited))
        .collect()
}

fn assemble<'a>(
    node: &'a SubagentNode,
    children_of: &HashMap<&'a str, Vec<&'a SubagentNode>>,
    visited: &mut std::collections::HashSet<&'a str>,
) -> TreeNode {
    let children: Vec<TreeNode> = if visited.insert(node.node_id.as_str()) {
        let mut out = Vec::new();
        if let Some(kids) = children_of.get(node.node_id.as_str()) {
            for c in kids {
                if !visited.contains(c.node_id.as_str()) {
                    out.push(assemble(c, children_of, visited));
                }
            }
        }
        out
    } else {
        Vec::new()
    };
    let rollup = compute_rollup(node, &children);
    TreeNode {
        node: node.clone(),
        children,
        rollup,
    }
}

#[allow(clippy::cast_precision_loss)] // hotness is a heuristic ratio
fn compute_rollup(node: &SubagentNode, children: &[TreeNode]) -> Rollup {
    let mut r = Rollup {
        descendant_count: 1,
        active_count: u32::from(node.lifecycle == NodeLifecycle::Running),
        total_tools: node.tool_count,
        total_duration_ms: node.elapsed_ms,
        max_depth_from_here: 0,
        hotness: 0.0,
    };
    for child in children {
        r.descendant_count += child.rollup.descendant_count;
        r.active_count += child.rollup.active_count;
        r.total_tools += child.rollup.total_tools;
        r.total_duration_ms += child.rollup.total_duration_ms;
        r.max_depth_from_here = r
            .max_depth_from_here
            .max(child.rollup.max_depth_from_here + 1);
    }
    // tools per second; guard the zero-duration window (no divide-by-zero).
    r.hotness = if r.total_duration_ms == 0 {
        0.0
    } else {
        (r.total_tools as f32) / (r.total_duration_ms as f32 / 1000.0)
    };
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent: Option<&str>, depth: u32, started: u64) -> SubagentNode {
        SubagentNode {
            node_id: id.to_string(),
            parent_id: parent.map(String::from),
            depth,
            root_session: "agent:sess".to_string(),
            task: format!("task {id}"),
            model: None,
            lifecycle: NodeLifecycle::Running,
            started_at_ms: started,
            elapsed_ms: 1000,
            tool_count: 1,
            last_tool: None,
            last_activity: None,
        }
    }

    #[test]
    fn flat_depth1_nodes_become_roots() {
        let flat = vec![node("a", None, 1, 10), node("b", None, 1, 20)];
        let tree = build_tree(&flat);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].node.node_id, "a"); // started earlier sorts first
        assert_eq!(tree[1].node.node_id, "b");
        assert!(tree[0].children.is_empty());
    }

    #[test]
    fn nested_parent_id_builds_multilevel() {
        let flat = vec![
            node("root", None, 1, 10),
            node("child", Some("root"), 2, 20),
            node("grand", Some("child"), 3, 30),
        ];
        let tree = build_tree(&flat);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].node.node_id, "root");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].node.node_id, "child");
        assert_eq!(tree[0].children[0].children[0].node.node_id, "grand");
        // rollup rolls up the whole subtree
        assert_eq!(tree[0].rollup.descendant_count, 3);
        assert_eq!(tree[0].rollup.max_depth_from_here, 2);
        assert_eq!(tree[0].rollup.total_tools, 3);
    }

    #[test]
    fn dangling_parent_becomes_root() {
        // parent "ghost" not present → orphan promoted to root, never dropped.
        let flat = vec![node("x", Some("ghost"), 2, 10)];
        let tree = build_tree(&flat);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].node.node_id, "x");
    }

    #[test]
    fn rollup_counts_active_and_hotness() {
        let mut a = node("a", None, 1, 10);
        a.tool_count = 10;
        a.elapsed_ms = 2000; // 10 tools / 2s = 5 tools/s
        a.lifecycle = NodeLifecycle::Completed;
        let mut b = node("b", Some("a"), 2, 20);
        b.lifecycle = NodeLifecycle::Running;
        b.tool_count = 0;
        b.elapsed_ms = 0;
        let tree = build_tree(&[a, b]);
        let root = &tree[0];
        assert_eq!(root.rollup.descendant_count, 2);
        assert_eq!(root.rollup.active_count, 1); // only b running
        assert_eq!(root.rollup.total_tools, 10);
        assert!((root.rollup.hotness - 5.0).abs() < 0.001);
    }

    #[test]
    fn zero_duration_hotness_is_zero_not_nan() {
        let mut a = node("a", None, 1, 10);
        a.elapsed_ms = 0;
        a.tool_count = 3;
        let tree = build_tree(&[a]);
        assert_eq!(tree[0].rollup.hotness, 0.0);
    }

    #[test]
    fn cycle_does_not_infinite_loop() {
        // a -> b -> a forms a cycle; both reachable only via the root bucket
        // when neither is a true root. With both parents present and pointing
        // at each other, neither lands in the "" bucket, so the forest is
        // empty — but the build must terminate regardless.
        let a = node("a", Some("b"), 2, 10);
        let b = node("b", Some("a"), 2, 20);
        let tree = build_tree(&[a, b]);
        // Neither is a root (both have present parents) → empty forest, no hang.
        assert!(tree.is_empty());
    }

    #[test]
    fn lifecycle_failure_classification() {
        assert!(!NodeLifecycle::Running.is_failure());
        assert!(!NodeLifecycle::Completed.is_failure());
        assert!(NodeLifecycle::Failed.is_failure());
        assert!(NodeLifecycle::Cancelled.is_failure());
        assert!(NodeLifecycle::TimedOut.is_failure());
    }
}

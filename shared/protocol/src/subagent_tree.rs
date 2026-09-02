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

/// Gateway topic the live tree events are published under. One constant for
/// the producer (`gateway::subagent_tree_relay`), the visibility scoper, and
/// every client filter — a topic string is a wire key, and wire keys kept as
/// per-crate literals cancel each other out silently when one side moves.
pub const TOPIC: &str = "run.subagent_tree";

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
    /// Round-8 — bounded preview of the terminal result (200 chars, UTF-8 safe,
    /// ellipsised on truncation). `Some` ONLY for completed/failed/cancelled/
    /// timed-out nodes whose outcome carried a non-empty payload. Lets a
    /// panel render "completed: '...'" inline from a single cold-start RPC
    /// instead of a follow-up `check_status` per node. `None` for running
    /// nodes (no result yet) and for completed nodes whose result was empty.
    ///
    /// `#[serde(skip_serializing_if = "Option::is_none")]` — old panels that
    /// never heard of this field drop the whole key without complaint. New
    /// panels see a real preview and use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    /// Session key of the child's own persisted transcript
    /// (`agent:{agent}:ephemeral:sub-bg-{request_id}`), when the spawn minted
    /// one (background children only). This is the address a client hands to
    /// the existing `chat.history` RPC to open the agent's run view — carried
    /// here so the derivation lives in ONE place (the tracker, which owns the
    /// spawn) instead of every client re-deriving the key format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session: Option<String>,
    /// Total tokens the child consumed, known only at settlement (the same
    /// figure the `Settled` event reports). `None` while running and for
    /// nodes that settled before this field existed — a cold-start snapshot
    /// can now show per-agent tokens instead of only live watchers seeing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
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

/// Merge one live event into a flat node list (keyed by `node_id`). Spawned
/// upserts; Progress / Settled patch an existing node (ignored if unknown —
/// the cold-start snapshot or an earlier Spawned will have created it).
///
/// Promoted from the Panel's view state so the Panel and the TUI run ONE merge
/// algorithm (same argument as [`build_tree`]: one implementation, two ends).
pub fn apply_event(nodes: &mut Vec<SubagentNode>, ev: SubagentTreeEvent) {
    match ev {
        SubagentTreeEvent::Spawned { node } => {
            match nodes.iter_mut().find(|n| n.node_id == node.node_id) {
                Some(existing) => *existing = node,
                None => nodes.push(node),
            }
        }
        SubagentTreeEvent::Progress {
            node_id,
            activity,
            tool_name,
            tool_count,
            ..
        } => {
            if let Some(n) = nodes.iter_mut().find(|n| n.node_id == node_id) {
                n.tool_count = tool_count;
                n.last_activity = Some(activity);
                if tool_name.is_some() {
                    n.last_tool = tool_name;
                }
            }
        }
        SubagentTreeEvent::Settled {
            node_id,
            lifecycle,
            duration_ms,
            tool_calls_made,
            total_tokens,
            ..
        } => {
            if let Some(n) = nodes.iter_mut().find(|n| n.node_id == node_id) {
                n.lifecycle = lifecycle;
                n.elapsed_ms = duration_ms;
                let final_tools = u32::try_from(tool_calls_made).unwrap_or(u32::MAX);
                n.tool_count = n.tool_count.max(final_tools);
                // 0 means "unreported", not "zero tokens" (a real run always
                // consumes tokens) — mirror the tracker's snapshot rule so the
                // live path and the cold path agree.
                if total_tokens > 0 {
                    n.total_tokens = Some(u64::try_from(total_tokens).unwrap_or(u64::MAX));
                }
            }
        }
    }
}

/// Header summary stats over a flat node list — drives the Panel's rollup
/// line and the TUI's "N running agents" status segment.
pub struct Summary {
    pub agents: usize,
    pub tools: u32,
    pub active: u32,
    pub max_depth: u32,
    pub total_duration_ms: u64,
    /// `depth_counts[i]` = nodes at depth `i` (drives the sparkline).
    pub depth_counts: Vec<u32>,
}

#[must_use]
pub fn summarize(nodes: &[SubagentNode]) -> Summary {
    let mut s = Summary {
        agents: nodes.len(),
        tools: 0,
        active: 0,
        max_depth: 0,
        total_duration_ms: 0,
        depth_counts: Vec::new(),
    };
    for n in nodes {
        s.tools += n.tool_count;
        if n.lifecycle == NodeLifecycle::Running {
            s.active += 1;
        }
        s.max_depth = s.max_depth.max(n.depth);
        s.total_duration_ms += n.elapsed_ms;
        let d = n.depth as usize;
        if s.depth_counts.len() <= d {
            s.depth_counts.resize(d + 1, 0);
        }
        s.depth_counts[d] += 1;
    }
    s
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
            result_preview: None,
            child_session: None,
            total_tokens: None,
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
    fn apply_spawned_then_progress_then_settled() {
        let mut nodes = Vec::new();
        apply_event(
            &mut nodes,
            SubagentTreeEvent::Spawned {
                node: node("a", None, 1, 10),
            },
        );
        assert_eq!(nodes.len(), 1);
        apply_event(
            &mut nodes,
            SubagentTreeEvent::Progress {
                node_id: "a".into(),
                root_session: "agent:sess".into(),
                step: 2,
                activity: "tool_called".into(),
                tool_name: Some("grep".into()),
                tool_count: 5,
            },
        );
        assert_eq!(nodes[0].tool_count, 5);
        assert_eq!(nodes[0].last_tool.as_deref(), Some("grep"));
        apply_event(
            &mut nodes,
            SubagentTreeEvent::Settled {
                node_id: "a".into(),
                root_session: "agent:sess".into(),
                lifecycle: NodeLifecycle::Completed,
                duration_ms: 4200,
                iterations: 3,
                tool_calls_made: 9,
                total_tokens: 100,
            },
        );
        assert_eq!(nodes[0].lifecycle, NodeLifecycle::Completed);
        assert_eq!(nodes[0].elapsed_ms, 4200);
        assert_eq!(nodes[0].tool_count, 9);
        assert_eq!(nodes[0].total_tokens, Some(100));
    }

    #[test]
    fn settled_zero_tokens_stays_unknown_not_zero() {
        let mut nodes = vec![node("a", None, 1, 10)];
        apply_event(
            &mut nodes,
            SubagentTreeEvent::Settled {
                node_id: "a".into(),
                root_session: "agent:sess".into(),
                lifecycle: NodeLifecycle::Failed,
                duration_ms: 10,
                iterations: 0,
                tool_calls_made: 0,
                total_tokens: 0,
            },
        );
        assert_eq!(nodes[0].total_tokens, None);
    }

    #[test]
    fn summarize_counts_active_and_depth() {
        let mut running = node("a", None, 1, 10);
        running.lifecycle = NodeLifecycle::Running;
        let mut done = node("b", None, 1, 20);
        done.lifecycle = NodeLifecycle::Completed;
        let s = summarize(&[running, done]);
        assert_eq!(s.agents, 2);
        assert_eq!(s.active, 1);
        assert_eq!(s.tools, 2);
        assert_eq!(s.max_depth, 1);
        assert_eq!(s.depth_counts.get(1).copied(), Some(2));
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

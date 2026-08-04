//! Pure state logic for the sub-agent tree view: merging live events into the
//! flat node list, summarizing, and sort/filter over the rebuilt forest. No
//! Leptos; host-testable. The forest itself is rebuilt via the shared
//! `aleph_protocol::subagent_tree::build_tree` (single source, native + WASM).

use aleph_protocol::subagent_tree::{NodeLifecycle, SubagentNode, SubagentTreeEvent, TreeNode};

/// Merge one live event into the flat node list (keyed by `node_id`). Spawned
/// upserts; Progress / Settled patch an existing node (ignored if unknown — the
/// cold-start snapshot or an earlier Spawned will have created it).
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
            ..
        } => {
            if let Some(n) = nodes.iter_mut().find(|n| n.node_id == node_id) {
                n.lifecycle = lifecycle;
                n.elapsed_ms = duration_ms;
                let final_tools = u32::try_from(tool_calls_made).unwrap_or(u32::MAX);
                n.tool_count = n.tool_count.max(final_tools);
            }
        }
    }
}

/// How to order the top-level roots.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Status,
    Tools,
    Duration,
    Depth,
}

/// Which nodes to surface.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    Running,
    Failed,
    Leaves,
}

impl SortMode {
    pub const ALL: [Self; 4] = [Self::Status, Self::Tools, Self::Duration, Self::Depth];
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Tools => "tools",
            Self::Duration => "duration",
            Self::Depth => "depth",
        }
    }
}

impl FilterMode {
    pub const ALL: [Self; 4] = [Self::All, Self::Running, Self::Failed, Self::Leaves];
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Running => "running",
            Self::Failed => "failed",
            Self::Leaves => "leaves",
        }
    }
    /// Single-node predicate (descendants handled by `keep_recursive`).
    fn keep(self, node: &TreeNode) -> bool {
        match self {
            Self::All | Self::Leaves => true,
            Self::Running => node.node.lifecycle == NodeLifecycle::Running,
            Self::Failed => node.node.lifecycle.is_failure(),
        }
    }
}

/// Header summary stats.
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

/// Sort + filter the forest. `Leaves` flattens to leaf nodes (drops hierarchy);
/// other filters keep a node when it OR any descendant matches, so the path to
/// a match stays visible.
#[must_use]
pub fn arrange(forest: Vec<TreeNode>, sort: SortMode, filter: FilterMode) -> Vec<TreeNode> {
    let mut roots = if filter == FilterMode::Leaves {
        let mut leaves = Vec::new();
        collect_leaves(&forest, &mut leaves);
        leaves
    } else {
        let mut kept = forest;
        kept.retain(|n| keep_recursive(n, filter));
        kept
    };
    roots.sort_by(|a, b| match sort {
        SortMode::Depth => b
            .rollup
            .max_depth_from_here
            .cmp(&a.rollup.max_depth_from_here),
        SortMode::Tools => b.rollup.total_tools.cmp(&a.rollup.total_tools),
        SortMode::Duration => b.rollup.total_duration_ms.cmp(&a.rollup.total_duration_ms),
        SortMode::Status => status_rank(a).cmp(&status_rank(b)),
    });
    roots
}

fn keep_recursive(node: &TreeNode, filter: FilterMode) -> bool {
    filter.keep(node) || node.children.iter().any(|c| keep_recursive(c, filter))
}

fn collect_leaves(forest: &[TreeNode], out: &mut Vec<TreeNode>) {
    for node in forest {
        if node.children.is_empty() {
            out.push(node.clone());
        } else {
            collect_leaves(&node.children, out);
        }
    }
}

/// Ordering rank: running first, then failed, then completed.
const fn status_rank(n: &TreeNode) -> u8 {
    match n.node.lifecycle {
        NodeLifecycle::Running => 0,
        NodeLifecycle::Failed | NodeLifecycle::TimedOut | NodeLifecycle::Cancelled => 1,
        NodeLifecycle::Completed => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::subagent_tree::build_tree;

    fn node(id: &str, lc: NodeLifecycle) -> SubagentNode {
        SubagentNode {
            node_id: id.to_string(),
            parent_id: None,
            depth: 1,
            root_session: "agent:s".to_string(),
            task: format!("task {id}"),
            model: None,
            lifecycle: lc,
            started_at_ms: 0,
            elapsed_ms: 1000,
            tool_count: 1,
            last_tool: None,
            last_activity: None,
            result_preview: None,
        }
    }

    #[test]
    fn apply_spawned_then_progress_then_settled() {
        let mut nodes = Vec::new();
        apply_event(
            &mut nodes,
            SubagentTreeEvent::Spawned {
                node: node("a", NodeLifecycle::Running),
            },
        );
        assert_eq!(nodes.len(), 1);
        apply_event(
            &mut nodes,
            SubagentTreeEvent::Progress {
                node_id: "a".into(),
                root_session: "agent:s".into(),
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
                root_session: "agent:s".into(),
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
    }

    #[test]
    fn filter_failed_and_sort_status() {
        let flat = vec![
            node("ok", NodeLifecycle::Completed),
            node("bad", NodeLifecycle::Failed),
            node("run", NodeLifecycle::Running),
        ];
        let forest = build_tree(&flat);
        let failed = arrange(forest.clone(), SortMode::Status, FilterMode::Failed);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].node.node_id, "bad");
        // status sort: running first
        let all = arrange(forest, SortMode::Status, FilterMode::All);
        assert_eq!(all[0].node.node_id, "run");
    }

    #[test]
    fn summary_counts_active_and_depth() {
        let flat = vec![
            node("a", NodeLifecycle::Running),
            node("b", NodeLifecycle::Completed),
        ];
        let s = summarize(&flat);
        assert_eq!(s.agents, 2);
        assert_eq!(s.active, 1);
        assert_eq!(s.tools, 2);
        assert_eq!(s.max_depth, 1);
        // depth_counts[1] == 2 (both at depth 1)
        assert_eq!(s.depth_counts.get(1).copied(), Some(2));
    }
}

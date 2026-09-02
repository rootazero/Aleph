//! View-side state logic for the sub-agent tree: sort/filter over the rebuilt
//! forest. No Leptos; host-testable. The merge (`apply_event`), the summary
//! arithmetic (`summarize`), and the forest rebuild (`build_tree`) all live in
//! `aleph_protocol::subagent_tree` — ONE implementation shared by the native
//! server, this WASM panel, and the TUI. Only the sort/filter presentation
//! choices below are panel-specific.

use aleph_protocol::subagent_tree::{NodeLifecycle, TreeNode};

pub use aleph_protocol::subagent_tree::{apply_event, summarize};

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
    use aleph_protocol::subagent_tree::{build_tree, SubagentNode};

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
            child_session: None,
            total_tokens: None,
        }
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

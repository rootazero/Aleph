//! Property-based tests for TaskGraph DAG invariants.
//!
//! Uses proptest to verify structural properties of the task graph
//! hold across a wide range of randomly generated inputs.

use proptest::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;

use super::{
    FileOp, GraphValidationError, Task, TaskGraph, TaskResult, TaskStatus, TaskType,
};

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Generate a valid task ID (non-empty alphanumeric string).
fn arb_task_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,7}".prop_map(|s| s)
}

/// Generate a simple Task with a given ID.
fn make_task(id: &str) -> Task {
    Task::new(
        id,
        format!("task-{id}"),
        TaskType::FileOperation(FileOp::List {
            path: PathBuf::from("/tmp"),
        }),
    )
}

/// Generate a TaskStatus variant (for setting on tasks in graphs).
fn arb_task_status() -> impl Strategy<Value = TaskStatus> {
    prop_oneof![
        Just(TaskStatus::Pending),
        (0.0f32..=1.0).prop_map(|p| TaskStatus::running(p)),
        Just(TaskStatus::completed(TaskResult::default())),
        Just(TaskStatus::failed("test error")),
        Just(TaskStatus::Cancelled),
    ]
}

/// Generate a vector of unique task IDs (1..=max_tasks).
fn arb_unique_task_ids(max_tasks: usize) -> impl Strategy<Value = Vec<String>> {
    proptest::collection::hash_set(arb_task_id(), 1..=max_tasks)
        .prop_map(|set| set.into_iter().collect::<Vec<_>>())
}

/// Generate a TaskGraph with unique tasks and random (potentially cyclic) edges.
fn arb_task_graph(max_tasks: usize, max_edges: usize) -> impl Strategy<Value = TaskGraph> {
    arb_unique_task_ids(max_tasks).prop_flat_map(move |ids| {
        let n = ids.len();
        // Generate random edges as (from_idx, to_idx) pairs.
        let edge_strategy = proptest::collection::vec((0..n, 0..n), 0..=max_edges);
        (Just(ids), edge_strategy)
    }).prop_map(|(ids, edge_indices)| {
        let mut graph = TaskGraph::new("prop-graph", "Property Test Graph");
        for id in &ids {
            graph.add_task(make_task(id));
        }
        for (from_idx, to_idx) in edge_indices {
            if from_idx < ids.len() && to_idx < ids.len() {
                graph.add_dependency(&ids[from_idx], &ids[to_idx]);
            }
        }
        graph
    })
}

/// Generate a valid DAG (no self-loops, no cycles) by only adding edges
/// from lower-index to higher-index tasks.
fn arb_valid_dag(max_tasks: usize, max_edges: usize) -> impl Strategy<Value = TaskGraph> {
    arb_unique_task_ids(max_tasks).prop_flat_map(move |ids| {
        let n = ids.len();
        // Only allow edges from i -> j where i < j (guaranteed acyclic).
        let edge_strategy = if n >= 2 {
            proptest::collection::vec((0..n, 0..n), 0..=max_edges)
                .prop_map(move |pairs| {
                    pairs
                        .into_iter()
                        .filter(|(a, b)| a < b)
                        .collect::<Vec<_>>()
                })
                .boxed()
        } else {
            Just(vec![]).boxed()
        };
        (Just(ids), edge_strategy)
    }).prop_map(|(ids, edge_indices)| {
        let mut graph = TaskGraph::new("valid-dag", "Valid DAG");
        for id in &ids {
            graph.add_task(make_task(id));
        }
        for (from_idx, to_idx) in edge_indices {
            graph.add_dependency(&ids[from_idx], &ids[to_idx]);
        }
        graph
    })
}

// ---------------------------------------------------------------------------
// Property Tests
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Property 1: validate() never panics regardless of input.
    #[test]
    fn validate_never_panics(graph in arb_task_graph(8, 12)) {
        // Just call validate -- we don't care about the result, only that it
        // does not panic or cause undefined behavior.
        let _ = graph.validate();
    }

    // Property 2: A graph with tasks but no edges always validates successfully.
    #[test]
    fn no_edge_graphs_always_valid(ids in arb_unique_task_ids(8)) {
        let mut graph = TaskGraph::new("no-edges", "No Edges");
        for id in &ids {
            graph.add_task(make_task(id));
        }
        prop_assert!(
            graph.validate().is_ok(),
            "Graph with {} tasks and 0 edges should be valid",
            ids.len()
        );
    }

    // Property 3: Self-loops are always detected.
    #[test]
    fn self_loops_detected(id in arb_task_id()) {
        let mut graph = TaskGraph::new("self-loop", "Self Loop");
        graph.add_task(make_task(&id));
        graph.add_dependency(&id, &id);

        let err = graph.validate().unwrap_err();
        prop_assert!(
            matches!(err, GraphValidationError::SelfLoop { .. }),
            "Expected SelfLoop error, got: {err:?}"
        );
    }

    // Property 4: Valid DAGs produce complete topological orders that
    // include every task exactly once.
    #[test]
    fn valid_dag_complete_topo_order(graph in arb_valid_dag(8, 10)) {
        let order = graph.topological_order()
            .expect("Valid DAG should produce topological order");

        // The order must contain exactly all tasks.
        prop_assert_eq!(
            order.len(),
            graph.tasks.len(),
            "Topological order length should match task count"
        );

        // Every task ID must appear exactly once.
        let order_ids: HashSet<&str> = order.iter().map(|t| t.id.as_str()).collect();
        let task_ids: HashSet<&str> = graph.tasks.iter().map(|t| t.id.as_str()).collect();
        prop_assert_eq!(order_ids, task_ids, "Topo order must cover all task IDs");
    }

    // Property 5: Root tasks (from get_root_tasks) have no predecessors.
    #[test]
    fn root_tasks_have_no_predecessors(graph in arb_valid_dag(8, 10)) {
        let roots = graph.get_root_tasks();
        for root in &roots {
            let preds = graph.get_predecessors(&root.id);
            prop_assert!(
                preds.is_empty(),
                "Root task '{}' should have no predecessors, found: {:?}",
                root.id,
                preds
            );
        }
    }

    // Property 6: Leaf tasks (from get_leaf_tasks) have no successors.
    #[test]
    fn leaf_tasks_have_no_successors(graph in arb_valid_dag(8, 10)) {
        let leaves = graph.get_leaf_tasks();
        for leaf in &leaves {
            let succs = graph.get_successors(&leaf.id);
            prop_assert!(
                succs.is_empty(),
                "Leaf task '{}' should have no successors, found: {:?}",
                leaf.id,
                succs
            );
        }
    }

    // Property 7: overall_progress() is always in [0.0, 1.0].
    #[test]
    fn overall_progress_bounded(
        ids in arb_unique_task_ids(8),
        statuses in proptest::collection::vec(arb_task_status(), 1..=8),
    ) {
        let mut graph = TaskGraph::new("progress", "Progress Test");
        for (i, id) in ids.iter().enumerate() {
            let mut task = make_task(id);
            if i < statuses.len() {
                task.status = statuses[i].clone();
            }
            graph.add_task(task);
        }

        let progress = graph.overall_progress();
        prop_assert!(
            (0.0..=1.0).contains(&progress),
            "overall_progress() = {} is out of [0.0, 1.0]",
            progress
        );
    }

    // Property 7b: topological order respects all dependency edges.
    #[test]
    fn topo_order_respects_edges(graph in arb_valid_dag(8, 10)) {
        let order = graph.topological_order()
            .expect("Valid DAG should produce topological order");

        // Build position map.
        let pos: std::collections::HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.as_str(), i))
            .collect();

        for edge in &graph.edges {
            let from_pos = pos.get(edge.from.as_str()).unwrap();
            let to_pos = pos.get(edge.to.as_str()).unwrap();
            prop_assert!(
                from_pos < to_pos,
                "Edge {} -> {} violates topological order (pos {} >= {})",
                edge.from, edge.to, from_pos, to_pos
            );
        }
    }
}

//! DAG-based task scheduler

use std::collections::HashSet;
use tracing::{debug, info};

use super::{SchedulerConfig, TaskScheduler};
use crate::cowork::types::{Task, TaskGraph};

/// DAG-based task scheduler
///
/// Schedules tasks based on dependency graph, executing independent
/// tasks in parallel up to a configured limit.
pub struct DagScheduler {
    config: SchedulerConfig,
    /// Tasks that have been marked as completed
    completed: HashSet<String>,
    /// Tasks that have been marked as failed
    failed: HashSet<String>,
    /// Tasks currently being executed
    running: HashSet<String>,
}

impl DagScheduler {
    /// Create a new DAG scheduler with default configuration
    pub fn new() -> Self {
        Self::with_config(SchedulerConfig::default())
    }

    /// Create a new DAG scheduler with custom configuration
    pub fn with_config(config: SchedulerConfig) -> Self {
        Self {
            config,
            completed: HashSet::new(),
            failed: HashSet::new(),
            running: HashSet::new(),
        }
    }

    /// Check if all dependencies of a task are satisfied
    fn dependencies_satisfied(&self, task: &Task, graph: &TaskGraph) -> bool {
        let predecessors = graph.get_predecessors(&task.id);

        for pred_id in predecessors {
            // Check if predecessor is completed
            if !self.completed.contains(pred_id) {
                // Also check the actual task status in case we missed an update
                if let Some(pred_task) = graph.get_task(pred_id) {
                    if !pred_task.is_completed() {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }

        true
    }

    /// Check if a task should be blocked due to failed dependencies
    fn has_failed_dependency(&self, task: &Task, graph: &TaskGraph) -> bool {
        let predecessors = graph.get_predecessors(&task.id);

        for pred_id in predecessors {
            if self.failed.contains(pred_id) {
                return true;
            }
            if let Some(pred_task) = graph.get_task(pred_id) {
                if pred_task.is_failed() {
                    return true;
                }
            }
        }

        false
    }

    /// Mark a task as currently running
    pub fn mark_running(&mut self, task_id: &str) {
        self.running.insert(task_id.to_string());
        debug!("Task '{}' marked as running", task_id);
    }

    /// Get the number of currently running tasks
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Get available parallelism slots
    pub fn available_slots(&self) -> usize {
        self.config
            .max_parallelism
            .saturating_sub(self.running.len())
    }
}

impl Default for DagScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScheduler for DagScheduler {
    fn next_ready<'a>(&self, graph: &'a TaskGraph) -> Vec<&'a Task> {
        let available = self.available_slots();

        if available == 0 {
            return Vec::new();
        }

        let ready: Vec<&Task> = graph
            .tasks
            .iter()
            // Only pending tasks
            .filter(|t| t.is_pending())
            // Not already running
            .filter(|t| !self.running.contains(&t.id))
            // Not blocked by failed dependency
            .filter(|t| !self.has_failed_dependency(t, graph))
            // All dependencies satisfied
            .filter(|t| self.dependencies_satisfied(t, graph))
            // Take up to available slots
            .take(available)
            .collect();

        if !ready.is_empty() {
            debug!(
                "Scheduler found {} ready tasks: {:?}",
                ready.len(),
                ready.iter().map(|t| &t.id).collect::<Vec<_>>()
            );
        }

        ready
    }

    fn mark_completed(&mut self, task_id: &str) {
        self.running.remove(task_id);
        self.completed.insert(task_id.to_string());
        info!("Task '{}' completed", task_id);
    }

    fn mark_failed(&mut self, task_id: &str, error: &str) {
        self.running.remove(task_id);
        self.failed.insert(task_id.to_string());
        info!("Task '{}' failed: {}", task_id, error);
    }

    fn is_complete(&self, graph: &TaskGraph) -> bool {
        graph.tasks.iter().all(|t| t.is_finished())
    }

    fn reset(&mut self) {
        self.completed.clear();
        self.failed.clear();
        self.running.clear();
        debug!("Scheduler state reset");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{FileOp, TaskResult, TaskStatus, TaskType};
    use std::path::PathBuf;

    fn create_task(id: &str) -> Task {
        Task::new(
            id,
            format!("Task {}", id),
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
    }

    #[test]
    fn test_scheduler_basic() {
        let mut scheduler = DagScheduler::new();
        let mut graph = TaskGraph::new("test", "Test Graph");

        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));

        // a -> b -> c
        graph.add_dependency("a", "b");
        graph.add_dependency("b", "c");

        // Initially only 'a' should be ready
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "a");

        // Mark 'a' as running then completed
        scheduler.mark_running("a");
        assert_eq!(scheduler.running_count(), 1);

        scheduler.mark_completed("a");
        assert_eq!(scheduler.running_count(), 0);

        // Update task status in graph
        graph.get_task_mut("a").unwrap().status = TaskStatus::completed(TaskResult::default());

        // Now 'b' should be ready
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "b");
    }

    #[test]
    fn test_scheduler_parallel() {
        let scheduler = DagScheduler::with_config(SchedulerConfig { max_parallelism: 4 });
        let mut graph = TaskGraph::new("test", "Parallel Test");

        // Four independent tasks
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));

        // All four should be ready at once
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 4);
    }

    #[test]
    fn test_scheduler_parallelism_limit() {
        let scheduler = DagScheduler::with_config(SchedulerConfig { max_parallelism: 2 });
        let mut graph = TaskGraph::new("test", "Limited Parallel Test");

        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));

        // Only 2 should be returned due to limit
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn test_scheduler_failed_dependency() {
        let mut scheduler = DagScheduler::new();
        let mut graph = TaskGraph::new("test", "Failed Dependency Test");

        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_dependency("a", "b");

        // Start and fail 'a'
        scheduler.mark_running("a");
        scheduler.mark_failed("a", "Test failure");

        // Update graph
        graph.get_task_mut("a").unwrap().status = TaskStatus::failed("Test failure");

        // 'b' should not be ready because 'a' failed
        let ready = scheduler.next_ready(&graph);
        assert!(ready.is_empty());
    }

    #[test]
    fn test_scheduler_diamond_dependency() {
        let mut scheduler = DagScheduler::with_config(SchedulerConfig { max_parallelism: 4 });
        let mut graph = TaskGraph::new("test", "Diamond Test");

        //     a
        //    / \
        //   b   c
        //    \ /
        //     d
        graph.add_task(create_task("a"));
        graph.add_task(create_task("b"));
        graph.add_task(create_task("c"));
        graph.add_task(create_task("d"));

        graph.add_dependency("a", "b");
        graph.add_dependency("a", "c");
        graph.add_dependency("b", "d");
        graph.add_dependency("c", "d");

        // Only 'a' should be ready initially
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "a");

        // Complete 'a'
        scheduler.mark_running("a");
        scheduler.mark_completed("a");
        graph.get_task_mut("a").unwrap().status = TaskStatus::completed(TaskResult::default());

        // 'b' and 'c' should be ready in parallel
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 2);

        // Complete 'b' and 'c'
        scheduler.mark_running("b");
        scheduler.mark_running("c");
        scheduler.mark_completed("b");
        scheduler.mark_completed("c");
        graph.get_task_mut("b").unwrap().status = TaskStatus::completed(TaskResult::default());
        graph.get_task_mut("c").unwrap().status = TaskStatus::completed(TaskResult::default());

        // Now 'd' should be ready
        let ready = scheduler.next_ready(&graph);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "d");
    }

    #[test]
    fn test_scheduler_reset() {
        let mut scheduler = DagScheduler::new();

        scheduler.mark_running("a");
        scheduler.mark_completed("a");
        scheduler.mark_failed("b", "error");

        assert_eq!(scheduler.completed.len(), 1);
        assert_eq!(scheduler.failed.len(), 1);

        scheduler.reset();

        assert!(scheduler.completed.is_empty());
        assert!(scheduler.failed.is_empty());
        assert!(scheduler.running.is_empty());
    }
}

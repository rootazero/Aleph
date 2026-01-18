//! Task graph definitions

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::{Task, TaskStatus};

/// A directed acyclic graph of tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraph {
    /// Unique identifier for this graph
    pub id: String,

    /// List of tasks in the graph
    pub tasks: Vec<Task>,

    /// Dependencies between tasks (edges in the DAG)
    pub edges: Vec<TaskDependency>,

    /// Graph metadata
    pub metadata: TaskGraphMeta,
}

/// Dependency relationship between tasks
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDependency {
    /// ID of the predecessor task (must complete first)
    pub from: String,

    /// ID of the successor task (waits for predecessor)
    pub to: String,
}

impl TaskDependency {
    /// Create a new dependency
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

/// Metadata for a task graph
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskGraphMeta {
    /// Human-readable title
    pub title: String,

    /// Creation timestamp (Unix epoch seconds)
    pub created_at: u64,

    /// Estimated total duration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_duration: Option<Duration>,

    /// Original user request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_request: Option<String>,
}

impl TaskGraph {
    /// Create a new task graph
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            tasks: Vec::new(),
            edges: Vec::new(),
            metadata: TaskGraphMeta {
                title: title.into(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                estimated_duration: None,
                original_request: None,
            },
        }
    }

    /// Add a task to the graph
    pub fn add_task(&mut self, task: Task) {
        self.tasks.push(task);
    }

    /// Add a dependency between tasks
    pub fn add_dependency(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.edges.push(TaskDependency::new(from, to));
    }

    /// Get a task by ID
    pub fn get_task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Get a mutable task by ID
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Get all tasks that depend on the given task
    pub fn get_successors(&self, task_id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.from == task_id)
            .map(|e| e.to.as_str())
            .collect()
    }

    /// Get all tasks that the given task depends on
    pub fn get_predecessors(&self, task_id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.to == task_id)
            .map(|e| e.from.as_str())
            .collect()
    }

    /// Get tasks with no dependencies (entry points)
    pub fn get_root_tasks(&self) -> Vec<&Task> {
        let tasks_with_deps: HashSet<&str> = self.edges.iter().map(|e| e.to.as_str()).collect();

        self.tasks
            .iter()
            .filter(|t| !tasks_with_deps.contains(t.id.as_str()))
            .collect()
    }

    /// Get tasks with no successors (exit points)
    pub fn get_leaf_tasks(&self) -> Vec<&Task> {
        let tasks_with_successors: HashSet<&str> =
            self.edges.iter().map(|e| e.from.as_str()).collect();

        self.tasks
            .iter()
            .filter(|t| !tasks_with_successors.contains(t.id.as_str()))
            .collect()
    }

    /// Calculate overall progress (0.0 - 1.0)
    pub fn overall_progress(&self) -> f32 {
        if self.tasks.is_empty() {
            return 1.0;
        }

        let total_progress: f32 = self.tasks.iter().map(|t| t.progress()).sum();
        total_progress / self.tasks.len() as f32
    }

    /// Check if all tasks are finished
    pub fn is_complete(&self) -> bool {
        self.tasks.iter().all(|t| t.is_finished())
    }

    /// Check if any task has failed
    pub fn has_failures(&self) -> bool {
        self.tasks.iter().any(|t| t.is_failed())
    }

    /// Count tasks by status
    pub fn count_by_status(&self) -> TaskCountByStatus {
        let mut counts = TaskCountByStatus::default();
        for task in &self.tasks {
            match task.status {
                TaskStatus::Pending => counts.pending += 1,
                TaskStatus::Running { .. } => counts.running += 1,
                TaskStatus::Completed { .. } => counts.completed += 1,
                TaskStatus::Failed { .. } => counts.failed += 1,
                TaskStatus::Cancelled => counts.cancelled += 1,
            }
        }
        counts
    }

    /// Validate the task graph
    pub fn validate(&self) -> Result<(), GraphValidationError> {
        // Check for empty graph
        if self.tasks.is_empty() {
            return Err(GraphValidationError::EmptyGraph);
        }

        // Build task ID set for quick lookup
        let task_ids: HashSet<&str> = self.tasks.iter().map(|t| t.id.as_str()).collect();

        // Check for duplicate task IDs
        if task_ids.len() != self.tasks.len() {
            return Err(GraphValidationError::DuplicateTaskIds);
        }

        // Validate all edges reference existing tasks
        for edge in &self.edges {
            if !task_ids.contains(edge.from.as_str()) {
                return Err(GraphValidationError::InvalidDependency {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    reason: format!("Task '{}' does not exist", edge.from),
                });
            }
            if !task_ids.contains(edge.to.as_str()) {
                return Err(GraphValidationError::InvalidDependency {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    reason: format!("Task '{}' does not exist", edge.to),
                });
            }
        }

        // Check for self-loops
        for edge in &self.edges {
            if edge.from == edge.to {
                return Err(GraphValidationError::SelfLoop {
                    task_id: edge.from.clone(),
                });
            }
        }

        // Check for cycles using DFS
        if let Some(cycle) = self.detect_cycle() {
            return Err(GraphValidationError::CycleDetected { cycle });
        }

        Ok(())
    }

    /// Detect cycles in the graph using DFS
    fn detect_cycle(&self) -> Option<Vec<String>> {
        // Build adjacency list
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for task in &self.tasks {
            adj.insert(&task.id, Vec::new());
        }
        for edge in &self.edges {
            adj.get_mut(edge.from.as_str()).unwrap().push(&edge.to);
        }

        // Track visited and recursion stack
        let mut visited: HashSet<&str> = HashSet::new();
        let mut rec_stack: HashSet<&str> = HashSet::new();
        let mut path: Vec<String> = Vec::new();

        for task in &self.tasks {
            if !visited.contains(task.id.as_str())
                && self.dfs_detect_cycle(&task.id, &adj, &mut visited, &mut rec_stack, &mut path)
            {
                return Some(path);
            }
        }

        None
    }

    fn dfs_detect_cycle<'a>(
        &self,
        node: &'a str,
        adj: &HashMap<&str, Vec<&'a str>>,
        visited: &mut HashSet<&'a str>,
        rec_stack: &mut HashSet<&'a str>,
        path: &mut Vec<String>,
    ) -> bool {
        visited.insert(node);
        rec_stack.insert(node);
        path.push(node.to_string());

        if let Some(neighbors) = adj.get(node) {
            for &neighbor in neighbors {
                if !visited.contains(neighbor) {
                    if self.dfs_detect_cycle(neighbor, adj, visited, rec_stack, path) {
                        return true;
                    }
                } else if rec_stack.contains(neighbor) {
                    // Found a cycle - add the closing node
                    path.push(neighbor.to_string());
                    return true;
                }
            }
        }

        rec_stack.remove(node);
        path.pop();
        false
    }

    /// Get topological order of tasks
    pub fn topological_order(&self) -> Result<Vec<&Task>, GraphValidationError> {
        self.validate()?;

        // Kahn's algorithm
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

        for task in &self.tasks {
            in_degree.insert(&task.id, 0);
            adj.insert(&task.id, Vec::new());
        }

        for edge in &self.edges {
            *in_degree.get_mut(edge.to.as_str()).unwrap() += 1;
            adj.get_mut(edge.from.as_str()).unwrap().push(&edge.to);
        }

        // Start with nodes having zero in-degree
        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut result: Vec<&Task> = Vec::new();

        while let Some(node) = queue.pop() {
            if let Some(task) = self.get_task(node) {
                result.push(task);
            }

            if let Some(neighbors) = adj.get(node) {
                for &neighbor in neighbors {
                    let deg = in_degree.get_mut(neighbor).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(neighbor);
                    }
                }
            }
        }

        Ok(result)
    }
}

/// Count of tasks by status
#[derive(Debug, Clone, Default)]
pub struct TaskCountByStatus {
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl TaskCountByStatus {
    /// Total number of tasks
    pub fn total(&self) -> usize {
        self.pending + self.running + self.completed + self.failed + self.cancelled
    }
}

/// Errors that can occur during graph validation
#[derive(Debug, Clone, thiserror::Error)]
pub enum GraphValidationError {
    #[error("Task graph is empty")]
    EmptyGraph,

    #[error("Duplicate task IDs found")]
    DuplicateTaskIds,

    #[error("Invalid dependency: {from} -> {to}: {reason}")]
    InvalidDependency {
        from: String,
        to: String,
        reason: String,
    },

    #[error("Self-loop detected on task '{task_id}'")]
    SelfLoop { task_id: String },

    #[error("Cycle detected in task graph: {}", cycle.join(" -> "))]
    CycleDetected { cycle: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{FileOp, TaskType};
    use std::path::PathBuf;

    fn create_file_task(id: &str, name: &str) -> Task {
        Task::new(
            id,
            name,
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        )
    }

    #[test]
    fn test_graph_creation() {
        let mut graph = TaskGraph::new("graph_1", "Test Graph");

        graph.add_task(create_file_task("task_1", "Task 1"));
        graph.add_task(create_file_task("task_2", "Task 2"));
        graph.add_dependency("task_1", "task_2");

        assert_eq!(graph.tasks.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn test_graph_validation_empty() {
        let graph = TaskGraph::new("empty", "Empty Graph");
        assert!(matches!(
            graph.validate(),
            Err(GraphValidationError::EmptyGraph)
        ));
    }

    #[test]
    fn test_graph_validation_invalid_dependency() {
        let mut graph = TaskGraph::new("invalid", "Invalid Graph");
        graph.add_task(create_file_task("task_1", "Task 1"));
        graph.add_dependency("task_1", "nonexistent");

        assert!(matches!(
            graph.validate(),
            Err(GraphValidationError::InvalidDependency { .. })
        ));
    }

    #[test]
    fn test_graph_validation_self_loop() {
        let mut graph = TaskGraph::new("selfloop", "Self Loop Graph");
        graph.add_task(create_file_task("task_1", "Task 1"));
        graph.add_dependency("task_1", "task_1");

        assert!(matches!(
            graph.validate(),
            Err(GraphValidationError::SelfLoop { .. })
        ));
    }

    #[test]
    fn test_graph_validation_cycle() {
        let mut graph = TaskGraph::new("cycle", "Cycle Graph");
        graph.add_task(create_file_task("a", "A"));
        graph.add_task(create_file_task("b", "B"));
        graph.add_task(create_file_task("c", "C"));
        graph.add_dependency("a", "b");
        graph.add_dependency("b", "c");
        graph.add_dependency("c", "a"); // Creates cycle

        assert!(matches!(
            graph.validate(),
            Err(GraphValidationError::CycleDetected { .. })
        ));
    }

    #[test]
    fn test_topological_order() {
        let mut graph = TaskGraph::new("topo", "Topological Graph");
        graph.add_task(create_file_task("a", "A"));
        graph.add_task(create_file_task("b", "B"));
        graph.add_task(create_file_task("c", "C"));
        graph.add_task(create_file_task("d", "D"));

        // a -> b -> d
        // a -> c -> d
        graph.add_dependency("a", "b");
        graph.add_dependency("a", "c");
        graph.add_dependency("b", "d");
        graph.add_dependency("c", "d");

        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 4);

        // a must come before b, c, d
        let pos_a = order.iter().position(|t| t.id == "a").unwrap();
        let pos_b = order.iter().position(|t| t.id == "b").unwrap();
        let pos_c = order.iter().position(|t| t.id == "c").unwrap();
        let pos_d = order.iter().position(|t| t.id == "d").unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn test_root_and_leaf_tasks() {
        let mut graph = TaskGraph::new("rootleaf", "Root Leaf Graph");
        graph.add_task(create_file_task("a", "A"));
        graph.add_task(create_file_task("b", "B"));
        graph.add_task(create_file_task("c", "C"));
        graph.add_dependency("a", "b");
        graph.add_dependency("b", "c");

        let roots = graph.get_root_tasks();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, "a");

        let leaves = graph.get_leaf_tasks();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].id, "c");
    }

    #[test]
    fn test_overall_progress() {
        let mut graph = TaskGraph::new("progress", "Progress Graph");
        graph.add_task(create_file_task("a", "A"));
        graph.add_task(create_file_task("b", "B"));

        assert_eq!(graph.overall_progress(), 0.0);

        graph.tasks[0].status = TaskStatus::running(0.5);
        assert_eq!(graph.overall_progress(), 0.25);

        graph.tasks[0].status = TaskStatus::completed(super::super::TaskResult::default());
        graph.tasks[1].status = TaskStatus::completed(super::super::TaskResult::default());
        assert_eq!(graph.overall_progress(), 1.0);
    }
}

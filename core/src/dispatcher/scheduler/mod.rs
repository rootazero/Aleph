//! Task Scheduler module
//!
//! This module provides DAG-based task scheduling with parallel execution support.

mod dag;

pub use dag::{DagScheduler, ExecutionResult, GraphTaskExecutor};

use crate::dispatcher::agent_types::{Task, TaskGraph};
use super::engine::{MAX_PARALLELISM, MAX_TASK_RETRIES};

/// Configuration for the scheduler
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum number of tasks to run in parallel
    pub max_parallelism: usize,
    /// Maximum number of retry attempts for failed tasks
    pub max_task_retries: u32,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_parallelism: MAX_PARALLELISM,
            max_task_retries: MAX_TASK_RETRIES,
        }
    }
}

/// Trait for task schedulers
///
/// A scheduler determines which tasks are ready to execute based on
/// dependencies and execution status.
pub trait TaskScheduler: Send + Sync {
    /// Get the next batch of tasks that are ready to execute
    ///
    /// Returns tasks that:
    /// - Are in Pending status
    /// - Have all dependencies completed
    /// - Fit within the parallelism limit
    fn next_ready<'a>(&self, graph: &'a TaskGraph) -> Vec<&'a Task>;

    /// Mark a task as completed
    fn mark_completed(&mut self, task_id: &str);

    /// Mark a task as failed
    fn mark_failed(&mut self, task_id: &str, error: &str);

    /// Check if all tasks are finished (completed, failed, or cancelled)
    fn is_complete(&self, graph: &TaskGraph) -> bool;

    /// Reset the scheduler state
    fn reset(&mut self);
}

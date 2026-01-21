//! Task Monitor module
//!
//! This module provides progress tracking and event broadcasting
//! for task execution.

mod events;
mod progress;

pub use events::*;
pub use progress::{CallbackSubscriber, ProgressMonitor};

use crate::dispatcher::agent_types::{Task, TaskGraph, TaskResult};

/// Trait for subscribing to progress events
pub trait ProgressSubscriber: Send + Sync {
    /// Called when a progress event occurs
    fn on_event(&self, event: ProgressEvent);
}

/// Trait for task monitors
///
/// A task monitor tracks execution progress and broadcasts events
/// to subscribers.
pub trait TaskMonitor: Send + Sync {
    /// Called when a task starts executing
    fn on_task_start(&self, task: &Task);

    /// Called when task progress updates
    fn on_progress(&self, task_id: &str, progress: f32, message: Option<&str>);

    /// Called when a task completes successfully
    fn on_task_complete(&self, task: &Task, result: &TaskResult);

    /// Called when a task fails
    fn on_task_failed(&self, task: &Task, error: &str);

    /// Called when a task is cancelled
    fn on_task_cancelled(&self, task: &Task);

    /// Called when the entire graph completes
    fn on_graph_complete(&self, graph: &TaskGraph);
}

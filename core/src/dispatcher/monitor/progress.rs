//! Progress monitor implementation

use std::sync::{Arc, RwLock};
use tracing::{debug, info};

use super::{ProgressEvent, ProgressSubscriber, TaskMonitor};
use crate::dispatcher::agent_types::{Task, TaskGraph, TaskResult};

/// Thread-safe progress monitor with subscriber support
pub struct ProgressMonitor {
    subscribers: RwLock<Vec<Arc<dyn ProgressSubscriber>>>,
}

impl ProgressMonitor {
    /// Create a new progress monitor
    pub fn new() -> Self {
        Self {
            subscribers: RwLock::new(Vec::new()),
        }
    }

    /// Subscribe to progress events
    pub fn subscribe(&self, subscriber: Arc<dyn ProgressSubscriber>) {
        let mut subs = self.subscribers.write().unwrap();
        subs.push(subscriber);
        debug!("New subscriber added, total: {}", subs.len());
    }

    /// Unsubscribe all subscribers
    pub fn clear_subscribers(&self) {
        let mut subs = self.subscribers.write().unwrap();
        subs.clear();
        debug!("All subscribers cleared");
    }

    /// Get the number of subscribers
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.read().unwrap().len()
    }

    /// Broadcast an event to all subscribers
    fn broadcast(&self, event: ProgressEvent) {
        let subs = self.subscribers.read().unwrap();
        for sub in subs.iter() {
            sub.on_event(event.clone());
        }
    }

    /// Update overall graph progress
    pub fn update_graph_progress(&self, graph: &TaskGraph) {
        let counts = graph.count_by_status();
        let event = ProgressEvent::graph_progress(
            &graph.id,
            graph.overall_progress(),
            counts.running,
            counts.pending,
        );
        self.broadcast(event);
    }
}

impl Default for ProgressMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskMonitor for ProgressMonitor {
    fn on_task_start(&self, task: &Task) {
        info!("Task started: {} ({})", task.name, task.id);
        let event = ProgressEvent::task_started(&task.id, &task.name);
        self.broadcast(event);
    }

    fn on_progress(&self, task_id: &str, progress: f32, message: Option<&str>) {
        debug!("Task {} progress: {:.1}%", task_id, progress * 100.0);
        let event = if let Some(msg) = message {
            ProgressEvent::progress_with_message(task_id, progress, msg)
        } else {
            ProgressEvent::progress(task_id, progress)
        };
        self.broadcast(event);
    }

    fn on_task_complete(&self, task: &Task, result: &TaskResult) {
        info!(
            "Task completed: {} ({}) in {:?}",
            task.name, task.id, result.duration
        );
        let event = ProgressEvent::task_completed(&task.id, &task.name, result.clone());
        self.broadcast(event);
    }

    fn on_task_failed(&self, task: &Task, error: &str) {
        info!("Task failed: {} ({}) - {}", task.name, task.id, error);
        let event = ProgressEvent::task_failed(&task.id, &task.name, error);
        self.broadcast(event);
    }

    fn on_task_cancelled(&self, task: &Task) {
        info!("Task cancelled: {} ({})", task.name, task.id);
        let event = ProgressEvent::task_cancelled(&task.id, &task.name);
        self.broadcast(event);
    }

    fn on_graph_complete(&self, graph: &TaskGraph) {
        let counts = graph.count_by_status();
        info!(
            "Graph completed: {} - {} completed, {} failed",
            graph.id, counts.completed, counts.failed
        );
        let event = ProgressEvent::graph_completed(
            &graph.id,
            counts.total(),
            counts.completed,
            counts.failed,
        );
        self.broadcast(event);
    }
}

/// A simple callback-based subscriber
pub struct CallbackSubscriber<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    callback: F,
}

impl<F> CallbackSubscriber<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    /// Create a new callback subscriber
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<F> ProgressSubscriber for CallbackSubscriber<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    fn on_event(&self, event: ProgressEvent) {
        (self.callback)(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{FileOp, TaskType};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn test_progress_monitor_basic() {
        let monitor = ProgressMonitor::new();
        let task = create_task("test_1");

        // No subscribers, should not panic
        monitor.on_task_start(&task);
        monitor.on_progress("test_1", 0.5, None);
        monitor.on_task_complete(&task, &TaskResult::default());
    }

    #[test]
    fn test_progress_monitor_with_subscriber() {
        let monitor = ProgressMonitor::new();
        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = event_count.clone();

        let subscriber = CallbackSubscriber::new(move |_event| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        });

        monitor.subscribe(Arc::new(subscriber));
        assert_eq!(monitor.subscriber_count(), 1);

        let task = create_task("test_1");
        monitor.on_task_start(&task);
        monitor.on_progress("test_1", 0.5, Some("Working..."));
        monitor.on_task_complete(&task, &TaskResult::default());

        // Should have received 3 events
        assert_eq!(event_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_progress_monitor_multiple_subscribers() {
        let monitor = ProgressMonitor::new();
        let event_count_1 = Arc::new(AtomicUsize::new(0));
        let event_count_2 = Arc::new(AtomicUsize::new(0));

        let count_1 = event_count_1.clone();
        let count_2 = event_count_2.clone();

        monitor.subscribe(Arc::new(CallbackSubscriber::new(move |_| {
            count_1.fetch_add(1, Ordering::SeqCst);
        })));

        monitor.subscribe(Arc::new(CallbackSubscriber::new(move |_| {
            count_2.fetch_add(1, Ordering::SeqCst);
        })));

        let task = create_task("test_1");
        monitor.on_task_start(&task);

        // Both subscribers should receive the event
        assert_eq!(event_count_1.load(Ordering::SeqCst), 1);
        assert_eq!(event_count_2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_clear_subscribers() {
        let monitor = ProgressMonitor::new();
        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = event_count.clone();

        monitor.subscribe(Arc::new(CallbackSubscriber::new(move |_| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        })));

        assert_eq!(monitor.subscriber_count(), 1);

        monitor.clear_subscribers();
        assert_eq!(monitor.subscriber_count(), 0);

        // Events should not be received after clearing
        let task = create_task("test_1");
        monitor.on_task_start(&task);
        assert_eq!(event_count.load(Ordering::SeqCst), 0);
    }
}

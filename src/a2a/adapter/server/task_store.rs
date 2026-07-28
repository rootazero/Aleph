use std::collections::HashMap;

use chrono::Utc;

use crate::a2a::domain::{
    A2AError, A2AMessage, A2ATask, Artifact, ListTasksParams, ListTasksResult, TaskState,
    TaskStatus,
};
use crate::a2a::port::{A2AResult, A2ATaskManager};
use crate::sync_primitives::AsyncRwLock;

/// Upper bound on retained tasks. The `A2ATaskManager` trait exposes no
/// remove/prune operation, so without a cap a long-running daemon
/// accumulates one `A2ATask` (full history + artifacts) per request
/// forever — a memory leak and a `DoS` amplifier (distinct `taskId`s pin
/// arbitrary memory). When the cap is reached we evict the oldest
/// *terminal* tasks (active tasks are never evicted).
const MAX_TASKS: usize = 10_000;

/// In-memory implementation of the `A2ATaskManager` port.
///
/// Uses `AsyncRwLock` for concurrent access. Suitable for
/// single-process deployments and testing; production may swap in
/// a database-backed adapter via the same trait.
pub struct TaskStore {
    tasks: AsyncRwLock<HashMap<String, A2ATask>>,
}

/// Evict the oldest terminal tasks until the map is back under `MAX_TASKS`
/// (or no further terminal tasks remain). Only terminal tasks are reclaimed
/// so an in-flight task is never dropped out from under its handler.
fn evict_terminal_tasks(tasks: &mut HashMap<String, A2ATask>) {
    if tasks.len() < MAX_TASKS {
        return;
    }
    let mut terminal: Vec<(String, chrono::DateTime<Utc>)> = tasks
        .iter()
        .filter(|(_, t)| t.status.state.is_terminal())
        .map(|(id, t)| (id.clone(), t.status.timestamp))
        .collect();
    // Oldest first.
    terminal.sort_by_key(|a| a.1);
    // Reclaim down to ~90% capacity to avoid evicting on every insert.
    let target = MAX_TASKS * 9 / 10;
    for (id, _) in terminal {
        if tasks.len() <= target {
            break;
        }
        tasks.remove(&id);
    }
}

impl TaskStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: AsyncRwLock::new(HashMap::new()),
        }
    }
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl A2ATaskManager for TaskStore {
    async fn create_task(&self, task_id: &str, context_id: &str) -> A2AResult<A2ATask> {
        let task = A2ATask::new(task_id, context_id);
        let mut tasks = self.tasks.write().await;
        if tasks.contains_key(task_id) {
            return Err(A2AError::InvalidRequest(format!(
                "Task already exists: {task_id}"
            )));
        }
        evict_terminal_tasks(&mut tasks);
        tasks.insert(task_id.to_string(), task.clone());
        Ok(task)
    }

    async fn get_task(&self, task_id: &str, history_length: Option<usize>) -> A2AResult<A2ATask> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .get(task_id)
            .ok_or_else(|| A2AError::TaskNotFound(task_id.to_string()))?;
        let mut task = task.clone();
        if let Some(limit) = history_length {
            if task.history.len() > limit {
                let start = task.history.len() - limit;
                task.history = task
                    .history
                    .get(start..)
                    .expect("invariant: start < history.len() by check above")
                    .to_vec();
            }
        }
        Ok(task)
    }

    async fn update_status(
        &self,
        task_id: &str,
        state: TaskState,
        message: Option<A2AMessage>,
    ) -> A2AResult<A2ATask> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| A2AError::TaskNotFound(task_id.to_string()))?;
        if !task.status.state.can_transition_to(&state) {
            return Err(A2AError::InvalidRequest(format!(
                "Cannot transition task {} from {:?} to {:?}",
                task_id, task.status.state, state
            )));
        }
        task.status = TaskStatus {
            state,
            message: message.clone(),
            timestamp: Utc::now(),
        };
        if let Some(msg) = message {
            task.history.push(msg);
        }
        Ok(task.clone())
    }

    async fn cancel_task(&self, task_id: &str) -> A2AResult<A2ATask> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| A2AError::TaskNotFound(task_id.to_string()))?;
        if !task.status.state.is_cancelable() {
            return Err(A2AError::TaskNotCancelable(task.status.state));
        }
        task.status = TaskStatus {
            state: TaskState::Canceled,
            message: None,
            timestamp: Utc::now(),
        };
        Ok(task.clone())
    }

    async fn list_tasks(&self, params: ListTasksParams) -> A2AResult<ListTasksResult> {
        let tasks = self.tasks.read().await;
        let mut result: Vec<A2ATask> = tasks
            .values()
            .filter(|t| {
                if let Some(ref ctx) = params.context_id {
                    if t.context_id != *ctx {
                        return false;
                    }
                }
                if let Some(ref states) = params.state_filter {
                    if !states.contains(&t.status.state) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        // Sort by timestamp descending, then by ID ascending for deterministic output
        result.sort_by(|a, b| {
            b.status
                .timestamp
                .cmp(&a.status.timestamp)
                .then_with(|| a.id.cmp(&b.id))
        });
        let limit = params.limit.unwrap_or(100);
        result.truncate(limit);
        Ok(ListTasksResult {
            tasks: result,
            next_cursor: None, // In-memory store doesn't support cursor pagination
        })
    }

    async fn add_artifact(&self, task_id: &str, artifact: Artifact) -> A2AResult<()> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| A2AError::TaskNotFound(task_id.to_string()))?;
        task.artifacts.push(artifact);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::{A2ARole, Part};

    #[tokio::test]
    async fn create_and_get_task() {
        let store = TaskStore::new();
        let task = store.create_task("t1", "ctx1").await.unwrap();
        assert_eq!(task.id, "t1");
        assert_eq!(task.context_id, "ctx1");
        assert_eq!(task.status.state, TaskState::Submitted);

        let fetched = store.get_task("t1", None).await.unwrap();
        assert_eq!(fetched.id, "t1");
        assert_eq!(fetched.context_id, "ctx1");
    }

    #[tokio::test]
    async fn get_nonexistent_task_returns_not_found() {
        let store = TaskStore::new();
        let err = store.get_task("nonexistent", None).await.unwrap_err();
        assert!(matches!(err, A2AError::TaskNotFound(id) if id == "nonexistent"));
    }

    #[tokio::test]
    async fn update_status_transitions() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        let msg = A2AMessage::text(A2ARole::Agent, "Working on it");
        let task = store
            .update_status("t1", TaskState::Working, Some(msg))
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Working);
        assert!(task.status.message.is_some());
        assert_eq!(task.history.len(), 1);

        // Transition to completed without message
        let task = store
            .update_status("t1", TaskState::Completed, None)
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
        assert!(task.status.message.is_none());
        assert_eq!(task.history.len(), 1); // No new message added
    }

    #[tokio::test]
    async fn cancel_cancelable_task() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        let task = store.cancel_task("t1").await.unwrap();
        assert_eq!(task.status.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn cancel_terminal_task_returns_error() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();
        store
            .update_status("t1", TaskState::Completed, None)
            .await
            .unwrap();

        let err = store.cancel_task("t1").await.unwrap_err();
        assert!(matches!(
            err,
            A2AError::TaskNotCancelable(TaskState::Completed)
        ));
    }

    #[tokio::test]
    async fn list_with_context_filter() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx-a").await.unwrap();
        store.create_task("t2", "ctx-b").await.unwrap();
        store.create_task("t3", "ctx-a").await.unwrap();

        let result = store
            .list_tasks(ListTasksParams {
                context_id: Some("ctx-a".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.tasks.len(), 2);
        assert!(result.tasks.iter().all(|t| t.context_id == "ctx-a"));
    }

    #[tokio::test]
    async fn list_with_state_filter() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();
        store.create_task("t2", "ctx1").await.unwrap();
        store
            .update_status("t2", TaskState::Working, None)
            .await
            .unwrap();

        let result = store
            .list_tasks(ListTasksParams {
                state_filter: Some(vec![TaskState::Working]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.tasks.len(), 1);
        assert_eq!(result.tasks[0].id, "t2");
    }

    #[tokio::test]
    async fn add_artifact_to_task() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        let artifact = Artifact {
            artifact_id: "art-1".to_string(),
            kind: "code".to_string(),
            parts: vec![Part::Text {
                text: "fn main() {}".to_string(),
                metadata: None,
            }],
            metadata: None,
        };
        store.add_artifact("t1", artifact).await.unwrap();

        let task = store.get_task("t1", None).await.unwrap();
        assert_eq!(task.artifacts.len(), 1);
        assert_eq!(task.artifacts[0].artifact_id, "art-1");
    }

    #[tokio::test]
    async fn add_artifact_nonexistent_task() {
        let store = TaskStore::new();
        let artifact = Artifact {
            artifact_id: "art-1".to_string(),
            kind: "code".to_string(),
            parts: vec![],
            metadata: None,
        };
        let err = store
            .add_artifact("nonexistent", artifact)
            .await
            .unwrap_err();
        assert!(matches!(err, A2AError::TaskNotFound(_)));
    }

    #[tokio::test]
    async fn history_length_truncation() {
        let store = TaskStore::new();
        store.create_task("t1", "ctx1").await.unwrap();

        // Add 5 messages to history
        for i in 0..5 {
            let msg = A2AMessage::text(A2ARole::Agent, format!("msg-{}", i));
            store
                .update_status("t1", TaskState::Working, Some(msg))
                .await
                .unwrap();
        }

        // Full history
        let task = store.get_task("t1", None).await.unwrap();
        assert_eq!(task.history.len(), 5);

        // Truncated to last 2
        let task = store.get_task("t1", Some(2)).await.unwrap();
        assert_eq!(task.history.len(), 2);
        assert_eq!(task.history[0].text_content(), "msg-3");
        assert_eq!(task.history[1].text_content(), "msg-4");

        // Limit larger than history returns all
        let task = store.get_task("t1", Some(100)).await.unwrap();
        assert_eq!(task.history.len(), 5);
    }

    #[tokio::test]
    async fn list_respects_limit() {
        let store = TaskStore::new();
        for i in 0..10 {
            store.create_task(&format!("t{}", i), "ctx1").await.unwrap();
        }

        let result = store
            .list_tasks(ListTasksParams {
                limit: Some(3),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.tasks.len(), 3);
        assert!(result.next_cursor.is_none());
    }
}

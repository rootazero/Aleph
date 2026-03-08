use crate::a2a::domain::*;

/// Convenience alias for A2A operations
pub type A2AResult<T> = std::result::Result<T, A2AError>;

/// Port for task lifecycle management.
///
/// Defines the contract for creating, querying, updating, and canceling A2A tasks.
/// Adapters (e.g. in-memory store, database-backed store) implement this trait.
#[async_trait::async_trait]
pub trait A2ATaskManager: Send + Sync {
    /// Create a new task in the Submitted state
    async fn create_task(&self, task_id: &str, context_id: &str) -> A2AResult<A2ATask>;

    /// Retrieve a task by ID, optionally limiting history length
    async fn get_task(&self, task_id: &str, history_length: Option<usize>) -> A2AResult<A2ATask>;

    /// Transition a task's state, optionally attaching a message
    async fn update_status(
        &self,
        task_id: &str,
        state: TaskState,
        message: Option<A2AMessage>,
    ) -> A2AResult<A2ATask>;

    /// Cancel a task (must be in a cancelable state)
    async fn cancel_task(&self, task_id: &str) -> A2AResult<A2ATask>;

    /// List tasks with optional filtering and pagination
    async fn list_tasks(&self, params: ListTasksParams) -> A2AResult<ListTasksResult>;

    /// Append an artifact to a task
    async fn add_artifact(&self, task_id: &str, artifact: Artifact) -> A2AResult<()>;
}

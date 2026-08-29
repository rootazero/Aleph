use crate::a2a::domain::{
    A2AError, A2AMessage, A2ATask, Artifact, ListTasksParams, ListTasksResult, TaskState,
};

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

    /// Atomically create-or-claim a task for execution.
    ///
    /// Returns `Ok(true)` if this call created a new task (the caller now
    /// exclusively owns the task in `Working` state). Returns `Ok(false)`
    /// when the task already existed and was concurrently transitioned
    /// into an active state by another caller (the caller MUST treat this
    /// as "another run is in flight" and refuse to start a duplicate).
    ///
    /// Default implementation falls back to the old `create_task` +
    /// `update_status` sequence for adapters that have not yet been
    /// updated; this fallback retains the documented TOCTOU race and
    /// adapters should override this method to close it.
    async fn claim_task(&self, task_id: &str, context_id: &str) -> A2AResult<bool> {
        match self.create_task(task_id, context_id).await {
            Ok(_) => {
                // Best-effort: promote to Working. If a concurrent claim beat
                // us, the task will not be in Submitted and we return false.
                match self.update_status(task_id, TaskState::Working, None).await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
            Err(_) => Ok(false),
        }
    }

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

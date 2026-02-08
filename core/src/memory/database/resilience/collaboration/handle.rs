//! Session Handle
//!
//! Provides the SessionHandle abstraction for controlling subagent sessions.
//! Enables the Handle Reuse pattern for efficient agent collaboration.

use crate::error::AlephError;
use crate::memory::database::resilience::{SessionStatus, SubagentSession};
use crate::memory::database::VectorDatabase;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Result from a subagent task execution
#[derive(Debug, Clone)]
pub struct TaskResult {
    /// The task that was executed
    pub task_prompt: String,

    /// Result content (may be empty for void tasks)
    pub content: String,

    /// Whether the task completed successfully
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Tokens used for this task
    pub tokens_used: u64,

    /// Tool calls made during this task
    pub tool_calls: u64,
}

impl TaskResult {
    /// Create a successful result
    pub fn success(task_prompt: &str, content: &str, tokens_used: u64, tool_calls: u64) -> Self {
        Self {
            task_prompt: task_prompt.to_string(),
            content: content.to_string(),
            success: true,
            error: None,
            tokens_used,
            tool_calls,
        }
    }

    /// Create a failed result
    pub fn failure(task_prompt: &str, error: &str) -> Self {
        Self {
            task_prompt: task_prompt.to_string(),
            content: String::new(),
            success: false,
            error: Some(error.to_string()),
            tokens_used: 0,
            tool_calls: 0,
        }
    }
}

/// Handle state for tracking execution
#[derive(Debug)]
enum HandleState {
    /// Handle is idle, ready for new task
    Idle,

    /// Handle is waiting for task completion
    Running {
        result_rx: oneshot::Receiver<TaskResult>,
    },

    /// Handle has been swapped to disk
    Swapped,

    /// Handle has been closed
    Closed,
}

/// Session Handle for controlling a subagent session
///
/// The SessionHandle provides a high-level interface for:
/// - Waiting for task completion
/// - Continuing with new tasks (handle reuse)
/// - Checking session status
/// - Managing session lifecycle
pub struct SessionHandle {
    /// Session ID
    session_id: String,

    /// Database reference
    db: Arc<VectorDatabase>,

    /// Current handle state
    state: RwLock<HandleState>,

    /// Sender for signaling task completion (held by executor)
    #[allow(dead_code)]
    result_tx: RwLock<Option<oneshot::Sender<TaskResult>>>,
}

impl SessionHandle {
    /// Create a new session handle
    pub fn new(session_id: String, db: Arc<VectorDatabase>) -> Self {
        Self {
            session_id,
            db,
            state: RwLock::new(HandleState::Idle),
            result_tx: RwLock::new(None),
        }
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Check if handle is idle and ready for new task
    pub async fn is_idle(&self) -> bool {
        matches!(*self.state.read().await, HandleState::Idle)
    }

    /// Check if handle is running a task
    pub async fn is_running(&self) -> bool {
        matches!(*self.state.read().await, HandleState::Running { .. })
    }

    /// Check if handle has been swapped to disk
    pub async fn is_swapped(&self) -> bool {
        matches!(*self.state.read().await, HandleState::Swapped)
    }

    /// Start a new task on this handle
    ///
    /// Returns a sender that the executor uses to signal completion.
    pub async fn start_task(&self) -> Result<oneshot::Sender<TaskResult>, AlephError> {
        let mut state = self.state.write().await;

        match *state {
            HandleState::Idle => {
                let (tx, rx) = oneshot::channel();
                *state = HandleState::Running { result_rx: rx };

                // Update session status in database
                self.db
                    .update_session_status(&self.session_id, SessionStatus::Active, None)
                    .await?;

                debug!(session_id = %self.session_id, "Task started on handle");
                Ok(tx)
            }
            HandleState::Running { .. } => Err(AlephError::config(
                "Handle is already running a task".to_string(),
            )),
            HandleState::Swapped => Err(AlephError::config(
                "Handle has been swapped, must restore first".to_string(),
            )),
            HandleState::Closed => Err(AlephError::config("Handle has been closed".to_string())),
        }
    }

    /// Wait for the current task to complete
    pub async fn wait(&self) -> Result<TaskResult, AlephError> {
        let rx = {
            let mut state = self.state.write().await;

            match std::mem::replace(&mut *state, HandleState::Idle) {
                HandleState::Running { result_rx } => result_rx,
                other => {
                    *state = other;
                    return Err(AlephError::config("Handle is not running a task".to_string()));
                }
            }
        };

        let result = rx
            .await
            .map_err(|_| AlephError::config("Task was cancelled".to_string()))?;

        // Update session status and usage
        self.db
            .update_session_status(&self.session_id, SessionStatus::Idle, None)
            .await?;
        self.db
            .update_session_usage(&self.session_id, result.tokens_used, result.tool_calls)
            .await?;

        info!(
            session_id = %self.session_id,
            success = result.success,
            tokens = result.tokens_used,
            "Task completed on handle"
        );

        Ok(result)
    }

    /// Continue with a new task (handle reuse pattern)
    ///
    /// This starts a new task and waits for its completion.
    pub async fn continue_with(
        &self,
        _task_prompt: &str,
    ) -> Result<(TaskResult, oneshot::Sender<TaskResult>), AlephError> {
        // Ensure handle is idle
        if !self.is_idle().await {
            return Err(AlephError::config(
                "Handle must be idle to continue with new task".to_string(),
            ));
        }

        // Start the new task
        let tx = self.start_task().await?;

        // Create a placeholder result (actual result will come from executor)
        let placeholder = TaskResult::success("", "", 0, 0);

        Ok((placeholder, tx))
    }

    /// Mark handle as swapped to disk
    pub async fn mark_swapped(&self) -> Result<(), AlephError> {
        let mut state = self.state.write().await;

        if !matches!(*state, HandleState::Idle) {
            return Err(AlephError::config(
                "Can only swap idle handles".to_string(),
            ));
        }

        *state = HandleState::Swapped;

        self.db
            .update_session_status(&self.session_id, SessionStatus::Swapped, None)
            .await?;

        info!(session_id = %self.session_id, "Handle marked as swapped");
        Ok(())
    }

    /// Restore handle from swapped state
    pub async fn restore(&self) -> Result<(), AlephError> {
        let mut state = self.state.write().await;

        if !matches!(*state, HandleState::Swapped) {
            return Err(AlephError::config("Handle is not swapped".to_string()));
        }

        *state = HandleState::Idle;

        self.db
            .update_session_status(&self.session_id, SessionStatus::Idle, None)
            .await?;

        info!(session_id = %self.session_id, "Handle restored from swap");
        Ok(())
    }

    /// Close the handle
    pub async fn close(&self) -> Result<(), AlephError> {
        let mut state = self.state.write().await;
        *state = HandleState::Closed;

        // Delete session from database
        self.db.delete_session(&self.session_id).await?;

        info!(session_id = %self.session_id, "Handle closed");
        Ok(())
    }

    /// Get session info from database
    pub async fn get_session(&self) -> Result<SubagentSession, AlephError> {
        self.db
            .get_session(&self.session_id)
            .await?
            .ok_or_else(|| AlephError::config(format!("Session not found: {}", self.session_id)))
    }
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle")
            .field("session_id", &self.session_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_result_success() {
        let result = TaskResult::success("test prompt", "test content", 100, 5);
        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.tokens_used, 100);
    }

    #[test]
    fn test_task_result_failure() {
        let result = TaskResult::failure("test prompt", "something went wrong");
        assert!(!result.success);
        assert!(result.error.is_some());
        assert_eq!(result.error.unwrap(), "something went wrong");
    }
}

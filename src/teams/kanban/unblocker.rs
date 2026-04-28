// ---------------------------------------------------------------------------
// KanbanAutoUnblocker — EventHandler for automatic task dependency unblocking
// ---------------------------------------------------------------------------

use async_trait::async_trait;
use std::sync::Arc;

use crate::event::{AlephEvent, EventContext, EventHandler, EventType, HandlerError};
use crate::teams::artifacts::SqliteArtifactStore;

/// Event handler that listens for team task completions and automatically
/// unblocks any dependent tasks that were waiting on the completed task.
pub struct KanbanAutoUnblocker {
    board: Arc<SqliteArtifactStore>,
}

impl KanbanAutoUnblocker {
    pub fn new(board: Arc<SqliteArtifactStore>) -> Self {
        Self { board }
    }
}

#[async_trait]
impl EventHandler for KanbanAutoUnblocker {
    fn name(&self) -> &'static str {
        "KanbanAutoUnblocker"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::TeamTaskCompleted]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        let AlephEvent::TeamTaskCompleted { task_id, .. } = event else {
            return Ok(vec![]);
        };

        match self.board.complete_task(task_id).await {
            Ok(unblocked) => {
                if !unblocked.is_empty() {
                    tracing::debug!(
                        task_id = task_id,
                        unblocked_count = unblocked.len(),
                        "Auto-unblocked dependent tasks after completion"
                    );
                }
                Ok(vec![])
            }
            Err(e) => {
                // If the task wasn't found or couldn't be completed (e.g., already
                // completed), that's fine — we only care about unblocking.
                tracing::debug!(
                    task_id = task_id,
                    error = %e,
                    "Task completion event received but no unblocking needed"
                );
                Ok(vec![])
            }
        }
    }
}

//! Dispatcher handling for workflow `clarify` steps.
//!
//! A clarify step never runs an agent. When the dispatcher selects a ready
//! clarify task it delivers the question to the originating channel and parks
//! the task in [`Paused`](crate::agents::swarm::tasks::CoordTaskStatus::Paused).
//! The task row — with its [`ClarifyTaskMeta`] — is the **durable awaiting
//! record**: it survives a restart with no in-memory state. The inbound router
//! completes it with the user's answer, after which the dumb loop unblocks the
//! dependents on its next tick (R10 — no new scheduler, no reasoning).

use super::TeamDispatcher;
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskUpdate};
use crate::gateway::channel::{ChannelId, OutboundMessage};
use crate::sync_primitives::Arc;
use crate::workflow::clarify::{ClarifyTaskMeta, CLARIFY_OWNER};

impl TeamDispatcher {
    /// Deliver a clarify step's question and park the task awaiting the reply.
    ///
    /// Failure modes all end in a terminal `Failed` status (with a clear reason)
    /// rather than a stalled task, so the DAG never hangs on an unanswerable
    /// clarification: dependents become `Unsatisfiable` and the run terminates.
    pub(crate) async fn handle_clarify_task(self: &Arc<Self>, task: &CoordTask) {
        let Some(meta) = ClarifyTaskMeta::from_metadata(&task.metadata) else {
            self.fail_task(task, "clarify task is missing its clarify metadata")
                .await;
            return;
        };
        let Some(channels) = self.channels.as_ref().and_then(|c| c.get()).cloned() else {
            self.fail_task(
                task,
                "clarify steps require channel delivery, which is not available in this \
                 dispatcher (no channel registry wired or not yet initialised)",
            )
            .await;
            return;
        };
        if meta.channel_id.is_empty() || meta.conversation_id.is_empty() {
            self.fail_task(
                task,
                "clarify step has no originating channel — the workflow run was \
                 started without an interactive channel to reach the user",
            )
            .await;
            return;
        }

        // Claim atomically so two concurrent ticks cannot both deliver. The lock
        // is released once the task is parked; resolution completes it without
        // needing the lock.
        if self
            .coord_store
            .acquire_lock(&task.id, CLARIFY_OWNER)
            .await
            .is_err()
        {
            return; // another tick is already handling this task
        }

        // Park BEFORE delivery: the awaiting record must exist before the user
        // can reply, and `Paused` removes the task from the schedulable set so a
        // racing tick won't re-handle it (only `Pending` is selected).
        if let Err(e) = self
            .coord_store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Paused),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!(task_id = %task.id, error = %e, "dispatcher: failed to park clarify task");
            if let Err(re) = self.coord_store.release_lock(&task.id, CLARIFY_OWNER).await {
                tracing::debug!(task_id = %task.id, error = %re, "dispatcher: clarify lock release failed after park error");
            }
            return;
        }

        // Deliver the question to the originating channel.
        let message = OutboundMessage::text(meta.conversation_id.clone(), meta.rendered_prompt());
        match channels
            .send(&ChannelId::new(&meta.channel_id), message)
            .await
        {
            Ok(_) => {
                tracing::info!(
                    task_id = %task.id,
                    channel = %meta.channel_id,
                    "dispatcher: clarify question delivered — awaiting user reply"
                );
            }
            Err(e) => {
                // Nobody can ever answer — fail so dependents stop waiting.
                tracing::warn!(task_id = %task.id, error = %e, "dispatcher: clarify delivery failed");
                self.fail_task(
                    task,
                    &format!(
                        "clarify: failed to deliver the question to channel '{}': {e}",
                        meta.channel_id
                    ),
                )
                .await;
            }
        }

        if let Err(e) = self.coord_store.release_lock(&task.id, CLARIFY_OWNER).await {
            tracing::debug!(task_id = %task.id, error = %e, "dispatcher: clarify lock release failed after delivery");
        }
    }
}

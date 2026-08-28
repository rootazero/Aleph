use super::{ExecutionEngine, RunRequest};
use crate::gateway::agent_instance::AgentInstance;
use crate::resilience::{AgentTask, Lane, RiskLevel, TaskStatus};
use tracing::warn;

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    pub(super) async fn persist_run_task_started(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
    ) -> bool {
        let Some(db) = self.state_database.as_ref() else {
            return false;
        };

        let metadata_json = serde_json::to_string(&serde_json::json!({
            "run_id": run_id,
            "session_key": request.session_key.to_key_string(),
            "channel_id": request.metadata.get("channel_id"),
            "sender_id": request.metadata.get("sender_id"),
            "conversation_id": request.metadata.get("conversation_id"),
            "source": "gateway_execution_engine"
        }))
        .ok();

        let mut task = AgentTask::new(
            run_id,
            request.session_key.to_key_string(),
            agent.id().to_string(),
            request.input.clone(),
            RiskLevel::High,
        )
        .with_lane(Lane::Main);
        task.metadata_json = metadata_json;

        if let Err(error) = db.insert_agent_task_if_absent(&task).await {
            warn!(
                run_id = %run_id,
                error = %error,
                "Failed to persist execution task"
            );
            return false;
        }

        // Redelivery gate: if a row with this run_id already exists and is in
        // a TERMINAL status (Completed / Failed / Interrupted), this is a
        // stale redelivery of a finished run — do NOT resurrect it to
        // Running, or the admin view flips a completed task back to a stale
        // running marker and the task's status history becomes
        // self-contradictory. A re-`Running` transition on a non-terminal row
        // (e.g. a crash between insert and the first status update) is still
        // applied so the task doesn't stay Pending forever.
        match db.get_agent_task(run_id).await {
            Ok(Some(existing))
                if matches!(
                    existing.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Interrupted
                ) =>
            {
                warn!(
                    run_id = %run_id,
                    status = %existing.status,
                    "redelivery of a terminal-status task; leaving the terminal state intact"
                );
                return true;
            }
            Ok(_) => {}
            Err(error) => {
                warn!(
                    run_id = %run_id,
                    error = %error,
                    "failed to read existing task status before Running transition; continuing"
                );
            }
        }

        if let Err(error) = db.update_task_status(run_id, TaskStatus::Running).await {
            warn!(
                run_id = %run_id,
                error = %error,
                "Failed to mark execution task as running"
            );
        }

        true
    }

    pub(super) async fn persist_run_task_status(&self, run_id: &str, status: TaskStatus) {
        let Some(db) = self.state_database.as_ref() else {
            return;
        };

        if let Err(error) = db.update_task_status(run_id, status).await {
            warn!(
                run_id = %run_id,
                status = %status,
                error = %error,
                "Failed to update execution task status"
            );
        }
    }
}

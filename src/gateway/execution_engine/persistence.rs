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

        if let Err(error) = db.insert_agent_task(&task).await {
            warn!(
                run_id = %run_id,
                error = %error,
                "Failed to persist execution task"
            );
            return false;
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

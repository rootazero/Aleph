//! Recovery Manager
//!
//! Handles task recovery on system startup, implementing the
//! Risk-Aware Recovery strategy from the architecture design.

use crate::error::AlephError;
use crate::resilience::{AgentTask, RiskLevel, TaskStatus};
use crate::resilience::database::StateDatabase;
use crate::sync_primitives::Arc;
use tracing::{info, warn};

use super::shadow_replay::ShadowReplayEngine;

/// Recovery decision for a task
#[derive(Debug, Clone)]
pub enum RecoveryDecision {
    /// Auto-resume: Low risk, can be automatically recovered
    AutoResume {
        task: AgentTask,
        replay_engine: Arc<ShadowReplayEngine>,
    },

    /// Pending confirmation: High risk, needs user approval
    PendingConfirmation { task: AgentTask },

    /// Skip: Task cannot or should not be recovered
    Skip { task_id: String, reason: String },
}

/// Summary of recovery scan results
#[derive(Debug, Clone, Default)]
pub struct RecoverySummary {
    /// Tasks that will be auto-resumed
    pub auto_resume_count: usize,

    /// Tasks pending user confirmation
    pub pending_confirmation_count: usize,

    /// Tasks skipped
    pub skipped_count: usize,

    /// Total tasks scanned
    pub total_count: usize,
}

/// Recovery Manager for handling interrupted tasks on startup
pub struct RecoveryManager {
    db: Arc<StateDatabase>,
    replay_engine: Arc<ShadowReplayEngine>,
}

impl RecoveryManager {
    /// Create a new Recovery Manager
    pub fn new(db: Arc<StateDatabase>) -> Self {
        let replay_engine = Arc::new(ShadowReplayEngine::new(db.clone()));
        Self { db, replay_engine }
    }

    /// Scan for recoverable tasks and make recovery decisions
    ///
    /// Returns a list of recovery decisions based on task risk levels.
    pub async fn scan_recoverable_tasks(&self) -> Result<Vec<RecoveryDecision>, AlephError> {
        let tasks = self.db.get_recoverable_tasks().await?;

        if tasks.is_empty() {
            info!("No recoverable tasks found");
            return Ok(Vec::new());
        }

        info!(
            task_count = tasks.len(),
            "Found recoverable tasks, analyzing..."
        );

        let mut decisions = Vec::with_capacity(tasks.len());

        for task in tasks {
            let decision = self.make_recovery_decision(task).await;
            decisions.push(decision);
        }

        Ok(decisions)
    }

    /// Get a summary of recoverable tasks
    pub async fn get_recovery_summary(&self) -> Result<RecoverySummary, AlephError> {
        let decisions = self.scan_recoverable_tasks().await?;

        let mut summary = RecoverySummary {
            total_count: decisions.len(),
            ..Default::default()
        };

        for decision in &decisions {
            match decision {
                RecoveryDecision::AutoResume { .. } => summary.auto_resume_count += 1,
                RecoveryDecision::PendingConfirmation { .. } => {
                    summary.pending_confirmation_count += 1
                }
                RecoveryDecision::Skip { .. } => summary.skipped_count += 1,
            }
        }

        Ok(summary)
    }

    /// Make a recovery decision for a single task
    async fn make_recovery_decision(&self, task: AgentTask) -> RecoveryDecision {
        // Check if task has traces for replay
        let trace_count = match self.db.get_trace_count(&task.id).await {
            Ok(count) => count,
            Err(e) => {
                warn!(
                    task_id = %task.id,
                    error = %e,
                    "Failed to get trace count, skipping task"
                );
                return RecoveryDecision::Skip {
                    task_id: task.id.clone(),
                    reason: format!("Failed to get traces: {}", e),
                };
            }
        };

        // Skip if no traces (nothing to replay)
        if trace_count == 0 {
            return RecoveryDecision::Skip {
                task_id: task.id.clone(),
                reason: "No execution traces available".to_string(),
            };
        }

        // Decision based on risk level
        match task.risk_level {
            RiskLevel::Low => {
                info!(
                    task_id = %task.id,
                    traces = trace_count,
                    "Task eligible for auto-resume"
                );
                RecoveryDecision::AutoResume {
                    task,
                    replay_engine: self.replay_engine.clone(),
                }
            }
            RiskLevel::High => {
                info!(
                    task_id = %task.id,
                    traces = trace_count,
                    "High-risk task pending user confirmation"
                );
                RecoveryDecision::PendingConfirmation { task }
            }
        }
    }

    /// Execute auto-resume for a task
    ///
    /// This loads the task's traces and prepares it for continuation.
    pub async fn execute_auto_resume(
        &self,
        task_id: &str,
    ) -> Result<super::shadow_replay::ReplayResult, AlephError> {
        // Update task status to running
        self.db
            .update_task_status(task_id, TaskStatus::Running)
            .await?;

        // Perform shadow replay
        let result = self.replay_engine.replay_task(task_id).await?;

        info!(
            task_id = %task_id,
            messages = result.messages.len(),
            last_step = result.last_step,
            "Task replay completed, ready for continuation"
        );

        Ok(result)
    }

    /// Confirm and resume a high-risk task
    pub async fn confirm_and_resume(
        &self,
        task_id: &str,
    ) -> Result<super::shadow_replay::ReplayResult, AlephError> {
        // Same as auto-resume, but called after user confirmation
        self.execute_auto_resume(task_id).await
    }

    /// Dismiss a pending task (user chose not to resume)
    pub async fn dismiss_task(&self, task_id: &str) -> Result<(), AlephError> {
        self.db
            .update_task_status(task_id, TaskStatus::Failed)
            .await?;
        info!(task_id = %task_id, "Task dismissed by user");
        Ok(())
    }

    /// Get the replay engine for advanced usage
    pub fn replay_engine(&self) -> &Arc<ShadowReplayEngine> {
        &self.replay_engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recovery_summary_default() {
        let summary = RecoverySummary::default();
        assert_eq!(summary.total_count, 0);
        assert_eq!(summary.auto_resume_count, 0);
        assert_eq!(summary.pending_confirmation_count, 0);
        assert_eq!(summary.skipped_count, 0);
    }
}

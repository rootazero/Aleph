//! `TaskReadArtifactTool` — read artifacts submitted for a task.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, TaskArtifact};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for reading task artifacts.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskReadArtifactArgs {
    /// The task whose artifacts to read
    pub task_id: String,
    /// If provided, read a single artifact by ID; otherwise read all for the task
    #[serde(default)]
    pub artifact_id: Option<String>,
}

/// Output from `task_read_artifact`.
#[derive(Debug, Clone, Serialize)]
pub struct TaskReadArtifactOutput {
    pub artifacts: Vec<TaskArtifact>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that reads artifacts submitted for a task.
#[derive(Clone)]
pub struct TaskReadArtifactTool {
    store: Arc<dyn ArtifactStore>,
}

impl TaskReadArtifactTool {
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AlephTool for TaskReadArtifactTool {
    const NAME: &'static str = "task_read_artifact";
    const DESCRIPTION: &'static str = "Read artifacts submitted for a task";

    type Args = TaskReadArtifactArgs;
    type Output = TaskReadArtifactOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if let Some(ref artifact_id) = args.artifact_id {
            debug!(
                task_id = %args.task_id,
                artifact_id = %artifact_id,
                "task_read_artifact: reading single artifact"
            );

            let artifact = self
                .store
                .get_artifact(artifact_id)
                .await
                .map_err(|e| AlephError::other(format!("Failed to read artifact: {e}")))?
                .ok_or_else(|| AlephError::other(format!("Artifact '{artifact_id}' not found")))?;

            // Verify the artifact belongs to the requested task
            if artifact.task_id != args.task_id {
                return Err(AlephError::other(format!(
                    "Artifact '{}' does not belong to task '{}'",
                    artifact_id, args.task_id
                )));
            }

            Ok(TaskReadArtifactOutput {
                artifacts: vec![artifact],
            })
        } else {
            debug!(
                task_id = %args.task_id,
                "task_read_artifact: reading all artifacts for task"
            );

            let artifacts = self
                .store
                .get_artifacts_for_task(&args.task_id)
                .await
                .map_err(|e| AlephError::other(format!("Failed to read artifacts: {e}")))?;

            Ok(TaskReadArtifactOutput { artifacts })
        }
    }
}

//! TaskSubmitTool — submit a structured artifact as the output of a task.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for submitting a task artifact.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskSubmitArgs {
    /// The task this artifact belongs to
    pub task_id: String,
    /// Kind of artifact being submitted
    pub artifact_type: ArtifactType,
    /// Short title for the artifact
    pub title: String,
    /// Markdown content body
    pub content: String,
    /// Arbitrary structured metadata (optional)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Output from task_submit.
#[derive(Debug, Clone, Serialize)]
pub struct TaskSubmitOutput {
    pub artifact_id: String,
    pub task_id: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that submits a structured artifact (report, code, review, discovery,
/// challenge) as the output of a task.
#[derive(Clone)]
pub struct TaskSubmitTool {
    store: Arc<dyn ArtifactStore>,
    current_agent_id: String,
}

impl TaskSubmitTool {
    pub fn new(store: Arc<dyn ArtifactStore>, current_agent_id: String) -> Self {
        Self {
            store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TaskSubmitTool {
    const NAME: &'static str = "task_submit";
    const DESCRIPTION: &'static str =
        "Submit a structured artifact (report, code, review, discovery, challenge) \
         as the output of a task";

    type Args = TaskSubmitArgs;
    type Output = TaskSubmitOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "task_submit(task_id='task-1', artifact_type='report', title='Analysis Report', content='# Findings\\n...')".to_string(),
            "task_submit(task_id='task-2', artifact_type='code', title='Implementation', content='```rust\\nfn main() {}\\n```')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            task_id = %args.task_id,
            artifact_type = %args.artifact_type.as_str(),
            title = %args.title,
            agent_id = %self.current_agent_id,
            "task_submit: creating artifact"
        );

        let artifact = self
            .store
            .create_artifact(NewArtifact {
                task_id: args.task_id.clone(),
                agent_id: self.current_agent_id.clone(),
                artifact_type: args.artifact_type,
                title: args.title,
                content: args.content,
                metadata: args.metadata,
            })
            .await
            .map_err(|e| {
                AlephError::other(format!("Failed to create artifact: {e}"))
            })?;

        Ok(TaskSubmitOutput {
            artifact_id: artifact.id,
            task_id: artifact.task_id,
            message: format!(
                "Artifact '{}' submitted successfully for task '{}'",
                artifact.title, args.task_id
            ),
        })
    }
}

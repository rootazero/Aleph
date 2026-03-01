//! PoeLoopCallback - Artifact tracking callback for POE worker execution.

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::agent_loop::{
    Action, ActionResult, GuardViolation, LoopCallback, LoopState, Thinking,
};
use crate::agent_loop::decision::QuestionGroup;
use crate::poe::types::{Artifact, ChangeType, StepLog};

// ============================================================================
// PoeLoopCallback - Artifact Tracking Callback
// ============================================================================

/// Callback implementation for POE worker execution.
///
/// This callback tracks file artifacts created or modified during execution
/// by monitoring tool calls for file operations.
pub(crate) struct PoeLoopCallback {
    /// Artifacts produced during execution
    artifacts: Arc<RwLock<Vec<Artifact>>>,
    /// Execution logs
    execution_log: Arc<RwLock<Vec<StepLog>>>,
    /// Workspace root for relative path calculation
    #[allow(dead_code)] // Architecture reserve: will convert absolute paths to workspace-relative paths
    workspace: PathBuf,
    /// Step counter for logging
    step_counter: Arc<RwLock<u32>>,
}

impl PoeLoopCallback {
    /// Create a new PoeLoopCallback.
    pub(crate) fn new(
        artifacts: Arc<RwLock<Vec<Artifact>>>,
        execution_log: Arc<RwLock<Vec<StepLog>>>,
        workspace: PathBuf,
    ) -> Self {
        Self {
            artifacts,
            execution_log,
            workspace,
            step_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Extract file path from tool arguments.
    pub(crate) fn extract_file_path(arguments: &Value) -> Option<PathBuf> {
        // Try common argument names for file paths
        arguments
            .get("path")
            .or_else(|| arguments.get("file_path"))
            .or_else(|| arguments.get("file"))
            .or_else(|| arguments.get("target"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    /// Compute SHA-256 hash of content.
    pub(crate) fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Determine change type from tool name.
    pub(crate) fn change_type_from_tool(tool_name: &str, arguments: &Value) -> ChangeType {
        let tool_lower = tool_name.to_lowercase();

        // Check for explicit operation field
        if let Some(op) = arguments.get("operation").and_then(|v| v.as_str()) {
            let op_lower = op.to_lowercase();
            if op_lower.contains("delete") || op_lower.contains("remove") {
                return ChangeType::Deleted;
            }
            if op_lower.contains("create") || op_lower.contains("mkdir") {
                return ChangeType::Created;
            }
        }

        // Infer from tool name
        if tool_lower.contains("write") || tool_lower.contains("create") {
            ChangeType::Created
        } else if tool_lower.contains("edit") || tool_lower.contains("modify") || tool_lower.contains("update") {
            ChangeType::Modified
        } else if tool_lower.contains("delete") || tool_lower.contains("remove") {
            ChangeType::Deleted
        } else {
            ChangeType::Modified
        }
    }
}

#[async_trait]
impl LoopCallback for PoeLoopCallback {
    async fn on_loop_start(&self, _state: &LoopState) {
        // Reset step counter
        *self.step_counter.write().await = 0;
    }

    async fn on_step_start(&self, _step: usize) {}

    async fn on_thinking_start(&self, _step: usize) {}

    async fn on_thinking_done(&self, _thinking: &Thinking) {}

    async fn on_action_start(&self, _action: &Action) {}

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        // Track file artifacts from write/edit tool calls
        if let Action::ToolCall {
            tool_name,
            arguments,
        } = action
        {
            // Check if this is a file operation tool
            let is_file_op = matches!(
                tool_name.to_lowercase().as_str(),
                "write_file" | "edit_file" | "write" | "edit" | "create_file"
                    | "file_ops" | "delete_file" | "remove_file"
            );

            if is_file_op {
                if let Some(path) = Self::extract_file_path(arguments) {
                    // Determine change type
                    let change_type = Self::change_type_from_tool(tool_name, arguments);

                    // Compute content hash if available
                    let content_hash = if let ActionResult::ToolSuccess { output, .. } = result {
                        // Try to get content from result or arguments
                        let content = arguments
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !content.is_empty() {
                            Self::compute_hash(content)
                        } else {
                            // Hash the output as fallback
                            Self::compute_hash(&output.to_string())
                        }
                    } else {
                        "unknown".to_string()
                    };

                    // Create artifact
                    let artifact = Artifact::new(path, change_type, content_hash);

                    // Add to artifacts list
                    self.artifacts.write().await.push(artifact);
                }
            }
        }

        // Log the step (acquire locks sequentially, not nested)
        let (_current_step, log_entry) = {
            let mut step_id = self.step_counter.write().await;
            let current = *step_id;
            *step_id += 1;
            let entry = StepLog::new(
                current,
                action.action_type(),
                result.summary(),
                0, // Duration tracked elsewhere
            );
            (current, entry)
        };
        // step_counter lock released before acquiring execution_log lock
        self.execution_log.write().await.push(log_entry);
    }

    async fn on_confirmation_required(&self, _tool_name: &str, _arguments: &Value) -> bool {
        // POE worker auto-confirms (the POE framework handles validation)
        true
    }

    async fn on_user_input_required(
        &self,
        _question: &str,
        _options: Option<&[String]>,
    ) -> String {
        // POE worker returns a default response
        // Real user interaction should be handled by the orchestrator
        "continue".to_string()
    }

    async fn on_user_multigroup_required(
        &self,
        _question: &str,
        _groups: &[QuestionGroup],
    ) -> String {
        "{\"default\":\"ok\"}".to_string()
    }

    async fn on_guard_triggered(&self, _violation: &GuardViolation) {}

    async fn on_complete(&self, _summary: &str) {}

    async fn on_failed(&self, _reason: &str) {}
}

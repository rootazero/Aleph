//! Result Collector for Sub-Agent Tool Aggregation
//!
//! Aggregates all tool executions and artifacts from sub-agent runs.
//! Inspired by OpenCode's result aggregation pattern.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::coordinator::{ToolCallState, ToolCallStatus, ToolCallSummary};
use super::traits::Artifact;

/// Maximum length for output preview
const OUTPUT_PREVIEW_MAX_LEN: usize = 200;

/// Record of a tool call during sub-agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectedToolCall {
    /// Unique tool call ID
    pub id: String,
    /// Tool name
    pub tool_name: String,
    /// Tool arguments
    pub arguments: Value,
    /// Current status
    pub status: CollectedToolStatus,
    /// Completion title (for UI display)
    pub title: Option<String>,
    /// When the call started
    #[serde(skip)]
    pub started_at: Option<Instant>,
    /// When the call completed
    #[serde(skip)]
    pub completed_at: Option<Instant>,
}

/// Status of a collected tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CollectedToolStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed { output_preview: String },
    #[serde(rename = "error")]
    Failed { error: String },
}

impl CollectedToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "error",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

impl From<ToolCallStatus> for CollectedToolStatus {
    fn from(status: ToolCallStatus) -> Self {
        match status {
            ToolCallStatus::Pending => Self::Pending,
            ToolCallStatus::Running => Self::Running,
            ToolCallStatus::Completed { output_preview } => Self::Completed { output_preview },
            ToolCallStatus::Failed { error } => Self::Failed { error },
        }
    }
}

/// Result Collector
///
/// Aggregates all tool calls and artifacts made during sub-agent execution.
pub struct ResultCollector {
    /// Tool call records indexed by request_id
    tool_records: RwLock<HashMap<String, Vec<CollectedToolCall>>>,
    /// Artifacts indexed by request_id
    artifacts: RwLock<HashMap<String, Vec<Artifact>>>,
}

impl ResultCollector {
    /// Create a new ResultCollector
    pub fn new() -> Self {
        Self {
            tool_records: RwLock::new(HashMap::new()),
            artifacts: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize collections for a new request
    pub async fn init_request(&self, request_id: &str) {
        {
            let mut records = self.tool_records.write().await;
            records.insert(request_id.to_string(), Vec::new());
        }
        {
            let mut artifacts = self.artifacts.write().await;
            artifacts.insert(request_id.to_string(), Vec::new());
        }
        debug!(request_id = %request_id, "Initialized result collection");
    }

    /// Record a tool call start
    pub async fn record_tool_start(
        &self,
        request_id: &str,
        call_id: &str,
        tool_name: &str,
        arguments: Value,
    ) {
        let record = CollectedToolCall {
            id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments,
            status: CollectedToolStatus::Running,
            title: None,
            started_at: Some(Instant::now()),
            completed_at: None,
        };

        let mut records = self.tool_records.write().await;
        if let Some(calls) = records.get_mut(request_id) {
            calls.push(record);
            debug!(
                request_id = %request_id,
                call_id = %call_id,
                tool = %tool_name,
                "Recorded tool call start"
            );
        } else {
            // Auto-initialize if not exists
            records.insert(request_id.to_string(), vec![record]);
        }
    }

    /// Update tool call status
    pub async fn update_tool_status(
        &self,
        request_id: &str,
        call_id: &str,
        status: CollectedToolStatus,
        title: Option<String>,
    ) {
        let mut records = self.tool_records.write().await;
        if let Some(calls) = records.get_mut(request_id) {
            if let Some(call) = calls.iter_mut().find(|c| c.id == call_id) {
                call.status = status;
                call.title = title;
                if call.status.is_terminal() {
                    call.completed_at = Some(Instant::now());
                }
                debug!(
                    request_id = %request_id,
                    call_id = %call_id,
                    status = %call.status.as_str(),
                    "Updated tool call status"
                );
            }
        }
    }

    /// Update tool status from coordinator status
    pub async fn update_from_coordinator_status(
        &self,
        request_id: &str,
        call_id: &str,
        status: ToolCallStatus,
    ) {
        let collected_status = CollectedToolStatus::from(status);
        self.update_tool_status(request_id, call_id, collected_status, None)
            .await;
    }

    /// Record an artifact
    pub async fn record_artifact(&self, request_id: &str, artifact: Artifact) {
        let mut artifacts = self.artifacts.write().await;
        if let Some(list) = artifacts.get_mut(request_id) {
            list.push(artifact.clone());
            debug!(
                request_id = %request_id,
                artifact_type = %artifact.artifact_type,
                path = %artifact.path,
                "Recorded artifact"
            );
        } else {
            artifacts.insert(request_id.to_string(), vec![artifact]);
        }
    }

    /// Get summary for a request (OpenCode-style)
    pub async fn get_summary(&self, request_id: &str) -> Vec<ToolCallSummary> {
        let records = self.tool_records.read().await;
        records
            .get(request_id)
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| ToolCallSummary {
                        id: call.id.clone(),
                        tool: call.tool_name.clone(),
                        state: ToolCallState {
                            status: call.status.as_str().to_string(),
                            title: call.title.clone(),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all artifacts for a request
    pub async fn get_artifacts(&self, request_id: &str) -> Vec<Artifact> {
        let artifacts = self.artifacts.read().await;
        artifacts.get(request_id).cloned().unwrap_or_default()
    }

    /// Get artifacts grouped by type
    pub async fn get_artifacts_by_type(&self, request_id: &str) -> HashMap<String, Vec<Artifact>> {
        let artifacts = self.get_artifacts(request_id).await;
        let mut grouped: HashMap<String, Vec<Artifact>> = HashMap::new();

        for artifact in artifacts {
            grouped
                .entry(artifact.artifact_type.clone())
                .or_default()
                .push(artifact);
        }

        grouped
    }

    /// Get all tool calls for a request
    pub async fn get_tool_calls(&self, request_id: &str) -> Vec<CollectedToolCall> {
        let records = self.tool_records.read().await;
        records.get(request_id).cloned().unwrap_or_default()
    }

    /// Get completed tool count for a request
    pub async fn get_completed_count(&self, request_id: &str) -> usize {
        let records = self.tool_records.read().await;
        records
            .get(request_id)
            .map(|calls| {
                calls
                    .iter()
                    .filter(|c| c.status.is_terminal())
                    .count()
            })
            .unwrap_or(0)
    }

    /// Get running tool count for a request
    pub async fn get_running_count(&self, request_id: &str) -> usize {
        let records = self.tool_records.read().await;
        records
            .get(request_id)
            .map(|calls| {
                calls
                    .iter()
                    .filter(|c| matches!(c.status, CollectedToolStatus::Running))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Get total tool count for a request
    pub async fn get_total_count(&self, request_id: &str) -> usize {
        let records = self.tool_records.read().await;
        records.get(request_id).map(|c| c.len()).unwrap_or(0)
    }

    /// Check if all tools have completed (empty request is considered complete)
    pub async fn all_completed(&self, request_id: &str) -> bool {
        let records = self.tool_records.read().await;
        records
            .get(request_id)
            .map(|calls| {
                // Empty list means no tools to wait for - considered complete
                calls.is_empty() || calls.iter().all(|c| c.status.is_terminal())
            })
            .unwrap_or(true)
    }

    /// Clean up completed request data
    pub async fn cleanup(&self, request_id: &str) {
        {
            let mut records = self.tool_records.write().await;
            records.remove(request_id);
        }
        {
            let mut artifacts = self.artifacts.write().await;
            artifacts.remove(request_id);
        }
        info!(request_id = %request_id, "Cleaned up result collection");
    }

    /// Check if request exists
    pub async fn has_request(&self, request_id: &str) -> bool {
        let records = self.tool_records.read().await;
        records.contains_key(request_id)
    }

    /// Get statistics
    pub async fn get_stats(&self) -> CollectorStats {
        let records = self.tool_records.read().await;
        let artifacts = self.artifacts.read().await;

        let total_tool_calls: usize = records.values().map(|v| v.len()).sum();
        let total_artifacts: usize = artifacts.values().map(|v| v.len()).sum();

        CollectorStats {
            tracked_requests: records.len(),
            total_tool_calls,
            total_artifacts,
        }
    }
}

impl Default for ResultCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about collector state
#[derive(Debug, Clone, Serialize)]
pub struct CollectorStats {
    pub tracked_requests: usize,
    pub total_tool_calls: usize,
    pub total_artifacts: usize,
}

/// Helper function to truncate output for preview
pub fn truncate_for_preview(output: &str) -> String {
    if output.len() <= OUTPUT_PREVIEW_MAX_LEN {
        output.to_string()
    } else {
        format!("{}...", &output[..OUTPUT_PREVIEW_MAX_LEN])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_collector_creation() {
        let collector = ResultCollector::new();
        let stats = collector.get_stats().await;
        assert_eq!(stats.tracked_requests, 0);
        assert_eq!(stats.total_tool_calls, 0);
    }

    #[tokio::test]
    async fn test_init_request() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        assert!(collector.has_request("req-1").await);
        assert_eq!(collector.get_total_count("req-1").await, 0);
    }

    #[tokio::test]
    async fn test_record_tool_start() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        collector
            .record_tool_start("req-1", "call-1", "bash", json!({"command": "ls"}))
            .await;

        assert_eq!(collector.get_total_count("req-1").await, 1);
        assert_eq!(collector.get_running_count("req-1").await, 1);
        assert_eq!(collector.get_completed_count("req-1").await, 0);
    }

    #[tokio::test]
    async fn test_update_tool_status() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        collector
            .record_tool_start("req-1", "call-1", "bash", json!({}))
            .await;

        collector
            .update_tool_status(
                "req-1",
                "call-1",
                CollectedToolStatus::Completed {
                    output_preview: "Success".to_string(),
                },
                Some("List files".to_string()),
            )
            .await;

        assert_eq!(collector.get_completed_count("req-1").await, 1);
        assert!(collector.all_completed("req-1").await);

        let summary = collector.get_summary("req-1").await;
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].state.status, "completed");
        assert_eq!(summary[0].state.title, Some("List files".to_string()));
    }

    #[tokio::test]
    async fn test_record_artifact() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        let artifact = Artifact::file("/tmp/output.txt");
        collector.record_artifact("req-1", artifact).await;

        let artifacts = collector.get_artifacts("req-1").await;
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].path, "/tmp/output.txt");
    }

    #[tokio::test]
    async fn test_get_artifacts_by_type() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        collector
            .record_artifact("req-1", Artifact::file("/tmp/file1.txt"))
            .await;
        collector
            .record_artifact("req-1", Artifact::file("/tmp/file2.txt"))
            .await;
        collector
            .record_artifact("req-1", Artifact::url("https://example.com"))
            .await;

        let grouped = collector.get_artifacts_by_type("req-1").await;
        assert_eq!(grouped.get("file").map(|v| v.len()), Some(2));
        assert_eq!(grouped.get("url").map(|v| v.len()), Some(1));
    }

    #[tokio::test]
    async fn test_get_summary() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        collector
            .record_tool_start("req-1", "call-1", "glob", json!({}))
            .await;
        collector
            .record_tool_start("req-1", "call-2", "read", json!({}))
            .await;
        collector
            .record_tool_start("req-1", "call-3", "edit", json!({}))
            .await;

        collector
            .update_tool_status(
                "req-1",
                "call-1",
                CollectedToolStatus::Completed {
                    output_preview: "Found files".to_string(),
                },
                None,
            )
            .await;
        collector
            .update_tool_status(
                "req-1",
                "call-2",
                CollectedToolStatus::Completed {
                    output_preview: "Read content".to_string(),
                },
                None,
            )
            .await;
        collector
            .update_tool_status(
                "req-1",
                "call-3",
                CollectedToolStatus::Failed {
                    error: "Permission denied".to_string(),
                },
                None,
            )
            .await;

        let summary = collector.get_summary("req-1").await;
        assert_eq!(summary.len(), 3);

        // Check ordering (by insertion order)
        assert_eq!(summary[0].tool, "glob");
        assert_eq!(summary[1].tool, "read");
        assert_eq!(summary[2].tool, "edit");

        // Check statuses
        assert_eq!(summary[0].state.status, "completed");
        assert_eq!(summary[1].state.status, "completed");
        assert_eq!(summary[2].state.status, "error");
    }

    #[tokio::test]
    async fn test_cleanup() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        collector
            .record_tool_start("req-1", "call-1", "bash", json!({}))
            .await;
        collector
            .record_artifact("req-1", Artifact::file("/tmp/test.txt"))
            .await;

        assert!(collector.has_request("req-1").await);

        collector.cleanup("req-1").await;

        assert!(!collector.has_request("req-1").await);
        assert_eq!(collector.get_artifacts("req-1").await.len(), 0);
    }

    #[tokio::test]
    async fn test_truncate_for_preview() {
        let short = "Hello";
        assert_eq!(truncate_for_preview(short), "Hello");

        let long = "a".repeat(300);
        let preview = truncate_for_preview(&long);
        assert!(preview.len() < 210);
        assert!(preview.ends_with("..."));
    }

    #[tokio::test]
    async fn test_all_completed() {
        let collector = ResultCollector::new();
        collector.init_request("req-1").await;

        // Empty request is considered all completed
        assert!(collector.all_completed("req-1").await);

        collector
            .record_tool_start("req-1", "call-1", "bash", json!({}))
            .await;

        // Running tool means not all completed
        assert!(!collector.all_completed("req-1").await);

        collector
            .update_tool_status(
                "req-1",
                "call-1",
                CollectedToolStatus::Completed {
                    output_preview: "Done".to_string(),
                },
                None,
            )
            .await;

        // Now all completed
        assert!(collector.all_completed("req-1").await);
    }

    #[tokio::test]
    async fn test_auto_init_on_record() {
        let collector = ResultCollector::new();

        // Record without init
        collector
            .record_tool_start("req-1", "call-1", "bash", json!({}))
            .await;

        // Should auto-initialize
        assert!(collector.has_request("req-1").await);
        assert_eq!(collector.get_total_count("req-1").await, 1);
    }
}

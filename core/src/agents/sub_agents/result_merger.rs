//! Result Merger for Sub-Agent Integration
//!
//! Utilities for merging sub-agent results back into the main agent loop state.
//! This module provides helpers for:
//! - Parsing DelegateResult from tool output
//! - Extracting artifacts for the main session
//! - Converting sub-agent tool calls to step records

use serde_json::Value;
use tracing::debug;

use super::delegate_tool::{ArtifactInfo, DelegateResult, ToolCallInfo};
use super::traits::{Artifact, ToolCallRecord};

/// Result of merging a delegate result
#[derive(Debug, Clone, Default)]
pub struct MergedResult {
    /// Summary to include in observation
    pub summary: String,
    /// Artifacts to add to main session
    pub artifacts: Vec<Artifact>,
    /// Tool calls made by sub-agent (for history/logging)
    pub tool_calls: Vec<ToolCallRecord>,
    /// Whether the delegation was successful
    pub success: bool,
    /// Error message if delegation failed
    pub error: Option<String>,
}

impl MergedResult {
    /// Create from a successful delegation
    pub fn success(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            success: true,
            ..Default::default()
        }
    }

    /// Create from a failed delegation
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Add artifacts
    pub fn with_artifacts(mut self, artifacts: Vec<Artifact>) -> Self {
        self.artifacts = artifacts;
        self
    }

    /// Add tool calls
    pub fn with_tool_calls(mut self, calls: Vec<ToolCallRecord>) -> Self {
        self.tool_calls = calls;
        self
    }
}

/// Merger for integrating sub-agent results into main agent state
pub struct ResultMerger;

impl ResultMerger {
    /// Parse a delegate result from tool output Value
    ///
    /// This is used when the main agent receives the output from the delegate tool
    /// and needs to extract structured information.
    pub fn parse_delegate_result(output: &Value) -> Option<DelegateResult> {
        serde_json::from_value(output.clone()).ok()
    }

    /// Check if a tool output is from the delegate tool
    pub fn is_delegate_result(output: &Value) -> bool {
        // Check for characteristic fields of DelegateResult
        output.get("agent_id").is_some()
            && output.get("success").is_some()
            && output.get("summary").is_some()
    }

    /// Convert DelegateResult to MergedResult for integration
    pub fn merge(delegate_result: &DelegateResult) -> MergedResult {
        debug!(
            "Merging delegate result: success={}, artifacts={}, tools={}",
            delegate_result.success,
            delegate_result.artifacts.len(),
            delegate_result.tools_called.len()
        );

        // Convert artifacts
        let artifacts: Vec<Artifact> = delegate_result
            .artifacts
            .iter()
            .map(|a| Self::convert_artifact(a))
            .collect();

        // Convert tool calls
        let tool_calls: Vec<ToolCallRecord> = delegate_result
            .tools_called
            .iter()
            .map(|tc| Self::convert_tool_call(tc))
            .collect();

        if delegate_result.success {
            MergedResult::success(&delegate_result.summary)
                .with_artifacts(artifacts)
                .with_tool_calls(tool_calls)
        } else {
            let error = delegate_result
                .error
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string());
            MergedResult::failure(error)
                .with_artifacts(artifacts)
                .with_tool_calls(tool_calls)
        }
    }

    /// Convert ArtifactInfo to Artifact
    fn convert_artifact(info: &ArtifactInfo) -> Artifact {
        let mut artifact = Artifact {
            artifact_type: info.artifact_type.clone(),
            path: info.path.clone(),
            mime_type: info.mime_type.clone(),
            metadata: Default::default(),
        };
        // Add source marker
        artifact.metadata.insert(
            "source".to_string(),
            Value::String("sub_agent".to_string()),
        );
        artifact
    }

    /// Convert ToolCallInfo to ToolCallRecord
    fn convert_tool_call(info: &ToolCallInfo) -> ToolCallRecord {
        ToolCallRecord {
            name: info.name.clone(),
            arguments: Value::Null, // Arguments not preserved in ToolCallInfo
            success: info.success,
            result_summary: info.result_summary.clone(),
        }
    }

    /// Create a formatted observation string from delegate result
    ///
    /// This can be used to add context to the LLM's next observation.
    pub fn format_for_observation(delegate_result: &DelegateResult) -> String {
        let mut lines = Vec::new();

        lines.push(format!(
            "Sub-agent [{}] completed: {}",
            delegate_result.agent_id,
            if delegate_result.success {
                "SUCCESS"
            } else {
                "FAILED"
            }
        ));

        lines.push(format!("Summary: {}", delegate_result.summary));

        if !delegate_result.artifacts.is_empty() {
            lines.push(format!(
                "Artifacts produced: {}",
                delegate_result
                    .artifacts
                    .iter()
                    .map(|a| format!("{}:{}", a.artifact_type, a.path))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        if !delegate_result.tools_called.is_empty() {
            lines.push(format!(
                "Tools used: {}",
                delegate_result
                    .tools_called
                    .iter()
                    .map(|t| format!("{}({})", t.name, if t.success { "ok" } else { "err" }))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        if let Some(ref error) = delegate_result.error {
            lines.push(format!("Error: {}", error));
        }

        lines.join("\n")
    }

    /// Extract artifacts from a raw tool output Value
    ///
    /// Use this when you have the raw output and want to extract artifacts
    /// without fully parsing the DelegateResult.
    pub fn extract_artifacts(output: &Value) -> Vec<Artifact> {
        if let Some(delegate_result) = Self::parse_delegate_result(output) {
            delegate_result
                .artifacts
                .iter()
                .map(|a| Self::convert_artifact(a))
                .collect()
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_delegate_result() {
        let json = serde_json::json!({
            "success": true,
            "summary": "Found 3 matching tools",
            "agent_id": "mcp",
            "output": {"tools": ["a", "b", "c"]},
            "artifacts": [],
            "tools_called": [],
            "iterations_used": 1,
            "error": null
        });

        let result = ResultMerger::parse_delegate_result(&json);
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.success);
        assert_eq!(result.summary, "Found 3 matching tools");
        assert_eq!(result.agent_id, "mcp");
    }

    #[test]
    fn test_is_delegate_result() {
        let delegate_json = serde_json::json!({
            "success": true,
            "summary": "Done",
            "agent_id": "mcp",
            "artifacts": [],
            "tools_called": [],
            "iterations_used": 0
        });
        assert!(ResultMerger::is_delegate_result(&delegate_json));

        let non_delegate = serde_json::json!({
            "query": "test",
            "results": []
        });
        assert!(!ResultMerger::is_delegate_result(&non_delegate));
    }

    #[test]
    fn test_merge_success_result() {
        let delegate_result = DelegateResult {
            success: true,
            summary: "Found matching tools".to_string(),
            agent_id: "mcp".to_string(),
            output: Some(serde_json::json!({"tools": ["search", "fetch"]})),
            artifacts: vec![ArtifactInfo {
                artifact_type: "file".to_string(),
                path: "/tmp/output.json".to_string(),
                mime_type: Some("application/json".to_string()),
            }],
            tools_called: vec![ToolCallInfo {
                name: "list_tools".to_string(),
                success: true,
                result_summary: "Listed 5 tools".to_string(),
            }],
            iterations_used: 2,
            error: None,
        };

        let merged = ResultMerger::merge(&delegate_result);

        assert!(merged.success);
        assert_eq!(merged.summary, "Found matching tools");
        assert_eq!(merged.artifacts.len(), 1);
        assert_eq!(merged.artifacts[0].path, "/tmp/output.json");
        assert_eq!(merged.tool_calls.len(), 1);
        assert_eq!(merged.tool_calls[0].name, "list_tools");
    }

    #[test]
    fn test_merge_failure_result() {
        let delegate_result = DelegateResult {
            success: false,
            summary: "".to_string(),
            agent_id: "skill".to_string(),
            output: None,
            artifacts: vec![],
            tools_called: vec![],
            iterations_used: 0,
            error: Some("No matching skill found".to_string()),
        };

        let merged = ResultMerger::merge(&delegate_result);

        assert!(!merged.success);
        assert_eq!(merged.error, Some("No matching skill found".to_string()));
    }

    #[test]
    fn test_format_for_observation() {
        let delegate_result = DelegateResult {
            success: true,
            summary: "Found 2 relevant skills".to_string(),
            agent_id: "skill".to_string(),
            output: None,
            artifacts: vec![ArtifactInfo {
                artifact_type: "file".to_string(),
                path: "/tmp/result.txt".to_string(),
                mime_type: None,
            }],
            tools_called: vec![
                ToolCallInfo {
                    name: "list_skills".to_string(),
                    success: true,
                    result_summary: "OK".to_string(),
                },
                ToolCallInfo {
                    name: "get_skill_info".to_string(),
                    success: true,
                    result_summary: "OK".to_string(),
                },
            ],
            iterations_used: 3,
            error: None,
        };

        let observation = ResultMerger::format_for_observation(&delegate_result);

        assert!(observation.contains("Sub-agent [skill] completed: SUCCESS"));
        assert!(observation.contains("Found 2 relevant skills"));
        assert!(observation.contains("Artifacts produced:"));
        assert!(observation.contains("file:/tmp/result.txt"));
        assert!(observation.contains("Tools used:"));
        assert!(observation.contains("list_skills(ok)"));
    }

    #[test]
    fn test_extract_artifacts() {
        let output = serde_json::json!({
            "success": true,
            "summary": "Done",
            "agent_id": "mcp",
            "artifacts": [
                {"artifact_type": "url", "path": "https://example.com/doc", "mime_type": null}
            ],
            "tools_called": [],
            "iterations_used": 1
        });

        let artifacts = ResultMerger::extract_artifacts(&output);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact_type, "url");
        assert_eq!(artifacts[0].path, "https://example.com/doc");
    }
}

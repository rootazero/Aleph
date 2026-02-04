//! sessions_spawn tool implementation.
//!
//! Spawns a sub-agent to execute a task.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::SpawnStatus;

/// Parameters for sessions_spawn tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsSpawnParams {
    /// Task description for the sub-agent
    pub task: String,

    /// Optional label for the child session
    #[serde(default)]
    pub label: Option<String>,

    /// Target agent ID (defaults to current agent)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Model override for child session
    #[serde(default)]
    pub model: Option<String>,

    /// Thinking level override (off, minimal, low, medium, high, xhigh)
    #[serde(default)]
    pub thinking: Option<String>,

    /// Run timeout in seconds (0 = no timeout)
    #[serde(default)]
    pub run_timeout_seconds: Option<u32>,

    /// Cleanup policy: "keep" or "delete" (default: "keep")
    #[serde(default = "default_cleanup")]
    pub cleanup: String,
}

fn default_cleanup() -> String {
    "keep".to_string()
}

impl SessionsSpawnParams {
    /// Validate params
    pub fn validate(&self) -> Result<(), String> {
        if self.task.trim().is_empty() {
            return Err("task cannot be empty".into());
        }
        if !["keep", "delete"].contains(&self.cleanup.as_str()) {
            return Err("cleanup must be 'keep' or 'delete'".into());
        }
        Ok(())
    }

    /// Check if cleanup is delete
    pub fn should_delete(&self) -> bool {
        self.cleanup == "delete"
    }
}

/// Result of sessions_spawn tool
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionsSpawnResult {
    /// Spawn status
    pub status: SpawnStatus,
    /// Run ID for tracking
    pub run_id: Option<String>,
    /// Child session key
    pub child_session_key: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
}

impl SessionsSpawnResult {
    /// Create accepted result
    pub fn accepted(run_id: String, child_session_key: String) -> Self {
        Self {
            status: SpawnStatus::Accepted,
            run_id: Some(run_id),
            child_session_key: Some(child_session_key),
            error: None,
        }
    }

    /// Create forbidden result
    pub fn forbidden(error: impl Into<String>) -> Self {
        Self {
            status: SpawnStatus::Forbidden,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Create error result
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            status: SpawnStatus::Error,
            error: Some(error.into()),
            ..Default::default()
        }
    }
}

/// Build system prompt for sub-agent
pub fn build_subagent_system_prompt(
    requester_key: &str,
    child_key: &str,
    label: Option<&str>,
    task: &str,
) -> String {
    let label_info = label
        .map(|l| format!("\nSession label: {}", l))
        .unwrap_or_default();

    format!(
        r#"You are a sub-agent spawned to execute a specific task.

Spawned by: {}
Your session: {}{}

## Task

{}

## Guidelines

1. Focus exclusively on the task above
2. Complete the task and report results
3. You may use available tools to accomplish the task
4. Do not spawn further sub-agents
5. Keep responses concise and focused

When complete, summarize what was accomplished."#,
        requester_key, child_key, label_info, task
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_validation() {
        // Valid
        let params = SessionsSpawnParams {
            task: "Do something".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "keep".into(),
        };
        assert!(params.validate().is_ok());

        // Invalid: empty task
        let params = SessionsSpawnParams {
            task: "   ".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "keep".into(),
        };
        assert!(params.validate().is_err());

        // Invalid: bad cleanup
        let params = SessionsSpawnParams {
            task: "Do something".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "invalid".into(),
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_should_delete() {
        let keep = SessionsSpawnParams {
            task: "task".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "keep".into(),
        };
        assert!(!keep.should_delete());

        let delete = SessionsSpawnParams {
            task: "task".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "delete".into(),
        };
        assert!(delete.should_delete());
    }

    #[test]
    fn test_result_constructors() {
        let accepted = SessionsSpawnResult::accepted("run1".into(), "child1".into());
        assert_eq!(accepted.status, SpawnStatus::Accepted);
        assert!(accepted.run_id.is_some());
        assert!(accepted.child_session_key.is_some());

        let forbidden = SessionsSpawnResult::forbidden("Not allowed");
        assert_eq!(forbidden.status, SpawnStatus::Forbidden);
        assert!(forbidden.error.is_some());
    }

    #[test]
    fn test_build_subagent_system_prompt() {
        let prompt = build_subagent_system_prompt(
            "agent:main:main",
            "agent:main:subagent:task1",
            Some("research"),
            "Find information about Rust async",
        );
        assert!(prompt.contains("agent:main:main"));
        assert!(prompt.contains("agent:main:subagent:task1"));
        assert!(prompt.contains("research"));
        assert!(prompt.contains("Find information about Rust async"));
    }
}

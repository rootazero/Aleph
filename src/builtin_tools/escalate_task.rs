//! EscalateTask tool — allows LLM to request routing escalation.
//!
//! When the LLM determines the current task is too complex for simple
//! think-act loop, it calls this tool to escalate to DAG/POE/Swarm.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EscalateTaskArgs {
    /// Target execution strategy: "multi_step", "critical", or "collaborative"
    pub target: String,

    /// Why this task should be escalated
    pub reason: String,

    /// Optional: suggested subtask decomposition
    #[serde(default)]
    pub subtasks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EscalateTaskOutput {
    pub accepted: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct EscalateTaskTool;

#[async_trait]
impl AlephTool for EscalateTaskTool {
    const NAME: &'static str = "escalate_task";
    const DESCRIPTION: &'static str = "Request escalation to a more capable execution strategy. \
        Call this when the current task requires multiple independent steps (multi_step), \
        strict quality verification with success criteria (critical), \
        or collaboration between different expert roles (collaborative). \
        Do NOT call for simple Q&A or single-tool tasks.";

    type Args = EscalateTaskArgs;
    type Output = EscalateTaskOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.target.as_str() {
            "multi_step" | "critical" | "collaborative" => {}
            other => {
                return Ok(EscalateTaskOutput {
                    accepted: false,
                    message: format!(
                        "Invalid target '{}'. Use: multi_step, critical, or collaborative.",
                        other
                    ),
                });
            }
        }

        Ok(EscalateTaskOutput {
            accepted: true,
            message: format!(
                "Escalation to '{}' accepted. Reason: {}",
                args.target, args.reason
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_valid_escalation() {
        let tool = EscalateTaskTool;
        let result = tool
            .call(EscalateTaskArgs {
                target: "multi_step".into(),
                reason: "needs DAG".into(),
                subtasks: vec![],
            })
            .await
            .unwrap();
        assert!(result.accepted);
    }

    #[tokio::test]
    async fn test_invalid_target() {
        let tool = EscalateTaskTool;
        let result = tool
            .call(EscalateTaskArgs {
                target: "invalid".into(),
                reason: "test".into(),
                subtasks: vec![],
            })
            .await
            .unwrap();
        assert!(!result.accepted);
    }
}

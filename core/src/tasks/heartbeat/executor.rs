//! L2 Agent turn executor for heartbeat tasks.
//!
//! Builds the heartbeat prompt with probe context and defines the
//! `HeartbeatExecutionAdapter` trait for actual agent execution.

use async_trait::async_trait;

use crate::tasks::heartbeat::config::HeartbeatTask;
use crate::tasks::heartbeat::probe::ProbeResult;

// ── L2 Result Types ──────────────────────────────────────────────────

/// Status of the L2 agent analysis.
#[derive(Debug)]
pub enum HeartbeatL2Status {
    /// Agent determined nothing noteworthy; suppress notification.
    Silent,
    /// Agent produced output that should be delivered to the user.
    NeedsDelivery(String),
    /// Agent execution encountered an error.
    Error(String),
}

/// Result of an L2 heartbeat agent turn.
#[derive(Debug)]
pub struct HeartbeatL2Result {
    pub status: HeartbeatL2Status,
    pub duration_ms: i64,
}

// ── Prompt Builder ───────────────────────────────────────────────────

/// Build the L2 prompt with probe context and optional wake reason.
pub fn build_heartbeat_prompt(
    task: &HeartbeatTask,
    probe_result: &ProbeResult,
    wake_reason: Option<&str>,
) -> String {
    let mut prompt = format!(
        "Heartbeat check for task '{}'. Probe '{}' returned: {}",
        task.name,
        task.probe.tool_name,
        serde_json::to_string_pretty(&probe_result.raw_value).unwrap_or_default()
    );
    if let Some(reason) = wake_reason {
        prompt.push_str(&format!("\nWake reason: {}", reason));
    }
    prompt.push_str(
        "\n\nCheck HEARTBEAT.md for your assigned tasks. \
         Use the heartbeat_report tool to report your findings.",
    );
    prompt
}

// ── Execution Adapter Trait ──────────────────────────────────────────

/// Abstraction over agent execution for L2 heartbeat turns.
///
/// The real implementation (wired in Task 9) calls into the gateway's
/// `ExecutionAdapter`. Tests can use a mock.
#[async_trait]
pub trait HeartbeatExecutionAdapter: Send + Sync {
    async fn execute_heartbeat(
        &self,
        agent_id: &str,
        prompt: &str,
        timeout_secs: u64,
    ) -> Result<HeartbeatL2Result, String>;
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::{ProbeConfig, TriggerCondition};
    use serde_json::json;

    fn make_task() -> HeartbeatTask {
        HeartbeatTask::new(
            "Gmail Check".to_string(),
            "main".to_string(),
            300_000,
            ProbeConfig {
                tool_name: "gmail.unread_count".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::GreaterThan(0.0),
            },
        )
    }

    #[test]
    fn build_prompt_basic() {
        let task = make_task();
        let probe_result = ProbeResult {
            raw_value: json!(5),
            triggered: true,
            duration_ms: 42,
        };

        let prompt = build_heartbeat_prompt(&task, &probe_result, None);
        assert!(prompt.contains("Gmail Check"));
        assert!(prompt.contains("gmail.unread_count"));
        assert!(prompt.contains("5"));
        assert!(prompt.contains("HEARTBEAT.md"));
        assert!(!prompt.contains("Wake reason"));
    }

    #[test]
    fn build_prompt_with_wake_reason() {
        let task = make_task();
        let probe_result = ProbeResult {
            raw_value: json!({"status": "error"}),
            triggered: true,
            duration_ms: 10,
        };

        let prompt = build_heartbeat_prompt(&task, &probe_result, Some("user requested"));
        assert!(prompt.contains("Wake reason: user requested"));
    }
}

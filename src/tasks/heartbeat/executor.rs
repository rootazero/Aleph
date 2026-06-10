//! L2 Agent turn executor for heartbeat tasks.
//!
//! Builds the heartbeat prompt with probe context and defines the
//! `HeartbeatExecutionAdapter` trait for actual agent execution.

use std::collections::HashMap;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::{CollectingEventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::router::SessionKey;
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
#[must_use]
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

// ── DefaultHeartbeatAdapter ──────────────────────────────────────────

/// Production heartbeat execution adapter that bridges to the gateway's
/// ExecutionAdapter and AgentRegistry.
pub struct DefaultHeartbeatAdapter {
    adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
}

impl DefaultHeartbeatAdapter {
    pub fn new(adapter: Arc<dyn ExecutionAdapter>, agent_registry: Arc<AgentRegistry>) -> Self {
        Self {
            adapter,
            agent_registry,
        }
    }
}

#[async_trait]
impl HeartbeatExecutionAdapter for DefaultHeartbeatAdapter {
    async fn execute_heartbeat(
        &self,
        agent_id: &str,
        prompt: &str,
        timeout_secs: u64,
    ) -> Result<HeartbeatL2Result, String> {
        let start = std::time::Instant::now();

        // Resolve agent — fall back to "main" if the requested agent is not found
        let agent = self.agent_registry.get(agent_id).await.or_else(|| {
            warn!(agent_id, "heartbeat agent not found, trying 'main'");
            // Cannot await inside or_else, use blocking get pattern
            None
        });

        // If first lookup failed, try "main" separately
        let agent = match agent {
            Some(a) => a,
            None => self.agent_registry.get("main").await.ok_or_else(|| {
                format!(
                    "Agent '{}' not found and fallback 'main' unavailable",
                    agent_id
                )
            })?,
        };

        // Build request following cron executor pattern
        let run_id = Uuid::new_v4().to_string();
        let task_id = format!("hb-{}", run_id);
        let session_key = SessionKey::task(agent_id, "heartbeat", &task_id);

        let mut metadata = HashMap::new();
        metadata.insert("heartbeat_agent_id".to_string(), agent_id.to_string());

        // System-initiated: heartbeat has no parent run, so no project
        // context to inherit. Same round-3 follow-up as cron applies if
        // a heartbeat ever needs to fire inside a specific project.
        let request = RunRequest {
            run_id,
            input: prompt.to_string(),
            session_key,
            timeout_secs: Some(timeout_secs),
            metadata,
            attachments: Vec::new(),
            pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        };

        // Collect events (no user-facing emitter): the L2 agent declares its
        // outcome by calling the `heartbeat_report` tool, and that tool call
        // is recovered from the collected stream after the run completes.
        let collector = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            Arc::clone(&collector) as _;

        info!(agent_id, "executing heartbeat L2 agent turn");

        match self.adapter.execute(request, agent, emitter).await {
            Ok(()) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                // Recover the agent's heartbeat_report decision from this
                // run's event stream (scoped to this run only — a stray
                // call from any other session cannot leak in here).
                let status = classify_l2_outcome(&collector.events().await);
                Ok(HeartbeatL2Result {
                    status,
                    duration_ms,
                })
            }
            Err(ExecutionError::Timeout) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                error!(agent_id, "heartbeat L2 timed out");
                Ok(HeartbeatL2Result {
                    status: HeartbeatL2Status::Error("Execution timed out".into()),
                    duration_ms,
                })
            }
            Err(ExecutionError::AgentBusy(msg)) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                warn!(agent_id, %msg, "heartbeat L2 skipped: agent busy");
                Ok(HeartbeatL2Result {
                    status: HeartbeatL2Status::Error(format!("Agent busy: {}", msg)),
                    duration_ms,
                })
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                error!(agent_id, error = %e, "heartbeat L2 failed");
                Ok(HeartbeatL2Result {
                    status: HeartbeatL2Status::Error(format!("{}", e)),
                    duration_ms,
                })
            }
        }
    }
}

// ── L2 Outcome Classification ────────────────────────────────────────

/// Recover the L2 agent's declared outcome from its event stream.
///
/// The L2 agent reports findings by calling the `heartbeat_report` tool.
/// We scan the collected `ToolStart` events for the last such call and read
/// its `action` / `message` arguments. If the agent never called the tool,
/// the outcome is `Silent` (no notification).
fn classify_l2_outcome(events: &[StreamEvent]) -> HeartbeatL2Status {
    for event in events.iter().rev() {
        let StreamEvent::ToolStart {
            tool_name, params, ..
        } = event
        else {
            continue;
        };
        if tool_name != "heartbeat_report" {
            continue;
        }
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("silent");
        if action == "notify" {
            let msg = params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if !msg.is_empty() {
                return HeartbeatL2Status::NeedsDelivery(msg.to_string());
            }
        }
        return HeartbeatL2Status::Silent;
    }
    HeartbeatL2Status::Silent
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::{ProbeConfig, TriggerCondition};
    use serde_json::json;

    fn tool_start(tool_name: &str, params: serde_json::Value) -> StreamEvent {
        StreamEvent::ToolStart {
            run_id: "run-1".into(),
            seq: 0,
            tool_name: tool_name.into(),
            tool_id: "tool-1".into(),
            params,
        }
    }

    #[test]
    fn classify_notify_yields_needs_delivery() {
        let events = vec![tool_start(
            "heartbeat_report",
            json!({"action": "notify", "message": "3 unread emails"}),
        )];
        match classify_l2_outcome(&events) {
            HeartbeatL2Status::NeedsDelivery(msg) => assert_eq!(msg, "3 unread emails"),
            other => panic!("expected NeedsDelivery, got {other:?}"),
        }
    }

    #[test]
    fn classify_silent_action_yields_silent() {
        let events = vec![tool_start("heartbeat_report", json!({"action": "silent"}))];
        assert!(matches!(
            classify_l2_outcome(&events),
            HeartbeatL2Status::Silent
        ));
    }

    #[test]
    fn classify_no_report_call_yields_silent() {
        let events = vec![tool_start("some_other_tool", json!({"x": 1}))];
        assert!(matches!(
            classify_l2_outcome(&events),
            HeartbeatL2Status::Silent
        ));
    }

    #[test]
    fn classify_notify_with_empty_message_yields_silent() {
        let events = vec![tool_start(
            "heartbeat_report",
            json!({"action": "notify", "message": "  "}),
        )];
        assert!(matches!(
            classify_l2_outcome(&events),
            HeartbeatL2Status::Silent
        ));
    }

    #[test]
    fn classify_uses_last_report_call() {
        let events = vec![
            tool_start("heartbeat_report", json!({"action": "silent"})),
            tool_start(
                "heartbeat_report",
                json!({"action": "notify", "message": "final answer"}),
            ),
        ];
        match classify_l2_outcome(&events) {
            HeartbeatL2Status::NeedsDelivery(msg) => assert_eq!(msg, "final answer"),
            other => panic!("expected NeedsDelivery, got {other:?}"),
        }
    }

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

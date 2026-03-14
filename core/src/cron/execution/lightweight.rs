use crate::cron::clock::Clock;
use crate::cron::config::{ExecutionResult, JobSnapshot, RunStatus};

/// Execute a lightweight job. The actual event injection into the main session
/// is wired by the service layer via a callback. This function produces the result structure.
pub fn execute_lightweight(snapshot: &JobSnapshot, clock: &dyn Clock) -> ExecutionResult {
    let started_at = clock.now_ms();
    ExecutionResult {
        started_at,
        ended_at: clock.now_ms(),
        duration_ms: 0,
        status: RunStatus::Ok,
        output: Some(snapshot.prompt.clone()),
        error: None,
        error_reason: None,
        delivery_status: None,
        agent_used_messaging_tool: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use crate::cron::config::{DeliveryConfig, DeliveryMode, SessionTarget, TriggerSource};

    fn make_snapshot() -> JobSnapshot {
        JobSnapshot {
            id: "job-1".to_string(),
            agent_id: Some("agent-1".to_string()),
            prompt: "Summarize today".to_string(),
            model: None,
            timeout_ms: None,
            delivery: Some(DeliveryConfig {
                mode: DeliveryMode::None,
                targets: vec![],
                fallback_target: None,
            }),
            session_target: SessionTarget::Main,
            marked_at: 1000,
            trigger_source: TriggerSource::Schedule,
        }
    }

    #[test]
    fn lightweight_produces_ok_result() {
        let clock = FakeClock::new(5000);
        let snapshot = make_snapshot();
        let result = execute_lightweight(&snapshot, &clock);

        assert_eq!(result.status, RunStatus::Ok);
        assert_eq!(result.started_at, 5000);
        assert_eq!(result.output.as_deref(), Some("Summarize today"));
        assert!(!result.agent_used_messaging_tool);
        assert!(result.error.is_none());
        assert!(result.error_reason.is_none());
    }
}

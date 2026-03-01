//! Retry Orchestrator
//!
//! This module provides the `RetryOrchestrator` which orchestrates retry and
//! failover logic for resilient API call execution. It integrates with:
//! - RetryPolicy for retry decisions
//! - BackoffStrategy for delay calculation
//! - FailoverChain for alternative model selection
//! - HealthManager for circuit breaker checks
//! - BudgetManager for cost control
//! - MetricsCollector for observability

mod engine;
mod events;
mod types;

// Re-export all public types for backward compatibility
pub use engine::{ExecutorFn, OrchestratorConfig, RetryOrchestrator};
pub use events::OrchestratorEvent;
pub use types::{AttemptRecord, ExecutionError, ExecutionRequest, ExecutionResult};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::resilience::budget::BudgetScope;
    use crate::dispatcher::model_router::resilience::failover::FailoverChain;
    use crate::dispatcher::model_router::resilience::retry::RetryPolicy;
    use crate::dispatcher::model_router::{CallOutcome, TaskIntent};
    use crate::sync_primitives::{AtomicU32, Ordering};
    use crate::sync_primitives::Arc;
    use std::time::Duration;

    #[test]
    fn test_execution_request_builder() {
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .with_input_tokens(1000)
            .with_estimated_output_tokens(500)
            .with_budget_scope(BudgetScope::project("test"))
            .with_metadata("key", "value")
            .without_failover();

        assert_eq!(request.id, "req-1");
        assert_eq!(request.preferred_model, "gpt-4o");
        assert_eq!(request.input_tokens, 1000);
        assert!(!request.allow_failover);
        assert_eq!(request.metadata.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_attempt_record() {
        let record = AttemptRecord::new(1, "gpt-4o")
            .with_duration(Duration::from_millis(500))
            .with_outcome(CallOutcome::Success)
            .as_failover()
            .with_backoff(Duration::from_millis(100));

        assert_eq!(record.attempt_number, 1);
        assert_eq!(record.model_id, "gpt-4o");
        assert_eq!(record.duration_ms, 500);
        assert!(record.is_failover);
        assert_eq!(record.backoff_delay_ms, Some(100));
    }

    #[test]
    fn test_execution_result_success() {
        let result: ExecutionResult<String> = ExecutionResult::success(
            "output".to_string(),
            1,
            vec!["gpt-4o".to_string()],
            Duration::from_secs(1),
            vec![],
        );

        assert!(result.is_success());
        assert!(!result.is_failure());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.final_model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_execution_result_failure() {
        let result: ExecutionResult<String> = ExecutionResult::failure(
            ExecutionError::MaxAttemptsExceeded {
                attempts: 3,
                last_outcome: CallOutcome::Timeout,
            },
            3,
            vec!["gpt-4o".to_string()],
            Duration::from_secs(10),
            vec![],
        );

        assert!(!result.is_success());
        assert!(result.is_failure());
        assert!(result.err().is_some());
    }

    #[test]
    fn test_execution_error_types() {
        let budget_err = ExecutionError::BudgetExceeded {
            message: "exceeded".into(),
        };
        assert!(budget_err.is_budget_error());
        assert!(!budget_err.is_health_error());

        let health_err = ExecutionError::CircuitOpen {
            model_id: "test".into(),
        };
        assert!(health_err.is_health_error());
        assert!(!health_err.is_budget_error());
    }

    #[test]
    fn test_orchestrator_config_default() {
        let config = OrchestratorConfig::default();
        assert!(config.budget_checks_enabled);
        assert!(config.health_checks_enabled);
        assert!(config.events_enabled);
        assert!(!config.reset_retries_on_failover);
    }

    #[tokio::test]
    async fn test_orchestrator_execute_success() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);
        let chain = FailoverChain::new("gpt-4o");

        let result = orchestrator
            .execute(request, &chain, |_model, _req| async { Ok("success") })
            .await;

        assert!(result.is_success());
        assert_eq!(result.attempts, 1);
        assert_eq!(result.ok(), Some("success"));
    }

    #[tokio::test]
    async fn test_orchestrator_execute_retry_then_success() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);
        let chain = FailoverChain::new("gpt-4o");

        let result = orchestrator
            .execute(request, &chain, move |_model, _req| {
                let count = call_count_clone.clone();
                async move {
                    let current = count.fetch_add(1, Ordering::SeqCst);
                    if current < 2 {
                        Err(CallOutcome::Timeout)
                    } else {
                        Ok("success after retries")
                    }
                }
            })
            .await;

        assert!(result.is_success());
        assert_eq!(result.attempts, 3);
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_orchestrator_execute_max_attempts_exceeded() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .with_retry_policy(RetryPolicy::new().with_max_attempts(2));
        let chain = FailoverChain::new("gpt-4o");

        let result: ExecutionResult<String> = orchestrator
            .execute(request.without_failover(), &chain, |_model, _req| async {
                Err(CallOutcome::Timeout)
            })
            .await;

        assert!(result.is_failure());
        assert_eq!(result.attempts, 2);

        match result.err() {
            Some(ExecutionError::MaxAttemptsExceeded { attempts, .. }) => {
                assert_eq!(*attempts, 2);
            }
            other => panic!("Expected MaxAttemptsExceeded, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_orchestrator_execute_failover() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration)
            .with_retry_policy(RetryPolicy::new().with_max_attempts(1)); // Only 1 attempt per model

        let chain =
            FailoverChain::new("gpt-4o").with_alternatives(vec!["claude-sonnet".to_string()]);

        let result = orchestrator
            .execute(request, &chain, move |model, _req| {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if model == "gpt-4o" {
                        Err(CallOutcome::Timeout)
                    } else {
                        Ok(format!("success from {}", model))
                    }
                }
            })
            .await;

        assert!(result.is_success());
        assert_eq!(result.models_tried.len(), 2);
        assert!(result.models_tried.contains(&"gpt-4o".to_string()));
        assert!(result.models_tried.contains(&"claude-sonnet".to_string()));
        assert_eq!(result.ok(), Some("success from claude-sonnet".to_string()));
    }

    #[tokio::test]
    async fn test_orchestrator_execute_simple() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);

        let result = orchestrator
            .execute_simple(request, |_model, _req| async { Ok(42) })
            .await;

        assert!(result.is_success());
        assert_eq!(result.ok(), Some(42));
    }

    #[tokio::test]
    async fn test_orchestrator_non_retryable_error() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let request =
            ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration).without_failover();
        let chain = FailoverChain::new("gpt-4o");

        let result: ExecutionResult<String> = orchestrator
            .execute(request, &chain, |_model, _req| async {
                Err(CallOutcome::ContentFiltered)
            })
            .await;

        // ContentFiltered is not retryable and failover is disabled
        assert!(result.is_failure());
        assert_eq!(result.attempts, 1);
    }

    #[tokio::test]
    async fn test_orchestrator_event_subscription() {
        let orchestrator = RetryOrchestrator::with_defaults();
        let mut receiver = orchestrator.subscribe().expect("should have subscriber");

        let request = ExecutionRequest::new("req-1", "gpt-4o", TaskIntent::CodeGeneration);
        let chain = FailoverChain::new("gpt-4o");

        // Execute in background
        let orchestrator_clone = Arc::new(orchestrator);
        let handle = tokio::spawn({
            let orch = orchestrator_clone.clone();
            async move {
                orch.execute(request, &chain, |_model, _req| async { Ok("done") })
                    .await
            }
        });

        // Should receive ExecutionStarted event
        let event = receiver.recv().await.expect("should receive event");
        match event {
            OrchestratorEvent::ExecutionStarted { request_id, .. } => {
                assert_eq!(request_id, "req-1");
            }
            _ => panic!("Expected ExecutionStarted event"),
        }

        handle.await.unwrap();
    }
}

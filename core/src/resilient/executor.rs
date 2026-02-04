//! ResilientExecutor - executes tasks with retry and fallback.
//!
//! Implements the three-level defense:
//! 1. Retry with exponential backoff
//! 2. Degradation to fallback
//! 3. Notification on failure

use std::time::{Duration, Instant};

use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use super::task::ResilientTask;
use super::types::{DegradationReason, DegradationStrategy, TaskContext, TaskOutcome};

/// Executor for resilient tasks
pub struct ResilientExecutor {
    /// Callback for notifications
    notify_callback: Option<Box<dyn Fn(&str, &str) + Send + Sync>>,
}

impl ResilientExecutor {
    /// Create a new executor
    pub fn new() -> Self {
        Self {
            notify_callback: None,
        }
    }

    /// Set notification callback
    pub fn with_notify_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, &str) + Send + Sync + 'static,
    {
        self.notify_callback = Some(Box::new(callback));
        self
    }

    /// Execute a resilient task with full protection
    pub async fn execute<T: ResilientTask>(&self, task: &T) -> TaskOutcome<T::Output> {
        let config = task.config();
        let task_id = task.task_id().to_string();
        let start = Instant::now();

        let mut ctx = TaskContext::new(&task_id);
        let mut last_error = String::new();

        // Retry loop
        for attempt in 1..=config.max_attempts {
            ctx.attempt = attempt;
            ctx.elapsed = start.elapsed();

            task.on_attempt(&ctx);

            // Execute with timeout
            let timeout_duration = Duration::from_millis(config.timeout_ms);
            let result = timeout(timeout_duration, task.execute(&ctx)).await;

            match result {
                Ok(Ok(output)) => {
                    task.on_success(&ctx);
                    return TaskOutcome::Success(output);
                }
                Ok(Err(e)) => {
                    last_error = e.to_string();
                    let should_retry = task.should_retry(&e, attempt);

                    debug!(
                        task_id = %task_id,
                        attempt = attempt,
                        error = %last_error,
                        should_retry = should_retry,
                        "Attempt failed"
                    );

                    if !should_retry || attempt >= config.max_attempts {
                        break;
                    }

                    // Apply backoff
                    let backoff = config.calculate_backoff(attempt);
                    debug!(
                        task_id = %task_id,
                        backoff = ?backoff,
                        "Waiting before retry"
                    );
                    tokio::time::sleep(backoff).await;

                    ctx = ctx.for_retry(last_error.clone(), start.elapsed());
                }
                Err(_) => {
                    // Timeout
                    last_error = "Task execution timed out".to_string();

                    if !config.retry_on_timeout || attempt >= config.max_attempts {
                        break;
                    }

                    let backoff = config.calculate_backoff(attempt);
                    tokio::time::sleep(backoff).await;

                    ctx = ctx.for_retry(last_error.clone(), start.elapsed());
                }
            }
        }

        // Primary execution failed, try degradation
        let degradation_reason = DegradationReason::RetriesExhausted {
            attempts: config.max_attempts,
            last_error: last_error.clone(),
        };

        self.handle_degradation(task, &ctx, degradation_reason, start.elapsed())
            .await
    }

    /// Handle degradation after primary failure
    async fn handle_degradation<T: ResilientTask>(
        &self,
        task: &T,
        ctx: &TaskContext,
        reason: DegradationReason,
        elapsed: Duration,
    ) -> TaskOutcome<T::Output> {
        let config = task.config();

        match &config.degradation_strategy {
            DegradationStrategy::Skip => {
                warn!(
                    task_id = %ctx.task_id,
                    reason = ?reason,
                    "Task skipped due to degradation"
                );
                TaskOutcome::Failed {
                    error: "Task skipped".to_string(),
                    attempts: ctx.attempt,
                    last_attempt_duration: elapsed,
                }
            }

            DegradationStrategy::Fallback { fallback_id } => {
                if task.has_fallback() {
                    info!(
                        task_id = %ctx.task_id,
                        fallback_id = %fallback_id,
                        "Attempting fallback"
                    );

                    let fallback_ctx = ctx.for_degradation();
                    match task.fallback(&fallback_ctx).await {
                        Ok(output) => {
                            task.on_degraded(ctx, &reason);
                            TaskOutcome::Degraded {
                                result: output,
                                reason,
                                attempts: ctx.attempt,
                            }
                        }
                        Err(e) => {
                            error!(
                                task_id = %ctx.task_id,
                                error = %e,
                                "Fallback also failed"
                            );
                            self.notify_failure(ctx, &format!("Fallback failed: {}", e));
                            TaskOutcome::Failed {
                                error: format!("Fallback failed: {}", e),
                                attempts: ctx.attempt,
                                last_attempt_duration: elapsed,
                            }
                        }
                    }
                } else {
                    warn!(
                        task_id = %ctx.task_id,
                        "No fallback available"
                    );
                    self.notify_failure(ctx, "No fallback available");
                    TaskOutcome::Failed {
                        error: "No fallback available".to_string(),
                        attempts: ctx.attempt,
                        last_attempt_duration: elapsed,
                    }
                }
            }

            DegradationStrategy::PartialResult => {
                // For partial results, try fallback which might return partial data
                if task.has_fallback() {
                    let fallback_ctx = ctx.for_degradation();
                    match task.fallback(&fallback_ctx).await {
                        Ok(output) => TaskOutcome::Degraded {
                            result: output,
                            reason,
                            attempts: ctx.attempt,
                        },
                        Err(e) => TaskOutcome::Failed {
                            error: e.to_string(),
                            attempts: ctx.attempt,
                            last_attempt_duration: elapsed,
                        },
                    }
                } else {
                    TaskOutcome::Failed {
                        error: "No partial result available".to_string(),
                        attempts: ctx.attempt,
                        last_attempt_duration: elapsed,
                    }
                }
            }

            DegradationStrategy::UseCached { max_age_secs: _ } => {
                // Cache lookup would be implemented here
                warn!(
                    task_id = %ctx.task_id,
                    "Cache not implemented, failing"
                );
                TaskOutcome::Failed {
                    error: "Cache not available".to_string(),
                    attempts: ctx.attempt,
                    last_attempt_duration: elapsed,
                }
            }

            DegradationStrategy::NotifyAndFail { notify_channels: _ } => {
                let error_msg = match &reason {
                    DegradationReason::RetriesExhausted { last_error, .. } => last_error.clone(),
                    DegradationReason::Timeout { .. } => "Timeout".to_string(),
                    _ => "Unknown error".to_string(),
                };

                self.notify_failure(ctx, &error_msg);

                task.on_failed(ctx, &error_msg);

                TaskOutcome::Failed {
                    error: error_msg,
                    attempts: ctx.attempt,
                    last_attempt_duration: elapsed,
                }
            }
        }
    }

    /// Send failure notification
    fn notify_failure(&self, ctx: &TaskContext, error: &str) {
        if let Some(callback) = &self.notify_callback {
            callback(&ctx.task_id, error);
        }
    }
}

impl Default for ResilientExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function to execute a task with default executor
pub async fn execute_resilient<T: ResilientTask>(task: &T) -> TaskOutcome<T::Output> {
    ResilientExecutor::new().execute(task).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AlephError;
    use crate::resilient::ResilienceConfig;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct CountingTask {
        id: String,
        attempts: Arc<AtomicU32>,
        fail_until: u32,
        config: ResilienceConfig,
    }

    impl ResilientTask for CountingTask {
        type Output = String;

        fn execute<'a>(
            &'a self,
            _ctx: &'a TaskContext,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<Self::Output>> + Send + 'a>>
        {
            let attempts = self.attempts.clone();
            let fail_until = self.fail_until;
            Box::pin(async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if count < fail_until {
                    Err(AlephError::NetworkError {
                        message: format!("Attempt {} failed", count),
                        suggestion: None,
                    })
                } else {
                    Ok(format!("Success on attempt {}", count))
                }
            })
        }

        fn fallback<'a>(
            &'a self,
            _ctx: &'a TaskContext,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<Self::Output>> + Send + 'a>>
        {
            Box::pin(async { Ok("Fallback result".to_string()) })
        }

        fn has_fallback(&self) -> bool {
            true
        }

        fn task_id(&self) -> &str {
            &self.id
        }

        fn config(&self) -> ResilienceConfig {
            self.config.clone()
        }
    }

    #[tokio::test]
    async fn test_executor_success_on_first_try() {
        let task = CountingTask {
            id: "test".to_string(),
            attempts: Arc::new(AtomicU32::new(0)),
            fail_until: 1, // Succeed on first try
            config: ResilienceConfig {
                use_jitter: false,
                ..Default::default()
            },
        };

        let executor = ResilientExecutor::new();
        let outcome = executor.execute(&task).await;

        assert!(matches!(outcome, TaskOutcome::Success(_)));
        assert_eq!(task.attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_executor_retry_then_success() {
        let task = CountingTask {
            id: "test".to_string(),
            attempts: Arc::new(AtomicU32::new(0)),
            fail_until: 3, // Succeed on third try
            config: ResilienceConfig {
                max_attempts: 3,
                initial_backoff_ms: 10, // Fast for testing
                use_jitter: false,
                ..Default::default()
            },
        };

        let executor = ResilientExecutor::new();
        let outcome = executor.execute(&task).await;

        assert!(matches!(outcome, TaskOutcome::Success(_)));
        assert_eq!(task.attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_executor_fallback_on_exhausted() {
        let task = CountingTask {
            id: "test".to_string(),
            attempts: Arc::new(AtomicU32::new(0)),
            fail_until: 100, // Always fail
            config: ResilienceConfig {
                max_attempts: 2,
                initial_backoff_ms: 10,
                use_jitter: false,
                degradation_strategy: DegradationStrategy::Fallback {
                    fallback_id: "fallback".to_string(),
                },
                ..Default::default()
            },
        };

        let executor = ResilientExecutor::new();
        let outcome = executor.execute(&task).await;

        match outcome {
            TaskOutcome::Degraded { result, .. } => {
                assert_eq!(result, "Fallback result");
            }
            _ => panic!("Expected Degraded outcome"),
        }
    }

    #[tokio::test]
    async fn test_executor_notify_on_fail() {
        use std::sync::Mutex;

        let notifications = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let notifications_clone = notifications.clone();

        let task = CountingTask {
            id: "notify-test".to_string(),
            attempts: Arc::new(AtomicU32::new(0)),
            fail_until: 100, // Always fail
            config: ResilienceConfig {
                max_attempts: 1,
                use_jitter: false,
                degradation_strategy: DegradationStrategy::NotifyAndFail {
                    notify_channels: vec!["test".to_string()],
                },
                ..Default::default()
            },
        };

        let executor = ResilientExecutor::new().with_notify_callback(move |task_id, error| {
            notifications_clone
                .lock()
                .unwrap()
                .push((task_id.to_string(), error.to_string()));
        });

        let outcome = executor.execute(&task).await;

        assert!(outcome.is_failed());
        let notifs = notifications.lock().unwrap();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].0, "notify-test");
    }
}

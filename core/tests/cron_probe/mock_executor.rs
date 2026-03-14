//! MockExecutor for cron probe tests.
//!
//! Records all job executions and allows configuring per-job behavior
//! (success, error, delayed) for deterministic testing.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use alephcore::cron::config::{
    DeliveryStatus, ErrorReason, ExecutionResult, JobSnapshot, RunStatus, TriggerSource,
};
use alephcore::cron::service::timer::JobExecutorFn;

/// Record of a single job execution.
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    pub job_id: String,
    pub trigger_source: TriggerSource,
    pub executed_at_ms: i64,
    pub prompt: String,
}

/// Configurable behavior for a mock executor.
#[derive(Clone)]
pub enum MockBehavior {
    /// Return Ok with the given output.
    Ok(String),
    /// Return Error with the given message and reason.
    Error {
        message: String,
        reason: ErrorReason,
    },
    /// Simulate a delayed execution (output after delay).
    Delayed {
        delay_ms: i64,
        output: String,
    },
}

/// A mock executor that records calls and returns configurable results.
pub struct MockExecutor {
    behaviors: Arc<Mutex<HashMap<String, MockBehavior>>>,
    call_log: Arc<Mutex<Vec<ExecutionRecord>>>,
}

impl MockExecutor {
    /// Create a new MockExecutor with no configured behaviors.
    pub fn new() -> Self {
        Self {
            behaviors: Arc::new(Mutex::new(HashMap::new())),
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Configure behavior for a specific job ID.
    pub fn on_job(&self, id: impl Into<String>, behavior: MockBehavior) {
        let mut behaviors = self.behaviors.lock().unwrap_or_else(|e| e.into_inner());
        behaviors.insert(id.into(), behavior);
    }

    /// Return the number of times a job was executed.
    pub fn call_count(&self, id: &str) -> usize {
        let log = self.call_log.lock().unwrap_or_else(|e| e.into_inner());
        log.iter().filter(|r| r.job_id == id).count()
    }

    /// Return all execution records.
    pub fn calls(&self) -> Vec<ExecutionRecord> {
        let log = self.call_log.lock().unwrap_or_else(|e| e.into_inner());
        log.clone()
    }

    /// Return execution records for a specific job ID.
    pub fn calls_for(&self, id: &str) -> Vec<ExecutionRecord> {
        let log = self.call_log.lock().unwrap_or_else(|e| e.into_inner());
        log.iter().filter(|r| r.job_id == id).cloned().collect()
    }

    /// Check if a job was executed at least once.
    pub fn was_executed(&self, id: &str) -> bool {
        self.call_count(id) > 0
    }

    /// Clear all execution records.
    pub fn reset_calls(&self) {
        let mut log = self.call_log.lock().unwrap_or_else(|e| e.into_inner());
        log.clear();
    }

    /// Convert into a `JobExecutorFn` for use with the cron service.
    pub fn into_executor_fn(&self) -> JobExecutorFn {
        let behaviors = Arc::clone(&self.behaviors);
        let call_log = Arc::clone(&self.call_log);

        Arc::new(move |snapshot: JobSnapshot| -> Pin<Box<dyn Future<Output = ExecutionResult> + Send>> {
            let behaviors = Arc::clone(&behaviors);
            let call_log = Arc::clone(&call_log);

            Box::pin(async move {
                // Record the call
                {
                    let mut log = call_log.lock().unwrap_or_else(|e| e.into_inner());
                    log.push(ExecutionRecord {
                        job_id: snapshot.id.clone(),
                        trigger_source: snapshot.trigger_source,
                        executed_at_ms: snapshot.marked_at,
                        prompt: snapshot.prompt.clone(),
                    });
                }

                // Look up configured behavior (default: Ok("ok"))
                let behavior = {
                    let behaviors = behaviors.lock().unwrap_or_else(|e| e.into_inner());
                    behaviors.get(&snapshot.id).cloned()
                };

                let behavior = behavior.unwrap_or(MockBehavior::Ok("ok".to_string()));

                match behavior {
                    MockBehavior::Ok(output) => ExecutionResult {
                        started_at: snapshot.marked_at,
                        ended_at: snapshot.marked_at + 100,
                        duration_ms: 100,
                        status: RunStatus::Ok,
                        output: Some(output),
                        error: None,
                        error_reason: None,
                        delivery_status: Some(DeliveryStatus::NotRequested),
                        agent_used_messaging_tool: false,
                    },
                    MockBehavior::Error { message, reason } => ExecutionResult {
                        started_at: snapshot.marked_at,
                        ended_at: snapshot.marked_at + 100,
                        duration_ms: 100,
                        status: RunStatus::Error,
                        output: None,
                        error: Some(message),
                        error_reason: Some(reason),
                        delivery_status: None,
                        agent_used_messaging_tool: false,
                    },
                    MockBehavior::Delayed { delay_ms, output } => {
                        // Simulate delay (we don't actually sleep — just report duration)
                        ExecutionResult {
                            started_at: snapshot.marked_at,
                            ended_at: snapshot.marked_at + delay_ms,
                            duration_ms: delay_ms,
                            status: RunStatus::Ok,
                            output: Some(output),
                            error: None,
                            error_reason: None,
                            delivery_status: Some(DeliveryStatus::NotRequested),
                            agent_used_messaging_tool: false,
                        }
                    }
                }
            })
        })
    }
}

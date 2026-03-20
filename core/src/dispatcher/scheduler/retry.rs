//! Retry logic with exponential backoff for task execution

use crate::sync_primitives::Arc;

use tracing::{debug, warn};

use super::executor::GraphTaskExecutor;
use crate::dispatcher::callback::ExecutionCallback;
use crate::dispatcher::context::TaskOutput;
use crate::dispatcher::agent_types::Task;
use crate::error::Result;

/// Exponential backoff configuration for retries
const INITIAL_RETRY_DELAY_MS: u64 = 100;
const MAX_RETRY_DELAY_MS: u64 = 5000;
const BACKOFF_MULTIPLIER: f64 = 2.0;

/// Calculate exponential backoff delay for a given attempt
///
/// Uses the formula: min(initial * multiplier^(attempt-1), max_delay)
/// With optional jitter (±10%) to prevent thundering herd
fn calculate_backoff_delay(attempt: u32) -> tokio::time::Duration {
    let base_delay = INITIAL_RETRY_DELAY_MS as f64 * BACKOFF_MULTIPLIER.powi(attempt as i32 - 1);
    let capped_delay = base_delay.min(MAX_RETRY_DELAY_MS as f64);

    // Add ±10% jitter to prevent thundering herd
    let jitter_factor = 0.9 + (rand::random::<f64>() * 0.2); // 0.9 to 1.1
    let final_delay = (capped_delay * jitter_factor) as u64;

    tokio::time::Duration::from_millis(final_delay)
}

/// Execute a task with exponential backoff retry logic
///
/// Attempts to execute the task up to `max_retries` times.
/// Uses exponential backoff with jitter between retries to handle
/// transient failures gracefully without overwhelming services.
///
/// Backoff formula: min(100ms * 2^(attempt-1), 5000ms) ± 10% jitter
/// - Attempt 1 failure: ~100ms delay
/// - Attempt 2 failure: ~200ms delay
/// - Attempt 3 failure: ~400ms delay
/// - etc., capped at 5000ms
///
/// # Arguments
/// * `task` - The task to execute
/// * `executor` - Task executor
/// * `callback` - UI callback for progress updates
/// * `context` - Prompt context for the task
/// * `max_retries` - Maximum number of retry attempts
///
/// # Returns
/// TaskOutput on success, or error after all retries failed
pub(crate) async fn execute_with_retry(
    task: &Task,
    executor: &Arc<dyn GraphTaskExecutor>,
    callback: &Arc<dyn ExecutionCallback>,
    context: &str,
    max_retries: u32,
) -> Result<TaskOutput> {
    let mut last_error = None;

    for attempt in 1..=max_retries {
        debug!("Executing task '{}' attempt {}/{}", task.id, attempt, max_retries);

        match executor.execute(task, context).await {
            Ok(output) => {
                return Ok(output);
            }
            Err(e) => {
                let error_msg = e.to_string();
                last_error = Some(e);

                if attempt < max_retries {
                    // Calculate exponential backoff delay
                    let delay = calculate_backoff_delay(attempt);

                    // Notify retry with delay info
                    callback.on_task_retry(&task.id, attempt, &error_msg).await;
                    warn!(
                        "Task '{}' failed on attempt {}/{}: {}, retrying in {:?}...",
                        task.id, attempt, max_retries, error_msg, delay
                    );

                    // Apply exponential backoff delay
                    tokio::time::sleep(delay).await;
                } else {
                    // All retries exhausted - notify deciding state
                    callback.on_task_deciding(&task.id, &error_msg).await;
                    warn!(
                        "Task '{}' failed after {} attempts: {}",
                        task.id, max_retries, error_msg
                    );
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        crate::error::AlephError::Other {
            message: format!("Task '{}' failed with unknown error", task.id),
            suggestion: None,
        }
    }))
}

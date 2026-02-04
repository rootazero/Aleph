//! ResilientTask trait definition.
//!
//! Defines the interface for tasks that support resilient execution.

use std::future::Future;
use std::pin::Pin;

use crate::error::Result;

use super::types::{DegradationReason, ResilienceConfig, TaskContext};

/// A task that can be executed with resilience guarantees.
///
/// Implementors should provide:
/// - `execute`: The primary task logic
/// - `fallback` (optional): A degraded alternative when primary fails
/// - `config`: Resilience configuration
pub trait ResilientTask: Send + Sync {
    /// The output type of the task
    type Output: Send;

    /// Execute the primary task logic.
    ///
    /// This is the main entry point that will be retried on failure.
    fn execute<'a>(
        &'a self,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Output>> + Send + 'a>>;

    /// Execute fallback logic when primary fails.
    ///
    /// Override this to provide graceful degradation.
    /// Default implementation returns an error.
    fn fallback<'a>(
        &'a self,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Output>> + Send + 'a>> {
        let _ = ctx;
        Box::pin(async {
            Err(crate::error::AlephError::Other {
                message: "No fallback available".to_string(),
                suggestion: None,
            })
        })
    }

    /// Check if this task has a fallback implementation.
    ///
    /// Override to return true if `fallback` is implemented.
    fn has_fallback(&self) -> bool {
        false
    }

    /// Get the task identifier.
    fn task_id(&self) -> &str;

    /// Get the resilience configuration.
    fn config(&self) -> ResilienceConfig {
        ResilienceConfig::default()
    }

    /// Check if a specific error should be retried.
    ///
    /// Override to customize retry behavior.
    fn should_retry(&self, error: &crate::error::AlephError, attempt: u32) -> bool {
        let config = self.config();
        if attempt >= config.max_attempts {
            return false;
        }

        // Check error type
        matches!(
            error,
            crate::error::AlephError::NetworkError { .. }
                | crate::error::AlephError::RateLimitError { .. }
                | crate::error::AlephError::ProviderError { .. }
        )
    }

    /// Called before each execution attempt.
    fn on_attempt(&self, ctx: &TaskContext) {
        tracing::debug!(
            task_id = %ctx.task_id,
            attempt = ctx.attempt,
            "Starting attempt"
        );
    }

    /// Called after successful execution.
    fn on_success(&self, ctx: &TaskContext) {
        tracing::info!(
            task_id = %ctx.task_id,
            attempt = ctx.attempt,
            elapsed = ?ctx.elapsed,
            "Task succeeded"
        );
    }

    /// Called after degraded execution.
    fn on_degraded(&self, ctx: &TaskContext, reason: &DegradationReason) {
        tracing::warn!(
            task_id = %ctx.task_id,
            attempt = ctx.attempt,
            reason = ?reason,
            "Task degraded to fallback"
        );
    }

    /// Called after failed execution.
    fn on_failed(&self, ctx: &TaskContext, error: &str) {
        tracing::error!(
            task_id = %ctx.task_id,
            attempt = ctx.attempt,
            error = %error,
            "Task failed"
        );
    }
}

/// A simple wrapper for closures as resilient tasks.
pub struct FnTask<F, O>
where
    F: Fn(&TaskContext) -> Pin<Box<dyn Future<Output = Result<O>> + Send + '_>> + Send + Sync,
    O: Send,
{
    id: String,
    execute_fn: F,
    fallback_fn:
        Option<Box<dyn Fn(&TaskContext) -> Pin<Box<dyn Future<Output = Result<O>> + Send + '_>> + Send + Sync>>,
    config: ResilienceConfig,
}

impl<F, O> FnTask<F, O>
where
    F: Fn(&TaskContext) -> Pin<Box<dyn Future<Output = Result<O>> + Send + '_>> + Send + Sync,
    O: Send,
{
    /// Create a new function-based task
    pub fn new(id: impl Into<String>, execute_fn: F) -> Self {
        Self {
            id: id.into(),
            execute_fn,
            fallback_fn: None,
            config: ResilienceConfig::default(),
        }
    }

    /// Set the fallback function
    pub fn with_fallback<Fb>(mut self, fallback_fn: Fb) -> Self
    where
        Fb: Fn(&TaskContext) -> Pin<Box<dyn Future<Output = Result<O>> + Send + '_>>
            + Send
            + Sync
            + 'static,
    {
        self.fallback_fn = Some(Box::new(fallback_fn));
        self
    }

    /// Set the resilience config
    pub fn with_config(mut self, config: ResilienceConfig) -> Self {
        self.config = config;
        self
    }
}

impl<F, O> ResilientTask for FnTask<F, O>
where
    F: Fn(&TaskContext) -> Pin<Box<dyn Future<Output = Result<O>> + Send + '_>> + Send + Sync,
    O: Send,
{
    type Output = O;

    fn execute<'a>(
        &'a self,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Output>> + Send + 'a>> {
        (self.execute_fn)(ctx)
    }

    fn fallback<'a>(
        &'a self,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Output>> + Send + 'a>> {
        if let Some(fb) = &self.fallback_fn {
            fb(ctx)
        } else {
            Box::pin(async {
                Err(crate::error::AlephError::Other {
                    message: "No fallback available".to_string(),
                    suggestion: None,
                })
            })
        }
    }

    fn has_fallback(&self) -> bool {
        self.fallback_fn.is_some()
    }

    fn task_id(&self) -> &str {
        &self.id
    }

    fn config(&self) -> ResilienceConfig {
        self.config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTask {
        id: String,
        should_fail: bool,
    }

    impl ResilientTask for TestTask {
        type Output = String;

        fn execute<'a>(
            &'a self,
            _ctx: &'a TaskContext,
        ) -> Pin<Box<dyn Future<Output = Result<Self::Output>> + Send + 'a>> {
            let should_fail = self.should_fail;
            Box::pin(async move {
                if should_fail {
                    Err(crate::error::AlephError::Other {
                        message: "Test failure".to_string(),
                        suggestion: None,
                    })
                } else {
                    Ok("success".to_string())
                }
            })
        }

        fn fallback<'a>(
            &'a self,
            _ctx: &'a TaskContext,
        ) -> Pin<Box<dyn Future<Output = Result<Self::Output>> + Send + 'a>> {
            Box::pin(async { Ok("fallback".to_string()) })
        }

        fn has_fallback(&self) -> bool {
            true
        }

        fn task_id(&self) -> &str {
            &self.id
        }
    }

    #[tokio::test]
    async fn test_resilient_task_success() {
        let task = TestTask {
            id: "test".to_string(),
            should_fail: false,
        };
        let ctx = TaskContext::new("test");
        let result = task.execute(&ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
    }

    #[tokio::test]
    async fn test_resilient_task_fallback() {
        let task = TestTask {
            id: "test".to_string(),
            should_fail: true,
        };
        let ctx = TaskContext::new("test");
        let result = task.fallback(&ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "fallback");
    }
}

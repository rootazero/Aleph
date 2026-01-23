//! Parallel execution for ensemble models
//!
//! This module provides the ParallelExecutor for running multiple model
//! requests concurrently with timeout and concurrency controls.

use super::types::{EnsembleConfig, ModelExecutionResult, TokenUsage};
use crate::dispatcher::model_router::ModelProfile;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

// ============================================================================
// Parallel Executor
// ============================================================================

/// Manages parallel execution of multiple models
pub struct ParallelExecutor {
    /// Timeout for entire ensemble
    timeout: Duration,
    /// Maximum concurrent requests
    pub max_concurrency: usize,
}

impl ParallelExecutor {
    /// Create a new parallel executor
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            max_concurrency: 5,
        }
    }

    /// Create with custom concurrency limit
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = max.max(1);
        self
    }

    /// Create from ensemble config
    pub fn from_config(config: &EnsembleConfig) -> Self {
        Self {
            timeout: config.timeout(),
            max_concurrency: config.max_concurrency,
        }
    }

    /// Get the timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Get max concurrency
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// Execute request across multiple models concurrently
    ///
    /// # Arguments
    /// * `models` - Models to execute (profile IDs)
    /// * `executor_fn` - Async function that executes a single model
    ///
    /// # Returns
    /// Vector of execution results (one per model, in same order)
    pub async fn execute_parallel<F, Fut>(
        &self,
        models: &[String],
        executor_fn: F,
    ) -> Vec<ModelExecutionResult>
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(String, TokenUsage, f64), String>> + Send,
    {
        use futures::future::join_all;
        use tokio::time::timeout;

        // Limit concurrency
        let models_to_run: Vec<_> = models.iter().take(self.max_concurrency).cloned().collect();
        let semaphore = Arc::new(Semaphore::new(self.max_concurrency));
        let executor_fn = Arc::new(executor_fn);

        let futures: Vec<_> = models_to_run
            .into_iter()
            .map(|model_id| {
                let sem = semaphore.clone();
                let timeout_duration = self.timeout;
                let model_id_clone = model_id.clone();
                let executor = executor_fn.clone();

                async move {
                    let _permit = sem.acquire().await.unwrap();
                    let start = Instant::now();

                    match timeout(timeout_duration, executor(model_id.clone())).await {
                        Ok(Ok((response, tokens, cost))) => ModelExecutionResult::success(
                            &model_id,
                            response,
                            start.elapsed().as_millis() as u64,
                        )
                        .with_tokens(tokens.input_tokens, tokens.output_tokens)
                        .with_cost(cost),
                        Ok(Err(error)) => ModelExecutionResult::failure(
                            &model_id,
                            error,
                            start.elapsed().as_millis() as u64,
                        ),
                        Err(_) => ModelExecutionResult::timeout(
                            &model_id_clone,
                            timeout_duration.as_millis() as u64,
                        ),
                    }
                }
            })
            .collect();

        join_all(futures).await
    }

    /// Execute with ModelProfile references (convenience method)
    pub async fn execute_parallel_profiles<F, Fut>(
        &self,
        profiles: &[&ModelProfile],
        executor_fn: F,
    ) -> Vec<ModelExecutionResult>
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(String, TokenUsage, f64), String>> + Send,
    {
        let model_ids: Vec<String> = profiles.iter().map(|p| p.id.clone()).collect();
        self.execute_parallel(&model_ids, executor_fn).await
    }
}

impl Default for ParallelExecutor {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

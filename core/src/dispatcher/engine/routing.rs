//! Model routing methods for AgentEngine

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use super::config::ExecutionState;
use super::core::AgentEngine;
use crate::dispatcher::agent_types::{
    ExecutionSummary, Task, TaskGraph, TaskResult, TaskStatus, TaskType,
};
use crate::dispatcher::executor::ExecutionContext;
use crate::dispatcher::model_router::{
    FallbackProvider, ModelMatcher, ModelProfile, ModelRouter, ModelRoutingRules, TaskContextManager,
};
use crate::dispatcher::monitor::TaskMonitor;
use crate::dispatcher::scheduler::TaskScheduler;
use crate::error::{AlephError, Result};

impl AgentEngine {
    // =========================================================================
    // Model Routing Methods
    // =========================================================================

    /// Get the model matcher (if available)
    pub fn model_matcher(&self) -> Option<&Arc<ModelMatcher>> {
        self.model_matcher.as_ref()
    }

    /// Check if model routing is enabled
    pub fn has_model_routing(&self) -> bool {
        self.model_matcher.is_some()
    }

    /// Set the fallback provider for model routing
    ///
    /// This should be called with the system's `default_provider` from config.
    /// When no suitable model is found through normal routing, the fallback
    /// provider will be used.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Set fallback from default_provider config
    /// if let Some(default_provider) = config.general.default_provider.as_ref() {
    ///     engine.set_fallback_provider(default_provider);
    /// }
    /// ```
    pub fn set_fallback_provider(&mut self, provider: &str) {
        if let Some(ref mut matcher) = self.model_matcher {
            // We need to get a mutable reference, so we clone and replace
            let mut new_matcher = (**matcher).clone();
            new_matcher.set_fallback_provider(Some(FallbackProvider::new(provider)));
            *matcher = Arc::new(new_matcher);
        } else {
            // If model routing is not enabled, create a minimal matcher with just fallback
            let rules = ModelRoutingRules::default();
            let matcher = ModelMatcher::new(vec![], rules).with_fallback_provider(provider);
            self.model_matcher = Some(Arc::new(matcher));
        }
    }

    /// Set the fallback provider with a specific model
    pub fn set_fallback_provider_with_model(&mut self, provider: &str, model: &str) {
        if let Some(ref mut matcher) = self.model_matcher {
            let mut new_matcher = (**matcher).clone();
            new_matcher.set_fallback_provider(Some(FallbackProvider::new(provider).with_model(model)));
            *matcher = Arc::new(new_matcher);
        } else {
            let rules = ModelRoutingRules::default();
            let matcher =
                ModelMatcher::new(vec![], rules).with_fallback_provider_and_model(provider, model);
            self.model_matcher = Some(Arc::new(matcher));
        }
    }

    /// Check if a fallback provider is configured
    pub fn has_fallback_provider(&self) -> bool {
        self.model_matcher
            .as_ref()
            .map(|m| m.has_fallback())
            .unwrap_or(false)
    }

    /// Route a task to the optimal model
    ///
    /// Returns the selected ModelProfile, or an error if routing fails.
    pub fn route_task(&self, task: &Task) -> Result<ModelProfile> {
        let matcher = self
            .model_matcher
            .as_ref()
            .ok_or_else(|| AlephError::config("Model routing is not enabled"))?;

        matcher
            .route(task)
            .map_err(|e| AlephError::config(e.to_string()))
    }

    /// Route by TaskIntent for unified routing integration
    ///
    /// This method bridges the legacy routing rules to the Model Router by:
    /// 1. Converting `intent_type` string to `TaskIntent` enum
    /// 2. Optionally applying `preferred_model` override
    /// 3. Returning the optimal `ModelProfile` for the intent
    ///
    /// # Arguments
    ///
    /// * `intent` - The TaskIntent to route
    /// * `preferred_model` - Optional model ID to override automatic selection
    ///
    /// # Returns
    ///
    /// * `Ok(ModelProfile)` - The selected model profile
    /// * `Err` - If routing fails or model routing is disabled
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use alephcore::dispatcher::model_router::TaskIntent;
    ///
    /// // Route based on matched routing rule
    /// let rule = config.find_matching_rule(input)?;
    /// let intent = rule.get_task_intent();
    /// let preferred = rule.get_preferred_model();
    /// let profile = engine.route_by_intent(&intent, preferred)?;
    /// ```
    pub fn route_by_intent(
        &self,
        intent: &crate::dispatcher::model_router::TaskIntent,
        preferred_model: Option<&str>,
    ) -> Result<ModelProfile> {
        let matcher = self
            .model_matcher
            .as_ref()
            .ok_or_else(|| AlephError::config("Model routing is not enabled"))?;

        matcher
            .route_by_intent_with_preference(intent, preferred_model)
            .ok_or_else(|| AlephError::config("No model available for intent"))
    }

    /// Route from a RoutingRuleConfig
    ///
    /// Convenience method that extracts TaskIntent and preferred_model from a rule
    /// and routes to the optimal model.
    ///
    /// # Arguments
    ///
    /// * `rule` - The matched routing rule
    ///
    /// # Returns
    ///
    /// * `Ok(ModelProfile)` - The selected model profile
    /// * `Err` - If routing fails
    pub fn route_from_rule(&self, rule: &crate::config::RoutingRuleConfig) -> Result<ModelProfile> {
        let intent = rule.get_task_intent();
        let preferred = rule.get_preferred_model();
        self.route_by_intent(&intent, preferred)
    }

    /// Execute a task graph with model routing
    ///
    /// This method routes AI tasks to optimal models based on task characteristics
    /// and executes them using the appropriate provider.
    ///
    /// # Arguments
    ///
    /// * `graph` - The task graph to execute
    ///
    /// # Returns
    ///
    /// * `Ok(ExecutionSummary)` - Summary with model routing information
    /// * `Err` - If execution fails
    pub async fn execute_with_routing(&self, mut graph: TaskGraph) -> Result<ExecutionSummary> {
        // If model routing is not enabled, fall back to regular execution
        if !self.has_model_routing() {
            info!("Model routing not enabled, using standard execution");
            return self.execute(graph).await;
        }

        info!(
            "Executing task graph with model routing: {} ({})",
            graph.metadata.title, graph.id
        );
        *self.state.write().await = ExecutionState::Executing;

        // Reset state
        self.paused.store(false, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.scheduler.write().await.reset();

        let start_time = Instant::now();
        let ctx = ExecutionContext::new(&graph.id);

        // Create context manager for tracking results
        let context_manager = TaskContextManager::new(&graph.id);

        // Main execution loop
        loop {
            // Check for cancellation
            if self.cancelled.load(Ordering::SeqCst) {
                warn!("Execution cancelled");
                *self.state.write().await = ExecutionState::Cancelled;
                break;
            }

            // Check for pause
            while self.paused.load(Ordering::SeqCst) {
                *self.state.write().await = ExecutionState::Paused;
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                if self.cancelled.load(Ordering::SeqCst) {
                    break;
                }
            }

            if self.cancelled.load(Ordering::SeqCst) {
                break;
            }

            *self.state.write().await = ExecutionState::Executing;

            // Get ready tasks
            let ready_tasks: Vec<String> = {
                let scheduler = self.scheduler.read().await;
                scheduler
                    .next_ready(&graph)
                    .iter()
                    .map(|t| t.id.clone())
                    .collect()
            };

            if ready_tasks.is_empty() {
                // Check if we're done
                let scheduler = self.scheduler.read().await;
                if scheduler.is_complete(&graph) {
                    break;
                }

                // Wait for running tasks to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }

            // Execute ready tasks
            for task_id in ready_tasks {
                let task = match graph.get_task(&task_id) {
                    Some(t) => t.clone(),
                    None => continue,
                };

                // Mark as running
                {
                    let mut scheduler = self.scheduler.write().await;
                    scheduler.mark_running(&task_id);
                }

                if let Some(graph_task) = graph.get_task_mut(&task_id) {
                    graph_task.status = TaskStatus::running(0.0);
                }

                self.monitor.on_task_start(&task);

                // Route AI tasks to optimal model
                let (result, model_used) = if matches!(task.task_type, TaskType::AiInference(_)) {
                    self.execute_ai_task_with_routing(&task, &ctx, &context_manager)
                        .await
                } else {
                    // Non-AI tasks use standard execution
                    let result = self.executors.execute(&task, &ctx).await;
                    (result, None)
                };

                match result {
                    Ok(task_result) => {
                        debug!(
                            "Task {} completed successfully{}",
                            task_id,
                            model_used
                                .as_ref()
                                .map(|m| format!(" (model: {})", m))
                                .unwrap_or_default()
                        );

                        // Store result in context manager
                        let _ = context_manager
                            .store_result(
                                &task,
                                task_result.clone(),
                                model_used.as_deref(),
                                None,
                                0,
                            )
                            .await;

                        {
                            let mut scheduler = self.scheduler.write().await;
                            scheduler.mark_completed(&task_id);
                        }

                        if let Some(graph_task) = graph.get_task_mut(&task_id) {
                            graph_task.status = TaskStatus::completed(task_result.clone());
                        }

                        self.monitor.on_task_complete(&task, &task_result);
                    }
                    Err(e) => {
                        error!("Task {} failed: {}", task_id, e);

                        let error_msg = e.to_string();
                        {
                            let mut scheduler = self.scheduler.write().await;
                            scheduler.mark_failed(&task_id, &error_msg);
                        }

                        if let Some(graph_task) = graph.get_task_mut(&task_id) {
                            graph_task.status = TaskStatus::failed(&error_msg);
                        }

                        self.monitor.on_task_failed(&task, &error_msg);
                    }
                }
            }

            // Update graph progress
            self.monitor.update_graph_progress(&graph);
        }

        // Mark remaining tasks as Cancelled if execution was cancelled
        if self.cancelled.load(Ordering::SeqCst) {
            for task in &mut graph.tasks {
                if task.is_pending() || task.is_running() {
                    task.status = TaskStatus::Cancelled;
                }
            }
        }

        // Build summary
        let counts = graph.count_by_status();
        let context_summary = context_manager.summary().await;

        let summary = ExecutionSummary {
            graph_id: graph.id.clone(),
            total_tasks: counts.total(),
            completed_tasks: counts.completed,
            failed_tasks: counts.failed,
            cancelled_tasks: counts.cancelled,
            total_duration: start_time.elapsed(),
            artifacts: Vec::new(),
            errors: graph
                .tasks
                .iter()
                .filter_map(|t| {
                    if let TaskStatus::Failed { error, .. } = &t.status {
                        Some(format!("{}: {}", t.name, error))
                    } else {
                        None
                    }
                })
                .collect(),
        };

        // Don't overwrite Cancelled state with Completed
        if !self.cancelled.load(Ordering::SeqCst) {
            *self.state.write().await = ExecutionState::Completed;
        }
        self.monitor.on_graph_complete(&graph);

        info!(
            "Execution with routing completed: {} tasks, {} completed, {} failed, {} tokens",
            summary.total_tasks,
            summary.completed_tasks,
            summary.failed_tasks,
            context_summary.total_tokens
        );

        Ok(summary)
    }

    /// Execute an AI task with model routing
    pub(crate) async fn execute_ai_task_with_routing(
        &self,
        task: &Task,
        _ctx: &ExecutionContext,
        _context_manager: &TaskContextManager,
    ) -> (Result<TaskResult>, Option<String>) {
        // Route task to optimal model
        let profile = match self.route_task(task) {
            Ok(p) => p,
            Err(e) => {
                return (Err(e), None);
            }
        };

        let model_id = profile.id.clone();
        debug!(
            "Routed task '{}' to model '{}' (provider: {})",
            task.name, model_id, profile.provider
        );

        // Execute with provider (if available)
        if let Some(provider) = &self.provider {
            // Extract prompt from AI task
            let prompt = if let TaskType::AiInference(ai_task) = &task.task_type {
                ai_task.prompt.clone()
            } else {
                return (
                    Err(AlephError::config("Task is not an AI inference task")),
                    Some(model_id),
                );
            };

            // Execute with provider
            match provider.process(&prompt, None).await {
                Ok(response) => {
                    let result = TaskResult {
                        output: serde_json::json!({
                            "response": response,
                            "model_used": model_id,
                            "provider": profile.provider,
                        }),
                        artifacts: Vec::new(),
                        duration: std::time::Duration::ZERO, // TODO: track actual duration
                        summary: Some(format!("Completed with model {}", model_id)),
                    };
                    (Ok(result), Some(model_id))
                }
                Err(e) => (Err(e), Some(model_id)),
            }
        } else {
            // No provider available, return placeholder result
            let result = TaskResult {
                output: serde_json::json!({
                    "model_routed": model_id,
                    "provider": profile.provider,
                    "note": "No provider available for execution",
                }),
                artifacts: Vec::new(),
                duration: std::time::Duration::ZERO,
                summary: Some(format!("Routed to model {} (not executed)", model_id)),
            };
            (Ok(result), Some(model_id))
        }
    }
}

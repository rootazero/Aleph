//! Plan Executor - Sequential Plan Execution Engine
//!
//! This module implements the `PlanExecutor` for executing multi-step task plans.
//!
//! # Key Features
//!
//! - Sequential step execution with timeout handling
//! - `$prev` reference resolution for result passing
//! - Event callbacks for UI progress updates
//! - Rollback support for reversible operations
//!
//! # Architecture
//!
//! ```text
//! TaskPlan (from L3TaskPlanner)
//!      ↓
//! ┌─────────────────────────────────────────┐
//! │           PlanExecutor                  │
//! │                                          │
//! │  ┌────────────────────────────────────┐ │
//! │  │ Step 1: Execute + Resolve $prev    │ │
//! │  │ → StepResult (output, rollback)    │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓                          │
//! │  ┌────────────────────────────────────┐ │
//! │  │ Step 2: Execute + Resolve $prev    │ │
//! │  │ → StepResult (uses Step 1 output)  │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓                          │
//! │  ... (sequential execution)              │
//! └──────────────────────────────────────────┘
//!      ↓
//! PlanExecutionResult (final output, all step results)
//! ```

use crate::error::{AetherError, Result};
use crate::event_handler::AetherEventHandler;
use crate::tools::NativeToolRegistry;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::plan::{
    PlanExecutionContext, PlanExecutionResult, PlanStep, StepResult, TaskPlan,
};

// =============================================================================
// PlanExecutor Configuration
// =============================================================================

/// Configuration for the PlanExecutor
#[derive(Debug, Clone)]
pub struct PlanExecutorConfig {
    /// Default timeout per step (milliseconds)
    pub default_step_timeout_ms: u64,

    /// Maximum total execution time (milliseconds)
    pub max_total_timeout_ms: u64,

    /// Whether to stop on first error
    pub stop_on_error: bool,

    /// Whether to attempt rollback on failure
    pub enable_rollback: bool,

    /// Whether to send progress events
    pub send_progress_events: bool,
}

impl Default for PlanExecutorConfig {
    fn default() -> Self {
        Self {
            default_step_timeout_ms: 30_000,  // 30 seconds per step
            max_total_timeout_ms: 300_000,    // 5 minutes total
            stop_on_error: true,
            enable_rollback: true,
            send_progress_events: true,
        }
    }
}

// =============================================================================
// PlanExecutor
// =============================================================================

/// Executor for running multi-step task plans
///
/// The PlanExecutor handles:
/// - Sequential step execution
/// - Timeout enforcement (per-step and total)
/// - $prev reference resolution
/// - Progress event callbacks
/// - Rollback on failure (when enabled)
///
/// # Example
///
/// ```rust,ignore
/// let executor = PlanExecutor::new(registry, event_handler);
/// let plan = TaskPlan::new("Search and summarize", steps);
/// let result = executor.execute(plan).await?;
///
/// if result.success {
///     println!("Final output: {:?}", result.final_output);
/// }
/// ```
pub struct PlanExecutor {
    /// Tool registry for executing tools
    registry: Arc<NativeToolRegistry>,

    /// Event handler for progress callbacks
    event_handler: Arc<dyn AetherEventHandler>,

    /// Executor configuration
    config: PlanExecutorConfig,
}

impl PlanExecutor {
    /// Create a new PlanExecutor
    pub fn new(
        registry: Arc<NativeToolRegistry>,
        event_handler: Arc<dyn AetherEventHandler>,
    ) -> Self {
        Self {
            registry,
            event_handler,
            config: PlanExecutorConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(mut self, config: PlanExecutorConfig) -> Self {
        self.config = config;
        self
    }

    /// Execute a task plan
    ///
    /// Runs all steps sequentially, handling timeouts and errors.
    /// Sends progress events if configured.
    ///
    /// # Arguments
    ///
    /// * `plan` - The task plan to execute
    ///
    /// # Returns
    ///
    /// `PlanExecutionResult` with the final output and step results
    pub async fn execute(&self, plan: TaskPlan) -> Result<PlanExecutionResult> {
        let start = Instant::now();
        let plan_id = plan.id;
        let total_steps = plan.steps.len() as u32;

        info!(
            plan_id = %plan_id,
            steps = total_steps,
            description = %plan.description,
            "PlanExecutor: Starting execution"
        );

        // Send plan started event
        if self.config.send_progress_events {
            self.event_handler.on_agent_started(
                plan_id.to_string(),
                total_steps,
                plan.description.clone(),
            );
        }

        // Create execution context
        let mut ctx = PlanExecutionContext::new(plan);

        // Execute steps sequentially
        while !ctx.is_complete() && !ctx.cancelled {
            // Check total timeout
            if start.elapsed().as_millis() as u64 > self.config.max_total_timeout_ms {
                warn!(
                    plan_id = %plan_id,
                    elapsed_ms = start.elapsed().as_millis(),
                    "PlanExecutor: Total timeout exceeded"
                );

                // Attempt rollback if enabled
                if self.config.enable_rollback {
                    self.attempt_rollback(&ctx).await;
                }

                // Send failure event
                if self.config.send_progress_events {
                    self.event_handler.on_agent_completed(
                        plan_id.to_string(),
                        false,
                        start.elapsed().as_millis() as u64,
                        "Total execution timeout exceeded".to_string(),
                    );
                }

                return Ok(PlanExecutionResult::failure(
                    plan_id,
                    "Total execution timeout exceeded",
                    ctx.step_results,
                ));
            }

            // Get current step
            let step_index = ctx.current_step;
            let step = ctx.plan.steps[step_index].clone();

            // Execute the step
            let result = self.execute_step(&step, &ctx).await;

            // Send step completed event
            if self.config.send_progress_events {
                self.event_handler.on_agent_tool_completed(
                    plan_id.to_string(),
                    step.index,
                    step.tool_name.clone(),
                    result.success,
                    truncate_preview(&result.output, 100),
                );
            }

            // Handle step failure
            if !result.success && self.config.stop_on_error {
                warn!(
                    plan_id = %plan_id,
                    step = step.index,
                    tool = %step.tool_name,
                    error = ?result.error,
                    "PlanExecutor: Step failed, stopping execution"
                );

                ctx.add_result(result);

                // Attempt rollback if enabled
                if self.config.enable_rollback {
                    self.attempt_rollback(&ctx).await;
                }

                // Send failure event
                if self.config.send_progress_events {
                    let error_msg = ctx
                        .step_results
                        .last()
                        .and_then(|r| r.error.clone())
                        .unwrap_or_else(|| "Unknown error".to_string());

                    self.event_handler.on_agent_completed(
                        plan_id.to_string(),
                        false,
                        start.elapsed().as_millis() as u64,
                        error_msg,
                    );
                }

                let error_msg = ctx
                    .step_results
                    .last()
                    .and_then(|r| r.error.clone())
                    .unwrap_or_else(|| "Step execution failed".to_string());

                return Ok(PlanExecutionResult::failure(
                    plan_id,
                    error_msg,
                    ctx.step_results,
                ));
            }

            // Add result and advance
            ctx.add_result(result);
        }

        // All steps completed successfully
        let final_output = ctx.final_output();
        let total_duration_ms = start.elapsed().as_millis() as u64;

        info!(
            plan_id = %plan_id,
            steps_executed = ctx.step_results.len(),
            duration_ms = total_duration_ms,
            "PlanExecutor: Execution completed successfully"
        );

        // Send completion event
        if self.config.send_progress_events {
            self.event_handler.on_agent_completed(
                plan_id.to_string(),
                true,
                total_duration_ms,
                truncate_preview(&final_output, 200),
            );
        }

        Ok(PlanExecutionResult::success(
            plan_id,
            final_output,
            ctx.step_results,
        ))
    }

    /// Execute a single step
    async fn execute_step(&self, step: &PlanStep, ctx: &PlanExecutionContext) -> StepResult {
        let start = Instant::now();

        debug!(
            step = step.index,
            tool = %step.tool_name,
            "PlanExecutor: Executing step"
        );

        // Send step started event
        if self.config.send_progress_events {
            self.event_handler.on_agent_tool_started(
                ctx.plan.id.to_string(),
                step.index,
                step.tool_name.clone(),
                step.description.clone(),
            );
        }

        // Resolve $prev references in parameters
        let resolved_params = match self.resolve_params(&step.parameters, ctx) {
            Ok(params) => params,
            Err(e) => {
                return StepResult::failure(
                    step.index,
                    format!("Parameter resolution failed: {}", e),
                    start.elapsed().as_millis() as u64,
                );
            }
        };

        // Convert parameters to string for tool execution
        let params_str = serde_json::to_string(&resolved_params).unwrap_or_default();

        // Determine timeout (use step-specific or default)
        let timeout_ms = if step.timeout_ms > 0 {
            step.timeout_ms
        } else {
            self.config.default_step_timeout_ms
        };

        // Execute tool with timeout
        let execution_result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            self.registry.execute(&step.tool_name, &params_str),
        )
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match execution_result {
            Ok(Ok(output)) => {
                debug!(
                    step = step.index,
                    tool = %step.tool_name,
                    duration_ms,
                    "PlanExecutor: Step completed successfully"
                );

                // Extract data from output (use content if data is None)
                let output_value = output.data.unwrap_or_else(|| {
                    serde_json::Value::String(output.content.clone())
                });

                // Create success result with optional rollback data
                let mut result = StepResult::success(step.index, output_value, duration_ms);

                // If tool is reversible, store rollback data
                if step.safety_level.is_reversible() {
                    // Rollback data could be stored in output.metadata if available
                    // For now, we store the original parameters as rollback hint
                    result = result.with_rollback_data(resolved_params);
                }

                result
            }
            Ok(Err(e)) => {
                warn!(
                    step = step.index,
                    tool = %step.tool_name,
                    error = %e,
                    "PlanExecutor: Step execution failed"
                );
                StepResult::failure(step.index, e.to_string(), duration_ms)
            }
            Err(_) => {
                warn!(
                    step = step.index,
                    tool = %step.tool_name,
                    timeout_ms,
                    "PlanExecutor: Step timed out"
                );
                StepResult::failure(
                    step.index,
                    format!("Step timed out after {}ms", timeout_ms),
                    duration_ms,
                )
            }
        }
    }

    /// Resolve $prev references in parameters
    ///
    /// Replaces "$prev" with the output from the previous step.
    /// Supports nested references like `{"input": "$prev"}`.
    ///
    /// # Arguments
    ///
    /// * `params` - The parameter Value (may contain $prev)
    /// * `ctx` - Execution context with previous step results
    ///
    /// # Returns
    ///
    /// Resolved parameters with $prev replaced
    pub fn resolve_params(&self, params: &Value, ctx: &PlanExecutionContext) -> Result<Value> {
        self.resolve_value(params, ctx)
    }

    /// Recursively resolve $prev in a JSON value
    fn resolve_value(&self, value: &Value, ctx: &PlanExecutionContext) -> Result<Value> {
        match value {
            // String: check for $prev reference
            Value::String(s) => {
                if s == "$prev" {
                    // Direct $prev reference
                    match ctx.prev_output() {
                        Some(output) => Ok(output.clone()),
                        None => {
                            // First step - no previous output
                            if ctx.current_step == 0 {
                                Err(AetherError::other(
                                    "$prev cannot be used in the first step",
                                ))
                            } else {
                                // Previous step had no output
                                Ok(Value::Null)
                            }
                        }
                    }
                } else if s.contains("$prev") {
                    // Embedded $prev in string (e.g., "Process: $prev")
                    match ctx.prev_output() {
                        Some(output) => {
                            let prev_str = match output {
                                Value::String(s) => s.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            };
                            Ok(Value::String(s.replace("$prev", &prev_str)))
                        }
                        None => {
                            if ctx.current_step == 0 {
                                Err(AetherError::other(
                                    "$prev cannot be used in the first step",
                                ))
                            } else {
                                Ok(Value::String(s.replace("$prev", "")))
                            }
                        }
                    }
                } else {
                    // No $prev reference
                    Ok(value.clone())
                }
            }

            // Object: recursively resolve each value
            Value::Object(map) => {
                let mut resolved = serde_json::Map::new();
                for (k, v) in map {
                    resolved.insert(k.clone(), self.resolve_value(v, ctx)?);
                }
                Ok(Value::Object(resolved))
            }

            // Array: recursively resolve each element
            Value::Array(arr) => {
                let resolved: Result<Vec<Value>> =
                    arr.iter().map(|v| self.resolve_value(v, ctx)).collect();
                Ok(Value::Array(resolved?))
            }

            // Other types: no resolution needed
            _ => Ok(value.clone()),
        }
    }

    /// Attempt to rollback completed steps
    ///
    /// Called when execution fails and rollback is enabled.
    /// Only rolls back steps that have rollback data and are reversible.
    async fn attempt_rollback(&self, ctx: &PlanExecutionContext) {
        if ctx.rollback_data.is_empty() {
            debug!("PlanExecutor: No rollback data available");
            return;
        }

        info!(
            plan_id = %ctx.plan.id,
            rollback_count = ctx.rollback_data.len(),
            "PlanExecutor: Attempting rollback"
        );

        // Rollback in reverse order
        for (step_index, rollback_data) in ctx.rollback_data.iter().rev() {
            // Get the step's safety level
            if let Some(step) = ctx.plan.steps.get(*step_index as usize) {
                if !step.safety_level.is_reversible() {
                    warn!(
                        step = step_index,
                        safety_level = ?step.safety_level,
                        "PlanExecutor: Step is not reversible, skipping rollback"
                    );
                    continue;
                }

                debug!(
                    step = step_index,
                    tool = %step.tool_name,
                    "PlanExecutor: Rolling back step"
                );

                // For now, we just log the rollback attempt
                // Actual rollback implementation would require tool-specific handlers
                // that implement the RollbackCapable trait (Phase 3.3)
                info!(
                    step = step_index,
                    rollback_data = %rollback_data,
                    "PlanExecutor: Rollback recorded (implementation in Phase 3.3)"
                );
            }
        }
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Truncate a JSON value to a preview string
fn truncate_preview(value: &Value, max_len: usize) -> String {
    let s = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };

    if s.len() <= max_len {
        s
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::plan::PlanStep;
    use serde_json::json;

    // Mock event handler for testing
    struct MockEventHandler;

    impl AetherEventHandler for MockEventHandler {
        fn on_state_changed(&self, _: crate::ProcessingState) {}
        fn on_error(&self, _: String, _: Option<String>) {}
        fn on_response_chunk(&self, _: String) {}
        fn on_error_typed(&self, _: crate::ErrorType, _: String) {}
        fn on_progress(&self, _: f32) {}
        fn on_ai_processing_started(&self, _: String, _: String) {}
        fn on_ai_response_received(&self, _: String) {}
        fn on_provider_fallback(&self, _: String, _: String) {}
        fn on_config_changed(&self) {}
        fn on_typewriter_progress(&self, _: f32) {}
        fn on_typewriter_cancelled(&self) {}
        fn on_clarification_needed(
            &self,
            _: crate::ClarificationRequest,
        ) -> crate::ClarificationResult {
            crate::ClarificationResult::cancelled()
        }
        fn on_conversation_started(&self, _: String) {}
        fn on_conversation_turn_completed(&self, _: crate::ConversationTurn) {}
        fn on_conversation_continuation_ready(&self) {}
        fn on_conversation_ended(&self, _: String, _: u32) {}
        fn on_confirmation_needed(&self, _: crate::PendingConfirmationInfo) {}
        fn on_confirmation_expired(&self, _: String) {}
        fn on_tools_changed(&self, _: u32) {}
        fn on_tools_refresh_needed(&self) {}
        fn on_mcp_startup_complete(&self, _: crate::McpStartupReportFFI) {}
        fn on_agent_started(&self, _: String, _: u32, _: String) {}
        fn on_agent_tool_started(&self, _: String, _: u32, _: String, _: String) {}
        fn on_agent_tool_completed(&self, _: String, _: u32, _: String, _: bool, _: String) {}
        fn on_agent_completed(&self, _: String, _: bool, _: u64, _: String) {}
    }

    fn create_test_context(prev_output: Option<Value>) -> PlanExecutionContext {
        let steps = vec![
            PlanStep::new(1, "step1", json!({}), "Step 1"),
            PlanStep::new(2, "step2", json!({"input": "$prev"}), "Step 2"),
        ];
        let plan = TaskPlan::new("Test plan", steps);
        let mut ctx = PlanExecutionContext::new(plan);

        if let Some(output) = prev_output {
            ctx.add_result(StepResult::success(1, output, 100));
        }

        ctx
    }

    #[test]
    fn test_resolve_params_simple_prev() {
        let registry = Arc::new(NativeToolRegistry::new());
        let handler: Arc<dyn AetherEventHandler> = Arc::new(MockEventHandler);
        let executor = PlanExecutor::new(registry, handler);

        // Context with previous output
        let ctx = create_test_context(Some(json!({"result": "test data"})));

        // Resolve $prev reference
        let params = json!({"input": "$prev"});
        let resolved = executor.resolve_params(&params, &ctx).unwrap();

        assert_eq!(resolved["input"]["result"], "test data");
    }

    #[test]
    fn test_resolve_params_embedded_prev() {
        let registry = Arc::new(NativeToolRegistry::new());
        let handler: Arc<dyn AetherEventHandler> = Arc::new(MockEventHandler);
        let executor = PlanExecutor::new(registry, handler);

        // Context with string output
        let ctx = create_test_context(Some(json!("hello world")));

        // Resolve embedded $prev
        let params = json!({"message": "Result: $prev"});
        let resolved = executor.resolve_params(&params, &ctx).unwrap();

        assert_eq!(resolved["message"], "Result: hello world");
    }

    #[test]
    fn test_resolve_params_nested() {
        let registry = Arc::new(NativeToolRegistry::new());
        let handler: Arc<dyn AetherEventHandler> = Arc::new(MockEventHandler);
        let executor = PlanExecutor::new(registry, handler);

        let ctx = create_test_context(Some(json!({"data": "test"})));

        // Nested structure with $prev
        let params = json!({
            "outer": {
                "inner": "$prev"
            },
            "list": ["$prev", "other"]
        });

        let resolved = executor.resolve_params(&params, &ctx).unwrap();

        assert_eq!(resolved["outer"]["inner"]["data"], "test");
        assert_eq!(resolved["list"][0]["data"], "test");
        assert_eq!(resolved["list"][1], "other");
    }

    #[test]
    fn test_resolve_params_first_step_error() {
        let registry = Arc::new(NativeToolRegistry::new());
        let handler: Arc<dyn AetherEventHandler> = Arc::new(MockEventHandler);
        let executor = PlanExecutor::new(registry, handler);

        // Empty context (first step)
        let ctx = create_test_context(None);

        // Try to use $prev in first step
        let params = json!({"input": "$prev"});
        let result = executor.resolve_params(&params, &ctx);

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_params_no_prev() {
        let registry = Arc::new(NativeToolRegistry::new());
        let handler: Arc<dyn AetherEventHandler> = Arc::new(MockEventHandler);
        let executor = PlanExecutor::new(registry, handler);

        let ctx = create_test_context(Some(json!("test")));

        // No $prev references
        let params = json!({"query": "search term", "limit": 10});
        let resolved = executor.resolve_params(&params, &ctx).unwrap();

        assert_eq!(resolved, params);
    }

    #[test]
    fn test_executor_config_default() {
        let config = PlanExecutorConfig::default();

        assert_eq!(config.default_step_timeout_ms, 30_000);
        assert_eq!(config.max_total_timeout_ms, 300_000);
        assert!(config.stop_on_error);
        assert!(config.enable_rollback);
        assert!(config.send_progress_events);
    }

    #[test]
    fn test_truncate_preview() {
        assert_eq!(truncate_preview(&json!("short"), 100), "short");
        assert_eq!(
            truncate_preview(&json!("a".repeat(150)), 50),
            format!("{}...", "a".repeat(47))
        );
    }
}

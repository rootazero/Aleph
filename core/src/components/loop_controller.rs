//! Loop controller component - manages agentic loop with protection mechanisms.
//!
//! Subscribes to: ToolCallCompleted, ToolCallFailed, PlanCreated
//! Publishes: LoopContinue, LoopStop, ToolCallRequested

use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::RwLock;

use crate::components::types::{ExecutionSession, ToolCallRecord};
use crate::event::{
    AlephEvent, EventContext, EventHandler, EventType, HandlerError, LoopState, PlanStep,
    StepStatus, StopReason, TaskPlan, ToolCallRequest,
};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the agentic loop
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// Maximum number of loop iterations before stopping
    pub max_iterations: u32,
    /// Maximum tokens per session before stopping
    pub max_tokens: u64,
    /// Number of consecutive identical calls to trigger doom loop detection
    pub doom_loop_threshold: u32,
    /// Time window in seconds for doom loop detection
    pub doom_loop_window_secs: u64,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_tokens: 100_000,
            doom_loop_threshold: 3,
            doom_loop_window_secs: 60,
        }
    }
}

impl LoopConfig {
    /// Create a new LoopConfig with custom values
    pub fn new(max_iterations: u32, max_tokens: u64, doom_loop_threshold: u32) -> Self {
        Self {
            max_iterations,
            max_tokens,
            doom_loop_threshold,
            doom_loop_window_secs: 60,
        }
    }
}

// ============================================================================
// Loop Controller Component
// ============================================================================

/// Loop Controller - manages the agentic loop with protection mechanisms
///
/// This component:
/// - Subscribes to ToolCallCompleted, ToolCallFailed, PlanCreated events
/// - Manages current plan execution and step progression
/// - Implements protection mechanisms:
///   - Maximum iteration limit
///   - Doom loop detection (repeated tool+input patterns)
///   - Token budget enforcement
///   - Abort signal handling
/// - Publishes LoopContinue, LoopStop, ToolCallRequested events
pub struct LoopController {
    /// Configuration for loop protection
    config: LoopConfig,
    /// Current plan being executed
    current_plan: RwLock<Option<TaskPlan>>,
    /// Internal iteration counter (incremented on each ToolCallCompleted)
    iteration_count: AtomicU32,
    /// Recent tool calls for doom loop detection
    recent_calls: RwLock<Vec<ToolCallRecord>>,
}

impl Default for LoopController {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopController {
    /// Create a new LoopController with default config
    pub fn new() -> Self {
        Self {
            config: LoopConfig::default(),
            current_plan: RwLock::new(None),
            iteration_count: AtomicU32::new(0),
            recent_calls: RwLock::new(Vec::new()),
        }
    }

    /// Create a LoopController with custom config
    pub fn with_config(config: LoopConfig) -> Self {
        Self {
            config,
            current_plan: RwLock::new(None),
            iteration_count: AtomicU32::new(0),
            recent_calls: RwLock::new(Vec::new()),
        }
    }

    // ========================================================================
    // Guard Checks
    // ========================================================================

    /// Check all protection guards
    ///
    /// Returns Some(StopReason) if any guard triggers, None otherwise.
    /// Checks in priority order:
    /// 1. Abort signal
    /// 2. Max iterations
    /// 3. Doom loop detection
    /// 4. Token limit
    pub fn check_guards(
        &self,
        session: &ExecutionSession,
        ctx: &EventContext,
    ) -> Option<StopReason> {
        // 1. Check abort signal (highest priority)
        if ctx.is_aborted() {
            return Some(StopReason::UserAborted);
        }

        // 2. Check max iterations
        if session.iteration_count >= self.config.max_iterations {
            return Some(StopReason::MaxIterationsReached);
        }

        // 3. Check doom loop
        if self.detect_doom_loop(&session.recent_calls) {
            return Some(StopReason::DoomLoopDetected);
        }

        // 4. Check token limit
        if session.total_tokens >= self.config.max_tokens {
            return Some(StopReason::TokenLimitReached);
        }

        None
    }

    /// Detect doom loop pattern in recent tool calls
    ///
    /// A doom loop is detected when the last N calls (where N = doom_loop_threshold)
    /// have the same tool name and input parameters.
    ///
    /// Returns true if doom loop is detected.
    pub fn detect_doom_loop(&self, recent_calls: &[ToolCallRecord]) -> bool {
        let threshold = self.config.doom_loop_threshold as usize;

        // Not enough calls to detect doom loop
        if recent_calls.len() < threshold {
            return false;
        }

        // Get the last N calls
        let last_n = &recent_calls[recent_calls.len() - threshold..];

        // Check if all calls have the same tool and input
        if last_n.is_empty() {
            return false;
        }

        let first_tool = &last_n[0].tool;
        let first_input = &last_n[0].input;

        last_n
            .iter()
            .all(|call| &call.tool == first_tool && &call.input == first_input)
    }

    // ========================================================================
    // Plan Management
    // ========================================================================

    /// Store a new plan and prepare for execution
    pub async fn set_plan(&self, plan: TaskPlan) {
        *self.current_plan.write().await = Some(plan);
    }

    /// Get the next executable step from the current plan
    ///
    /// A step is executable if:
    /// 1. Its status is Pending
    /// 2. All its dependencies are Completed
    ///
    /// Returns None if no executable step is found or no plan exists.
    pub async fn get_next_step(&self) -> Option<PlanStep> {
        let plan = self.current_plan.read().await;
        let plan = plan.as_ref()?;

        self.find_next_step(&plan.steps)
    }

    /// Find the next executable step in a list of steps
    fn find_next_step(&self, steps: &[PlanStep]) -> Option<PlanStep> {
        for step in steps {
            // Only consider pending steps
            if step.status != StepStatus::Pending {
                continue;
            }

            // Check if all dependencies are completed
            let deps_completed = step.depends_on.iter().all(|dep_id| {
                steps
                    .iter()
                    .find(|s| &s.id == dep_id)
                    .map(|s| matches!(s.status, StepStatus::Completed))
                    .unwrap_or(false)
            });

            if deps_completed {
                return Some(step.clone());
            }
        }

        None
    }

    /// Convert a PlanStep to a ToolCallRequest
    pub fn step_to_tool_call(&self, step: &PlanStep) -> ToolCallRequest {
        ToolCallRequest {
            tool: step.tool.clone(),
            parameters: step.parameters.clone(),
            plan_step_id: Some(step.id.clone()),
        }
    }

    /// Update the status of a step in the current plan
    pub async fn update_step_status(&self, step_id: &str, status: StepStatus) {
        let mut plan = self.current_plan.write().await;
        if let Some(ref mut plan) = *plan {
            if let Some(step) = plan.steps.iter_mut().find(|s| s.id == step_id) {
                step.status = status;
            }
        }
    }

    /// Check if the current plan is complete
    ///
    /// A plan is complete if:
    /// - There is no plan (considered complete)
    /// - All steps are either Completed, Failed, or Skipped
    pub async fn is_plan_complete(&self) -> bool {
        let plan = self.current_plan.read().await;
        match plan.as_ref() {
            None => true,
            Some(plan) => plan.steps.iter().all(|step| {
                matches!(
                    step.status,
                    StepStatus::Completed | StepStatus::Failed(_) | StepStatus::Skipped
                )
            }),
        }
    }

    /// Clear the current plan
    pub async fn clear_plan(&self) {
        *self.current_plan.write().await = None;
    }

    /// Get current plan (for testing/debugging)
    #[cfg(test)]
    pub async fn get_plan(&self) -> Option<TaskPlan> {
        self.current_plan.read().await.clone()
    }
}

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for LoopController {
    fn name(&self) -> &'static str {
        "LoopController"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
            EventType::PlanCreated,
        ]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        match event {
            // ================================================================
            // PlanCreated: Store plan and start first step
            // ================================================================
            AlephEvent::PlanCreated(plan) => {
                // Store the plan
                self.set_plan(plan.clone()).await;

                // Check if plan has any steps
                if plan.steps.is_empty() {
                    return Ok(vec![AlephEvent::LoopStop(StopReason::EmptyPlan)]);
                }

                // Get the first executable step
                if let Some(step) = self.get_next_step().await {
                    // Mark step as running
                    self.update_step_status(&step.id, StepStatus::Running).await;

                    // Convert to tool call request
                    let tool_request = self.step_to_tool_call(&step);

                    Ok(vec![AlephEvent::ToolCallRequested(tool_request)])
                } else {
                    // No executable step found (all blocked or complete)
                    Ok(vec![AlephEvent::LoopStop(StopReason::Completed)])
                }
            }

            // ================================================================
            // ToolCallCompleted: Update step, check guards, continue or stop
            // ================================================================
            AlephEvent::ToolCallCompleted(result) => {
                // Update step status if this was a plan step
                if let Some(step_id) = &result.input.get("plan_step_id").and_then(|v| v.as_str()) {
                    self.update_step_status(step_id, StepStatus::Completed)
                        .await;
                } else {
                    // Try to find step by matching the tool call
                    let step_id = {
                        let plan = self.current_plan.read().await;
                        plan.as_ref().and_then(|p| {
                            p.steps
                                .iter()
                                .find(|s| s.status == StepStatus::Running && s.tool == result.tool)
                                .map(|s| s.id.clone())
                        })
                    };
                    if let Some(id) = step_id {
                        self.update_step_status(&id, StepStatus::Completed).await;
                    }
                }

                // Track iteration count and recent calls internally
                let iteration = self.iteration_count.fetch_add(1, Ordering::SeqCst) + 1;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                {
                    let mut calls = self.recent_calls.write().await;
                    calls.push(ToolCallRecord {
                        tool: result.tool.clone(),
                        input: result.input.clone(),
                        timestamp: now,
                    });
                    // Keep only recent calls within doom loop window
                    let cutoff = now - self.config.doom_loop_window_secs as i64;
                    calls.retain(|c| c.timestamp >= cutoff);
                }

                let recent = self.recent_calls.read().await;
                let mut session = ExecutionSession::default();
                session.iteration_count = iteration;
                session.recent_calls = recent.clone();

                // Check guards
                if let Some(stop_reason) = self.check_guards(&session, ctx) {
                    return Ok(vec![AlephEvent::LoopStop(stop_reason)]);
                }

                // Check if plan is complete
                if self.is_plan_complete().await {
                    return Ok(vec![AlephEvent::LoopStop(StopReason::Completed)]);
                }

                // Get next step
                if let Some(step) = self.get_next_step().await {
                    // Mark step as running
                    self.update_step_status(&step.id, StepStatus::Running).await;

                    // Publish LoopContinue and ToolCallRequested
                    let loop_state = LoopState {
                        session_id: ctx
                            .get_session_id()
                            .await
                            .unwrap_or_else(|| "unknown".to_string()),
                        iteration: session.iteration_count + 1,
                        total_tokens: session.total_tokens,
                        last_tool: Some(result.tool.clone()),
                        model: session.model.clone(),
                    };

                    let tool_request = self.step_to_tool_call(&step);

                    Ok(vec![
                        AlephEvent::LoopContinue(loop_state),
                        AlephEvent::ToolCallRequested(tool_request),
                    ])
                } else {
                    // No more executable steps
                    Ok(vec![AlephEvent::LoopStop(StopReason::Completed)])
                }
            }

            // ================================================================
            // ToolCallFailed: Stop with error
            // ================================================================
            AlephEvent::ToolCallFailed(error) => {
                // Mark step as failed if this was a plan step
                let step_id = {
                    let plan = self.current_plan.read().await;
                    plan.as_ref().and_then(|p| {
                        p.steps
                            .iter()
                            .find(|s| s.status == StepStatus::Running && s.tool == error.tool)
                            .map(|s| s.id.clone())
                    })
                };
                if let Some(id) = step_id {
                    self.update_step_status(&id, StepStatus::Failed(error.error.clone()))
                        .await;
                }

                // Stop the loop with error
                Ok(vec![AlephEvent::LoopStop(StopReason::Error(format!(
                    "Tool '{}' failed: {}",
                    error.tool, error.error
                )))])
            }

            // Other events are ignored
            _ => Ok(vec![]),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBus;
    use serde_json::Value;

    // ========================================================================
    // LoopConfig Tests
    // ========================================================================

    #[test]
    fn test_loop_config_default() {
        let config = LoopConfig::default();

        assert_eq!(config.max_iterations, 50);
        assert_eq!(config.max_tokens, 100_000);
        assert_eq!(config.doom_loop_threshold, 3);
        assert_eq!(config.doom_loop_window_secs, 60);
    }

    #[test]
    fn test_loop_config_custom() {
        let config = LoopConfig::new(100, 200_000, 5);

        assert_eq!(config.max_iterations, 100);
        assert_eq!(config.max_tokens, 200_000);
        assert_eq!(config.doom_loop_threshold, 5);
    }

    // ========================================================================
    // Doom Loop Detection Tests
    // ========================================================================

    fn create_tool_call_record(tool: &str, input: Value) -> ToolCallRecord {
        ToolCallRecord {
            tool: tool.to_string(),
            input,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    #[test]
    fn test_doom_loop_detection_false_not_enough_calls() {
        let controller = LoopController::new();

        // Only 2 calls, threshold is 3
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
        ];

        assert!(!controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_detection_false_different_tools() {
        let controller = LoopController::new();

        // 3 calls but different tools
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("read", serde_json::json!({"q": "test"})),
            create_tool_call_record("write", serde_json::json!({"q": "test"})),
        ];

        assert!(!controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_detection_false_different_inputs() {
        let controller = LoopController::new();

        // 3 calls same tool but different inputs
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test1"})),
            create_tool_call_record("search", serde_json::json!({"q": "test2"})),
            create_tool_call_record("search", serde_json::json!({"q": "test3"})),
        ];

        assert!(!controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_detection_true() {
        let controller = LoopController::new();

        // 3 identical calls - should trigger doom loop
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
        ];

        assert!(controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_detection_true_more_than_threshold() {
        let controller = LoopController::new();

        // 5 identical calls at the end
        let calls = vec![
            create_tool_call_record("other", serde_json::json!({"x": 1})),
            create_tool_call_record("other", serde_json::json!({"x": 2})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
        ];

        assert!(controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_detection_false_pattern_broken() {
        let controller = LoopController::new();

        // Pattern broken by different call at the end
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "different"})),
        ];

        assert!(!controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_empty_calls() {
        let controller = LoopController::new();

        assert!(!controller.detect_doom_loop(&[]));
    }

    #[test]
    fn test_doom_loop_custom_threshold() {
        let config = LoopConfig::new(50, 100_000, 5);
        let controller = LoopController::with_config(config);

        // Only 3 identical calls, threshold is 5
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
        ];

        assert!(!controller.detect_doom_loop(&calls));

        // 5 identical calls
        let calls = vec![
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
            create_tool_call_record("search", serde_json::json!({"q": "test"})),
        ];

        assert!(controller.detect_doom_loop(&calls));
    }

    // ========================================================================
    // Guard Check Tests
    // ========================================================================

    #[test]
    fn test_check_guards_abort() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);
        ctx.abort();

        let session = ExecutionSession::default();
        let result = controller.check_guards(&session, &ctx);

        assert!(matches!(result, Some(StopReason::UserAborted)));
    }

    #[test]
    fn test_check_guards_max_iterations() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let session = ExecutionSession {
            iteration_count: 50, // At max
            ..ExecutionSession::default()
        };

        let result = controller.check_guards(&session, &ctx);

        assert!(matches!(result, Some(StopReason::MaxIterationsReached)));
    }

    #[test]
    fn test_check_guards_token_limit() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let session = ExecutionSession {
            total_tokens: 100_000, // At max
            ..ExecutionSession::default()
        };

        let result = controller.check_guards(&session, &ctx);

        assert!(matches!(result, Some(StopReason::TokenLimitReached)));
    }

    #[test]
    fn test_check_guards_doom_loop() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let session = ExecutionSession {
            recent_calls: vec![
                create_tool_call_record("search", serde_json::json!({"q": "test"})),
                create_tool_call_record("search", serde_json::json!({"q": "test"})),
                create_tool_call_record("search", serde_json::json!({"q": "test"})),
            ],
            ..ExecutionSession::default()
        };

        let result = controller.check_guards(&session, &ctx);

        assert!(matches!(result, Some(StopReason::DoomLoopDetected)));
    }

    #[test]
    fn test_check_guards_no_issues() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let session = ExecutionSession::default();
        let result = controller.check_guards(&session, &ctx);

        assert!(result.is_none());
    }

    // ========================================================================
    // Plan Step Management Tests
    // ========================================================================

    fn create_test_plan() -> TaskPlan {
        TaskPlan {
            id: "test-plan".to_string(),
            steps: vec![
                PlanStep {
                    id: "step_1".to_string(),
                    description: "First step".to_string(),
                    tool: "search".to_string(),
                    parameters: serde_json::json!({"q": "test"}),
                    depends_on: vec![],
                    status: StepStatus::Pending,
                },
                PlanStep {
                    id: "step_2".to_string(),
                    description: "Second step".to_string(),
                    tool: "read".to_string(),
                    parameters: serde_json::json!({"file": "test.txt"}),
                    depends_on: vec!["step_1".to_string()],
                    status: StepStatus::Pending,
                },
                PlanStep {
                    id: "step_3".to_string(),
                    description: "Third step".to_string(),
                    tool: "write".to_string(),
                    parameters: serde_json::json!({"file": "out.txt"}),
                    depends_on: vec!["step_2".to_string()],
                    status: StepStatus::Pending,
                },
            ],
            parallel_groups: vec![
                vec!["step_1".to_string()],
                vec!["step_2".to_string()],
                vec!["step_3".to_string()],
            ],
            current_step_index: 0,
        }
    }

    #[tokio::test]
    async fn test_get_next_step() {
        let controller = LoopController::new();
        let plan = create_test_plan();

        controller.set_plan(plan).await;

        // First call should return step_1 (no dependencies)
        let step = controller.get_next_step().await;
        assert!(step.is_some());
        assert_eq!(step.unwrap().id, "step_1");
    }

    #[tokio::test]
    async fn test_get_next_step_blocked() {
        let controller = LoopController::new();
        let mut plan = create_test_plan();

        // Mark step_1 as running (not completed)
        plan.steps[0].status = StepStatus::Running;

        controller.set_plan(plan).await;

        // step_2 depends on step_1, which is not completed
        let step = controller.get_next_step().await;
        assert!(step.is_none());
    }

    #[tokio::test]
    async fn test_get_next_step_with_completed_dependency() {
        let controller = LoopController::new();
        let mut plan = create_test_plan();

        // Mark step_1 as completed
        plan.steps[0].status = StepStatus::Completed;

        controller.set_plan(plan).await;

        // step_2 should now be executable
        let step = controller.get_next_step().await;
        assert!(step.is_some());
        assert_eq!(step.unwrap().id, "step_2");
    }

    #[tokio::test]
    async fn test_update_step_status() {
        let controller = LoopController::new();
        let plan = create_test_plan();

        controller.set_plan(plan).await;

        // Update step_1 to completed
        controller
            .update_step_status("step_1", StepStatus::Completed)
            .await;

        // Verify update
        let plan = controller.get_plan().await.unwrap();
        assert_eq!(plan.steps[0].status, StepStatus::Completed);
    }

    #[tokio::test]
    async fn test_is_plan_complete_false() {
        let controller = LoopController::new();
        let plan = create_test_plan();

        controller.set_plan(plan).await;

        assert!(!controller.is_plan_complete().await);
    }

    #[tokio::test]
    async fn test_is_plan_complete_true() {
        let controller = LoopController::new();
        let mut plan = create_test_plan();

        // Mark all steps as completed
        for step in &mut plan.steps {
            step.status = StepStatus::Completed;
        }

        controller.set_plan(plan).await;

        assert!(controller.is_plan_complete().await);
    }

    #[tokio::test]
    async fn test_is_plan_complete_no_plan() {
        let controller = LoopController::new();

        // No plan is considered complete
        assert!(controller.is_plan_complete().await);
    }

    #[tokio::test]
    async fn test_step_to_tool_call() {
        let controller = LoopController::new();

        let step = PlanStep {
            id: "test-step".to_string(),
            description: "Test step".to_string(),
            tool: "search".to_string(),
            parameters: serde_json::json!({"query": "test"}),
            depends_on: vec![],
            status: StepStatus::Pending,
        };

        let request = controller.step_to_tool_call(&step);

        assert_eq!(request.tool, "search");
        assert_eq!(request.parameters, serde_json::json!({"query": "test"}));
        assert_eq!(request.plan_step_id, Some("test-step".to_string()));
    }

    // ========================================================================
    // EventHandler Implementation Tests
    // ========================================================================

    #[test]
    fn test_handler_name() {
        let controller = LoopController::new();
        assert_eq!(controller.name(), "LoopController");
    }

    #[test]
    fn test_handler_subscriptions() {
        let controller = LoopController::new();
        let subs = controller.subscriptions();

        assert_eq!(subs.len(), 3);
        assert!(subs.contains(&EventType::ToolCallCompleted));
        assert!(subs.contains(&EventType::ToolCallFailed));
        assert!(subs.contains(&EventType::PlanCreated));
    }

    #[tokio::test]
    async fn test_handler_ignores_other_events() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // InputReceived event should be ignored
        let event = AlephEvent::InputReceived(crate::event::InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });
        let result = controller.handle(&event, &ctx).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_plan_created_starts_first_step() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let plan = create_test_plan();
        let event = AlephEvent::PlanCreated(plan);

        let result = controller.handle(&event, &ctx).await.unwrap();

        // Should return a ToolCallRequested event
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AlephEvent::ToolCallRequested(_)));

        if let AlephEvent::ToolCallRequested(request) = &result[0] {
            assert_eq!(request.tool, "search");
            assert_eq!(request.plan_step_id, Some("step_1".to_string()));
        }
    }

    #[tokio::test]
    async fn test_handler_plan_created_empty_plan() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let plan = TaskPlan {
            id: "empty-plan".to_string(),
            steps: vec![],
            parallel_groups: vec![],
            current_step_index: 0,
        };
        let event = AlephEvent::PlanCreated(plan);

        let result = controller.handle(&event, &ctx).await.unwrap();

        // Should return LoopStop with EmptyPlan
        assert_eq!(result.len(), 1);
        assert!(matches!(
            result[0],
            AlephEvent::LoopStop(StopReason::EmptyPlan)
        ));
    }

    #[tokio::test]
    async fn test_handler_tool_call_failed() {
        let controller = LoopController::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let error = crate::event::ToolCallError {
            call_id: "call-1".to_string(),
            tool: "search".to_string(),
            error: "Connection failed".to_string(),
            error_kind: crate::event::ErrorKind::Timeout,
            is_retryable: true,
            attempts: 3,
            session_id: None,
        };
        let event = AlephEvent::ToolCallFailed(error);

        let result = controller.handle(&event, &ctx).await.unwrap();

        // Should return LoopStop with Error
        assert_eq!(result.len(), 1);
        assert!(matches!(
            result[0],
            AlephEvent::LoopStop(StopReason::Error(_))
        ));
    }

    // ========================================================================
    // Builder Tests
    // ========================================================================

    #[test]
    fn test_loop_controller_new() {
        let controller = LoopController::new();
        assert_eq!(controller.config.max_iterations, 50);
    }

    #[test]
    fn test_loop_controller_with_config() {
        let config = LoopConfig::new(100, 50_000, 10);
        let controller = LoopController::with_config(config);

        assert_eq!(controller.config.max_iterations, 100);
        assert_eq!(controller.config.max_tokens, 50_000);
        assert_eq!(controller.config.doom_loop_threshold, 10);
    }

    #[test]
    fn test_loop_controller_default() {
        let controller = LoopController::default();
        assert_eq!(controller.config.max_iterations, 50);
    }
}

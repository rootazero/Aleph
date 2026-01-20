//! Integration tests for component event chain.
//!
//! These tests verify that components work together correctly through the event system.
//! Each component should produce the expected output events when given input events.

#[cfg(test)]
mod tests {
    use crate::components::*;
    use crate::event::{
        AetherEvent, ErrorKind, EventBus, EventContext, EventHandler, InputEvent, PlanRequest,
        PlanStep, StepStatus, StopReason, TaskPlan, TokenUsage, ToolCallError, ToolCallRequest,
        ToolCallResult,
    };

    // ============================================================================
    // Test Helpers
    // ============================================================================

    /// Create a test EventContext with a fresh EventBus
    fn create_test_context() -> EventContext {
        let bus = EventBus::new();
        EventContext::new(bus)
    }

    /// Create a test InputEvent with the given text
    fn create_test_input(text: &str) -> InputEvent {
        InputEvent {
            text: text.to_string(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    /// Create a test PlanRequest with the given text and detected steps
    fn create_plan_request(text: &str, steps: Vec<&str>) -> PlanRequest {
        PlanRequest {
            input: create_test_input(text),
            intent_type: None,
            detected_steps: steps.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Create a test TaskPlan with sequential steps
    fn create_test_plan(steps: Vec<(&str, &str)>) -> TaskPlan {
        let mut plan_steps: Vec<PlanStep> = Vec::new();
        let mut previous_step_id: Option<String> = None;

        for (index, (tool, description)) in steps.iter().enumerate() {
            let step_id = format!("step_{}", index + 1);
            let depends_on = previous_step_id.map(|id| vec![id]).unwrap_or_default();

            let step = PlanStep {
                id: step_id.clone(),
                description: description.to_string(),
                tool: tool.to_string(),
                parameters: serde_json::json!({"input": description}),
                depends_on,
                status: StepStatus::Pending,
            };

            plan_steps.push(step);
            previous_step_id = Some(step_id);
        }

        TaskPlan {
            id: uuid::Uuid::new_v4().to_string(),
            steps: plan_steps,
            parallel_groups: vec![],
            current_step_index: 0,
        }
    }

    /// Create a test ToolCallRequest
    fn create_tool_call_request(tool: &str, input: &str) -> ToolCallRequest {
        ToolCallRequest {
            tool: tool.to_string(),
            parameters: serde_json::json!({"input": input}),
            plan_step_id: None,
        }
    }

    /// Create a test ToolCallResult
    fn create_tool_call_result(tool: &str, output: &str) -> ToolCallResult {
        ToolCallResult {
            call_id: uuid::Uuid::new_v4().to_string(),
            tool: tool.to_string(),
            input: serde_json::json!({}),
            output: output.to_string(),
            started_at: chrono::Utc::now().timestamp_millis() - 100,
            completed_at: chrono::Utc::now().timestamp_millis(),
            token_usage: TokenUsage::default(),
        }
    }

    // ============================================================================
    // Test 1: IntentAnalyzer - Simple Input -> ToolCallRequested
    // ============================================================================

    #[tokio::test]
    async fn test_intent_analyzer_simple_input() {
        // Setup
        let analyzer = IntentAnalyzer::new();
        let ctx = create_test_context();

        // Create a simple input (no multi-step keywords, no step markers)
        let input = create_test_input("Hello, how are you?");
        let event = AetherEvent::InputReceived(input);

        // Handle the event
        let result = analyzer.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::ToolCallRequested(_)),
            "Should produce ToolCallRequested for simple input"
        );

        if let AetherEvent::ToolCallRequested(request) = &events[0] {
            // Simple conversational input should default to general_chat
            assert_eq!(
                request.tool, "general_chat",
                "Simple input should use general_chat tool"
            );
        }
    }

    // ============================================================================
    // Test 2: IntentAnalyzer - Complex Input -> PlanRequested
    // ============================================================================

    #[tokio::test]
    async fn test_intent_analyzer_complex_input() {
        // Setup
        let analyzer = IntentAnalyzer::new();
        let ctx = create_test_context();

        // Create a complex input with multi-step keywords
        let input = create_test_input(
            "First search for the file, then open it and finally edit the content",
        );
        let event = AetherEvent::InputReceived(input);

        // Handle the event
        let result = analyzer.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::PlanRequested(_)),
            "Should produce PlanRequested for complex input with multi-step keywords"
        );

        if let AetherEvent::PlanRequested(plan_request) = &events[0] {
            assert!(
                !plan_request.detected_steps.is_empty(),
                "Should detect steps from the input"
            );
        }
    }

    #[tokio::test]
    async fn test_intent_analyzer_complex_input_chinese() {
        // Setup
        let analyzer = IntentAnalyzer::new();
        let ctx = create_test_context();

        // Create a complex input with Chinese multi-step keywords
        let input = create_test_input("打开文件然后复制内容接着保存到新位置");
        let event = AetherEvent::InputReceived(input);

        // Handle the event
        let result = analyzer.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert!(
            matches!(events[0], AetherEvent::PlanRequested(_)),
            "Should produce PlanRequested for Chinese complex input"
        );
    }

    #[tokio::test]
    async fn test_intent_analyzer_numbered_list_input() {
        // Setup
        let analyzer = IntentAnalyzer::new();
        let ctx = create_test_context();

        // Create an input with numbered list (step markers)
        let input = create_test_input("1. Create a folder\n2. Copy files\n3. Delete old files");
        let event = AetherEvent::InputReceived(input);

        // Handle the event
        let result = analyzer.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert!(
            matches!(events[0], AetherEvent::PlanRequested(_)),
            "Should produce PlanRequested for numbered list input"
        );
    }

    // ============================================================================
    // Test 3: TaskPlanner - PlanRequested -> PlanCreated
    // ============================================================================

    #[tokio::test]
    async fn test_task_planner_creates_plan() {
        // Setup
        let planner = TaskPlanner::new();
        let ctx = create_test_context();

        // Create a PlanRequest with detected steps
        let plan_request = create_plan_request(
            "打开文件然后复制内容接着保存",
            vec!["打开文件", "复制内容", "保存"],
        );
        let event = AetherEvent::PlanRequested(plan_request);

        // Handle the event
        let result = planner.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::PlanCreated(_)),
            "Should produce PlanCreated event"
        );

        if let AetherEvent::PlanCreated(plan) = &events[0] {
            assert_eq!(plan.steps.len(), 3, "Plan should have 3 steps");

            // Verify step dependencies (sequential)
            assert!(
                plan.steps[0].depends_on.is_empty(),
                "First step should have no dependencies"
            );
            assert_eq!(
                plan.steps[1].depends_on,
                vec!["step_1"],
                "Second step should depend on first"
            );
            assert_eq!(
                plan.steps[2].depends_on,
                vec!["step_2"],
                "Third step should depend on second"
            );

            // Verify tool inference
            assert_eq!(
                plan.steps[0].tool, "file_read",
                "打开文件 should map to file_read"
            );
            assert_eq!(
                plan.steps[1].tool, "file_copy",
                "复制内容 should map to file_copy"
            );
            // "保存" defaults to chat since it doesn't match specific patterns
        }
    }

    #[tokio::test]
    async fn test_task_planner_empty_steps_uses_input() {
        // Setup
        let planner = TaskPlanner::new();
        let ctx = create_test_context();

        // Create a PlanRequest with no detected steps
        let plan_request = create_plan_request("搜索文件", vec![]);
        let event = AetherEvent::PlanRequested(plan_request);

        // Handle the event
        let result = planner.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        if let AetherEvent::PlanCreated(plan) = &events[0] {
            assert_eq!(
                plan.steps.len(),
                1,
                "Should create single step from input text"
            );
            assert_eq!(plan.steps[0].description, "搜索文件");
        }
    }

    // ============================================================================
    // Test 4: ToolExecutor - ToolCallRequested -> ToolCallCompleted
    // ============================================================================

    #[tokio::test]
    async fn test_tool_executor_handles_request() {
        // Setup
        let executor = ToolExecutor::new();
        let ctx = create_test_context();

        // Create a ToolCallRequest
        let request = create_tool_call_request("search", "find rust documentation");
        let event = AetherEvent::ToolCallRequested(request);

        // Handle the event
        let result = executor.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::ToolCallCompleted(_)),
            "Should produce ToolCallCompleted event"
        );

        if let AetherEvent::ToolCallCompleted(completed) = &events[0] {
            assert_eq!(completed.tool, "search", "Tool name should match");
            assert!(
                completed.completed_at >= completed.started_at,
                "Completed time should be after or equal to started time"
            );
            assert!(!completed.output.is_empty(), "Output should not be empty");
        }
    }

    // ============================================================================
    // Test 5: ToolExecutor - ToolCallRequested with abort -> ToolCallFailed
    // ============================================================================

    #[tokio::test]
    async fn test_tool_executor_respects_abort() {
        // Setup
        let executor = ToolExecutor::new();
        let ctx = create_test_context();

        // Signal abort before handling
        ctx.abort();

        // Create a ToolCallRequest
        let request = create_tool_call_request("search", "find something");
        let event = AetherEvent::ToolCallRequested(request);

        // Handle the event
        let result = executor.handle(&event, &ctx).await;

        // Verify
        assert!(
            result.is_ok(),
            "Handler should succeed (returning failure event)"
        );
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::ToolCallFailed(_)),
            "Should produce ToolCallFailed when aborted"
        );

        if let AetherEvent::ToolCallFailed(error) = &events[0] {
            assert_eq!(
                error.error_kind,
                ErrorKind::Aborted,
                "Error kind should be Aborted"
            );
            assert!(
                !error.is_retryable,
                "Aborted errors should not be retryable"
            );
        }
    }

    // ============================================================================
    // Test 6: LoopController - PlanCreated -> LoopContinue + ToolCallRequested
    // ============================================================================

    #[tokio::test]
    async fn test_loop_controller_starts_plan() {
        // Setup
        let controller = LoopController::new();
        let ctx = create_test_context();

        // Create a TaskPlan with steps
        let plan = create_test_plan(vec![
            ("search", "Search for files"),
            ("read", "Read file content"),
            ("write", "Write results"),
        ]);
        let event = AetherEvent::PlanCreated(plan);

        // Handle the event
        let result = controller.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        // Should produce ToolCallRequested for the first step
        assert!(!events.is_empty(), "Should produce at least one event");
        assert!(
            matches!(events[0], AetherEvent::ToolCallRequested(_)),
            "Should produce ToolCallRequested to start first step"
        );

        if let AetherEvent::ToolCallRequested(request) = &events[0] {
            assert_eq!(
                request.tool, "search",
                "Should start with first step's tool"
            );
            assert_eq!(
                request.plan_step_id,
                Some("step_1".to_string()),
                "Should reference the plan step ID"
            );
        }
    }

    #[tokio::test]
    async fn test_loop_controller_empty_plan_stops() {
        // Setup
        let controller = LoopController::new();
        let ctx = create_test_context();

        // Create an empty TaskPlan
        let plan = TaskPlan {
            id: "empty-plan".to_string(),
            steps: vec![],
            parallel_groups: vec![],
            current_step_index: 0,
        };
        let event = AetherEvent::PlanCreated(plan);

        // Handle the event
        let result = controller.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::LoopStop(StopReason::EmptyPlan)),
            "Should stop with EmptyPlan reason for empty plan"
        );
    }

    #[tokio::test]
    async fn test_loop_controller_handles_tool_failure() {
        // Setup
        let controller = LoopController::new();
        let ctx = create_test_context();

        // First, set up a plan
        let plan = create_test_plan(vec![("search", "Search for files")]);
        controller.set_plan(plan).await;

        // Create a ToolCallFailed event
        let error = ToolCallError {
            call_id: "call-1".to_string(),
            tool: "search".to_string(),
            error: "Connection timeout".to_string(),
            error_kind: ErrorKind::Timeout,
            is_retryable: true,
            attempts: 3,
        };
        let event = AetherEvent::ToolCallFailed(error);

        // Handle the event
        let result = controller.handle(&event, &ctx).await;

        // Verify
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();

        assert_eq!(events.len(), 1, "Should produce exactly one event");
        assert!(
            matches!(events[0], AetherEvent::LoopStop(StopReason::Error(_))),
            "Should stop with Error reason on tool failure"
        );
    }

    // ============================================================================
    // Test 7: SessionRecorder - InputReceived persisted to SQLite
    // ============================================================================

    #[tokio::test]
    async fn test_session_recorder_persists_events() {
        // Setup
        let recorder = SessionRecorder::new_in_memory().expect("Should create in-memory recorder");
        let ctx = create_test_context();

        // Create a session first
        let session_id = "test-session-001";
        recorder
            .create_session(session_id, "test-model")
            .expect("Should create session");
        ctx.set_session_id(session_id.to_string()).await;

        // Create an InputReceived event
        let input = create_test_input("Hello, this is a test input");
        let event = AetherEvent::InputReceived(input.clone());

        // Handle the event
        let result = recorder.handle(&event, &ctx).await;

        // Verify handler returns no events (recorder doesn't publish)
        assert!(result.is_ok(), "Handler should succeed");
        let events = result.unwrap();
        assert!(
            events.is_empty(),
            "SessionRecorder should not publish events"
        );

        // Verify the event was persisted
        let parts = recorder
            .get_session_parts(session_id)
            .expect("Should retrieve session parts");

        assert_eq!(parts.len(), 1, "Should have one persisted part");
        assert!(
            matches!(&parts[0], SessionPart::UserInput(_)),
            "Should be a UserInput part"
        );

        if let SessionPart::UserInput(user_input) = &parts[0] {
            assert_eq!(
                user_input.text, input.text,
                "Persisted text should match input"
            );
        }
    }

    #[tokio::test]
    async fn test_session_recorder_persists_multiple_events() {
        // Setup
        let recorder = SessionRecorder::new_in_memory().expect("Should create in-memory recorder");
        let ctx = create_test_context();

        // Create a session
        let session_id = "test-session-002";
        recorder.create_session(session_id, "test-model").unwrap();
        ctx.set_session_id(session_id.to_string()).await;

        // Handle multiple events
        let input_event = AetherEvent::InputReceived(create_test_input("User query"));
        recorder.handle(&input_event, &ctx).await.unwrap();

        let tool_result =
            AetherEvent::ToolCallCompleted(create_tool_call_result("search", "Found results"));
        recorder.handle(&tool_result, &ctx).await.unwrap();

        // Verify all events were persisted
        let parts = recorder.get_session_parts(session_id).unwrap();
        assert_eq!(parts.len(), 2, "Should have two persisted parts");

        assert!(matches!(&parts[0], SessionPart::UserInput(_)));
        assert!(matches!(&parts[1], SessionPart::ToolCall(_)));
    }

    // ============================================================================
    // Test 8: Full Event Chain - Complete Flow
    // ============================================================================

    #[tokio::test]
    async fn test_full_event_chain() {
        // This test verifies the complete flow:
        // InputReceived -> IntentAnalyzer -> PlanRequested -> TaskPlanner -> PlanCreated
        //     -> LoopController -> ToolCallRequested -> ToolExecutor -> ToolCallCompleted
        //     -> LoopController -> (next step or complete)

        // Setup all components
        let analyzer = IntentAnalyzer::new();
        let planner = TaskPlanner::new();
        let controller = LoopController::new();
        let executor = ToolExecutor::new();
        let recorder = SessionRecorder::new_in_memory().expect("Should create recorder");

        let ctx = create_test_context();
        let session_id = "full-chain-session";
        recorder.create_session(session_id, "test-model").unwrap();
        ctx.set_session_id(session_id.to_string()).await;

        // Step 1: Start with complex input
        let input = create_test_input("First search for docs then read the results");
        let input_event = AetherEvent::InputReceived(input);

        // Record the input
        recorder.handle(&input_event, &ctx).await.unwrap();

        // Step 2: IntentAnalyzer processes input -> should produce PlanRequested
        let analyzer_result = analyzer.handle(&input_event, &ctx).await.unwrap();
        assert_eq!(analyzer_result.len(), 1);
        assert!(
            matches!(analyzer_result[0], AetherEvent::PlanRequested(_)),
            "Complex input should produce PlanRequested"
        );

        // Step 3: TaskPlanner processes PlanRequested -> produces PlanCreated
        let planner_result = planner.handle(&analyzer_result[0], &ctx).await.unwrap();
        assert_eq!(planner_result.len(), 1);
        assert!(
            matches!(planner_result[0], AetherEvent::PlanCreated(_)),
            "PlanRequested should produce PlanCreated"
        );

        // Record the plan
        recorder.handle(&planner_result[0], &ctx).await.unwrap();

        // Step 4: LoopController processes PlanCreated -> produces ToolCallRequested
        let controller_result = controller.handle(&planner_result[0], &ctx).await.unwrap();
        assert!(!controller_result.is_empty());
        assert!(
            matches!(controller_result[0], AetherEvent::ToolCallRequested(_)),
            "PlanCreated should produce ToolCallRequested"
        );

        // Step 5: ToolExecutor processes ToolCallRequested -> produces ToolCallCompleted
        let executor_result = executor.handle(&controller_result[0], &ctx).await.unwrap();
        assert_eq!(executor_result.len(), 1);
        assert!(
            matches!(executor_result[0], AetherEvent::ToolCallCompleted(_)),
            "ToolCallRequested should produce ToolCallCompleted"
        );

        // Record the tool result
        recorder.handle(&executor_result[0], &ctx).await.unwrap();

        // Step 6: LoopController processes ToolCallCompleted -> continues or completes
        let final_result = controller.handle(&executor_result[0], &ctx).await.unwrap();
        assert!(
            !final_result.is_empty(),
            "Should produce continuation or completion events"
        );

        // Verify session was recorded
        let parts = recorder.get_session_parts(session_id).unwrap();
        assert!(
            parts.len() >= 2,
            "Should have at least input and tool call recorded"
        );
    }

    // ============================================================================
    // Additional Integration Tests
    // ============================================================================

    #[tokio::test]
    async fn test_event_chain_simple_input_direct_execution() {
        // Test flow for simple input (no planning needed):
        // InputReceived -> IntentAnalyzer -> ToolCallRequested -> ToolExecutor -> ToolCallCompleted

        let analyzer = IntentAnalyzer::new();
        let executor = ToolExecutor::new();
        let ctx = create_test_context();

        // Simple input (no multi-step keywords)
        let input = create_test_input("What is the weather today?");
        let input_event = AetherEvent::InputReceived(input);

        // IntentAnalyzer should produce ToolCallRequested directly
        let analyzer_result = analyzer.handle(&input_event, &ctx).await.unwrap();
        assert_eq!(analyzer_result.len(), 1);
        assert!(
            matches!(analyzer_result[0], AetherEvent::ToolCallRequested(_)),
            "Simple input should skip planning and produce ToolCallRequested"
        );

        // ToolExecutor processes the request
        let executor_result = executor.handle(&analyzer_result[0], &ctx).await.unwrap();
        assert_eq!(executor_result.len(), 1);
        assert!(
            matches!(executor_result[0], AetherEvent::ToolCallCompleted(_)),
            "ToolCallRequested should produce ToolCallCompleted"
        );
    }

    #[tokio::test]
    async fn test_loop_controller_progresses_through_plan() {
        // Test that LoopController correctly progresses through multi-step plans

        let controller = LoopController::new();
        let ctx = create_test_context();

        // Create a 3-step plan
        let plan = create_test_plan(vec![
            ("search", "Search for files"),
            ("read", "Read file content"),
            ("write", "Write results"),
        ]);
        let plan_event = AetherEvent::PlanCreated(plan);

        // Handle PlanCreated -> should start first step
        let result1 = controller.handle(&plan_event, &ctx).await.unwrap();
        assert!(matches!(result1[0], AetherEvent::ToolCallRequested(_)));

        // Simulate first step completion
        let completion1 = AetherEvent::ToolCallCompleted(ToolCallResult {
            call_id: "call-1".to_string(),
            tool: "search".to_string(),
            input: serde_json::json!({}),
            output: "Found files".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
        });

        // Handle ToolCallCompleted -> should continue to next step or complete
        let result2 = controller.handle(&completion1, &ctx).await.unwrap();
        assert!(
            !result2.is_empty(),
            "Should produce events after step completion"
        );

        // Should either continue with next step or complete the plan
        let has_tool_request = result2
            .iter()
            .any(|e| matches!(e, AetherEvent::ToolCallRequested(_)));
        let has_loop_stop = result2
            .iter()
            .any(|e| matches!(e, AetherEvent::LoopStop(_)));

        assert!(
            has_tool_request || has_loop_stop,
            "Should either request next tool or stop the loop"
        );
    }

    #[tokio::test]
    async fn test_components_handle_only_subscribed_events() {
        // Verify that components correctly ignore events they don't subscribe to

        let analyzer = IntentAnalyzer::new();
        let planner = TaskPlanner::new();
        let controller = LoopController::new();
        let executor = ToolExecutor::new();
        let ctx = create_test_context();

        // Create an event that none should handle (LoopStop)
        let stop_event = AetherEvent::LoopStop(StopReason::Completed);

        // Each component should return empty when given non-subscribed event
        let analyzer_result = analyzer.handle(&stop_event, &ctx).await.unwrap();
        assert!(
            analyzer_result.is_empty(),
            "IntentAnalyzer should ignore LoopStop"
        );

        let planner_result = planner.handle(&stop_event, &ctx).await.unwrap();
        assert!(
            planner_result.is_empty(),
            "TaskPlanner should ignore LoopStop"
        );

        // LoopController doesn't subscribe to LoopStop
        let controller_result = controller.handle(&stop_event, &ctx).await.unwrap();
        assert!(
            controller_result.is_empty(),
            "LoopController should ignore LoopStop"
        );

        let executor_result = executor.handle(&stop_event, &ctx).await.unwrap();
        assert!(
            executor_result.is_empty(),
            "ToolExecutor should ignore LoopStop"
        );
    }

    #[tokio::test]
    async fn test_abort_propagates_through_chain() {
        // Verify that abort signal is respected throughout the chain

        let executor = ToolExecutor::new();
        let controller = LoopController::new();
        let ctx = create_test_context();

        // Set up a plan
        let plan = create_test_plan(vec![("search", "Search")]);
        controller.set_plan(plan).await;

        // Abort the context
        ctx.abort();

        // ToolExecutor should fail on abort
        let request = create_tool_call_request("search", "test");
        let event = AetherEvent::ToolCallRequested(request);
        let result = executor.handle(&event, &ctx).await.unwrap();

        assert!(
            matches!(result[0], AetherEvent::ToolCallFailed(ref e) if e.error_kind == ErrorKind::Aborted),
            "ToolExecutor should fail with Aborted on abort signal"
        );

        // LoopController's guards should also respect abort
        let session = ExecutionSession::default();
        let stop_reason = controller.check_guards(&session, &ctx);

        assert!(
            matches!(stop_reason, Some(StopReason::UserAborted)),
            "LoopController guards should detect abort"
        );
    }
}

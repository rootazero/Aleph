//! CallbackBridge - Forwards internal events to Swift callbacks.
//!
//! This component subscribes to UI-relevant events and converts them
//! to callbacks via the AetherEventHandler trait.

use std::sync::Arc;

use async_trait::async_trait;

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError,
    StopReason,
};
use crate::ffi::AetherEventHandler;

/// CallbackBridge forwards events to the Swift layer
pub struct CallbackBridge {
    handler: Arc<dyn AetherEventHandler>,
}

impl CallbackBridge {
    /// Create a new CallbackBridge
    pub fn new(handler: Arc<dyn AetherEventHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl EventHandler for CallbackBridge {
    fn name(&self) -> &'static str {
        "CallbackBridge"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::SessionCreated,
            EventType::ToolCallStarted,
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
            EventType::LoopContinue,
            EventType::LoopStop,
            EventType::PlanCreated,
            EventType::SubAgentStarted,
            EventType::SubAgentCompleted,
            EventType::AiResponseGenerated,
        ]
    }

    async fn handle(&self, event: &AetherEvent, ctx: &EventContext) -> Result<Vec<AetherEvent>, HandlerError> {
        match event {
            AetherEvent::SessionCreated(info) => {
                self.handler.on_session_started(info.id.clone());
            }
            AetherEvent::ToolCallStarted(info) => {
                self.handler.on_tool_call_started(
                    info.call_id.clone(),
                    info.tool.clone(),
                );
            }
            AetherEvent::ToolCallCompleted(result) => {
                self.handler.on_tool_call_completed(
                    result.call_id.clone(),
                    result.output.clone(),
                );
            }
            AetherEvent::ToolCallFailed(error) => {
                self.handler.on_tool_call_failed(
                    error.call_id.clone(),
                    error.error.clone(),
                    error.is_retryable,
                );
            }
            AetherEvent::LoopContinue(state) => {
                let status = match &state.last_tool {
                    Some(tool) => format!("Running tool: {}", tool),
                    None => format!("Iteration {}", state.iteration),
                };
                self.handler.on_loop_progress(
                    state.session_id.clone(),
                    state.iteration,
                    status,
                );
            }
            AetherEvent::LoopStop(reason) => {
                if let Some(session_id) = ctx.get_session_id().await {
                    let summary = match reason {
                        StopReason::Completed => "Task completed successfully".to_string(),
                        StopReason::MaxIterationsReached => "Reached max iterations".to_string(),
                        StopReason::UserAborted => "Cancelled by user".to_string(),
                        StopReason::Error(e) => format!("Error: {}", e),
                        StopReason::TokenLimitReached => "Token limit reached".to_string(),
                        StopReason::DoomLoopDetected => "Detected repetitive loop".to_string(),
                        StopReason::EmptyPlan => "No steps to execute".to_string(),
                    };
                    self.handler.on_session_completed(session_id, summary);
                }
            }
            AetherEvent::PlanCreated(plan) => {
                if let Some(session_id) = ctx.get_session_id().await {
                    let steps: Vec<String> = plan.steps.iter()
                        .map(|s| s.description.clone())
                        .collect();
                    self.handler.on_plan_created(session_id, steps);
                }
            }
            AetherEvent::SubAgentStarted(request) => {
                self.handler.on_subagent_started(
                    request.parent_session_id.clone(),
                    request.child_session_id.clone(),
                    request.agent_id.clone(),
                );
            }
            AetherEvent::SubAgentCompleted(result) => {
                self.handler.on_subagent_completed(
                    result.child_session_id.clone(),
                    result.success,
                    result.summary.clone(),
                );
            }
            AetherEvent::AiResponseGenerated(response) => {
                // Forward streaming chunks via on_stream_chunk
                self.handler.on_stream_chunk(response.content.clone());
                if response.is_final {
                    self.handler.on_complete(response.content.clone());
                }
            }
            _ => {}
        }

        // CallbackBridge doesn't produce new events
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        EventBus, SessionInfo, TaskPlan, PlanStep, StepStatus,
        ToolCallStarted, ToolCallResult, ToolCallError, ErrorKind,
        LoopState, AiResponse, SubAgentRequest, SubAgentResult, TokenUsage,
    };
    use crate::event_handler::McpStartupReportFFI;
    use crate::intent::ExecutableTaskFFI;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct MockHandler {
        session_started_count: AtomicU32,
        tool_started_count: AtomicU32,
        tool_completed_count: AtomicU32,
        tool_failed_count: AtomicU32,
        loop_progress_count: AtomicU32,
        session_completed_count: AtomicU32,
        plan_created_count: AtomicU32,
        subagent_started_count: AtomicU32,
        subagent_completed_count: AtomicU32,
        stream_chunk_count: AtomicU32,
    }

    impl MockHandler {
        fn new() -> Self {
            Self {
                session_started_count: AtomicU32::new(0),
                tool_started_count: AtomicU32::new(0),
                tool_completed_count: AtomicU32::new(0),
                tool_failed_count: AtomicU32::new(0),
                loop_progress_count: AtomicU32::new(0),
                session_completed_count: AtomicU32::new(0),
                plan_created_count: AtomicU32::new(0),
                subagent_started_count: AtomicU32::new(0),
                subagent_completed_count: AtomicU32::new(0),
                stream_chunk_count: AtomicU32::new(0),
            }
        }
    }

    impl AetherEventHandler for MockHandler {
        fn on_thinking(&self) {}
        fn on_tool_start(&self, _: String) {}
        fn on_tool_result(&self, _: String, _: String) {}
        fn on_stream_chunk(&self, _: String) {
            self.stream_chunk_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_complete(&self, _: String) {}
        fn on_error(&self, _: String) {}
        fn on_memory_stored(&self) {}
        fn on_agent_mode_detected(&self, _: ExecutableTaskFFI) {}
        fn on_tools_changed(&self, _: u32) {}
        fn on_mcp_startup_complete(&self, _: McpStartupReportFFI) {}

        fn on_session_started(&self, _: String) {
            self.session_started_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_tool_call_started(&self, _: String, _: String) {
            self.tool_started_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_tool_call_completed(&self, _: String, _: String) {
            self.tool_completed_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_tool_call_failed(&self, _: String, _: String, _: bool) {
            self.tool_failed_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_loop_progress(&self, _: String, _: u32, _: String) {
            self.loop_progress_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_plan_created(&self, _: String, _: Vec<String>) {
            self.plan_created_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_session_completed(&self, _: String, _: String) {
            self.session_completed_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_subagent_started(&self, _: String, _: String, _: String) {
            self.subagent_started_count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_subagent_completed(&self, _: String, _: bool, _: String) {
            self.subagent_completed_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn create_test_context() -> EventContext {
        EventContext::new(EventBus::new())
    }

    fn create_handler() -> Arc<dyn AetherEventHandler> {
        Arc::new(MockHandler::new()) as Arc<dyn AetherEventHandler>
    }

    fn create_handler_with_ref() -> (Arc<MockHandler>, Arc<dyn AetherEventHandler>) {
        let mock = Arc::new(MockHandler::new());
        let handler: Arc<dyn AetherEventHandler> = Arc::clone(&mock) as Arc<dyn AetherEventHandler>;
        (mock, handler)
    }

    #[tokio::test]
    async fn test_callback_bridge_name() {
        let handler = create_handler();
        let bridge = CallbackBridge::new(handler);
        assert_eq!(bridge.name(), "CallbackBridge");
    }

    #[tokio::test]
    async fn test_callback_bridge_subscriptions() {
        let handler = create_handler();
        let bridge = CallbackBridge::new(handler);
        let subs = bridge.subscriptions();

        assert!(subs.contains(&EventType::SessionCreated));
        assert!(subs.contains(&EventType::ToolCallStarted));
        assert!(subs.contains(&EventType::ToolCallCompleted));
        assert!(subs.contains(&EventType::ToolCallFailed));
        assert!(subs.contains(&EventType::LoopContinue));
        assert!(subs.contains(&EventType::LoopStop));
        assert!(subs.contains(&EventType::PlanCreated));
        assert!(subs.contains(&EventType::SubAgentStarted));
        assert!(subs.contains(&EventType::SubAgentCompleted));
    }

    #[tokio::test]
    async fn test_session_created_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::SessionCreated(SessionInfo {
            id: "test-session".into(),
            parent_id: None,
            agent_id: "main".into(),
            model: "test".into(),
            created_at: 0,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.session_started_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_tool_call_started_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::ToolCallStarted(ToolCallStarted {
            call_id: "call-1".into(),
            tool: "web_fetch".into(),
            input: serde_json::json!({}),
            timestamp: 0,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.tool_started_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_tool_call_completed_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::ToolCallCompleted(ToolCallResult {
            call_id: "call-1".into(),
            tool: "web_fetch".into(),
            input: serde_json::json!({}),
            output: "test output".into(),
            started_at: 0,
            completed_at: 1,
            token_usage: TokenUsage { input_tokens: 0, output_tokens: 0 },
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.tool_completed_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_tool_call_failed_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::ToolCallFailed(ToolCallError {
            call_id: "call-1".into(),
            tool: "web_fetch".into(),
            error: "Connection failed".into(),
            error_kind: ErrorKind::ServiceUnavailable,
            is_retryable: true,
            attempts: 1,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.tool_failed_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_loop_stop_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();
        ctx.set_session_id("test-session".into()).await;

        let event = AetherEvent::LoopStop(StopReason::Completed);

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.session_completed_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_plan_created_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();
        ctx.set_session_id("test-session".into()).await;

        let event = AetherEvent::PlanCreated(TaskPlan {
            id: "plan-1".into(),
            steps: vec![
                PlanStep {
                    id: "step-1".into(),
                    description: "First step".into(),
                    tool: "tool1".into(),
                    parameters: serde_json::json!({}),
                    depends_on: vec![],
                    status: StepStatus::Pending,
                },
            ],
            parallel_groups: vec![],
            current_step_index: 0,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.plan_created_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_subagent_started_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::SubAgentStarted(SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent".into(),
            child_session_id: "child".into(),
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.subagent_started_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_subagent_completed_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::SubAgentCompleted(SubAgentResult {
            agent_id: "explore".into(),
            child_session_id: "child".into(),
            summary: "Found 5 files".into(),
            success: true,
            error: None,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.subagent_completed_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_ai_response_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::AiResponseGenerated(AiResponse {
            content: "Hello".into(),
            is_final: false,
            reasoning: None,
            timestamp: 0,
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.stream_chunk_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_loop_continue_callback() {
        let (mock, handler) = create_handler_with_ref();
        let bridge = CallbackBridge::new(handler);
        let ctx = create_test_context();

        let event = AetherEvent::LoopContinue(LoopState {
            session_id: "test-session".into(),
            iteration: 5,
            total_tokens: 1000,
            last_tool: Some("web_fetch".into()),
        });

        bridge.handle(&event, &ctx).await.unwrap();
        assert_eq!(mock.loop_progress_count.load(Ordering::SeqCst), 1);
    }
}

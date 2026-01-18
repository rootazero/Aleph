// Aether/core/src/event/integration_test.rs
//! Integration tests for the event system.

#[cfg(test)]
mod tests {
    use crate::event::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use async_trait::async_trait;

    /// Simulates IntentAnalyzer: receives InputReceived, publishes ToolCallRequested
    struct MockIntentAnalyzer;

    #[async_trait]
    impl EventHandler for MockIntentAnalyzer {
        fn name(&self) -> &'static str { "MockIntentAnalyzer" }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::InputReceived]
        }

        async fn handle(
            &self,
            event: &AetherEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AetherEvent>, HandlerError> {
            if let AetherEvent::InputReceived(input) = event {
                // Simulate: detect intent and request tool call
                Ok(vec![AetherEvent::ToolCallRequested(ToolCallRequest {
                    tool: "search".to_string(),
                    parameters: serde_json::json!({"query": input.text}),
                    plan_step_id: None,
                })])
            } else {
                Ok(vec![])
            }
        }
    }

    /// Simulates ToolExecutor: receives ToolCallRequested, publishes ToolCallCompleted
    struct MockToolExecutor;

    #[async_trait]
    impl EventHandler for MockToolExecutor {
        fn name(&self) -> &'static str { "MockToolExecutor" }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::ToolCallRequested]
        }

        async fn handle(
            &self,
            event: &AetherEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AetherEvent>, HandlerError> {
            if let AetherEvent::ToolCallRequested(req) = event {
                Ok(vec![AetherEvent::ToolCallCompleted(ToolCallResult {
                    call_id: uuid::Uuid::new_v4().to_string(),
                    tool: req.tool.clone(),
                    input: req.parameters.clone(),
                    output: "search results".to_string(),
                    started_at: chrono::Utc::now().timestamp_millis(),
                    completed_at: chrono::Utc::now().timestamp_millis(),
                    token_usage: TokenUsage::default(),
                })])
            } else {
                Ok(vec![])
            }
        }
    }

    /// Simulates LoopController: receives ToolCallCompleted, publishes LoopStop
    struct MockLoopController {
        iterations: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EventHandler for MockLoopController {
        fn name(&self) -> &'static str { "MockLoopController" }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::ToolCallCompleted]
        }

        async fn handle(
            &self,
            _event: &AetherEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AetherEvent>, HandlerError> {
            let count = self.iterations.fetch_add(1, Ordering::SeqCst);

            // First iteration: emit LoopContinue, then LoopStop
            // This simulates a simple one-shot task that completes after one tool call
            if count == 0 {
                // Emit both LoopContinue (to show the loop is running) and LoopStop (to end)
                Ok(vec![
                    AetherEvent::LoopContinue(LoopState {
                        session_id: "test-session".to_string(),
                        iteration: count as u32,
                        total_tokens: 0,
                        last_tool: Some("search".to_string()),
                    }),
                    AetherEvent::LoopStop(StopReason::Completed),
                ])
            } else {
                Ok(vec![AetherEvent::LoopStop(StopReason::Completed)])
            }
        }
    }

    /// Test the complete event flow
    #[tokio::test]
    async fn test_complete_event_flow() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let iterations = Arc::new(AtomicUsize::new(0));

        let mut registry = EventHandlerRegistry::new();
        registry.register(Arc::new(MockIntentAnalyzer));
        registry.register(Arc::new(MockToolExecutor));
        registry.register(Arc::new(MockLoopController {
            iterations: iterations.clone()
        }));

        // Subscribe to watch for LoopStop
        let mut watcher = bus.subscribe_filtered(vec![EventType::LoopStop]);

        // Start handlers
        let handles = registry.start(ctx.clone()).await;

        // Give handlers time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        // Trigger the flow with an input event
        bus.publish(AetherEvent::InputReceived(InputEvent {
            text: "search for rust async".to_string(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        })).await;

        // Wait for LoopStop with timeout
        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            watcher.recv()
        ).await;

        assert!(result.is_ok(), "Should receive LoopStop event");
        let event = result.unwrap().unwrap();
        assert_eq!(event.event.event_type(), EventType::LoopStop);

        // Verify the flow executed
        assert!(iterations.load(Ordering::SeqCst) >= 1);

        // Check history
        let history = bus.history().await;
        assert!(history.len() >= 4, "Should have at least 4 events in history");

        // Verify event sequence: Input -> ToolCallRequested -> ToolCallCompleted -> LoopStop
        let event_types: Vec<_> = history.iter()
            .map(|e| e.event.event_type())
            .collect();

        assert!(event_types.contains(&EventType::InputReceived));
        assert!(event_types.contains(&EventType::ToolCallRequested));
        assert!(event_types.contains(&EventType::ToolCallCompleted));

        // Cleanup
        registry.stop(&ctx);
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                handle
            ).await;
        }
    }

    /// Test abort signal propagation
    #[tokio::test]
    async fn test_abort_stops_handlers() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let counter = Arc::new(AtomicUsize::new(0));

        struct SlowHandler {
            counter: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl EventHandler for SlowHandler {
            fn name(&self) -> &'static str { "SlowHandler" }

            fn subscriptions(&self) -> Vec<EventType> {
                vec![EventType::All]
            }

            async fn handle(
                &self,
                _event: &AetherEvent,
                ctx: &EventContext,
            ) -> Result<Vec<AetherEvent>, HandlerError> {
                if ctx.is_aborted() {
                    return Err(HandlerError::Aborted);
                }
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(vec![])
            }
        }

        let mut registry = EventHandlerRegistry::new();
        registry.register(Arc::new(SlowHandler { counter: counter.clone() }));

        let handles = registry.start(ctx.clone()).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish one event
        bus.publish(AetherEvent::LoopStop(StopReason::Completed)).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let count_before = counter.load(Ordering::SeqCst);

        // Abort
        registry.stop(&ctx);

        // Wait for handlers to stop
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(200),
                handle
            ).await;
        }

        // Verify handler processed at least one event before abort
        assert!(count_before >= 1);
    }
}

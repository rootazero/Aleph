//! SessionCompactor core tests, EventHandler tests, and Integration tests

use super::helpers::create_test_session;
use crate::components::session_compactor::*;
use crate::components::types::{
    AiResponsePart, ExecutionSession, SessionPart, UserInputPart,
};
use crate::event::{AlephEvent, EventContext, EventHandler, EventType};
use serde_json::json;

    // ========================================================================
    // SessionCompactor Tests
    // ========================================================================

    #[test]
    fn test_session_compactor_new() {
        let compactor = SessionCompactor::new();
        assert_eq!(compactor.keep_recent_tools, 10);
    }

    #[test]
    fn test_session_compactor_with_keep_recent() {
        let compactor = SessionCompactor::with_keep_recent(5);
        assert_eq!(compactor.keep_recent_tools, 5);
    }

    #[test]
    fn test_prune_old_tool_outputs() {
        let compactor = SessionCompactor::with_keep_recent(5);
        let mut session = create_test_session();

        // We have 15 tool calls, should prune 10
        compactor.prune_old_tool_outputs(&mut session);

        // Count pruned vs non-pruned
        let (pruned, kept): (Vec<_>, Vec<_>) = session
            .parts
            .iter()
            .filter_map(|part| {
                if let SessionPart::ToolCall(tc) = part {
                    Some(tc.output.as_ref().unwrap().as_str())
                } else {
                    None
                }
            })
            .partition(|output| *output == "[Output pruned to save context]");

        assert_eq!(pruned.len(), 10);
        assert_eq!(kept.len(), 5);
    }

    #[test]
    fn test_prune_old_tool_outputs_no_pruning_needed() {
        let compactor = SessionCompactor::with_keep_recent(20);
        let mut session = create_test_session();

        // We have 15 tool calls, keep_recent is 20, so no pruning
        compactor.prune_old_tool_outputs(&mut session);

        // All outputs should be preserved
        let pruned_count = session
            .parts
            .iter()
            .filter(|part| {
                if let SessionPart::ToolCall(tc) = part {
                    tc.output.as_ref().is_some_and(|o| o.contains("pruned"))
                } else {
                    false
                }
            })
            .count();

        assert_eq!(pruned_count, 0);
    }

    #[test]
    fn test_generate_summary() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let summary = compactor.generate_summary(&session);

        // Summary should contain original request
        assert!(summary.contains("Please help me analyze this code"));

        // Summary should mention completed steps
        assert!(summary.contains("Completed Steps"));

        // Summary should contain iteration count
        assert!(summary.contains("Iterations"));
    }

    #[test]
    fn test_generate_summary_empty_session() {
        let compactor = SessionCompactor::new();
        let session = ExecutionSession::new();

        let summary = compactor.generate_summary(&session);

        // Should handle empty session gracefully
        assert!(summary.contains("[No original request found]"));
    }

    #[test]
    fn test_replace_with_summary() {
        let compactor = SessionCompactor::with_keep_recent(5);
        let mut session = create_test_session();

        let original_count = session.parts.len();
        let summary = "Test summary content".to_string();

        compactor.replace_with_summary(&mut session, summary.clone());

        // Should have 1 summary + 5 kept parts = 6 total
        assert_eq!(session.parts.len(), 6);

        // First part should be summary
        if let SessionPart::Summary(s) = &session.parts[0] {
            assert_eq!(s.content, "Test summary content");
            assert_eq!(s.original_count as usize, original_count - 5);
        } else {
            panic!("First part should be Summary");
        }
    }

    #[test]
    fn test_recalculate_tokens() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        // Add some parts
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Hello world".to_string(), // 11 chars * 0.4 = 5 tokens (ceil)
            context: None,
            timestamp: 0,
        }));

        session.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: "Hi there!".to_string(), // 9 chars * 0.4 = 4 tokens (ceil)
            reasoning: None,
            timestamp: 0,
        }));

        compactor.recalculate_tokens(&mut session);

        // Total should be approximately 5 + 4 = 9 tokens
        assert!(session.total_tokens > 0);
        assert!(session.total_tokens < 20); // Reasonable bounds
    }

    #[test]
    fn test_compact_reduces_tokens() {
        let compactor = SessionCompactor::with_keep_recent(3);
        let mut session = create_test_session();

        // First calculate current tokens
        compactor.recalculate_tokens(&mut session);
        let before = session.total_tokens;

        // Perform compaction
        let compacted = compactor.compact(&mut session);

        assert!(compacted);
        assert!(session.total_tokens < before);
    }

    // ========================================================================
    // EventHandler Tests
    // ========================================================================

    #[test]
    fn test_handler_name() {
        let compactor = SessionCompactor::new();
        assert_eq!(compactor.name(), "SessionCompactor");
    }

    #[test]
    fn test_handler_subscriptions() {
        let compactor = SessionCompactor::new();
        let subs = compactor.subscriptions();

        assert_eq!(subs.len(), 2);
        assert!(subs.contains(&EventType::ToolCallCompleted));
        assert!(subs.contains(&EventType::LoopContinue));
    }

    #[tokio::test]
    async fn test_handler_ignores_other_events() {
        use crate::event::{EventBus, InputEvent};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // InputReceived event should be ignored
        let event = AlephEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            session_id: None,
            context: None,
            timestamp: 0,
        });

        let result = compactor.handle(&event, &ctx).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_handles_tool_call_completed() {
        use crate::event::{EventBus, TokenUsage, ToolCallResult};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let result_event = ToolCallResult {
            call_id: "test-call".to_string(),
            tool: "search".to_string(),
            input: json!({}),
            output: "results".to_string(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: TokenUsage::default(),
            session_id: None,
        };

        let event = AlephEvent::ToolCallCompleted(result_event);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // In the stub implementation, this returns empty
        // In full implementation, it would check overflow and potentially compact
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_handles_loop_continue() {
        use crate::event::{EventBus, LoopState};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let loop_state = LoopState {
            session_id: "test-session".to_string(),
            iteration: 5,
            total_tokens: 10000,
            last_tool: Some("search".to_string()),
            model: "gpt-4-turbo".to_string(),
        };

        let event = AlephEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Returns empty since tokens are below threshold
        assert!(result.is_empty());
    }

    // ========================================================================
    // Integration Tests
    // ========================================================================

    #[tokio::test]
    async fn test_check_and_compact_no_overflow() {
        let compactor = SessionCompactor::new();
        let mut session = create_test_session();
        session.total_tokens = 1000; // Well below threshold

        let result = compactor.check_and_compact(&mut session).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_check_and_compact_overflow() {
        let compactor = SessionCompactor::with_keep_recent(3);
        let mut session = create_test_session();

        // Set tokens above threshold for gpt-4-turbo (128000 * 0.8 = 102400)
        session.total_tokens = 110000;

        // First calculate actual tokens
        compactor.recalculate_tokens(&mut session);

        // Manually set high token count to trigger compaction
        session.total_tokens = 110000;

        let result = compactor.check_and_compact(&mut session).await;

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.tokens_before, 110000);
        assert!(info.tokens_after < info.tokens_before);
    }

//! Filter Compacted tests (Task 5) and EventHandler Integration tests (Task 6)

use super::helpers::create_test_session;
use crate::components::session_compactor::*;
use crate::components::types::{
    AiResponsePart, ExecutionSession, SessionPart, SummaryPart, UserInputPart,
};
use crate::event::{AlephEvent, EventContext, EventHandler};
use serde_json::json;

    // ========================================================================
    // Filter Compacted Tests (Task 5)
    // ========================================================================

    #[test]
    fn test_filter_compacted_creates_boundary() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // Add some history before compaction
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, true)));

        // Add summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Summary of old context".to_string(),
            original_count: 5,
            compacted_at: 2000,
        }));

        // Add new history after compaction
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "New request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Should only return parts after the compaction boundary (summary + new)
        assert_eq!(filtered.len(), 2, "Expected 2 parts (summary + new user input), got {}", filtered.len());
        assert!(matches!(filtered[0], SessionPart::Summary(_)), "First part should be Summary");
        assert!(matches!(filtered[1], SessionPart::UserInput(_)), "Second part should be UserInput");

        // Verify the content
        if let SessionPart::Summary(s) = &filtered[0] {
            assert_eq!(s.content, "Summary of old context");
        }
        if let SessionPart::UserInput(u) = &filtered[1] {
            assert_eq!(u.text, "New request");
        }
    }

    #[test]
    fn test_filter_compacted_no_boundary() {
        let session = create_test_session(); // No compaction markers
        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Without compaction, should return all parts
        assert_eq!(filtered.len(), session.parts.len(),
            "Without compaction boundary, all {} parts should be returned", session.parts.len());
    }

    #[test]
    fn test_filter_compacted_incomplete_summary() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // Add old history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, true)));

        // Add incomplete summary (compacted_at = 0)
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Incomplete summary".to_string(),
            original_count: 5,
            compacted_at: 0, // Not completed
        }));

        // Add new history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "New request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // With incomplete summary (compacted_at = 0), should return all parts
        // because we never find a "completed" summary to trigger boundary detection
        assert_eq!(filtered.len(), session.parts.len(),
            "With incomplete summary, all parts should be returned");
    }

    #[test]
    fn test_filter_compacted_multiple_boundaries() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // First compaction cycle
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Very old request".to_string(),
            context: None,
            timestamp: 1000,
        }));
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, true)));
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "First summary".to_string(),
            original_count: 3,
            compacted_at: 2000,
        }));

        // Second compaction cycle
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 3000,
        }));
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(4000, false)));
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Second summary".to_string(),
            original_count: 5,
            compacted_at: 4000,
        }));

        // Current context
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Current request".to_string(),
            context: None,
            timestamp: 5000,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Should only return parts after the LAST compaction boundary
        // (second summary + current request)
        assert_eq!(filtered.len(), 2, "Expected 2 parts after last boundary, got {}", filtered.len());

        if let SessionPart::Summary(s) = &filtered[0] {
            assert_eq!(s.content, "Second summary", "Should have the most recent summary");
        } else {
            panic!("First filtered part should be Summary");
        }

        if let SessionPart::UserInput(u) = &filtered[1] {
            assert_eq!(u.text, "Current request");
        } else {
            panic!("Second filtered part should be UserInput");
        }
    }

    #[test]
    fn test_get_filtered_session() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");
        session.id = "test-session-123".to_string();

        // Add old history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, true)));

        // Add summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Summary".to_string(),
            original_count: 5,
            compacted_at: 2000,
        }));

        // Add new history
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "New request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let compactor = SessionCompactor::new();
        let filtered_session = compactor.get_filtered_session(&session);

        // Session metadata should be preserved
        assert_eq!(filtered_session.id, "test-session-123");
        assert_eq!(filtered_session.model, "gpt-4-turbo");

        // Parts should be filtered
        assert_eq!(filtered_session.parts.len(), 2);
    }

    #[test]
    fn test_insert_compaction_marker_auto() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        compactor.insert_compaction_marker(&mut session, true);

        assert_eq!(session.parts.len(), 1);
        if let SessionPart::CompactionMarker(m) = &session.parts[0] {
            assert!(m.auto, "Auto flag should be true");
            assert!(m.timestamp > 0, "Timestamp should be set");
        } else {
            panic!("Should have added CompactionMarker");
        }
    }

    #[test]
    fn test_insert_compaction_marker_manual() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        compactor.insert_compaction_marker(&mut session, false);

        assert_eq!(session.parts.len(), 1);
        if let SessionPart::CompactionMarker(m) = &session.parts[0] {
            assert!(!m.auto, "Auto flag should be false for manual trigger");
        } else {
            panic!("Should have added CompactionMarker");
        }
    }

    #[test]
    fn test_filter_compacted_preserves_order() {
        use crate::components::types::CompactionMarker;

        let mut session = ExecutionSession::new();

        // Old content
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Old".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Compaction
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, true)));
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Summary".to_string(),
            original_count: 1,
            compacted_at: 2000,
        }));

        // New content in specific order
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Request 1".to_string(),
            context: None,
            timestamp: 3000,
        }));
        session.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: "Response 1".to_string(),
            reasoning: None,
            timestamp: 3100,
        }));
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Request 2".to_string(),
            context: None,
            timestamp: 3200,
        }));

        let compactor = SessionCompactor::new();
        let filtered = compactor.filter_compacted(&session);

        // Should preserve chronological order
        assert_eq!(filtered.len(), 4);
        assert!(matches!(filtered[0], SessionPart::Summary(_)));
        assert!(matches!(filtered[1], SessionPart::UserInput(_)));
        assert!(matches!(filtered[2], SessionPart::AiResponse(_)));
        assert!(matches!(filtered[3], SessionPart::UserInput(_)));

        // Verify specific order
        if let SessionPart::UserInput(u) = &filtered[1] {
            assert_eq!(u.text, "Request 1");
        }
        if let SessionPart::UserInput(u) = &filtered[3] {
            assert_eq!(u.text, "Request 2");
        }
    }

    #[test]
    fn test_compaction_marker_type_name() {
        use crate::components::types::CompactionMarker;

        let marker = SessionPart::CompactionMarker(CompactionMarker::with_timestamp(1000, true));

        assert_eq!(marker.type_name(), "compaction_marker");
    }

    #[test]
    fn test_build_summary_context_with_compaction_marker() {
        use crate::components::types::CompactionMarker;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(1000, true)));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Compaction Marker]:"), "Should contain marker");
        assert!(context.contains("1000"), "Should contain timestamp");
        assert!(context.contains("auto"), "Should indicate auto trigger");
    }

    #[test]
    fn test_build_summary_context_with_manual_compaction_marker() {
        use crate::components::types::CompactionMarker;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, false)));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Compaction Marker]:"));
        assert!(context.contains("manual"), "Should indicate manual trigger");
    }

    #[test]
    fn test_recalculate_tokens_with_compaction_marker() {
        use crate::components::types::CompactionMarker;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new();

        // Add a compaction marker
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(1000, true)));

        // Add some actual content for comparison
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Hello".to_string(),
            context: None,
            timestamp: 2000,
        }));

        compactor.recalculate_tokens(&mut session);

        // Compaction markers should not add to token count
        // Only the "Hello" text should contribute (5 chars * 0.4 = 2 tokens)
        assert_eq!(session.total_tokens, 2, "Only user input should contribute tokens");
    }

    // ========================================================================
    // EventHandler Integration Tests (Task 6)
    // ========================================================================

    #[tokio::test]
    async fn test_event_handler_respects_config() {
        use crate::event::{EventBus, LoopState};

        let config = CompactionConfig {
            auto_compact: false,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let loop_state = LoopState {
            session_id: "test".to_string(),
            iteration: 5,
            total_tokens: 150_000,
            last_tool: None,
            model: "gpt-4-turbo".to_string(),
        };

        let event = AlephEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty when auto_compact is disabled
        assert!(result.is_empty());
    }

    #[test]
    fn test_is_overflow_for_model() {
        let compactor = SessionCompactor::new();

        // gpt-4-turbo has 128K context, 80% threshold = 102.4K
        assert!(!compactor.is_overflow_for_model(100_000, "gpt-4-turbo"));
        assert!(compactor.is_overflow_for_model(110_000, "gpt-4-turbo"));

        // claude-3-opus has 200K context, 80% threshold = 160K
        assert!(!compactor.is_overflow_for_model(150_000, "claude-3-opus"));
        assert!(compactor.is_overflow_for_model(170_000, "claude-3-opus"));

        // Unknown model uses default (128K, 80% = 102.4K)
        assert!(!compactor.is_overflow_for_model(100_000, "unknown-model"));
        assert!(compactor.is_overflow_for_model(110_000, "unknown-model"));
    }

    #[tokio::test]
    async fn test_event_handler_with_high_tokens() {
        use crate::event::{EventBus, LoopState};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Create a loop state with tokens above compaction threshold
        // gpt-4-turbo: 128K * 0.8 = 102.4K threshold
        let loop_state = LoopState {
            session_id: "overflow-test".to_string(),
            iteration: 10,
            total_tokens: 110_000, // Above threshold
            last_tool: Some("search".to_string()),
            model: "gpt-4-turbo".to_string(),
        };

        let event = AlephEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Currently returns empty (would return SessionCompacted in full impl)
        // The logging would indicate compaction is needed
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_event_handler_tool_completed_with_prune_disabled() {
        use crate::event::{EventBus, TokenUsage, ToolCallResult};

        let config = CompactionConfig {
            prune_enabled: false,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
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

        // Should return empty (prune is disabled)
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_event_handler_tool_completed_with_prune_enabled() {
        use crate::event::{EventBus, TokenUsage, ToolCallResult};

        let config = CompactionConfig {
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
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

        // Should return empty but with debug logging
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_event_handler_with_model_prefix_match() {
        use crate::event::{EventBus, LoopState};

        let compactor = SessionCompactor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Use versioned model name that should match prefix
        let loop_state = LoopState {
            session_id: "prefix-test".to_string(),
            iteration: 5,
            total_tokens: 90_000, // Below threshold
            last_tool: None,
            model: "claude-3-opus-20240229".to_string(), // Should match "claude-3-opus"
        };

        let event = AlephEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty since below 160K threshold (200K * 0.8)
        assert!(result.is_empty());
    }

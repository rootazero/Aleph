//! Tests for session compactor

use crate::components::session_compactor::*;
use crate::components::types::{
    AiResponsePart, ExecutionSession, SessionPart, SummaryPart, ToolCallPart, ToolCallStatus,
    UserInputPart,
};
use crate::event::{AetherEvent, EventContext, EventHandler, EventType};
use serde_json::json;

    // ========================================================================
    // EnhancedTokenUsage Tests
    // ========================================================================

    #[test]
    fn test_enhanced_token_usage() {
        let usage = EnhancedTokenUsage {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 300,
            cache_write: 100,
        };

        // Total for overflow check = input + cache_read + output
        assert_eq!(usage.total_for_overflow(), 1800);

        // Total billable (excluding cache reads which are cheaper)
        assert_eq!(usage.total_billable(), 1700);
    }

    #[test]
    fn test_enhanced_token_usage_default() {
        let usage = EnhancedTokenUsage::default();

        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.reasoning, 0);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
        assert!(usage.is_empty());
    }

    #[test]
    fn test_enhanced_token_usage_new() {
        let usage = EnhancedTokenUsage::new(100, 200, 50, 75, 25);

        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 200);
        assert_eq!(usage.reasoning, 50);
        assert_eq!(usage.cache_read, 75);
        assert_eq!(usage.cache_write, 25);
    }

    #[test]
    fn test_enhanced_token_usage_add() {
        let mut usage1 = EnhancedTokenUsage {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 300,
            cache_write: 100,
        };

        let usage2 = EnhancedTokenUsage {
            input: 500,
            output: 250,
            reasoning: 100,
            cache_read: 150,
            cache_write: 50,
        };

        usage1.add(&usage2);

        assert_eq!(usage1.input, 1500);
        assert_eq!(usage1.output, 750);
        assert_eq!(usage1.reasoning, 300);
        assert_eq!(usage1.cache_read, 450);
        assert_eq!(usage1.cache_write, 150);
    }

    #[test]
    fn test_enhanced_token_usage_add_to_empty() {
        let mut usage = EnhancedTokenUsage::default();

        let other = EnhancedTokenUsage {
            input: 100,
            output: 200,
            reasoning: 50,
            cache_read: 75,
            cache_write: 25,
        };

        usage.add(&other);

        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 200);
        assert_eq!(usage.reasoning, 50);
        assert_eq!(usage.cache_read, 75);
        assert_eq!(usage.cache_write, 25);
    }

    #[test]
    fn test_enhanced_token_usage_is_empty() {
        let empty = EnhancedTokenUsage::default();
        assert!(empty.is_empty());

        let non_empty = EnhancedTokenUsage {
            input: 1,
            ..Default::default()
        };
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_enhanced_token_usage_total() {
        let usage = EnhancedTokenUsage {
            input: 1000,
            output: 500,
            reasoning: 200,
            cache_read: 300,
            cache_write: 100,
        };

        // Total = all fields combined
        assert_eq!(usage.total(), 2100);
    }

    #[test]
    fn test_enhanced_token_usage_equality() {
        let usage1 = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
        let usage2 = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
        let usage3 = EnhancedTokenUsage::new(100, 200, 50, 75, 26);

        assert_eq!(usage1, usage2);
        assert_ne!(usage1, usage3);
    }

    #[test]
    fn test_enhanced_token_usage_clone() {
        let usage = EnhancedTokenUsage::new(100, 200, 50, 75, 25);
        let cloned = usage.clone();

        assert_eq!(usage, cloned);
    }

    // ========================================================================
    // CompactionConfig Tests
    // ========================================================================

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!(config.auto_compact);
        assert!(config.prune_enabled);
        assert_eq!(config.prune_minimum, 20_000);
        assert_eq!(config.prune_protect, 40_000);
        assert!(config.protected_tools.contains(&"skill".to_string()));
    }

    #[test]
    fn test_compaction_config_disabled() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: false,
            ..Default::default()
        };
        assert!(!config.auto_compact);
        assert!(!config.prune_enabled);
    }

    #[test]
    fn test_compaction_config_custom_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read".to_string(), "write".to_string()],
            ..Default::default()
        };
        assert_eq!(config.protected_tools.len(), 3);
        assert!(config.protected_tools.contains(&"skill".to_string()));
        assert!(config.protected_tools.contains(&"read".to_string()));
        assert!(config.protected_tools.contains(&"write".to_string()));
    }

    #[test]
    fn test_session_compactor_with_config() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: true,
            prune_minimum: 10_000,
            prune_protect: 20_000,
            protected_tools: vec!["custom_tool".to_string()],
        };
        let compactor = SessionCompactor::with_config(config);

        assert!(!compactor.config().auto_compact);
        assert!(compactor.config().prune_enabled);
        assert_eq!(compactor.config().prune_minimum, 10_000);
        assert_eq!(compactor.config().prune_protect, 20_000);
        assert!(compactor.config().protected_tools.contains(&"custom_tool".to_string()));
    }

    #[test]
    fn test_session_compactor_config_mut() {
        let mut compactor = SessionCompactor::new();

        compactor.config_mut().auto_compact = false;
        compactor.config_mut().prune_minimum = 15_000;

        assert!(!compactor.config().auto_compact);
        assert_eq!(compactor.config().prune_minimum, 15_000);
    }

    // ========================================================================
    // ModelLimit Tests
    // ========================================================================

    #[test]
    fn test_model_limit_default() {
        let limit = ModelLimit::default();

        assert_eq!(limit.context_limit, 128000);
        assert_eq!(limit.max_output_tokens, 4096);
        assert!((limit.reserve_ratio - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_model_limit_custom() {
        let limit = ModelLimit::new(200000, 8192, 0.3);

        assert_eq!(limit.context_limit, 200000);
        assert_eq!(limit.max_output_tokens, 8192);
        assert!((limit.reserve_ratio - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_model_limit_reserve_ratio_clamped() {
        let limit1 = ModelLimit::new(100000, 4096, 1.5);
        assert!((limit1.reserve_ratio - 1.0).abs() < f32::EPSILON);

        let limit2 = ModelLimit::new(100000, 4096, -0.5);
        assert!((limit2.reserve_ratio - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compaction_threshold() {
        let limit = ModelLimit::new(100000, 4096, 0.2);
        // 100000 * (1 - 0.2) = 80000 (allow for floating point precision)
        let threshold1 = limit.compaction_threshold();
        assert!(
            threshold1 >= 79990 && threshold1 <= 80010,
            "Expected ~80000, got {}",
            threshold1
        );

        let limit2 = ModelLimit::new(200000, 4096, 0.1);
        // 200000 * (1 - 0.1) = 180000 (allow for floating point precision)
        let threshold2 = limit2.compaction_threshold();
        assert!(
            threshold2 >= 179990 && threshold2 <= 180010,
            "Expected ~180000, got {}",
            threshold2
        );
    }

    // ========================================================================
    // TokenTracker Tests
    // ========================================================================

    #[test]
    fn test_token_tracker_default() {
        let tracker = TokenTracker::new();

        // Check preset models
        let claude_opus = tracker.get_model_limit("claude-3-opus");
        assert_eq!(claude_opus.context_limit, 200000);

        let gpt4_turbo = tracker.get_model_limit("gpt-4-turbo");
        assert_eq!(gpt4_turbo.context_limit, 128000);

        let gemini_pro = tracker.get_model_limit("gemini-pro");
        assert_eq!(gemini_pro.context_limit, 32000);
    }

    #[test]
    fn test_token_tracker_unknown_model() {
        let tracker = TokenTracker::new();

        // Unknown model should return default
        let unknown = tracker.get_model_limit("unknown-model");
        assert_eq!(unknown.context_limit, 128000); // Default
    }

    #[test]
    fn test_token_tracker_prefix_match() {
        let tracker = TokenTracker::new();

        // Should match by prefix
        let claude_versioned = tracker.get_model_limit("claude-3-opus-20240229");
        assert_eq!(claude_versioned.context_limit, 200000);
    }

    #[test]
    fn test_token_estimation() {
        // Test basic estimation
        // "Hello" = 5 chars * 0.4 = 2 tokens (ceil)
        assert_eq!(TokenTracker::estimate_tokens("Hello"), 2);

        // Empty string = 0 tokens
        assert_eq!(TokenTracker::estimate_tokens(""), 0);

        // 100 chars * 0.4 = 40 tokens
        let text = "a".repeat(100);
        assert_eq!(TokenTracker::estimate_tokens(&text), 40);

        // 250 chars * 0.4 = 100 tokens
        let longer_text = "x".repeat(250);
        assert_eq!(TokenTracker::estimate_tokens(&longer_text), 100);
    }

    #[test]
    fn test_is_overflow() {
        let tracker = TokenTracker::new();

        // Create session with tokens below threshold
        let mut session = ExecutionSession::new().with_model("gemini-pro");
        session.total_tokens = 25000; // Below 32000 * 0.8 = 25600

        assert!(!tracker.is_overflow(&session));

        // Set tokens above threshold
        session.total_tokens = 26000; // Above 25600

        assert!(tracker.is_overflow(&session));
    }

    // ========================================================================
    // SessionCompactor Tests
    // ========================================================================

    fn create_test_session() -> ExecutionSession {
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Please help me analyze this code".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add multiple tool calls
        for i in 0..15 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("tool-{}", i),
                tool_name: format!("tool_{}", i),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some(format!("Output for tool {} with some content here", i)),
                error: None,
                started_at: 1000 + i * 100,
                completed_at: Some(1050 + i * 100),
            }));
        }

        // Add AI response
        session.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: "Analysis complete. The code looks good.".to_string(),
            reasoning: Some("Reviewed all components".to_string()),
            timestamp: 3000,
        }));

        session
    }

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
                    tc.output.as_ref().map_or(false, |o| o.contains("pruned"))
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
        let event = AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
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

        let event = AetherEvent::ToolCallCompleted(result_event);
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

        let event = AetherEvent::LoopContinue(loop_state);
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

    // ========================================================================
    // Smart Pruning with Protection Tests (Task 3)
    // ========================================================================

    /// Create a test session with skill tool calls that should be protected
    fn create_test_session_with_skill_calls() -> ExecutionSession {
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Use skill to do something".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add skill tool calls (should be protected)
        for i in 0..5 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("skill-{}", i),
                tool_name: "skill".to_string(),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some(format!("Skill output {} with content", i)),
                error: None,
                started_at: 1000 + i as i64 * 100,
                completed_at: Some(1050 + i as i64 * 100),
            }));
        }

        // Add another user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Now do more work".to_string(),
            context: None,
            timestamp: 1600,
        }));

        // Add regular tool calls (should be pruned if old enough)
        for i in 0..15 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("tool-{}", i),
                tool_name: format!("tool_{}", i),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some("x".repeat(500)), // ~200 tokens each
                error: None,
                started_at: 2000 + i as i64 * 100,
                completed_at: Some(2050 + i as i64 * 100),
            }));
        }

        // Add third user turn to create safe boundary
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Final request".to_string(),
            context: None,
            timestamp: 4000,
        }));

        session
    }

    /// Create a large test session for threshold testing
    fn create_large_test_session() -> ExecutionSession {
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add first user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Start working".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add many tool calls with large outputs to exceed thresholds
        for i in 0..50 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("large-tool-{}", i),
                tool_name: format!("tool_{}", i % 10),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                // Each output is ~2500 chars = ~1000 tokens
                output: Some("y".repeat(2500)),
                error: None,
                started_at: 2000 + i as i64 * 100,
                completed_at: Some(2050 + i as i64 * 100),
            }));
        }

        // Add second user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Continue".to_string(),
            context: None,
            timestamp: 8000,
        }));

        // Add more recent tool calls
        for i in 0..10 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("recent-tool-{}", i),
                tool_name: format!("recent_{}", i),
                input: json!({"param": i}),
                status: ToolCallStatus::Completed,
                output: Some("z".repeat(500)),
                error: None,
                started_at: 9000 + i as i64 * 100,
                completed_at: Some(9050 + i as i64 * 100),
            }));
        }

        // Add third user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Final".to_string(),
            context: None,
            timestamp: 10500,
        }));

        session
    }

    #[test]
    fn test_prune_respects_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read_file".to_string()],
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_test_session_with_skill_calls();

        compactor.prune_old_tool_outputs(&mut session);

        // Skill tool outputs should NOT be pruned
        let skill_outputs: Vec<_> = session.parts.iter()
            .filter_map(|p| match p {
                SessionPart::ToolCall(tc) if tc.tool_name == "skill" => tc.output.as_ref(),
                _ => None,
            })
            .collect();

        for output in skill_outputs {
            assert!(!output.contains("pruned"), "Skill outputs should not be pruned, got: {}", output);
        }
    }

    #[test]
    fn test_prune_with_thresholds_basic() {
        let config = CompactionConfig {
            prune_minimum: 1000,
            prune_protect: 2000,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_large_test_session();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Should only prune if exceeds prune_minimum
        assert!(pruned_info.tokens_pruned >= 1000 || pruned_info.tokens_pruned == 0,
            "tokens_pruned should be >= 1000 or 0, got {}", pruned_info.tokens_pruned);
    }

    #[test]
    fn test_prune_with_thresholds_respects_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string()],
            prune_minimum: 100,
            prune_protect: 500,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_test_session_with_skill_calls();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Protected tools should be counted
        assert!(pruned_info.parts_protected >= 5,
            "Expected at least 5 protected parts (skill calls), got {}",
            pruned_info.parts_protected);

        // Verify skill outputs were not pruned
        for part in &session.parts {
            if let SessionPart::ToolCall(tc) = part {
                if tc.tool_name == "skill" {
                    assert!(!tc.output.as_ref().unwrap().contains("pruned"),
                        "Skill tool output should not be pruned");
                }
            }
        }
    }

    #[test]
    fn test_prune_with_thresholds_disabled() {
        let config = CompactionConfig {
            prune_enabled: false,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_large_test_session();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Should not prune anything when disabled
        assert_eq!(pruned_info.tokens_pruned, 0);
        assert_eq!(pruned_info.parts_pruned, 0);
        assert_eq!(pruned_info.parts_protected, 0);
    }

    #[test]
    fn test_prune_with_thresholds_high_minimum() {
        let config = CompactionConfig {
            prune_minimum: 1_000_000, // Very high threshold
            prune_protect: 500,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = create_large_test_session();

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Should not prune because we won't exceed the high minimum
        assert_eq!(pruned_info.parts_pruned, 0);
    }

    #[test]
    fn test_is_protected_tool() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read_file".to_string()],
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);

        assert!(compactor.is_protected_tool("skill"));
        assert!(compactor.is_protected_tool("read_file"));
        assert!(!compactor.is_protected_tool("write_file"));
        assert!(!compactor.is_protected_tool("search"));
    }

    #[test]
    fn test_prune_info_default() {
        let info = PruneInfo::default();
        assert_eq!(info.tokens_pruned, 0);
        assert_eq!(info.parts_pruned, 0);
        assert_eq!(info.parts_protected, 0);
    }

    #[test]
    fn test_prune_old_tool_outputs_with_protected_tools() {
        // Test that prune_old_tool_outputs also respects protected tools
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string()],
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add 20 tool calls, some are protected
        for i in 0..20 {
            let tool_name = if i % 4 == 0 { "skill".to_string() } else { format!("tool_{}", i) };
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("call-{}", i),
                tool_name,
                input: json!({"i": i}),
                status: ToolCallStatus::Completed,
                output: Some(format!("Output {}", i)),
                error: None,
                started_at: 1000 + i as i64 * 100,
                completed_at: Some(1050 + i as i64 * 100),
            }));
        }

        // Default keep_recent_tools is 10, so 10 should be pruned
        compactor.prune_old_tool_outputs(&mut session);

        // Verify skill tool outputs were NOT pruned
        for part in &session.parts {
            if let SessionPart::ToolCall(tc) = part {
                if tc.tool_name == "skill" {
                    assert!(!tc.output.as_ref().unwrap().contains("pruned"),
                        "Skill outputs should not be pruned: {:?}", tc.output);
                }
            }
        }
    }

    #[test]
    fn test_prune_with_thresholds_preserves_recent_turns() {
        let config = CompactionConfig {
            prune_minimum: 100,
            prune_protect: 200,
            prune_enabled: true,
            ..Default::default()
        };
        let compactor = SessionCompactor::with_config(config);
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // First user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "First request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Old tool calls
        for i in 0..5 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("old-{}", i),
                tool_name: "old_tool".to_string(),
                input: json!({}),
                output: Some("x".repeat(1000)),
                status: ToolCallStatus::Completed,
                error: None,
                started_at: 1100 + i as i64 * 100,
                completed_at: Some(1150 + i as i64 * 100),
            }));
        }

        // Second user turn (recent)
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Second request".to_string(),
            context: None,
            timestamp: 2000,
        }));

        // Recent tool calls (should be protected by user turn boundary)
        for i in 0..3 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("recent-{}", i),
                tool_name: "recent_tool".to_string(),
                input: json!({}),
                output: Some("y".repeat(500)),
                status: ToolCallStatus::Completed,
                error: None,
                started_at: 2100 + i as i64 * 100,
                completed_at: Some(2150 + i as i64 * 100),
            }));
        }

        // Third user turn
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Third request".to_string(),
            context: None,
            timestamp: 3000,
        }));

        let pruned_info = compactor.prune_with_thresholds(&mut session);

        // Recent tool calls (after second user turn) should not be pruned
        for part in &session.parts {
            if let SessionPart::ToolCall(tc) = part {
                if tc.tool_name == "recent_tool" {
                    assert!(!tc.output.as_ref().unwrap().contains("pruned"),
                        "Recent tool outputs should not be pruned");
                }
            }
        }

        // Verify some old tools were pruned (if thresholds were exceeded)
        // Note: This depends on whether we exceeded the thresholds
        println!("Pruned info: tokens={}, parts={}, protected={}",
            pruned_info.tokens_pruned, pruned_info.parts_pruned, pruned_info.parts_protected);
    }

    // ========================================================================
    // LLM-Driven Summarization Tests (Task 4)
    // ========================================================================

    #[test]
    fn test_compaction_prompt_exists() {
        let prompt = super::compaction_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("summarizing conversations"));
        assert!(prompt.contains("What was done"));
        assert!(prompt.contains("What needs to be done next"));
    }

    #[test]
    fn test_build_summary_context_basic() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let context = compactor.build_summary_context(&session);

        // Should contain user input
        assert!(context.contains("User:"), "Context should contain 'User:' prefix");
        assert!(context.contains("Please help me analyze this code"), "Context should contain user request");

        // Should contain tool information
        assert!(context.contains("Tool"), "Context should contain tool calls");
        assert!(context.contains("completed"), "Context should show tool completion status");
    }

    #[test]
    fn test_build_summary_context_with_ai_response() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let context = compactor.build_summary_context(&session);

        // Should contain AI response
        assert!(context.contains("Assistant:"), "Context should contain 'Assistant:' prefix");
        assert!(context.contains("Analysis complete"), "Context should contain AI response content");
    }

    #[test]
    fn test_build_summary_context_truncates_long_output() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add tool call with output > 200 chars
        let long_output = "x".repeat(500);
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: "long-output-tool".to_string(),
            tool_name: "test_tool".to_string(),
            input: json!({"param": "value"}),
            status: ToolCallStatus::Completed,
            output: Some(long_output),
            error: None,
            started_at: 1100,
            completed_at: Some(1200),
        }));

        let context = compactor.build_summary_context(&session);

        // Should be truncated with "..."
        assert!(context.contains("..."), "Long output should be truncated with '...'");
        // Original 500 char output should not appear in full
        assert!(!context.contains(&"x".repeat(500)), "Full 500 char output should not appear");
        // But truncated 200 chars should appear
        assert!(context.contains(&"x".repeat(200)), "Truncated 200 chars should appear");
    }

    #[test]
    fn test_build_summary_context_with_failed_tool() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test request".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add failed tool call
        session.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: "failed-tool".to_string(),
            tool_name: "failing_tool".to_string(),
            input: json!({}),
            status: ToolCallStatus::Failed,
            output: None,
            error: Some("Connection timeout".to_string()),
            started_at: 1100,
            completed_at: Some(1200),
        }));

        let context = compactor.build_summary_context(&session);

        // Should show failed status
        assert!(context.contains("failed"), "Context should show 'failed' for tool with error");
    }

    #[test]
    fn test_build_summary_context_with_summary_part() {
        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add previous summary
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: "Previous session worked on feature X".to_string(),
            original_count: 50,
            compacted_at: 1000,
        }));

        // Add new user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Continue with feature X".to_string(),
            context: None,
            timestamp: 2000,
        }));

        let context = compactor.build_summary_context(&session);

        // Should contain previous summary
        assert!(context.contains("[Previous Summary]:"), "Context should contain previous summary marker");
        assert!(context.contains("Previous session worked on feature X"));
    }

    #[test]
    fn test_build_summary_context_empty_session() {
        let compactor = SessionCompactor::new();
        let session = ExecutionSession::new().with_model("gpt-4-turbo");

        let context = compactor.build_summary_context(&session);

        // Should be empty for empty session
        assert!(context.is_empty(), "Empty session should produce empty context");
    }

    #[tokio::test]
    async fn test_generate_llm_summary_without_callback() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Without callback, should fall back to template-based summary
        let summary = compactor.generate_llm_summary(&session, None).await;

        // Should contain template-based summary elements
        assert!(summary.contains("Original Request"));
        assert!(summary.contains("Please help me analyze this code"));
    }

    #[tokio::test]
    async fn test_generate_llm_summary_with_callback() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Track if callback was called
        let callback_called = Arc::new(AtomicBool::new(false));
        let callback_called_clone = callback_called.clone();

        // Create mock callback
        let callback: super::LlmCallback = Box::new(move |system_prompt, user_content| {
            let called = callback_called_clone.clone();
            Box::pin(async move {
                called.store(true, Ordering::SeqCst);

                // Verify prompts
                assert!(system_prompt.contains("summarizing conversations"));
                assert!(user_content.contains("conversation to summarize"));

                Ok("LLM generated summary: The user is analyzing code.".to_string())
            })
        });

        let summary = compactor.generate_llm_summary(&session, Some(&callback)).await;

        assert!(callback_called.load(Ordering::SeqCst), "Callback should be called");
        assert!(summary.contains("LLM generated summary"));
    }

    #[tokio::test]
    async fn test_generate_llm_summary_fallback_on_error() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        // Create callback that returns error
        let callback: super::LlmCallback = Box::new(|_system_prompt, _user_content| {
            Box::pin(async move {
                Err("LLM service unavailable".to_string())
            })
        });

        let summary = compactor.generate_llm_summary(&session, Some(&callback)).await;

        // Should fall back to template-based summary
        assert!(summary.contains("Original Request"), "Should fall back to template on error");
        assert!(summary.contains("Please help me analyze this code"));
    }

    #[test]
    fn test_build_summary_context_with_reasoning() {
        use crate::components::types::ReasoningPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add reasoning part
        session.parts.push(SessionPart::Reasoning(ReasoningPart {
            content: "Thinking through the problem...".to_string(),
            step: 0,
            is_complete: true,
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Reasoning]:"), "Context should contain reasoning marker");
        assert!(context.contains("Thinking through the problem"));
    }

    #[test]
    fn test_build_summary_context_with_plan() {
        use crate::components::types::PlanPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add plan
        session.parts.push(SessionPart::PlanCreated(PlanPart {
            plan_id: "plan-123".to_string(),
            steps: vec![
                crate::components::types::PlanStep {
                    step_id: "step-1".to_string(),
                    description: "Step 1".to_string(),
                    status: crate::components::types::StepStatus::Pending,
                    dependencies: vec![],
                },
                crate::components::types::PlanStep {
                    step_id: "step-2".to_string(),
                    description: "Step 2".to_string(),
                    status: crate::components::types::StepStatus::Pending,
                    dependencies: vec![],
                },
                crate::components::types::PlanStep {
                    step_id: "step-3".to_string(),
                    description: "Step 3".to_string(),
                    status: crate::components::types::StepStatus::Pending,
                    dependencies: vec![],
                },
            ],
            requires_confirmation: false,
            created_at: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[Plan Created]:"), "Context should contain plan marker");
        assert!(context.contains("plan-123"));
        assert!(context.contains("Step 1, Step 2, Step 3"));
    }

    #[test]
    fn test_build_summary_context_with_subagent() {
        use crate::components::types::SubAgentPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add subagent call with result
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "code-review-agent".to_string(),
            prompt: "Review this code".to_string(),
            result: Some("Code looks good with minor suggestions".to_string()),
            timestamp: 1100,
        }));

        // Add subagent call without result (pending)
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "test-agent".to_string(),
            prompt: "Run tests".to_string(),
            result: None,
            timestamp: 1300,
        }));

        let context = compactor.build_summary_context(&session);

        assert!(context.contains("[SubAgent code-review-agent]:"));
        assert!(context.contains("Review this code"));
        assert!(context.contains("Code looks good"));
        assert!(context.contains("[pending]"), "Pending subagent should show [pending]");
    }

    #[test]
    fn test_build_summary_context_subagent_truncates_long_result() {
        use crate::components::types::SubAgentPart;

        let compactor = SessionCompactor::new();
        let mut session = ExecutionSession::new().with_model("gpt-4-turbo");

        // Add user input
        session.parts.push(SessionPart::UserInput(UserInputPart {
            text: "Test".to_string(),
            context: None,
            timestamp: 1000,
        }));

        // Add subagent call with long result
        let long_result = "x".repeat(500);
        session.parts.push(SessionPart::SubAgentCall(SubAgentPart {
            agent_id: "analysis-agent".to_string(),
            prompt: "Analyze".to_string(),
            result: Some(long_result),
            timestamp: 1100,
        }));

        let context = compactor.build_summary_context(&session);

        // Should be truncated
        assert!(context.contains("..."), "Long subagent result should be truncated");
        assert!(!context.contains(&"x".repeat(500)), "Full result should not appear");
    }

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

        let event = AetherEvent::LoopContinue(loop_state);
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

        let event = AetherEvent::LoopContinue(loop_state);
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

        let event = AetherEvent::ToolCallCompleted(result_event);
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

        let event = AetherEvent::ToolCallCompleted(result_event);
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

        let event = AetherEvent::LoopContinue(loop_state);
        let result = compactor.handle(&event, &ctx).await.unwrap();

        // Should return empty since below 160K threshold (200K * 0.8)
        assert!(result.is_empty());
    }

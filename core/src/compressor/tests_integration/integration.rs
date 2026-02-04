// Aether/core/src/compressor/tests_integration/integration.rs
//! Integration tests for the Smart Compaction system.
//!
//! These tests verify that:
//! 1. SmartCompactor properly protects turns and truncates outputs
//! 2. All compaction components work together correctly
//! 3. Session parts are handled consistently across compaction operations

#[cfg(test)]
mod tests {
    use crate::compressor::{
        CompactionAction, SmartCompactionStrategy, SmartCompactor, ToolTruncator, TurnProtector,
    };
    use crate::components::{
        AiResponsePart, CompactionMarker, SessionPart, StepFinishPart, StepFinishReason,
        StepStartPart, ToolCallPart, ToolCallStatus, UserInputPart,
    };
    use serde_json::json;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_user_input(text: &str) -> SessionPart {
        SessionPart::UserInput(UserInputPart {
            text: text.to_string(),
            context: None,
            timestamp: 1000,
        })
    }

    fn create_ai_response(content: &str) -> SessionPart {
        SessionPart::AiResponse(AiResponsePart {
            content: content.to_string(),
            reasoning: None,
            timestamp: 1000,
        })
    }

    fn create_tool_call(name: &str, output: Option<String>) -> SessionPart {
        SessionPart::ToolCall(ToolCallPart {
            id: format!("call-{}", name),
            tool_name: name.to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output,
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        })
    }

    fn create_step_start(step: usize) -> SessionPart {
        SessionPart::StepStart(StepStartPart::new(step))
    }

    fn create_step_finish(step: usize) -> SessionPart {
        SessionPart::StepFinish(StepFinishPart::new(step, StepFinishReason::Completed, 1000))
    }

    fn create_compaction_marker() -> SessionPart {
        SessionPart::CompactionMarker(CompactionMarker::with_details(
            true,
            "marker-1".to_string(),
            5,
            1000,
        ))
    }

    // =========================================================================
    // SmartCompactor + TurnProtector Integration
    // =========================================================================

    #[test]
    fn test_compactor_respects_turn_protection() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(2)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(5000);

        // 4 turns total, last 2 should be protected
        let parts = vec![
            // Turn 0: NOT protected
            create_user_input("Turn 0"),
            create_tool_call("read", Some(large_output.clone())),
            // Turn 1: NOT protected
            create_user_input("Turn 1"),
            create_tool_call("read", Some(large_output.clone())),
            // Turn 2: PROTECTED
            create_user_input("Turn 2"),
            create_tool_call("read", Some(large_output.clone())),
            // Turn 3: PROTECTED
            create_user_input("Turn 3"),
            create_tool_call("read", Some(large_output.clone())),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Turns 0 and 1 should be truncated
        // Turns 2 and 3 should be preserved

        // Check turn 0 tool was truncated
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            assert!(
                tc.output.as_ref().unwrap().len() <= 100,
                "Turn 0 tool should be truncated"
            );
        }

        // Check turn 1 tool was truncated
        if let SessionPart::ToolCall(tc) = &result.parts[3] {
            assert!(
                tc.output.as_ref().unwrap().len() <= 100,
                "Turn 1 tool should be truncated"
            );
        }

        // Check turn 2 tool was NOT truncated
        if let SessionPart::ToolCall(tc) = &result.parts[5] {
            assert_eq!(
                tc.output.as_ref().unwrap().len(),
                5000,
                "Turn 2 tool should NOT be truncated (protected)"
            );
        }

        // Check turn 3 tool was NOT truncated
        if let SessionPart::ToolCall(tc) = &result.parts[7] {
            assert_eq!(
                tc.output.as_ref().unwrap().len(),
                5000,
                "Turn 3 tool should NOT be truncated (protected)"
            );
        }

        // Only 2 parts should have been compacted
        assert_eq!(result.parts_compacted, 2);
    }

    #[test]
    fn test_turn_protector_counts_turns_correctly() {
        let protector = TurnProtector::new(2);

        // Session with mixed parts
        let parts = vec![
            create_user_input("Hello"),
            create_ai_response("Hi!"),
            create_tool_call("search", Some("results".to_string())),
            create_user_input("Find files"),
            create_tool_call("read", Some("content".to_string())),
            create_ai_response("Found them"),
            create_user_input("Thanks"),
        ];

        let turn_count = protector.count_turns(&parts);
        assert_eq!(turn_count, 3, "Should have 3 turns (3 UserInput parts)");

        let protected = protector.protected_part_indices(&parts);
        // Last 2 turns: Turn 1 (parts 3,4,5) and Turn 2 (part 6)
        assert!(protected.contains(&3));
        assert!(protected.contains(&4));
        assert!(protected.contains(&5));
        assert!(protected.contains(&6));
    }

    // =========================================================================
    // SmartCompactor + ToolTruncator Integration
    // =========================================================================

    #[test]
    fn test_compactor_uses_truncator_correctly() {
        let truncator = ToolTruncator::new(150)
            .with_summary_template("[COMPACT: {tool_name}, {original_len} -> {truncated_len}]");

        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(150);

        let compactor = SmartCompactor::with_strategy(strategy).with_truncator(truncator);

        let parts = vec![
            create_user_input("Read file"),
            create_tool_call("read_file", Some("File content here\n".repeat(100))),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Verify truncation with custom template
        if let SessionPart::ToolCall(tc) = &result.parts[1] {
            let output = tc.output.as_ref().unwrap();
            assert!(output.len() <= 150);
            assert!(output.contains("[COMPACT:"));
            assert!(output.contains("read_file"));
        }
    }

    #[test]
    fn test_truncator_handles_multiline_output() {
        let truncator = ToolTruncator::new(200);

        let multiline_output = format!(r#"Line 1: Important header
Line 2: Some content
Line 3: More content
Line 4: Even more content
{}
"#, "Additional content\n".repeat(50));

        let result = truncator.truncate(&multiline_output, "read_file");

        assert!(result.was_truncated);
        assert!(result.summary.contains("Line 1:"));
        assert!(result.content.len() <= 200);
    }

    // =========================================================================
    // SmartCompactionStrategy Tests
    // =========================================================================

    #[test]
    fn test_strategy_evaluate_part_priority() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(1)
            .with_tool_output_max_chars(100)
            .add_protected_tool("skill");

        let large_output = "x".repeat(5000);

        // Test: Protected turn takes priority
        let part = SessionPart::ToolCall(ToolCallPart {
            id: "call-1".to_string(),
            tool_name: "read".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output.clone()),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        // Turn 1 in 2 turns is protected
        let action = strategy.evaluate_part(&part, 1, 2);
        assert_eq!(action, CompactionAction::Keep);

        // Turn 0 in 2 turns is NOT protected, should truncate
        let action = strategy.evaluate_part(&part, 0, 2);
        assert!(matches!(action, CompactionAction::Truncate { .. }));
    }

    #[test]
    fn test_strategy_protected_tools_in_unprotected_turns() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0) // No turn protection
            .with_tool_output_max_chars(100)
            .add_protected_tool("skill")
            .add_protected_tool("plan");

        let large_output = "x".repeat(5000);

        // Protected tool: skill
        let skill_part = SessionPart::ToolCall(ToolCallPart {
            id: "call-skill".to_string(),
            tool_name: "skill".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output.clone()),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        let action = strategy.evaluate_part(&skill_part, 0, 5);
        assert_eq!(action, CompactionAction::Keep);

        // Non-protected tool: read_file
        let read_part = SessionPart::ToolCall(ToolCallPart {
            id: "call-read".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({}),
            status: ToolCallStatus::Completed,
            output: Some(large_output),
            error: None,
            started_at: 1000,
            completed_at: Some(2000),
        });

        let action = strategy.evaluate_part(&read_part, 0, 5);
        assert!(matches!(action, CompactionAction::Truncate { .. }));
    }

    // =========================================================================
    // SessionPart Serialization Tests
    // =========================================================================

    #[test]
    fn test_step_start_serialization() {
        let part = StepStartPart::new(5);

        let json = serde_json::to_string(&part).unwrap();
        let deserialized: StepStartPart = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.step_id, 5);
    }

    #[test]
    fn test_step_finish_serialization() {
        let part = StepFinishPart::new(3, StepFinishReason::Completed, 500);

        let json = serde_json::to_string(&part).unwrap();
        let deserialized: StepFinishPart = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.step_id, 3);
        assert!(matches!(deserialized.reason, StepFinishReason::Completed));
        assert_eq!(deserialized.duration_ms, 500);
    }

    #[test]
    fn test_compaction_marker_serialization() {
        let marker = CompactionMarker::with_details(true, "marker-123".to_string(), 10, 5000);

        let json = serde_json::to_string(&marker).unwrap();
        let deserialized: CompactionMarker = serde_json::from_str(&json).unwrap();

        assert!(deserialized.auto);
        assert_eq!(deserialized.marker_id, Some("marker-123".to_string()));
        assert_eq!(deserialized.parts_compacted, Some(10));
        assert_eq!(deserialized.tokens_freed, Some(5000));
    }

    #[test]
    fn test_session_part_step_start_serialization() {
        let part = create_step_start(5);

        let json = serde_json::to_string(&part).unwrap();
        let deserialized: SessionPart = serde_json::from_str(&json).unwrap();

        if let SessionPart::StepStart(step_start) = deserialized {
            assert_eq!(step_start.step_id, 5);
        } else {
            panic!("Expected StepStart");
        }
    }

    #[test]
    fn test_session_part_step_finish_serialization() {
        let part = create_step_finish(3);

        let json = serde_json::to_string(&part).unwrap();
        let deserialized: SessionPart = serde_json::from_str(&json).unwrap();

        if let SessionPart::StepFinish(step_finish) = deserialized {
            assert_eq!(step_finish.step_id, 3);
        } else {
            panic!("Expected StepFinish");
        }
    }

    #[test]
    fn test_session_part_compaction_marker_serialization() {
        let part = create_compaction_marker();

        let json = serde_json::to_string(&part).unwrap();
        let deserialized: SessionPart = serde_json::from_str(&json).unwrap();

        if let SessionPart::CompactionMarker(marker) = deserialized {
            assert!(marker.auto);
            assert_eq!(marker.marker_id, Some("marker-1".to_string()));
        } else {
            panic!("Expected CompactionMarker");
        }
    }

    // =========================================================================
    // Full Session Flow Integration Tests
    // =========================================================================

    #[test]
    fn test_compaction_with_step_parts() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(1)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(1000);

        // Session with step boundaries
        let parts = vec![
            create_step_start(1),
            create_user_input("Find config"),
            create_tool_call("search", Some(large_output.clone())),
            create_step_finish(1),
            create_step_start(2),
            create_user_input("Update settings"),
            create_tool_call("edit", Some(large_output.clone())),
            create_step_finish(2),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Verify all parts are preserved
        assert_eq!(result.parts.len(), 8);

        // Only turn 0 (turn 1 is protected) tool should be compacted
        assert_eq!(result.parts_compacted, 1);

        // Verify step boundaries are preserved as-is
        if let SessionPart::StepStart(step_start) = &result.parts[0] {
            assert_eq!(step_start.step_id, 1);
        } else {
            panic!("Expected StepStart at index 0");
        }

        // Verify step finish is preserved
        if let SessionPart::StepFinish(step_finish) = &result.parts[3] {
            assert_eq!(step_finish.step_id, 1);
        } else {
            panic!("Expected StepFinish at index 3");
        }
    }

    #[test]
    fn test_compaction_preserves_compaction_markers() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(1000);

        // Session that already has a compaction marker
        let parts = vec![
            create_user_input("Previous query"),
            create_compaction_marker(), // Previous compaction marker
            create_user_input("New query"),
            create_tool_call("read", Some(large_output)),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Old marker should be preserved
        if let SessionPart::CompactionMarker(marker) = &result.parts[1] {
            assert_eq!(marker.marker_id, Some("marker-1".to_string()));
        } else {
            panic!("Expected CompactionMarker at index 1");
        }

        // New compaction should have occurred
        assert!(result.marker.is_some());
        assert_eq!(result.parts_compacted, 1);
    }

    // =========================================================================
    // Edge Case Integration Tests
    // =========================================================================

    #[test]
    fn test_compaction_with_only_step_parts() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_compaction_threshold(0.50); // Low threshold to trigger

        let compactor = SmartCompactor::with_strategy(strategy);

        // Session with only step parts (no tool calls)
        let parts = vec![
            create_step_start(1),
            create_step_finish(1),
            create_step_start(2),
            create_step_finish(2),
        ];

        let result = compactor.compact(&parts, 0.80);

        // No ToolCall parts to compact
        assert_eq!(result.parts_compacted, 0);
        assert_eq!(result.parts.len(), 4);
    }

    #[test]
    fn test_compaction_mixed_part_types() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(500);

        let parts = vec![
            create_step_start(1),
            create_user_input("Query"),
            create_ai_response("Let me search..."),
            create_tool_call("search", Some(large_output.clone())),
            create_compaction_marker(),
            create_tool_call("read", Some(large_output)),
            create_ai_response("Found results"),
            create_step_finish(1),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Both ToolCall parts should be compacted
        assert_eq!(result.parts_compacted, 2);

        // All other parts preserved
        assert_eq!(result.parts.len(), 8);

        // Verify structure is preserved
        assert!(matches!(&result.parts[0], SessionPart::StepStart(_)));
        assert!(matches!(&result.parts[1], SessionPart::UserInput(_)));
        assert!(matches!(&result.parts[2], SessionPart::AiResponse(_)));
        assert!(matches!(&result.parts[3], SessionPart::ToolCall(_)));
        assert!(matches!(&result.parts[4], SessionPart::CompactionMarker(_)));
        assert!(matches!(&result.parts[5], SessionPart::ToolCall(_)));
        assert!(matches!(&result.parts[6], SessionPart::AiResponse(_)));
        assert!(matches!(&result.parts[7], SessionPart::StepFinish(_)));
    }

    // =========================================================================
    // Token Estimation Tests
    // =========================================================================

    #[test]
    fn test_tokens_freed_estimation_accuracy() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        // Create output with known size
        let large_output = "x".repeat(4000); // 4000 chars = ~1000 tokens

        let parts = vec![
            create_user_input("Query"),
            create_tool_call("read", Some(large_output)),
        ];

        let result = compactor.compact(&parts, 0.90);

        // Original: 4000 chars, truncated to ~100 chars
        // Freed: ~3900 chars = ~975 tokens
        assert!(
            result.tokens_freed_estimate >= 900 && result.tokens_freed_estimate <= 1000,
            "Token estimate should be approximately 975, got {}",
            result.tokens_freed_estimate
        );
    }

    #[test]
    fn test_multiple_tools_tokens_freed() {
        let strategy = SmartCompactionStrategy::new()
            .with_protected_turns(0)
            .with_tool_output_max_chars(100);

        let compactor = SmartCompactor::with_strategy(strategy);

        let large_output = "x".repeat(2000); // 2000 chars = ~500 tokens each

        let parts = vec![
            create_user_input("Query"),
            create_tool_call("read1", Some(large_output.clone())),
            create_tool_call("read2", Some(large_output.clone())),
            create_tool_call("read3", Some(large_output)),
        ];

        let result = compactor.compact(&parts, 0.90);

        // 3 tools, each freeing ~475 tokens
        assert!(
            result.tokens_freed_estimate >= 1300,
            "Total tokens freed should be at least 1300, got {}",
            result.tokens_freed_estimate
        );
        assert_eq!(result.parts_compacted, 3);
    }
}

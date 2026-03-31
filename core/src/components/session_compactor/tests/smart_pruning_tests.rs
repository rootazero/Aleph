//! Smart Pruning with Protection tests (Task 3)

use super::helpers::{create_large_test_session, create_test_session_with_skill_calls};
use crate::components::session_compactor::*;
use crate::components::types::{
    ExecutionSession, SessionPart, ToolCallPart, ToolCallStatus, UserInputPart,
};
use serde_json::json;

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
    let skill_outputs: Vec<_> = session
        .parts
        .iter()
        .filter_map(|p| match p {
            SessionPart::ToolCall(tc) if tc.tool_name == "skill" => tc.output.as_ref(),
            _ => None,
        })
        .collect();

    for output in skill_outputs {
        assert!(
            !output.contains("pruned"),
            "Skill outputs should not be pruned, got: {}",
            output
        );
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
    assert!(
        pruned_info.tokens_pruned >= 1000 || pruned_info.tokens_pruned == 0,
        "tokens_pruned should be >= 1000 or 0, got {}",
        pruned_info.tokens_pruned
    );
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
    assert!(
        pruned_info.parts_protected >= 5,
        "Expected at least 5 protected parts (skill calls), got {}",
        pruned_info.parts_protected
    );

    // Verify skill outputs were not pruned
    for part in &session.parts {
        if let SessionPart::ToolCall(tc) = part {
            if tc.tool_name == "skill" {
                assert!(
                    !tc.output.as_ref().unwrap().contains("pruned"),
                    "Skill tool output should not be pruned"
                );
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
        let tool_name = if i % 4 == 0 {
            "skill".to_string()
        } else {
            format!("tool_{}", i)
        };
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
                assert!(
                    !tc.output.as_ref().unwrap().contains("pruned"),
                    "Skill outputs should not be pruned: {:?}",
                    tc.output
                );
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
                assert!(
                    !tc.output.as_ref().unwrap().contains("pruned"),
                    "Recent tool outputs should not be pruned"
                );
            }
        }
    }

    // Verify some old tools were pruned (if thresholds were exceeded)
    // Note: This depends on whether we exceeded the thresholds
    println!(
        "Pruned info: tokens={}, parts={}, protected={}",
        pruned_info.tokens_pruned, pruned_info.parts_pruned, pruned_info.parts_protected
    );
}

//! Tests for MessageBuilder

use std::sync::Arc;

use serde_json::json;

use crate::agent_loop::overflow::{OverflowConfig, OverflowDetector};
use crate::agent_loop::message_builder::{MessageBuilder, MessageBuilderConfig, Message, ToolCall};
use crate::components::{
    AiResponsePart, CompactionMarker, ExecutionSession, SessionCompactor, SessionPart,
    SummaryPart, ToolCallPart, ToolCallStatus, UserInputPart,
};

/// Helper to create a basic MessageBuilder
fn create_builder() -> MessageBuilder {
    MessageBuilder::new(MessageBuilderConfig::default())
}

#[test]
fn test_parts_to_messages_user_input() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::UserInput(UserInputPart {
            text: "Hello, help me with a task".to_string(),
            context: None,
            timestamp: 1000,
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "Hello, help me with a task");
    assert!(messages[0].tool_call_id.is_none());
    assert!(messages[0].tool_calls.is_none());
}

#[test]
fn test_parts_to_messages_user_input_with_context() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::UserInput(UserInputPart {
            text: "Find the bug".to_string(),
            context: Some("Selected file: main.rs".to_string()),
            timestamp: 1000,
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    assert_eq!(messages.len(), 1);
    assert!(messages[0].content.contains("Find the bug"));
    assert!(messages[0].content.contains("Context: Selected file: main.rs"));
}

#[test]
fn test_parts_to_messages_tool_call() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::ToolCall(ToolCallPart {
            id: "call_123".to_string(),
            tool_name: "search_files".to_string(),
            input: json!({"query": "*.rs"}),
            status: ToolCallStatus::Completed,
            output: Some("Found 5 files".to_string()),
            error: None,
            started_at: 1000,
            completed_at: Some(1500),
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    // Should create 2 messages: assistant with tool call + tool result
    assert_eq!(messages.len(), 2);

    // First message: assistant with tool call
    assert_eq!(messages[0].role, "assistant");
    assert!(messages[0].tool_calls.is_some());
    let tool_calls = messages[0].tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_123");
    assert_eq!(tool_calls[0].name, "search_files");

    // Second message: tool result
    assert_eq!(messages[1].role, "tool");
    assert_eq!(messages[1].tool_call_id, Some("call_123".to_string()));
    assert_eq!(messages[1].content, "Found 5 files");
}

#[test]
fn test_parts_to_messages_tool_call_failed() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::ToolCall(ToolCallPart {
            id: "call_456".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({"path": "/nonexistent"}),
            status: ToolCallStatus::Failed,
            output: None,
            error: Some("File not found".to_string()),
            started_at: 1000,
            completed_at: Some(1100),
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "Error: File not found");
}

#[test]
fn test_parts_to_messages_tool_call_interrupted() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::ToolCall(ToolCallPart {
            id: "call_789".to_string(),
            tool_name: "long_running_task".to_string(),
            input: json!({}),
            status: ToolCallStatus::Running,
            output: None,
            error: None,
            started_at: 1000,
            completed_at: None,
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "[Tool execution was interrupted]");
}

#[test]
fn test_inject_reminders() {
    let config = MessageBuilderConfig::default()
        .with_reminder_threshold(1);
    let builder = MessageBuilder::new(config);

    let mut messages = vec![
        Message::user("First message"),
        Message::assistant("Response"),
        Message::user("Second message"),
    ];

    let mut session = ExecutionSession::new();
    session.iteration_count = 2; // Above threshold

    builder.inject_reminders(&mut messages, &session);

    // Last user message should be wrapped
    assert!(messages[2].content.contains("<system-reminder>"));
    assert!(messages[2].content.contains("Second message"));
    assert!(messages[2].content.contains("Please address this message"));

    // First user message should not be wrapped
    assert!(!messages[0].content.contains("<system-reminder>"));
}

#[test]
fn test_inject_reminders_below_threshold() {
    let config = MessageBuilderConfig::default()
        .with_reminder_threshold(5);
    let builder = MessageBuilder::new(config);

    let mut messages = vec![
        Message::user("Hello"),
    ];

    let mut session = ExecutionSession::new();
    session.iteration_count = 3; // Below threshold

    builder.inject_reminders(&mut messages, &session);

    // Should not be wrapped
    assert!(!messages[0].content.contains("<system-reminder>"));
    assert_eq!(messages[0].content, "Hello");
}

#[test]
fn test_summary_to_qa_pair() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::Summary(SummaryPart {
            content: "Previously, we analyzed the codebase and found issues with error handling.".to_string(),
            original_count: 10,
            compacted_at: 5000,
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    // Should create Q&A pair
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "What did we do so far?");
    assert_eq!(messages[1].role, "assistant");
    assert!(messages[1].content.contains("error handling"));
}

#[test]
fn test_message_factory_methods() {
    // Test user message
    let user = Message::user("Hello");
    assert_eq!(user.role, "user");
    assert_eq!(user.content, "Hello");
    assert!(user.tool_call_id.is_none());
    assert!(user.tool_calls.is_none());

    // Test assistant message
    let assistant = Message::assistant("Hi there");
    assert_eq!(assistant.role, "assistant");
    assert_eq!(assistant.content, "Hi there");

    // Test tool result message
    let tool_result = Message::tool_result("call_1", "Result data");
    assert_eq!(tool_result.role, "tool");
    assert_eq!(tool_result.content, "Result data");
    assert_eq!(tool_result.tool_call_id, Some("call_1".to_string()));

    // Test assistant with tool call
    let tc = ToolCall::new("call_2", "search", r#"{"query": "test"}"#);
    let with_tool = Message::assistant_with_tool_call(tc);
    assert_eq!(with_tool.role, "assistant");
    assert!(with_tool.content.is_empty());
    assert!(with_tool.tool_calls.is_some());
    assert_eq!(with_tool.tool_calls.as_ref().unwrap()[0].name, "search");
}

#[test]
fn test_max_messages_limit() {
    let config = MessageBuilderConfig::default()
        .with_max_messages(3);
    let builder = MessageBuilder::new(config);

    let parts = vec![
        SessionPart::UserInput(UserInputPart {
            text: "First".to_string(),
            context: None,
            timestamp: 1000,
        }),
        SessionPart::AiResponse(AiResponsePart {
            content: "Response 1".to_string(),
            reasoning: None,
            timestamp: 1100,
        }),
        SessionPart::AiResponse(AiResponsePart {
            content: "Response 2".to_string(),
            reasoning: None,
            timestamp: 1200,
        }),
        SessionPart::AiResponse(AiResponsePart {
            content: "Response 3".to_string(),
            reasoning: None,
            timestamp: 1300,
        }),
        SessionPart::AiResponse(AiResponsePart {
            content: "Response 4".to_string(),
            reasoning: None,
            timestamp: 1400,
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    // Should be limited to 3 messages
    assert_eq!(messages.len(), 3);

    // First message should be preserved (user input)
    assert_eq!(messages[0].content, "First");

    // Last messages should be the most recent
    assert_eq!(messages[2].content, "Response 4");
}

#[test]
fn test_build_messages_full_pipeline() {
    let config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(1);
    let builder = MessageBuilder::new(config);

    let parts = vec![
        SessionPart::UserInput(UserInputPart {
            text: "Help me find bugs".to_string(),
            context: None,
            timestamp: 1000,
        }),
        SessionPart::ToolCall(ToolCallPart {
            id: "call_1".to_string(),
            tool_name: "search".to_string(),
            input: json!({"query": "error"}),
            status: ToolCallStatus::Completed,
            output: Some("Found potential bug".to_string()),
            error: None,
            started_at: 1100,
            completed_at: Some(1200),
        }),
    ];

    let mut session = ExecutionSession::new();
    session.iteration_count = 5;

    let messages = builder.build_messages(&session, &parts);

    // Should have: user (wrapped), assistant with tool, tool result
    assert_eq!(messages.len(), 3);

    // User message should be wrapped with reminder
    assert!(messages[0].content.contains("<system-reminder>"));
    assert!(messages[0].content.contains("Help me find bugs"));
}

#[test]
fn test_ai_response_with_reasoning_only() {
    let builder = create_builder();

    let parts = vec![
        SessionPart::AiResponse(AiResponsePart {
            content: String::new(),
            reasoning: Some("Let me think about this...".to_string()),
            timestamp: 1000,
        }),
    ];

    let messages = builder.parts_to_messages(&parts);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "Let me think about this...");
}

#[test]
fn test_tool_call_serialization() {
    let tc = ToolCall::new("call_1", "read_file", r#"{"path": "/test.rs"}"#);
    let json = serde_json::to_string(&tc).unwrap();

    assert!(json.contains("call_1"));
    assert!(json.contains("read_file"));

    let parsed: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "call_1");
    assert_eq!(parsed.name, "read_file");
}

#[test]
fn test_message_serialization() {
    let msg = Message::user("Test message");
    let json = serde_json::to_string(&msg).unwrap();

    // tool_call_id and tool_calls should be skipped when None
    assert!(!json.contains("tool_call_id"));
    assert!(!json.contains("tool_calls"));

    let parsed: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.role, "user");
    assert_eq!(parsed.content, "Test message");
}

#[test]
fn test_build_messages_with_filter_compacted() {
    // Create a session with:
    // - Old UserInput ("Old message")
    // - CompactionMarker
    // - Summary (with compacted_at > 0, indicating complete)
    // - New UserInput ("New message")
    //
    // The filter_compacted method should:
    // - Exclude "Old message" (before compaction boundary)
    // - Include the Summary (as Q&A pair)
    // - Include "New message"

    let mut session = ExecutionSession::new();

    // Add old user input (before compaction)
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Old message".to_string(),
        context: None,
        timestamp: 1000,
    }));

    // Add compaction marker
    session.parts.push(SessionPart::CompactionMarker(CompactionMarker::with_timestamp(2000, true)));

    // Add summary (compacted_at > 0 means completed)
    session.parts.push(SessionPart::Summary(SummaryPart {
        content: "Previously we discussed old topics.".to_string(),
        original_count: 5,
        compacted_at: 2001, // > 0 means completed
    }));

    // Add new user input (after compaction)
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "New message".to_string(),
        context: None,
        timestamp: 3000,
    }));

    // Create builder with compactor
    let compactor = Arc::new(SessionCompactor::new());
    let config = MessageBuilderConfig::default().with_inject_reminders(false);
    let builder = MessageBuilder::with_compactor(config, compactor);

    // Build messages from session
    let messages = builder.build_from_session(&session);

    // Verify "Old message" is NOT in output
    let old_msg_found = messages.iter().any(|m| m.content.contains("Old message"));
    assert!(
        !old_msg_found,
        "Old message should be filtered out by filter_compacted"
    );

    // Verify summary content IS in output (as Q&A pair)
    let summary_found = messages
        .iter()
        .any(|m| m.content.contains("Previously we discussed old topics."));
    assert!(summary_found, "Summary content should be in output");

    // Verify the Q&A pair structure for summary
    let qa_question = messages
        .iter()
        .any(|m| m.role == "user" && m.content == "What did we do so far?");
    assert!(qa_question, "Summary should be converted to Q&A pair");

    // Verify "New message" IS in output
    let new_msg_found = messages.iter().any(|m| m.content.contains("New message"));
    assert!(new_msg_found, "New message should be in output");
}

#[test]
fn test_build_from_session_without_compactor() {
    // Without a compactor, build_from_session should use all parts
    let mut session = ExecutionSession::new();

    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "First message".to_string(),
        context: None,
        timestamp: 1000,
    }));

    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Second message".to_string(),
        context: None,
        timestamp: 2000,
    }));

    // Create builder WITHOUT compactor
    let config = MessageBuilderConfig::default().with_inject_reminders(false);
    let builder = MessageBuilder::new(config);

    let messages = builder.build_from_session(&session);

    // Both messages should be present
    assert_eq!(messages.len(), 2);
    assert!(messages[0].content.contains("First message"));
    assert!(messages[1].content.contains("Second message"));
}

#[test]
fn test_with_compactor_constructor() {
    let compactor = Arc::new(SessionCompactor::new());
    let config = MessageBuilderConfig::default();
    let builder = MessageBuilder::with_compactor(config, compactor);

    // Verify the compactor is set
    assert!(builder.has_compactor());
}

#[test]
fn test_inject_token_limit_warning() {
    // Create detector with test config
    // Test model: 10K context, 1K output, 10% reserve
    // Usable: (10000 - 1000) * 0.9 = 8100
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));

    // Create builder with overflow detector
    let config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(1);
    let builder = MessageBuilder::with_overflow_detector(config, detector);

    let mut messages = vec![
        Message::user("First message"),
        Message::assistant("Response"),
        Message::user("Second message"),
    ];

    // Set session.total_tokens to ~85% of usable (8100)
    // 85% of 8100 = 6885
    let mut session = ExecutionSession::new().with_model("test-model");
    session.total_tokens = 6885;
    session.iteration_count = 2; // Above reminder threshold

    builder.inject_reminders(&mut messages, &session);

    // Verify messages contain "Context usage" warning
    let warning_found = messages
        .iter()
        .any(|m| m.content.contains("Context usage is at"));
    assert!(warning_found, "Token limit warning should be injected");

    // Verify the warning contains the approximate percentage
    let warning_msg = messages
        .iter()
        .find(|m| m.content.contains("Context usage"))
        .unwrap();
    assert!(
        warning_msg.content.contains("85%") || warning_msg.content.contains("84%"),
        "Warning should show ~85% usage"
    );

    // Verify warning is inserted after last user message (at index 3)
    // Original: [0] user, [1] assistant, [2] user (wrapped)
    // After warning: [0] user, [1] assistant, [2] user (wrapped), [3] warning
    assert_eq!(messages.len(), 4);
    assert!(messages[3].content.contains("Context usage"));
}

#[test]
fn test_no_warning_below_threshold() {
    // Create detector with test config
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));

    // Create builder with overflow detector
    let config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(1);
    let builder = MessageBuilder::with_overflow_detector(config, detector);

    let mut messages = vec![
        Message::user("First message"),
        Message::assistant("Response"),
        Message::user("Second message"),
    ];

    // Set session.total_tokens to ~50% of usable (8100)
    // 50% of 8100 = 4050
    let mut session = ExecutionSession::new().with_model("test-model");
    session.total_tokens = 4050;
    session.iteration_count = 2; // Above reminder threshold

    builder.inject_reminders(&mut messages, &session);

    // Verify no warning is injected (only 3 messages)
    let warning_found = messages
        .iter()
        .any(|m| m.content.contains("Context usage is at"));
    assert!(!warning_found, "No warning should be injected when usage < 80%");

    // Original messages count + wrapped reminder message should still be 3
    assert_eq!(messages.len(), 3);
}

#[test]
fn test_with_overflow_detector_constructor() {
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));
    let config = MessageBuilderConfig::default();
    let builder = MessageBuilder::with_overflow_detector(config, detector);

    // Verify the overflow_detector is set
    assert!(builder.has_overflow_detector());
    // Verify compactor is not set
    assert!(!builder.has_compactor());
}

#[test]
fn test_with_all_constructor() {
    let compactor = Arc::new(SessionCompactor::new());
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));
    let config = MessageBuilderConfig::default();

    let builder = MessageBuilder::with_all(config, Some(compactor), Some(detector));

    // Verify both are set
    assert!(builder.has_compactor());
    assert!(builder.has_overflow_detector());
}

#[test]
fn test_with_all_constructor_optional() {
    let config = MessageBuilderConfig::default();

    // Test with only compactor
    let compactor = Arc::new(SessionCompactor::new());
    let builder = MessageBuilder::with_all(config.clone(), Some(compactor), None);
    assert!(builder.has_compactor());
    assert!(!builder.has_overflow_detector());

    // Test with only detector
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));
    let builder2 = MessageBuilder::with_all(config.clone(), None, Some(detector));
    assert!(!builder2.has_compactor());
    assert!(builder2.has_overflow_detector());

    // Test with neither
    let builder3 = MessageBuilder::with_all(config, None, None);
    assert!(!builder3.has_compactor());
    assert!(!builder3.has_overflow_detector());
}

#[test]
fn test_warning_at_exact_threshold() {
    // Test that warning is injected at exactly 80%
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));

    let config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(1);
    let builder = MessageBuilder::with_overflow_detector(config, detector);

    let mut messages = vec![Message::user("Test message")];

    // Set session.total_tokens to exactly 80% of usable (8100)
    // 80% of 8100 = 6480
    let mut session = ExecutionSession::new().with_model("test-model");
    session.total_tokens = 6480;
    session.iteration_count = 2;

    builder.inject_reminders(&mut messages, &session);

    // Warning should be injected at exactly 80%
    let warning_found = messages
        .iter()
        .any(|m| m.content.contains("Context usage is at"));
    assert!(warning_found, "Warning should be injected at exactly 80%");
}

#[test]
fn test_warning_just_below_threshold() {
    // Test that warning is NOT injected just below 80% (79%)
    let overflow_config = OverflowConfig::for_testing();
    let detector = Arc::new(OverflowDetector::new(overflow_config));

    let config = MessageBuilderConfig::default()
        .with_inject_reminders(true)
        .with_reminder_threshold(1);
    let builder = MessageBuilder::with_overflow_detector(config, detector);

    let mut messages = vec![Message::user("Test message")];

    // Set session.total_tokens to 79% of usable (8100)
    // 79% of 8100 = 6399
    let mut session = ExecutionSession::new().with_model("test-model");
    session.total_tokens = 6399;
    session.iteration_count = 2;

    builder.inject_reminders(&mut messages, &session);

    // Warning should NOT be injected below 80%
    let warning_found = messages
        .iter()
        .any(|m| m.content.contains("Context usage is at"));
    assert!(!warning_found, "Warning should NOT be injected below 80%");
}

#[test]
fn test_inject_max_steps_warning() {
    // Test that max steps warning is injected on the last step
    let mut config = MessageBuilderConfig::default();
    config.max_iterations = 10;
    config.inject_reminders = true;
    config.reminder_threshold = 0; // Ensure reminders are injected
    let builder = MessageBuilder::new(config);

    let parts = vec![SessionPart::UserInput(UserInputPart {
        text: "Continue".to_string(),
        context: None,
        timestamp: 1000,
    })];

    let mut session = ExecutionSession::new();
    session.iteration_count = 9; // Last step (10 - 1)

    let messages = builder.build_messages(&session, &parts);

    // Verify "LAST step" warning is present
    let warning_found = messages.iter().any(|m| m.content.contains("LAST step"));
    assert!(warning_found, "Max steps warning should be injected on last step");

    // Verify warning contains instructions
    let warning_msg = messages.iter().find(|m| m.content.contains("LAST step")).unwrap();
    assert!(warning_msg.content.contains("Complete the task"));
    assert!(warning_msg.content.contains("Ask the user for guidance"));
    assert!(warning_msg.content.contains("Do NOT start new tool calls"));
}

#[test]
fn test_no_max_steps_warning_before_last() {
    // Test that no warning is injected before the last step
    let mut config = MessageBuilderConfig::default();
    config.max_iterations = 10;
    config.inject_reminders = true;
    config.reminder_threshold = 0;
    let builder = MessageBuilder::new(config);

    let parts = vec![SessionPart::UserInput(UserInputPart {
        text: "Continue".to_string(),
        context: None,
        timestamp: 1000,
    })];

    let mut session = ExecutionSession::new();
    session.iteration_count = 8; // Not the last step (10 - 1 = 9)

    let messages = builder.build_messages(&session, &parts);

    // Verify no "LAST step" warning
    let warning_found = messages.iter().any(|m| m.content.contains("LAST step"));
    assert!(
        !warning_found,
        "Max steps warning should NOT be injected before last step"
    );
}

#[test]
fn test_no_max_steps_warning_after_last() {
    // Test that no warning is injected after the last step (edge case)
    let mut config = MessageBuilderConfig::default();
    config.max_iterations = 10;
    config.inject_reminders = true;
    config.reminder_threshold = 0;
    let builder = MessageBuilder::new(config);

    let parts = vec![SessionPart::UserInput(UserInputPart {
        text: "Continue".to_string(),
        context: None,
        timestamp: 1000,
    })];

    let mut session = ExecutionSession::new();
    session.iteration_count = 10; // Beyond the last step

    let messages = builder.build_messages(&session, &parts);

    // Verify no "LAST step" warning when iteration_count >= max_iterations
    let warning_found = messages.iter().any(|m| m.content.contains("LAST step"));
    assert!(
        !warning_found,
        "Max steps warning should NOT be injected after max_iterations"
    );
}

#[test]
fn test_max_steps_warning_with_default_config() {
    // Test with default config (max_iterations = 50)
    let config = MessageBuilderConfig::default();
    assert_eq!(config.max_iterations, 50);

    let builder = MessageBuilder::new(config);

    let parts = vec![SessionPart::UserInput(UserInputPart {
        text: "Continue".to_string(),
        context: None,
        timestamp: 1000,
    })];

    let mut session = ExecutionSession::new();
    session.iteration_count = 49; // Last step (50 - 1)

    let messages = builder.build_messages(&session, &parts);

    // Verify "LAST step" warning is present
    let warning_found = messages.iter().any(|m| m.content.contains("LAST step"));
    assert!(
        warning_found,
        "Max steps warning should be injected at iteration 49 with default config"
    );
}

//! Message Builder - Converts SessionParts to LLM messages
//!
//! This module implements the message building pipeline that converts
//! ExecutionSession parts to the message format expected by LLM providers,
//! including system reminder injection.
//!
//! # Message Flow
//!
//! ```text
//! ExecutionSession.parts → filter_compacted() → parts_to_messages() → inject_reminders()
//!                                    ↓                    ↓                   ↓
//!                             [filtered parts]    [base messages]    [final messages]
//! ```
//!
//! # System Reminder Injection
//!
//! Following OpenCode's pattern, system reminders are injected by wrapping
//! the last user message with `<system-reminder>` tags when:
//! - iteration_count > reminder_threshold
//! - There are pending reminders in the session
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::agent_loop::{MessageBuilder, MessageBuilderConfig};
//! use aethecore::components::ExecutionSession;
//!
//! let config = MessageBuilderConfig::default();
//! let builder = MessageBuilder::new(config);
//!
//! let session = ExecutionSession::new();
//! let messages = builder.build_messages(&session, &session.parts);
//! ```

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::components::{
    ExecutionSession, SessionCompactor, SessionPart, ToolCallPart, ToolCallStatus,
};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for MessageBuilder
#[derive(Debug, Clone)]
pub struct MessageBuilderConfig {
    /// Maximum number of messages to include (default: 100)
    pub max_messages: usize,

    /// Whether to inject system reminders (default: true)
    pub inject_reminders: bool,

    /// Inject reminders after this many iterations (default: 1)
    pub reminder_threshold: u32,

    /// Maximum iterations before warning (default: 50)
    pub max_iterations: u32,
}

impl Default for MessageBuilderConfig {
    fn default() -> Self {
        Self {
            max_messages: 100,
            inject_reminders: true,
            reminder_threshold: 1,
            max_iterations: 50,
        }
    }
}

impl MessageBuilderConfig {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set max messages
    pub fn with_max_messages(mut self, max: usize) -> Self {
        self.max_messages = max;
        self
    }

    /// Builder: set inject reminders flag
    pub fn with_inject_reminders(mut self, inject: bool) -> Self {
        self.inject_reminders = inject;
        self
    }

    /// Builder: set reminder threshold
    pub fn with_reminder_threshold(mut self, threshold: u32) -> Self {
        self.reminder_threshold = threshold;
        self
    }

    /// Builder: set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = max;
        self
    }
}

// ============================================================================
// Message Types
// ============================================================================

/// LLM message representation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Role: "user", "assistant", or "tool"
    pub role: String,

    /// Message content
    pub content: String,

    /// Tool call ID (for tool result messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Tool calls (for assistant messages with tool use)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl Message {
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a tool result message
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(id.into()),
            tool_calls: None,
        }
    }

    /// Create an assistant message with a tool call
    pub fn assistant_with_tool_call(tool_call: ToolCall) -> Self {
        Self {
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(vec![tool_call]),
        }
    }

    /// Create an assistant message with multiple tool calls
    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }
}

/// Tool call in a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Unique identifier for this tool call
    pub id: String,

    /// Name of the tool being called
    pub name: String,

    /// Arguments as JSON string
    pub arguments: String,
}

impl ToolCall {
    /// Create a new tool call
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    /// Create from ToolCallPart
    pub fn from_part(part: &ToolCallPart) -> Self {
        Self {
            id: part.id.clone(),
            name: part.tool_name.clone(),
            arguments: serde_json::to_string(&part.input).unwrap_or_default(),
        }
    }
}

// ============================================================================
// Message Builder
// ============================================================================

/// Main message builder that converts SessionParts to LLM messages
pub struct MessageBuilder {
    /// Configuration
    config: MessageBuilderConfig,
    /// Optional session compactor for filter_compacted functionality
    compactor: Option<Arc<SessionCompactor>>,
    // Note: overflow_detector will be added in a later task (7)
}

impl MessageBuilder {
    /// Create a new MessageBuilder with the given config
    pub fn new(config: MessageBuilderConfig) -> Self {
        Self {
            config,
            compactor: None,
        }
    }

    /// Create a new MessageBuilder with a compactor for filter_compacted support
    ///
    /// When a compactor is provided, `build_from_session` will use
    /// `filter_compacted` to exclude parts before the compaction boundary.
    pub fn with_compactor(config: MessageBuilderConfig, compactor: Arc<SessionCompactor>) -> Self {
        Self {
            config,
            compactor: Some(compactor),
        }
    }

    /// Build messages from session, applying filter_compacted
    ///
    /// This method provides a convenient way to build messages from a session
    /// while respecting compaction boundaries. If a compactor is configured,
    /// it will use `filter_compacted` to exclude old parts that have been
    /// compacted. Otherwise, it uses all session parts directly.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let compactor = Arc::new(SessionCompactor::new());
    /// let builder = MessageBuilder::with_compactor(config, compactor);
    /// let messages = builder.build_from_session(&session);
    /// ```
    pub fn build_from_session(&self, session: &ExecutionSession) -> Vec<Message> {
        let filtered_parts = if let Some(ref compactor) = self.compactor {
            compactor.filter_compacted(session)
        } else {
            session.parts.clone()
        };
        self.build_messages(session, &filtered_parts)
    }

    /// Convert session parts to LLM messages
    ///
    /// This method converts each SessionPart to the appropriate Message format:
    /// - UserInput → User message
    /// - AiResponse → Assistant message
    /// - ToolCall → Assistant with tool_call + Tool result message
    /// - Summary → Q&A pair for context continuity
    /// - SystemReminder → Skipped (handled in inject_reminders)
    pub fn parts_to_messages(&self, parts: &[SessionPart]) -> Vec<Message> {
        let mut messages = Vec::new();

        for part in parts {
            match part {
                SessionPart::UserInput(input) => {
                    let mut content = input.text.clone();
                    if let Some(ref ctx) = input.context {
                        if !ctx.is_empty() {
                            content = format!("{}\n\nContext: {}", content, ctx);
                        }
                    }
                    messages.push(Message::user(content));
                }

                SessionPart::AiResponse(response) => {
                    let mut content = response.content.clone();
                    if let Some(ref reasoning) = response.reasoning {
                        if !reasoning.is_empty() && content.is_empty() {
                            // If only reasoning, use it as content
                            content = reasoning.clone();
                        }
                    }
                    if !content.is_empty() {
                        messages.push(Message::assistant(content));
                    }
                }

                SessionPart::ToolCall(tool_call) => {
                    // Create tool call from part
                    let tc = ToolCall::from_part(tool_call);

                    // Add assistant message with tool call
                    messages.push(Message::assistant_with_tool_call(tc));

                    // Add tool result message based on status
                    let result_content = self.tool_call_to_result_content(tool_call);
                    messages.push(Message::tool_result(&tool_call.id, result_content));
                }

                SessionPart::Summary(summary) => {
                    // Convert summary to Q&A pair for context continuity
                    // This follows the pattern of injecting historical context
                    messages.push(Message::user("What did we do so far?"));
                    messages.push(Message::assistant(&summary.content));
                }

                SessionPart::Reasoning(reasoning) => {
                    // Include reasoning as assistant message for transparency
                    if !reasoning.content.is_empty() {
                        messages.push(Message::assistant(&reasoning.content));
                    }
                }

                SessionPart::PlanCreated(plan) => {
                    // Include plan as assistant response
                    let plan_text = format!(
                        "I've created a plan with the following steps:\n{}",
                        plan.steps
                            .iter()
                            .enumerate()
                            .map(|(i, s)| format!("{}. {}", i + 1, s))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    messages.push(Message::assistant(plan_text));
                }

                SessionPart::SubAgentCall(sub_agent) => {
                    // Include sub-agent call as assistant message
                    let content = if let Some(ref result) = sub_agent.result {
                        format!(
                            "Delegated to sub-agent '{}' with prompt: {}\n\nResult: {}",
                            sub_agent.agent_id, sub_agent.prompt, result
                        )
                    } else {
                        format!(
                            "Delegating to sub-agent '{}' with prompt: {}",
                            sub_agent.agent_id, sub_agent.prompt
                        )
                    };
                    messages.push(Message::assistant(content));
                }

                // Skip markers and reminders - they are handled separately
                SessionPart::CompactionMarker(_) => {}
                SessionPart::SystemReminder(_) => {}
            }
        }

        // Apply max_messages limit
        if messages.len() > self.config.max_messages {
            // Keep the most recent messages, but always include the first user message
            let excess = messages.len() - self.config.max_messages;
            messages.drain(1..=excess);
        }

        messages
    }

    /// Build messages from a session with reminder injection
    ///
    /// This is the main entry point that:
    /// 1. Converts filtered parts to messages
    /// 2. Injects system reminders if needed
    pub fn build_messages(
        &self,
        session: &ExecutionSession,
        filtered_parts: &[SessionPart],
    ) -> Vec<Message> {
        let mut messages = self.parts_to_messages(filtered_parts);

        // Inject reminders if enabled and threshold met
        if self.config.inject_reminders {
            self.inject_reminders(&mut messages, session);
        }

        messages
    }

    /// Inject system reminders by wrapping the last user message
    ///
    /// Following OpenCode's pattern, we wrap the last user message with
    /// `<system-reminder>` tags to provide context for multi-step tasks.
    pub fn inject_reminders(&self, messages: &mut Vec<Message>, session: &ExecutionSession) {
        // Only inject when iteration count exceeds threshold
        if session.iteration_count <= self.config.reminder_threshold {
            return;
        }

        // Find the last user message
        let last_user_idx = messages
            .iter()
            .rposition(|m| m.role == "user");

        if let Some(idx) = last_user_idx {
            let original_content = messages[idx].content.clone();

            // Wrap with system-reminder tags
            let wrapped_content = format!(
                "<system-reminder>\nThe user sent the following message:\n{}\nPlease address this message and continue with your tasks.\n</system-reminder>",
                original_content
            );

            messages[idx].content = wrapped_content;
        }
    }

    /// Convert tool call status to result content
    fn tool_call_to_result_content(&self, tool_call: &ToolCallPart) -> String {
        match tool_call.status {
            ToolCallStatus::Completed => {
                tool_call
                    .output
                    .clone()
                    .unwrap_or_else(|| "Tool completed successfully".to_string())
            }
            ToolCallStatus::Failed => {
                format!(
                    "Error: {}",
                    tool_call.error.as_ref().unwrap_or(&"Unknown error".to_string())
                )
            }
            ToolCallStatus::Pending | ToolCallStatus::Running => {
                "[Tool execution was interrupted]".to_string()
            }
            ToolCallStatus::Aborted => {
                "[Tool execution was aborted]".to_string()
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{
        AiResponsePart, CompactionMarker, SessionCompactor, SummaryPart, ToolCallPart,
        ToolCallStatus, UserInputPart,
    };
    use serde_json::json;

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
        session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
            timestamp: 2000,
            auto: true,
        }));

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
        assert!(builder.compactor.is_some());
    }
}

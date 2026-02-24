//! Message Builder test context for BDD scenarios
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::agent_loop::message_builder::{Message, MessageBuilder, MessageBuilderConfig, ToolCall};
use alephcore::agent_loop::overflow::{ModelLimit, OverflowConfig, OverflowDetector};
use alephcore::components::{
    AiResponsePart, CompactionMarker, ExecutionSession, SessionCompactor, SessionPart,
    SummaryPart, ToolCallPart, ToolCallStatus, UserInputPart,
};
use serde_json::Value;

/// Message Builder test context
/// Stores state for MessageBuilder BDD scenarios
#[derive(Default)]
pub struct MessageBuilderContext {
    // ═══ Configuration ═══
    /// Message builder configuration
    pub config: MessageBuilderConfig,

    // ═══ Builder State ═══
    /// MessageBuilder instance
    pub builder: Option<MessageBuilder>,
    /// Session parts for building messages
    pub parts: Vec<SessionPart>,
    /// Execution session for reminders and context
    pub session: ExecutionSession,

    // ═══ Build Results ═══
    /// Built messages
    pub messages: Vec<Message>,
    /// Serialization results (for JSON round-trip tests)
    pub serialized_json: Option<String>,
    pub deserialized_message: Option<Message>,
    pub deserialized_tool_call: Option<ToolCall>,

    // ═══ Components ═══
    /// Session compactor (optional)
    pub compactor: Option<Arc<SessionCompactor>>,
    /// Overflow detector (optional)
    pub overflow_detector: Option<Arc<OverflowDetector>>,
}

impl std::fmt::Debug for MessageBuilderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageBuilderContext")
            .field("config", &"<MessageBuilderConfig>")
            .field("builder", &self.builder.as_ref().map(|_| "<MessageBuilder>"))
            .field("parts_count", &self.parts.len())
            .field("messages_count", &self.messages.len())
            .field("session_iteration", &self.session.iteration_count)
            .field("has_compactor", &self.compactor.is_some())
            .field("has_overflow_detector", &self.overflow_detector.is_some())
            .finish()
    }
}

impl MessageBuilderContext {
    /// Create a new context with default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize builder with current config and no optional components
    pub fn init_builder(&mut self) {
        self.builder = Some(MessageBuilder::new(self.config.clone()));
    }

    /// Initialize builder with compactor
    pub fn init_builder_with_compactor(&mut self) {
        let compactor = self.compactor.clone().unwrap_or_else(|| Arc::new(SessionCompactor::new()));
        self.compactor = Some(compactor.clone());
        self.builder = Some(MessageBuilder::with_compactor(self.config.clone(), compactor));
    }

    /// Initialize builder with overflow detector
    pub fn init_builder_with_overflow_detector(&mut self) {
        if self.overflow_detector.is_none() {
            self.setup_testing_overflow_detector();
        }
        self.builder = Some(MessageBuilder::with_overflow_detector(
            self.config.clone(),
            self.overflow_detector.clone().unwrap(),
        ));
    }

    /// Initialize builder with both compactor and overflow detector
    pub fn init_builder_with_all(&mut self) {
        self.builder = Some(MessageBuilder::with_all(
            self.config.clone(),
            self.compactor.clone(),
            self.overflow_detector.clone(),
        ));
    }

    /// Add a user input part
    pub fn add_user_input(&mut self, text: &str, context: Option<&str>, timestamp: i64) {
        self.parts.push(SessionPart::UserInput(UserInputPart {
            text: text.to_string(),
            context: context.map(|s| s.to_string()),
            timestamp,
        }));
    }

    /// Add an AI response part
    pub fn add_ai_response(&mut self, content: &str, reasoning: Option<&str>, timestamp: i64) {
        self.parts.push(SessionPart::AiResponse(AiResponsePart {
            content: content.to_string(),
            reasoning: reasoning.map(|s| s.to_string()),
            timestamp,
        }));
    }

    /// Add a tool call part
    pub fn add_tool_call(
        &mut self,
        id: &str,
        tool_name: &str,
        input: Value,
        status: ToolCallStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) {
        self.parts.push(SessionPart::ToolCall(ToolCallPart {
            id: id.to_string(),
            tool_name: tool_name.to_string(),
            input,
            status,
            output: output.map(|s| s.to_string()),
            error: error.map(|s| s.to_string()),
            started_at: 1000,
            completed_at: Some(1500),
        }));
    }

    /// Add a summary part
    pub fn add_summary(&mut self, content: &str, original_count: u32, compacted_at: i64) {
        self.parts.push(SessionPart::Summary(SummaryPart {
            content: content.to_string(),
            original_count,
            compacted_at,
        }));
    }

    /// Add a compaction marker
    pub fn add_compaction_marker(&mut self, timestamp: i64, completed: bool) {
        self.parts.push(SessionPart::CompactionMarker(
            CompactionMarker::with_timestamp(timestamp, completed),
        ));
    }

    /// Convert parts to messages
    pub fn parts_to_messages(&mut self) {
        if let Some(builder) = &self.builder {
            self.messages = builder.parts_to_messages(&self.parts);
        }
    }

    /// Build messages from session with reminders
    pub fn build_messages(&mut self) {
        if let Some(builder) = &self.builder {
            self.messages = builder.build_messages(&self.session, &self.parts);
        }
    }

    /// Build messages from session (using build_from_session which applies filter_compacted)
    pub fn build_from_session(&mut self) {
        // Copy parts to session for this method
        self.session.parts = self.parts.clone();
        if let Some(builder) = &self.builder {
            self.messages = builder.build_from_session(&self.session);
        }
    }

    /// Inject reminders into messages
    pub fn inject_reminders(&mut self) {
        if let Some(builder) = &self.builder {
            builder.inject_reminders(&mut self.messages, &self.session);
        }
    }

    /// Get the number of messages
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Get message by index
    pub fn get_message(&self, index: usize) -> Option<&Message> {
        self.messages.get(index)
    }

    /// Find message containing text
    pub fn find_message_containing(&self, text: &str) -> Option<&Message> {
        self.messages.iter().find(|m| m.content.contains(text))
    }

    /// Check if any message contains text
    pub fn any_message_contains(&self, text: &str) -> bool {
        self.messages.iter().any(|m| m.content.contains(text))
    }

    /// Check if no message contains text
    pub fn no_message_contains(&self, text: &str) -> bool {
        !self.any_message_contains(text)
    }

    /// Get messages with specific role
    pub fn messages_with_role(&self, role: &str) -> Vec<&Message> {
        self.messages.iter().filter(|m| m.role == role).collect()
    }

    /// Serialize a message to JSON
    pub fn serialize_message(&mut self, msg: &Message) {
        self.serialized_json = serde_json::to_string(msg).ok();
    }

    /// Serialize a tool call to JSON
    pub fn serialize_tool_call(&mut self, tc: &ToolCall) {
        self.serialized_json = serde_json::to_string(tc).ok();
    }

    /// Deserialize JSON to message
    pub fn deserialize_message(&mut self) {
        if let Some(json) = &self.serialized_json {
            self.deserialized_message = serde_json::from_str(json).ok();
        }
    }

    /// Deserialize JSON to tool call
    pub fn deserialize_tool_call(&mut self) {
        if let Some(json) = &self.serialized_json {
            self.deserialized_tool_call = serde_json::from_str(json).ok();
        }
    }

    /// Create test OverflowConfig for testing
    /// Test model: 10K context, 1K output, 10% reserve
    /// Usable: (10000 - 1000) * 0.9 = 8100
    pub fn setup_testing_overflow_detector(&mut self) {
        let mut limits = HashMap::new();
        limits.insert(
            "test-model".to_string(),
            ModelLimit::new(10_000, 1_000, 0.1),
        );
        let config = OverflowConfig::new()
            .with_model_limit("test-model", ModelLimit::new(10_000, 1_000, 0.1))
            .with_default_limit(ModelLimit::new(10_000, 1_000, 0.1));
        self.overflow_detector = Some(Arc::new(OverflowDetector::new(config)));
    }

    /// Create session compactor
    pub fn setup_compactor(&mut self) {
        self.compactor = Some(Arc::new(SessionCompactor::new()));
    }
}

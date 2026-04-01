//! Conversation History for Agent Loop
//!
//! Manages the message history for multi-turn agent interactions.

use super::types::{ToolCallInfo, ToolCallResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// Message Role
// =============================================================================

/// Role of a message in the conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// System message (instructions)
    System,
    /// User message
    User,
    /// Assistant (AI) message
    Assistant,
    /// Tool result message
    Tool,
}

impl MessageRole {
    /// Get the role as a string for API calls
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

// =============================================================================
// Chat Message
// =============================================================================

/// A single message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role
    pub role: MessageRole,

    /// Message content (text)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Tool calls made by the assistant (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallInfo>>,

    /// Tool call ID (for tool result messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Name of the tool (for tool result messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create an assistant message with text content
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Create an assistant message with tool calls
    pub fn assistant_with_tool_calls(
        content: Option<String>,
        tool_calls: Vec<ToolCallInfo>,
    ) -> Self {
        Self {
            role: MessageRole::Assistant,
            content,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    /// Create a tool result message
    pub fn tool_result(result: &ToolCallResult) -> Self {
        let content = if result.success {
            result.content.clone()
        } else {
            format!(
                "Error: {}",
                result.error.as_deref().unwrap_or("Unknown error")
            )
        };

        Self {
            role: MessageRole::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id.clone()),
            name: Some(result.name.clone()),
        }
    }

    /// Check if this message has tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty())
    }

    /// Convert to JSON value for API calls (OpenAI format)
    pub fn to_openai_format(&self) -> Value {
        let mut msg = serde_json::json!({
            "role": self.role.as_str()
        });

        if let Some(content) = &self.content {
            msg["content"] = Value::String(content.clone());
        }

        if let Some(tool_calls) = &self.tool_calls {
            let calls: Vec<Value> = tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()
                        }
                    })
                })
                .collect();
            msg["tool_calls"] = Value::Array(calls);
        }

        if let Some(tool_call_id) = &self.tool_call_id {
            msg["tool_call_id"] = Value::String(tool_call_id.clone());
        }

        if self.role == MessageRole::Tool {
            if let Some(name) = &self.name {
                msg["name"] = Value::String(name.clone());
            }
        }

        msg
    }

    /// Convert to JSON value for API calls (Anthropic format)
    pub fn to_anthropic_format(&self) -> Value {
        match self.role {
            MessageRole::System => {
                // Anthropic handles system prompt separately
                serde_json::json!({
                    "role": "user",
                    "content": self.content.clone().unwrap_or_default()
                })
            }
            MessageRole::User => {
                serde_json::json!({
                    "role": "user",
                    "content": self.content.clone().unwrap_or_default()
                })
            }
            MessageRole::Assistant => {
                if let Some(tool_calls) = &self.tool_calls {
                    let tool_use: Vec<Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments
                            })
                        })
                        .collect();

                    let mut content: Vec<Value> = vec![];
                    if let Some(text) = &self.content {
                        if !text.is_empty() {
                            content.push(serde_json::json!({
                                "type": "text",
                                "text": text
                            }));
                        }
                    }
                    content.extend(tool_use);

                    serde_json::json!({
                        "role": "assistant",
                        "content": content
                    })
                } else {
                    serde_json::json!({
                        "role": "assistant",
                        "content": self.content.clone().unwrap_or_default()
                    })
                }
            }
            MessageRole::Tool => {
                serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": self.tool_call_id.clone().unwrap_or_default(),
                        "content": self.content.clone().unwrap_or_default()
                    }]
                })
            }
        }
    }
}

// =============================================================================
// Conversation History
// =============================================================================

/// Manages conversation history for the agent loop
#[derive(Debug, Clone, Default)]
pub struct ConversationHistory {
    /// System prompt (if any)
    system_prompt: Option<String>,

    /// List of messages in the conversation
    messages: Vec<ChatMessage>,
}

impl ConversationHistory {
    /// Create a new empty conversation
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with a system prompt
    pub fn with_system_prompt(prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: Some(prompt.into()),
            messages: Vec::new(),
        }
    }

    /// Set the system prompt
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    /// Get the system prompt
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage::user(content));
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage::assistant(content));
    }

    /// Add an assistant message with tool calls
    pub fn add_assistant_with_tool_calls(
        &mut self,
        content: Option<String>,
        tool_calls: Vec<ToolCallInfo>,
    ) {
        self.messages
            .push(ChatMessage::assistant_with_tool_calls(content, tool_calls));
    }

    /// Add a tool result message
    pub fn add_tool_result(&mut self, result: &ToolCallResult) {
        self.messages.push(ChatMessage::tool_result(result));
    }

    /// Get all messages (excluding system prompt)
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Get the number of messages
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if conversation is empty
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get the last message
    pub fn last_message(&self) -> Option<&ChatMessage> {
        self.messages.last()
    }

    /// Check if the last message has pending tool calls
    pub fn has_pending_tool_calls(&self) -> bool {
        self.messages.last().is_some_and(|m| m.has_tool_calls())
    }

    /// Convert to OpenAI API format
    pub fn to_openai_messages(&self) -> Vec<Value> {
        let mut messages = Vec::new();

        // Add system prompt if present
        if let Some(prompt) = &self.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": prompt
            }));
        }

        // Add all messages
        for msg in &self.messages {
            messages.push(msg.to_openai_format());
        }

        messages
    }

    /// Convert to Anthropic API format
    pub fn to_anthropic_messages(&self) -> (Option<String>, Vec<Value>) {
        let messages: Vec<Value> = self
            .messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .map(|m| m.to_anthropic_format())
            .collect();

        (self.system_prompt.clone(), messages)
    }

    /// Clear all messages (keep system prompt)
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_role() {
        assert_eq!(MessageRole::System.as_str(), "system");
        assert_eq!(MessageRole::User.as_str(), "user");
        assert_eq!(MessageRole::Assistant.as_str(), "assistant");
        assert_eq!(MessageRole::Tool.as_str(), "tool");
    }

    #[test]
    fn test_chat_message_creation() {
        let system = ChatMessage::system("You are helpful");
        assert_eq!(system.role, MessageRole::System);
        assert_eq!(system.content, Some("You are helpful".to_string()));

        let user = ChatMessage::user("Hello");
        assert_eq!(user.role, MessageRole::User);

        let assistant = ChatMessage::assistant("Hi there!");
        assert_eq!(assistant.role, MessageRole::Assistant);
        assert!(!assistant.has_tool_calls());
    }

    #[test]
    fn test_chat_message_with_tool_calls() {
        let tool_call =
            ToolCallInfo::new("call_123", "search", serde_json::json!({"query": "test"}));

        let msg = ChatMessage::assistant_with_tool_calls(None, vec![tool_call]);

        assert!(msg.has_tool_calls());
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_tool_result_message() {
        let result = ToolCallResult::success("call_123", "search", "Found results", 100);
        let msg = ChatMessage::tool_result(&result);

        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
        assert_eq!(msg.name, Some("search".to_string()));
    }

    #[test]
    fn test_conversation_history() {
        let mut history = ConversationHistory::with_system_prompt("Be helpful");

        assert_eq!(history.system_prompt(), Some("Be helpful"));
        assert!(history.is_empty());

        history.add_user_message("Hello");
        history.add_assistant_message("Hi!");

        assert_eq!(history.len(), 2);
        assert!(!history.is_empty());
    }

    #[test]
    fn test_conversation_to_openai_format() {
        let mut history = ConversationHistory::with_system_prompt("You are a helper");
        history.add_user_message("Hello");
        history.add_assistant_message("Hi!");

        let messages = history.to_openai_messages();

        assert_eq!(messages.len(), 3); // system + user + assistant
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
    }

    #[test]
    fn test_conversation_to_anthropic_format() {
        let mut history = ConversationHistory::with_system_prompt("You are a helper");
        history.add_user_message("Hello");
        history.add_assistant_message("Hi!");

        let (system, messages) = history.to_anthropic_messages();

        assert_eq!(system, Some("You are a helper".to_string()));
        assert_eq!(messages.len(), 2); // user + assistant (no system in messages)
    }

    #[test]
    fn test_openai_format_with_tool_calls() {
        let tool_call = ToolCallInfo::new(
            "call_abc",
            "search",
            serde_json::json!({"query": "weather"}),
        );

        let msg = ChatMessage::assistant_with_tool_calls(
            Some("Let me search".to_string()),
            vec![tool_call],
        );
        let json = msg.to_openai_format();

        assert_eq!(json["role"], "assistant");
        assert!(json["tool_calls"].is_array());
        assert_eq!(json["tool_calls"][0]["function"]["name"], "search");
    }

    #[test]
    fn test_has_pending_tool_calls() {
        let mut history = ConversationHistory::new();
        history.add_user_message("Search for weather");

        assert!(!history.has_pending_tool_calls());

        let tool_call = ToolCallInfo::new("call_1", "search", serde_json::json!({}));
        history.add_assistant_with_tool_calls(None, vec![tool_call]);

        assert!(history.has_pending_tool_calls());

        let result = ToolCallResult::success("call_1", "search", "Results", 100);
        history.add_tool_result(&result);

        assert!(!history.has_pending_tool_calls());
    }
}

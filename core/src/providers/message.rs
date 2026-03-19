//! Unified message types for provider-agnostic conversation representation.
//!
//! These types are the single data model for all provider interactions.
//! Protocol adapters convert these to their native API formats.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified message type — the single data model for all provider interactions.
///
/// Modeled after pi-mono's `Message = UserMessage | AssistantMessage | ToolResultMessage`.
/// Each protocol adapter converts these to its native format in `convert_messages()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UnifiedMessage {
    /// User message
    User { content: Vec<ContentBlock> },
    /// Assistant message (one turn may contain text + thinking + tool calls)
    Assistant { content: Vec<ContentBlock> },
    /// Tool execution result
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
}

/// Content block — one atomic unit within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text
    Text { text: String },
    /// Structured JSON (preserves tool output structure)
    Json { value: Value },
    /// Thinking/reasoning trace
    Thinking { thinking: String },
    /// Tool call (only in Assistant messages)
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
    },
    /// Image (base64-encoded)
    Image { data: String, mime_type: String },
}

// === Convenience constructors ===

impl UnifiedMessage {
    /// Single text user message
    pub fn user(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![ContentBlock::Text {
                text: text.into(),
            }],
        }
    }

    /// Single text assistant message
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::Assistant {
            content: vec![ContentBlock::Text {
                text: text.into(),
            }],
        }
    }

    /// Tool result with text output
    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: name.into(),
            content: vec![ContentBlock::Text {
                text: output.into(),
            }],
            is_error,
        }
    }

    /// Tool result with structured JSON output
    pub fn tool_result_json(
        call_id: impl Into<String>,
        name: impl Into<String>,
        value: Value,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: call_id.into(),
            tool_name: name.into(),
            content: vec![ContentBlock::Json { value }],
            is_error,
        }
    }

    /// Build an Assistant message from a ProviderResponse
    pub fn from_provider_response(resp: &super::adapter::ProviderResponse) -> Self {
        let mut content = Vec::new();
        if let Some(ref thinking) = resp.thinking {
            content.push(ContentBlock::Thinking {
                thinking: thinking.clone(),
            });
        }
        if let Some(ref text) = resp.text {
            content.push(ContentBlock::Text { text: text.clone() });
        }
        for tc in &resp.tool_calls {
            content.push(ContentBlock::ToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            });
        }
        UnifiedMessage::Assistant { content }
    }

    /// Get mutable access to content blocks (for PII filtering)
    pub fn content_blocks_mut(&mut self) -> &mut Vec<ContentBlock> {
        match self {
            Self::User { content } => content,
            Self::Assistant { content } => content,
            Self::ToolResult { content, .. } => content,
        }
    }

    /// Get read access to content blocks
    pub fn content_blocks(&self) -> &[ContentBlock] {
        match self {
            Self::User { content } => content,
            Self::Assistant { content } => content,
            Self::ToolResult { content, .. } => content,
        }
    }

    /// Extract concatenated text from a slice of messages (for leak detection)
    pub fn extract_all_text(messages: &[UnifiedMessage]) -> String {
        let mut parts = Vec::new();
        for msg in messages {
            for block in msg.content_blocks() {
                if let ContentBlock::Text { text } = block {
                    parts.push(text.as_str());
                }
            }
        }
        parts.join("\n")
    }

    /// Extract all text content from a message as a single concatenated string.
    ///
    /// Covers Text blocks and Json (serialized). Used for token estimation.
    pub fn text_content(&self) -> String {
        let mut parts = Vec::new();
        for block in self.content_blocks() {
            match block {
                ContentBlock::Text { text } => parts.push(text.as_str().to_owned()),
                ContentBlock::Thinking { thinking } => parts.push(thinking.as_str().to_owned()),
                ContentBlock::Json { value } => parts.push(value.to_string()),
                ContentBlock::ToolCall { name, arguments, .. } => {
                    parts.push(format!("{} {}", name, arguments));
                }
                ContentBlock::Image { .. } => {}
            }
        }
        parts.join(" ")
    }

    /// Returns true if this is an Assistant message.
    pub fn is_assistant(&self) -> bool {
        matches!(self, Self::Assistant { .. })
    }

    /// Returns true if this is a User message.
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }

    /// Returns true if this is a ToolResult message.
    pub fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult { .. })
    }

    /// Extract (tool_name, content_text) from a ToolResult message.
    ///
    /// Returns `None` if this is not a ToolResult.
    pub fn tool_result_info(&self) -> Option<(&str, String)> {
        match self {
            Self::ToolResult { tool_name, content, .. } => {
                let text = content
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.as_str().to_owned(),
                        ContentBlock::Json { value } => value.to_string(),
                        _ => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                Some((tool_name.as_str(), text))
            }
            _ => None,
        }
    }

    /// Replace the content of a ToolResult message with a single text block.
    ///
    /// No-op if this is not a ToolResult.
    pub fn replace_tool_result_content(&mut self, new_content: String) {
        if let Self::ToolResult { content, .. } = self {
            *content = vec![ContentBlock::Text { text: new_content }];
        }
    }

    /// Check if this is a ToolCall-bearing Assistant message
    pub fn has_tool_calls(&self) -> bool {
        match self {
            Self::Assistant { content } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolCall { .. })),
            _ => false,
        }
    }

    /// Extract tool calls from an Assistant message
    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        match self {
            Self::Assistant { content } => content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some((id.as_str(), name.as_str(), arguments)),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        }
    }
}

impl ContentBlock {
    /// Extract text content if this is a Text block
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

// === Message pre-processing ===

/// Pre-process messages before sending to any provider.
///
/// 1. Repairs orphaned tool calls (Assistant ToolCall without matching ToolResult)
/// 2. Normalizes cross-model content (no-op for now, reserved for thinking signatures)
pub fn transform_messages(
    messages: &[UnifiedMessage],
    _target_provider: Option<&str>,
) -> Vec<UnifiedMessage> {
    let mut result = messages.to_vec();
    repair_orphaned_tool_calls(&mut result);
    // normalize_cross_model is a no-op for now
    result
}

/// Scan for Assistant ToolCall blocks without matching ToolResult.
/// Insert synthetic error ToolResult for each orphan.
fn repair_orphaned_tool_calls(messages: &mut Vec<UnifiedMessage>) {
    // Collect all tool_call_ids that have a matching ToolResult
    let answered_ids: std::collections::HashSet<&str> = messages
        .iter()
        .filter_map(|m| match m {
            UnifiedMessage::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();

    // Find orphaned tool calls (in Assistant messages, no matching ToolResult)
    let mut orphans: Vec<(String, String)> = Vec::new();
    for msg in messages.iter() {
        if let UnifiedMessage::Assistant { content } = msg {
            for block in content {
                if let ContentBlock::ToolCall { id, name, .. } = block {
                    if !answered_ids.contains(id.as_str()) {
                        orphans.push((id.clone(), name.clone()));
                    }
                }
            }
        }
    }

    // Insert synthetic error ToolResult for each orphan
    for (id, name) in orphans {
        messages.push(UnifiedMessage::tool_result(
            id,
            name,
            "No result provided — tool call was interrupted",
            true,
        ));
    }
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_user_convenience() {
        let msg = UnifiedMessage::user("hello");
        match &msg {
            UnifiedMessage::User { content } => {
                assert_eq!(content.len(), 1);
                assert_eq!(content[0].as_text(), Some("hello"));
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn test_assistant_convenience() {
        let msg = UnifiedMessage::assistant("response");
        match &msg {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content[0].as_text(), Some("response"));
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn test_tool_result_convenience() {
        let msg = UnifiedMessage::tool_result("call_1", "search", "found 3 results", false);
        match &msg {
            UnifiedMessage::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
                content,
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(tool_name, "search");
                assert!(!is_error);
                assert_eq!(content[0].as_text(), Some("found 3 results"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_tool_result_json() {
        let msg = UnifiedMessage::tool_result_json(
            "call_1",
            "search",
            json!({"results": [1, 2, 3]}),
            false,
        );
        match &msg {
            UnifiedMessage::ToolResult { content, .. } => {
                assert!(matches!(&content[0], ContentBlock::Json { .. }));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_from_provider_response() {
        use super::super::adapter::{NativeToolCall, ProviderResponse};
        let resp = ProviderResponse {
            text: Some("I'll search for that.".into()),
            tool_calls: vec![NativeToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: json!({"query": "rust"}),
            }],
            thinking: Some("Let me think...".into()),
            ..Default::default()
        };
        let msg = UnifiedMessage::from_provider_response(&resp);
        match &msg {
            UnifiedMessage::Assistant { content } => {
                assert_eq!(content.len(), 3); // thinking + text + tool_call
                assert!(matches!(&content[0], ContentBlock::Thinking { .. }));
                assert!(matches!(&content[1], ContentBlock::Text { .. }));
                assert!(matches!(&content[2], ContentBlock::ToolCall { .. }));
            }
            _ => panic!("expected Assistant"),
        }
    }

    #[test]
    fn test_extract_all_text() {
        let messages = vec![
            UnifiedMessage::user("hello"),
            UnifiedMessage::assistant("world"),
        ];
        let text = UnifiedMessage::extract_all_text(&messages);
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_has_tool_calls() {
        let msg = UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "searching".into(),
                },
                ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({}),
                },
            ],
        };
        assert!(msg.has_tool_calls());
        assert!(!UnifiedMessage::user("hello").has_tool_calls());
    }

    #[test]
    fn test_repair_orphaned_tool_calls_no_orphans() {
        let messages = vec![
            UnifiedMessage::user("search for rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            },
            UnifiedMessage::tool_result("c1", "search", "found", false),
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3); // no synthetic results added
    }

    #[test]
    fn test_repair_orphaned_tool_calls_with_orphan() {
        let messages = vec![
            UnifiedMessage::user("search for rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            },
            // Missing ToolResult for c1!
        ];
        let result = transform_messages(&messages, None);
        assert_eq!(result.len(), 3); // synthetic ToolResult added
        match &result[2] {
            UnifiedMessage::ToolResult {
                tool_call_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_call_id, "c1");
                assert!(is_error);
            }
            _ => panic!("expected synthetic ToolResult"),
        }
    }

    #[test]
    fn test_content_blocks_mut() {
        let mut msg = UnifiedMessage::user("original");
        for block in msg.content_blocks_mut() {
            if let ContentBlock::Text { ref mut text } = block {
                *text = "filtered".to_string();
            }
        }
        assert_eq!(msg.content_blocks()[0].as_text(), Some("filtered"));
    }
}

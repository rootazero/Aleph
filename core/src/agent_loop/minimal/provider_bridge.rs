//! AiProviderBridge — connects MinimalProvider to existing AiProvider implementations.
//!
//! The bridge converts between the minimal loop's local `ToolDefinition` (3 fields)
//! and the dispatcher's `ToolDefinition` (7 fields), and formats `LoopMessage`
//! history into a structured text input for the provider's `process_with_payload`.

use async_trait::async_trait;
use serde_json::Value;

use crate::dispatcher::ToolCategory;
use crate::dispatcher::ToolDefinition as DispatcherToolDefinition;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::loop_core::{LoopMessage, MinimalProvider};
use super::tool::ToolDefinition as MinimalToolDefinition;

/// Bridge from `MinimalProvider` to any `Arc<dyn AiProvider>`.
///
/// Translates LoopMessage conversation history into a single input string
/// and converts minimal ToolDefinitions into dispatcher ToolDefinitions
/// for the underlying provider's `process_with_payload` method.
pub struct AiProviderBridge {
    provider: Arc<dyn AiProvider>,
}

impl AiProviderBridge {
    /// Create a new bridge wrapping an existing AiProvider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Format LoopMessages into structured text for the provider input.
    ///
    /// Uses XML-like tags to preserve conversation structure:
    /// - `<user>` for user messages
    /// - `<assistant>` for assistant text
    /// - `<tool_use>` for tool call requests
    /// - `<tool_result>` / `<tool_error>` for tool execution results
    fn format_messages(messages: &[LoopMessage]) -> String {
        let mut parts = Vec::with_capacity(messages.len());

        for msg in messages {
            match msg {
                LoopMessage::User(text) => {
                    parts.push(format!("<user>{}</user>", text));
                }
                LoopMessage::Assistant(text) => {
                    parts.push(format!("<assistant>{}</assistant>", text));
                }
                LoopMessage::ToolUse { id, name, input } => {
                    let args = format_json_compact(input);
                    parts.push(format!(
                        "<tool_use id=\"{}\" name=\"{}\">{}</tool_use>",
                        id, name, args
                    ));
                }
                LoopMessage::ToolResult {
                    id,
                    output,
                    is_error,
                } => {
                    let content = format_json_compact(output);
                    if *is_error {
                        parts.push(format!(
                            "<tool_error id=\"{}\">{}</tool_error>",
                            id, content
                        ));
                    } else {
                        parts.push(format!(
                            "<tool_result id=\"{}\">{}</tool_result>",
                            id, content
                        ));
                    }
                }
            }
        }

        parts.join("\n")
    }

    /// Convert a minimal ToolDefinition to the dispatcher's ToolDefinition.
    fn convert_tool_def(minimal: &MinimalToolDefinition) -> DispatcherToolDefinition {
        DispatcherToolDefinition {
            name: minimal.name.clone(),
            description: minimal.description.clone(),
            parameters: minimal.parameters.clone(),
            requires_confirmation: false,
            category: ToolCategory::Builtin,
            llm_context: None,
            strict: false,
        }
    }
}

/// Format a JSON value compactly, falling back to Display for strings.
fn format_json_compact(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[async_trait]
impl MinimalProvider for AiProviderBridge {
    async fn call(
        &self,
        messages: &[LoopMessage],
        system_prompt: &str,
        tools: &[MinimalToolDefinition],
    ) -> anyhow::Result<ProviderResponse> {
        let input = Self::format_messages(messages);

        let dispatcher_tools: Vec<DispatcherToolDefinition> =
            tools.iter().map(Self::convert_tool_def).collect();

        let payload = RequestPayload {
            input: &input,
            system_prompt: Some(system_prompt),
            tools: if dispatcher_tools.is_empty() {
                None
            } else {
                Some(&dispatcher_tools)
            },
            ..Default::default()
        };

        self.provider
            .process_with_payload(payload)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_messages_user() {
        let messages = vec![LoopMessage::User("Hello, world!".to_string())];
        let result = AiProviderBridge::format_messages(&messages);
        assert_eq!(result, "<user>Hello, world!</user>");
    }

    #[test]
    fn test_format_messages_assistant() {
        let messages = vec![LoopMessage::Assistant("I can help.".to_string())];
        let result = AiProviderBridge::format_messages(&messages);
        assert_eq!(result, "<assistant>I can help.</assistant>");
    }

    #[test]
    fn test_format_messages_tool_use() {
        let messages = vec![LoopMessage::ToolUse {
            id: "call_1".to_string(),
            name: "search".to_string(),
            input: json!({"query": "rust"}),
        }];
        let result = AiProviderBridge::format_messages(&messages);
        assert!(result.contains("<tool_use id=\"call_1\" name=\"search\">"));
        assert!(result.contains("</tool_use>"));
        assert!(result.contains("\"query\""));
        assert!(result.contains("\"rust\""));
    }

    #[test]
    fn test_format_messages_tool_result_success() {
        let messages = vec![LoopMessage::ToolResult {
            id: "call_1".to_string(),
            output: json!({"found": 42}),
            is_error: false,
        }];
        let result = AiProviderBridge::format_messages(&messages);
        assert!(result.contains("<tool_result id=\"call_1\">"));
        assert!(result.contains("</tool_result>"));
        assert!(result.contains("42"));
    }

    #[test]
    fn test_format_messages_tool_result_error() {
        let messages = vec![LoopMessage::ToolResult {
            id: "call_2".to_string(),
            output: Value::String("permission denied".to_string()),
            is_error: true,
        }];
        let result = AiProviderBridge::format_messages(&messages);
        assert!(result.contains("<tool_error id=\"call_2\">"));
        assert!(result.contains("permission denied"));
        assert!(result.contains("</tool_error>"));
    }

    #[test]
    fn test_convert_tool_def() {
        let minimal = MinimalToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        };

        let converted = AiProviderBridge::convert_tool_def(&minimal);

        assert_eq!(converted.name, "search");
        assert_eq!(converted.description, "Search the web");
        assert_eq!(converted.parameters, minimal.parameters);
        assert!(!converted.requires_confirmation);
        assert_eq!(converted.category, ToolCategory::Builtin);
        assert!(converted.llm_context.is_none());
        assert!(!converted.strict);
    }

    #[test]
    fn test_format_messages_full_conversation() {
        let messages = vec![
            LoopMessage::User("Find Rust tutorials".to_string()),
            LoopMessage::ToolUse {
                id: "call_1".to_string(),
                name: "search".to_string(),
                input: json!({"query": "rust tutorials"}),
            },
            LoopMessage::ToolResult {
                id: "call_1".to_string(),
                output: json!({"results": ["tutorial1", "tutorial2"]}),
                is_error: false,
            },
            LoopMessage::Assistant("Here are some Rust tutorials.".to_string()),
        ];

        let result = AiProviderBridge::format_messages(&messages);

        // Verify ordering and structure
        let user_pos = result.find("<user>").unwrap();
        let tool_use_pos = result.find("<tool_use").unwrap();
        let tool_result_pos = result.find("<tool_result").unwrap();
        let assistant_pos = result.find("<assistant>").unwrap();

        assert!(user_pos < tool_use_pos);
        assert!(tool_use_pos < tool_result_pos);
        assert!(tool_result_pos < assistant_pos);

        // Verify content
        assert!(result.contains("Find Rust tutorials"));
        assert!(result.contains("rust tutorials"));
        assert!(result.contains("tutorial1"));
        assert!(result.contains("Here are some Rust tutorials."));
    }

    #[test]
    fn test_format_messages_empty() {
        let messages: Vec<LoopMessage> = vec![];
        let result = AiProviderBridge::format_messages(&messages);
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_json_compact_string() {
        let val = Value::String("hello".to_string());
        assert_eq!(format_json_compact(&val), "hello");
    }

    #[test]
    fn test_format_json_compact_object() {
        let val = json!({"key": "value"});
        let result = format_json_compact(&val);
        assert!(result.contains("key"));
        assert!(result.contains("value"));
    }
}

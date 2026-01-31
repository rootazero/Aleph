//! Stream event definitions.
//!
//! Events emitted during streaming response processing.

use serde::{Deserialize, Serialize};

/// Events emitted during streaming
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Assistant message started
    AssistantStart {
        message_index: u32,
    },

    /// Text content delta
    TextDelta {
        delta: String,
        accumulated: String,
    },

    /// Thinking content delta
    ThinkingDelta {
        delta: String,
        accumulated: String,
    },

    /// Thinking block completed
    ThinkingComplete {
        content: String,
    },

    /// Tool execution started
    ToolStart {
        tool_id: String,
        tool_name: String,
    },

    /// Tool execution completed
    ToolComplete {
        tool_id: String,
        result: serde_json::Value,
    },

    /// Block reply (for TTS/chunked output)
    BlockReply {
        text: String,
        is_final: bool,
    },

    /// Assistant message completed
    AssistantComplete {
        content: String,
        thinking: Option<String>,
        usage: Option<TokenUsage>,
    },

    /// Error occurred
    Error {
        message: String,
        recoverable: bool,
    },
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens consumed
    pub input_tokens: u32,

    /// Output tokens generated
    pub output_tokens: u32,

    /// Thinking tokens used (if available)
    pub thinking_tokens: Option<u32>,

    /// Tokens read from cache
    pub cache_read_tokens: Option<u32>,

    /// Tokens written to cache
    pub cache_creation_tokens: Option<u32>,
}

impl TokenUsage {
    /// Create a new TokenUsage with input and output tokens
    pub fn new(input: u32, output: u32) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        }
    }

    /// Total tokens (input + output)
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_serialize() {
        let event = StreamEvent::TextDelta {
            delta: "Hello".to_string(),
            accumulated: "Hello".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("text_delta"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_token_usage() {
        let usage = TokenUsage::new(100, 50);
        assert_eq!(usage.total(), 150);
    }
}

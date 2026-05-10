//! AnthropicProtocol implementation — construction and internal helpers.

use std::collections::{HashMap, VecDeque};

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{RequestPayload, StopReason, TokenUsage};
use crate::providers::anthropic::{
    AnthropicTool, ContentBlock, ImageSource, Message, MessageContent, MessagesRequest,
    SystemBlock, ThinkingBlock,
};
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::{Arc, RwLock};
use reqwest::Client;
use tracing::{debug, warn};

use super::{sanitize_anthropic_tool_name, AnthropicProtocol, ToolNameMap};
impl AnthropicProtocol {
    /// Create a new Anthropic protocol adapter
    pub fn new(client: Client) -> Self {
        Self {
            client,
            name_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build the endpoint URL
    pub(super) fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        // Normalize URL
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        format!("{}/v1/messages", base_url)
    }

    /// Convert UnifiedMessages to Anthropic Messages
    pub(super) fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Message> {
        let mut result = Vec::new();
        let mut i = 0;
        while i < messages.len() {
            match &messages[i] {
                UnifiedMessage::User { content } => {
                    let mut blocks = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                blocks.push(ContentBlock::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::Image { data, mime_type } => {
                                blocks.push(ContentBlock::Image {
                                    source: ImageSource {
                                        source_type: "base64".to_string(),
                                        media_type: mime_type.clone(),
                                        data: data.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    let image_count = blocks
                        .iter()
                        .filter(|b| matches!(b, ContentBlock::Image { .. }))
                        .count();
                    if image_count > 0 {
                        tracing::info!(
                            target: "multimodal",
                            probe = "P6_provider",
                            role = "user",
                            content_type = "multimodal",
                            image_count = image_count,
                            "Anthropic multimodal message converted"
                        );
                    }
                    // Anthropic API rejects messages with empty content (HTTP 400:
                    // "must not be empty"). Emit a single-space placeholder so historical
                    // empty-turn artifacts (e.g. tokens=0 streaming aborts) don't poison
                    // subsequent requests.
                    if blocks.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: " ".to_string(),
                        });
                    }
                    if blocks.len() == 1 {
                        if let ContentBlock::Text { text, .. } = &blocks[0] {
                            result.push(Message {
                                role: "user".to_string(),
                                content: MessageContent::Text {
                                    content: text.clone(),
                                },
                            });
                            i += 1;
                            continue;
                        }
                    }
                    result.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal { content: blocks },
                    });
                    i += 1;
                }
                UnifiedMessage::Assistant { content } => {
                    let mut blocks = Vec::new();
                    // Track the most recent signed thinking block so we can inject
                    // reasoning_content into the next ToolUse when thinking is enabled.
                    let mut pending_thinking: Option<String> = None;
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                // Skip empty / whitespace-only text blocks: Anthropic-compatible
                                // backends (e.g. Kimi for Coding) reject them with HTTP 400
                                // "must not be empty". The empty-blocks fallback below inserts
                                // a placeholder if the entire turn ends up empty.
                                if !text.trim().is_empty() {
                                    blocks.push(ContentBlock::Text { text: text.clone() });
                                }
                            }
                            crate::providers::message::ContentBlock::Thinking {
                                thinking,
                                signature: Some(sig),
                            } => {
                                // Replay the signed thinking block when we have its signature.
                                // Anthropic requires a verbatim replay (thinking + signature)
                                // whenever the assistant turn also carries tool_use blocks.
                                // Without a signature the API would reject the message, so
                                // drop unsigned thinking — providers that don't sign (Gemini,
                                // OpenAI) never produce it for an Anthropic-bound turn.
                                if !thinking.is_empty() {
                                    blocks.push(ContentBlock::Thinking {
                                        thinking: thinking.clone(),
                                        signature: sig.clone(),
                                    });
                                    // Remember this thinking for the next ToolCall
                                    pending_thinking = Some(thinking.clone());
                                }
                            }
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                // Sanitize tool_use_id for Anthropic
                                let sanitized_id: String = id
                                    .chars()
                                    .map(|c| {
                                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                            c
                                        } else {
                                            '_'
                                        }
                                    })
                                    .take(64)
                                    .collect();
                                // Anthropic API requires input to be a dictionary, never a string.
                                // When thinking is enabled and precedes a tool call, we must
                                // include reasoning_content in the tool_use input or the API
                                // rejects the request with:
                                //   "thinking is enabled but reasoning_content is missing
                                //    in assistant tool call message".
                                let mut input = if arguments.is_object() {
                                    arguments.clone()
                                } else {
                                    serde_json::json!({})
                                };
                                if let Some(ref reasoning) = pending_thinking {
                                    if let Some(obj) = input.as_object_mut() {
                                        obj.insert(
                                            "reasoning_content".to_string(),
                                            serde_json::Value::String(reasoning.clone()),
                                        );
                                    }
                                    // Keep pending_thinking set: Anthropic requires
                                    // reasoning_content in EVERY tool_use block that
                                    // follows a signed thinking block within the same
                                    // assistant message, not just the first one.
                                }
                                blocks.push(ContentBlock::ToolUse {
                                    id: sanitized_id,
                                    name: sanitize_anthropic_tool_name(name),
                                    input,
                                });
                            }
                            _ => {}
                        }
                    }
                    // Anthropic API rejects messages with empty content (HTTP 400:
                    // "must not be empty"). Emit a single-space placeholder so historical
                    // empty-turn artifacts (e.g. tokens=0 streaming aborts) don't poison
                    // subsequent requests.
                    if blocks.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: " ".to_string(),
                        });
                    }
                    result.push(Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Multimodal { content: blocks },
                    });
                    i += 1;
                }
                UnifiedMessage::ToolResult { .. } => {
                    // Collect consecutive ToolResults into one user message
                    let mut tool_blocks = Vec::new();
                    while i < messages.len() {
                        if let UnifiedMessage::ToolResult {
                            tool_call_id,
                            content,
                            is_error,
                            ..
                        } = &messages[i]
                        {
                            let output = content
                                .iter()
                                .map(|b| match b {
                                    crate::providers::message::ContentBlock::Text {
                                        text, ..
                                    } => text.clone(),
                                    crate::providers::message::ContentBlock::Json { value } => {
                                        serde_json::to_string(value).unwrap_or_default()
                                    }
                                    _ => String::new(),
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            // Sanitize tool_use_id
                            let sanitized_id: String = tool_call_id
                                .chars()
                                .map(|c| {
                                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                        c
                                    } else {
                                        '_'
                                    }
                                })
                                .take(64)
                                .collect();
                            tool_blocks.push(ContentBlock::ToolResult {
                                tool_use_id: sanitized_id,
                                content: output,
                                is_error: *is_error,
                            });
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    result.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal {
                            content: tool_blocks,
                        },
                    });
                }
            }
        }
        result
    }

    /// Build the comma-separated anthropic-beta header value for a given model.
    ///
    /// Always includes interleaved-thinking and fine-grained-tool-streaming.
    /// Adds the 128k output beta for large context models (opus-4, sonnet-4).
    pub(super) fn build_beta_headers(model: &str) -> String {
        let mut betas = vec![
            "interleaved-thinking-2025-05-14",
            "fine-grained-tool-streaming-2025-05-14",
        ];
        if Self::is_large_context_model(model) {
            betas.push("output-128k-2025-02-19");
        }
        betas.join(",")
    }

    /// Returns true for large context models that support 128k output tokens.
    pub(super) fn is_large_context_model(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("opus-4") || m.contains("sonnet-4")
    }

    /// Map ThinkLevel to budget_tokens
    pub(super) fn map_think_level(level: &ThinkLevel) -> Option<u32> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(1024),
            ThinkLevel::Low => Some(4096),
            ThinkLevel::Medium => Some(10000),
            ThinkLevel::High => Some(20000),
            ThinkLevel::XHigh => Some(50000),
        }
    }

    // =============================================================================
    // Kimi for Coding model optimizations
    // =============================================================================

    /// Detect if model is a Kimi for Coding model
    pub(super) fn is_kimi_model(model: &str) -> bool {
        let m = model.to_lowercase();
        m.contains("kimi-k2") || m.contains("kimi-k2.5") || m.contains("kimi-k2p5")
    }

    /// Get default temperature for Kimi models
    pub(super) fn kimi_default_temperature(model: &str) -> Option<f32> {
        let m = model.to_lowercase();
        if m.contains("k2.5") || m.contains("k2p5") || m.contains("k2-5") {
            Some(1.0)
        } else if m.contains("kimi-k2") {
            Some(0.6)
        } else {
            None
        }
    }
}


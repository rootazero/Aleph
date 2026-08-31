//! `impl OpenAiProtocol` — construction and internal helpers.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::message::ContentBlock as UCB;
use crate::providers::message::UnifiedMessage;
use crate::providers::openai::types::{OpenAiFunctionCallOut, OpenAiToolCallOut};
use crate::providers::openai::{
    ContentBlock as OaiContentBlock, ImageUrl, Message, MessageContent,
};
use crate::providers::openai::{OpenAiFunctionCall, OpenAiToolCall};
use crate::sync_primitives::Arc;

use super::{sanitize_tool_name, OpenAiProtocol};

impl OpenAiProtocol {
    /// Create a new `OpenAI` protocol adapter with the given HTTP client
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            stream_idle_timeout_secs: Arc::new(crate::sync_primitives::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }

    /// Build the endpoint URL from provider configuration
    pub(super) fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map_or_else(
                || "https://api.openai.com/v1".to_string(),
                |s| s.to_string(),
            );

        // Defence in depth: reject non-HTTP schemes before they reach reqwest.
        // The value is operator config / a preset, but a typo or tampered
        // preset must not smuggle `file://`, `javascript:`, etc. into the URL
        // parser. Host-level filtering (loopback / RFC1918 / cloud metadata)
        // is intentionally left to the operator's network policy.
        if let Err(e) = crate::providers::protocols::http_client::validate_provider_base_url(
            &raw_base_url,
        ) {
            tracing::error!(error = %e, "OpenAI provider base_url failed validation");
            return raw_base_url;
        }

        // Detect API version from the URL (v1 or v3)
        let is_v3_api = raw_base_url.contains("/v3") || raw_base_url.contains("/api/v3");

        // Normalize URL: remove trailing slashes and version suffixes
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v3")
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        // Build endpoint with appropriate API version
        if is_v3_api {
            format!("{base_url}/v3/chat/completions")
        } else {
            format!("{base_url}/v1/chat/completions")
        }
    }

    /// Convert `UnifiedMessages` to `OpenAI` Messages
    pub(super) fn convert_messages(
        messages: &[UnifiedMessage],
        system_prompt: Option<&str>,
    ) -> Vec<Message> {
        let mut result = Vec::new();

        // Add system message if provided
        if let Some(prompt) = system_prompt {
            result.push(Message::text("system", prompt.to_string()));
        }

        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    let has_images = content.iter().any(|b| matches!(b, UCB::Image { .. }));

                    if has_images {
                        // Use OpenAI's multimodal content array format
                        let blocks: Vec<OaiContentBlock> = content
                            .iter()
                            .filter_map(|b| match b {
                                UCB::Text { text, .. } => {
                                    // rust-doctor-disable-next-line excessive-clone
                                    Some(OaiContentBlock::Text { text: text.clone() })
                                }
                                UCB::Image { data, mime_type } => Some(OaiContentBlock::ImageUrl {
                                    image_url: ImageUrl {
                                        url: format!("data:{mime_type};base64,{data}"),
                                        detail: Some("auto".to_string()),
                                    },
                                }),
                                _ => None,
                            })
                            .collect();
                        let image_count = blocks
                            .iter()
                            .filter(|b| matches!(b, OaiContentBlock::ImageUrl { .. }))
                            .count();
                        tracing::info!(
                            target: "multimodal",
                            probe = "P6_provider",
                            role = "user",
                            content_type = "multimodal",
                            image_count = image_count,
                            "OpenAI multimodal message converted"
                        );
                        result.push(Message {
                            role: "user".to_string(),
                            tool_call_id: None,
                            tool_calls: None,
                            content: MessageContent::Multimodal { content: blocks },
                        });
                    } else {
                        // Text-only path
                        let text = content
                            .iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("\n");
                        result.push(Message::text("user", text));
                    }
                }
                UnifiedMessage::Assistant { content } => {
                    let text: String = content
                        .iter()
                        .filter_map(|b| match b {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    // Extract tool calls
                    let tool_calls: Vec<_> = content
                        .iter()
                        .filter_map(|b| match b {
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                                ..
                            } => Some(OpenAiToolCall {
                                // rust-doctor-disable-next-line excessive-clone
                                id: id.clone(),
                                call_type: Some("function".to_string()),
                                function: OpenAiFunctionCall {
                                    name: sanitize_tool_name(name),
                                    arguments: serde_json::to_string(arguments).unwrap_or_default(),
                                },
                            }),
                            _ => None,
                        })
                        .collect();

                    let msg_content = if text.is_empty() { None } else { Some(text) };

                    if tool_calls.is_empty() {
                        result.push(Message::text("assistant", msg_content.unwrap_or_default()));
                    } else {
                        // Convert to serializable tool call format
                        let tc_out: Vec<OpenAiToolCallOut> = tool_calls
                            .into_iter()
                            .map(|tc| OpenAiToolCallOut {
                                id: tc.id,
                                call_type: "function".to_string(),
                                function: OpenAiFunctionCallOut {
                                    name: tc.function.name,
                                    arguments: tc.function.arguments,
                                },
                            })
                            .collect();
                        result.push(Message::assistant_with_tool_calls(msg_content, tc_out));
                    }
                }
                UnifiedMessage::ToolResult {
                    tool_call_id,
                    content,
                    ..
                } => {
                    let output = content
                        .iter()
                        .filter_map(|b| match b {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                // rust-doctor-disable-next-line excessive-clone
                                Some(text.clone())
                            }
                            crate::providers::message::ContentBlock::Json { value } => {
                                Some(serde_json::to_string(value).unwrap_or_default())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    // Each ToolResult as separate tool message with required tool_call_id
                    // rust-doctor-disable-next-line excessive-clone
                    result.push(Message::tool_result(tool_call_id.clone(), output));
                }
            }
        }
        result
    }

    /// Map `ThinkLevel` to `OpenAI` `reasoning_effort`.
    ///
    /// Emits all five non-`Off` levels faithfully — `minimal` and `xhigh` are
    /// real effort values on the gpt-5 family, so collapsing them (the old
    /// behavior: `Minimal → None`, `XHigh → "high"`) silently dropped fidelity.
    /// The endpoint-level `supports_reasoning_effort` capability still strips
    /// the whole field for backends that don't accept it.
    ///
    /// # `Off` is a value, not an absence
    ///
    /// `Off` used to return `None`, i.e. omit `reasoning_effort` entirely. But on
    /// a reasoning model, omitting the field does not disable reasoning — it
    /// selects the SERVER's default, which is `medium`. So "thinking off" quietly
    /// bought medium reasoning and billed it at the output rate: the one setting
    /// a cost-conscious user reaches for was the one that didn't work.
    ///
    /// `"none"` is a real effort value on the families that can disable reasoning
    /// (gpt-5.1 / gpt-5.2 / codex-max — see `supported_efforts`). On families that
    /// cannot, `clamp_effort` maps it to the cheapest effort they do support
    /// rather than to silence. Either way the caller gets the least reasoning the
    /// model is capable of, which is what they asked for.
    ///
    /// The distinction that matters: `Off` (the user said "off") emits a value;
    /// `think_level: None` (nobody said anything) never reaches here at all and
    /// leaves the provider on its own default.
    pub(super) fn map_think_level(level: &ThinkLevel) -> Option<String> {
        match level {
            ThinkLevel::Off => Some("none".to_string()),
            ThinkLevel::Minimal => Some("minimal".to_string()),
            ThinkLevel::Low => Some("low".to_string()),
            ThinkLevel::Medium => Some("medium".to_string()),
            ThinkLevel::High => Some("high".to_string()),
            ThinkLevel::XHigh => Some("xhigh".to_string()),
        }
    }
}

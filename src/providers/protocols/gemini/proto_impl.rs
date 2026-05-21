//! `impl GeminiProtocol` — construction and internal helpers.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::gemini::{
    Content,
    Part, ThinkingConfig,
};
use crate::providers::message::UnifiedMessage;

use super::GeminiProtocol;

impl GeminiProtocol {
    /// Create a new Gemini protocol adapter
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            stream_idle_timeout_secs: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
                crate::providers::protocols::stream_idle::DEFAULT_STREAM_IDLE_SECS,
            )),
        }
    }

    /// Build the endpoint URL — always uses the streaming endpoint (stream-first architecture)
    pub(super) fn build_endpoint(config: &ProviderConfig, model_override: Option<&str>) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        // Normalize URL: strip trailing slashes and /v1 suffix
        // (user may have /v1 from switching between OpenAI/Anthropic protocols)
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        let model = model_override.unwrap_or_else(|| config.default_model());

        // Always use the streaming endpoint
        format!("{}/v1beta/models/{}:streamGenerateContent", base_url, model)
    }

    /// Convert UnifiedMessages to Gemini Contents
    pub(super) fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Content> {
        let mut result = Vec::new();
        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    let mut parts = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                parts.push(Part::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::Image { data, mime_type } => {
                                parts.push(Part::InlineData {
                                    inline_data: crate::providers::gemini::InlineData {
                                        mime_type: mime_type.clone(),
                                        data: data.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    // Keep the request valid even if the message carried no
                    // text/image blocks (e.g. only unsupported block kinds).
                    if parts.is_empty() {
                        parts.push(Part::Text {
                            text: String::new(),
                        });
                    }
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
                UnifiedMessage::Assistant { content } => {
                    let mut parts = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                parts.push(Part::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                parts.push(Part::FunctionCall {
                                    function_call: crate::providers::gemini::GeminiFunctionCall {
                                        name: name.clone(),
                                        args: arguments.clone(),
                                        // Replay the id so the assistant's functionCall
                                        // and the matching functionResponse stay paired
                                        // (required for Gemini 3 native tool-call ids).
                                        id: Some(id.clone()),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    if parts.is_empty() {
                        parts.push(Part::Text {
                            text: String::new(),
                        });
                    }
                    result.push(Content {
                        role: Some("model".to_string()),
                        parts,
                    });
                }
                UnifiedMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    ..
                } => {
                    // A lone structured-JSON object passes through directly as the
                    // functionResponse payload; text / mixed content is wrapped in
                    // `{"result": ...}` so `response` is always a JSON object.
                    let response = match content.as_slice() {
                        [crate::providers::message::ContentBlock::Json { value }]
                            if value.is_object() =>
                        {
                            value.clone()
                        }
                        _ => {
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
                            serde_json::json!({ "result": output })
                        }
                    };
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts: vec![Part::FunctionResponse {
                            function_response: crate::providers::gemini::GeminiFunctionResponse {
                                name: tool_name.clone(),
                                response,
                                id: Some(tool_call_id.clone()),
                            },
                        }],
                    });
                }
            }
        }
        result
    }

    /// Build system instruction from system prompt
    pub(super) fn build_system_instruction(system_prompt: Option<&str>) -> Option<Content> {
        system_prompt.map(|prompt| Content {
            role: None, // system instruction doesn't have a role
            parts: vec![Part::Text {
                text: prompt.to_string(),
            }],
        })
    }

    /// Map ThinkLevel to Gemini ThinkingConfig.
    ///
    /// - Gemini 2.5 models → `thinkingBudget` (integer)
    /// - All others (Gemini 3+) → `thinkingLevel` (enum)
    pub(super) fn map_think_level(level: &ThinkLevel, model: &str) -> Option<ThinkingConfig> {
        if *level == ThinkLevel::Off {
            return None;
        }
        // Gemini 2.5 models use thinkingBudget; all others use thinkingLevel
        let use_budget = model.contains("gemini-2.5");
        if use_budget {
            let budget = match level {
                ThinkLevel::Minimal => 500,
                ThinkLevel::Low => 1000,
                ThinkLevel::Medium => 2000,
                ThinkLevel::High => 4000,
                ThinkLevel::XHigh => 8000,
                ThinkLevel::Off => unreachable!(),
            };
            Some(ThinkingConfig {
                thinking_budget: Some(budget),
                thinking_level: None,
                include_thoughts: Some(true),
            })
        } else {
            let level_str = match level {
                ThinkLevel::Minimal => "MINIMAL",
                ThinkLevel::Low => "LOW",
                ThinkLevel::Medium => "MEDIUM",
                ThinkLevel::High | ThinkLevel::XHigh => "HIGH",
                ThinkLevel::Off => unreachable!(),
            };
            Some(ThinkingConfig {
                thinking_budget: None,
                thinking_level: Some(level_str.into()),
                include_thoughts: Some(true),
            })
        }
    }
}

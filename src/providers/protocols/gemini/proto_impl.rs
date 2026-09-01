//! `impl GeminiProtocol` — construction and internal helpers.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::gemini::{Content, Part, ThinkingConfig};
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::Arc;

use super::GeminiProtocol;

impl GeminiProtocol {
    /// Create a new Gemini protocol adapter
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            stream_idle_timeout_secs: Arc::new(crate::sync_primitives::AtomicU64::new(
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
            .map_or_else(
                || "https://generativelanguage.googleapis.com".to_string(),
                |s| s.to_string(),
            );

        // Defence in depth: reject non-HTTP schemes before reqwest sees the URL.
        if let Err(e) =
            crate::providers::protocols::http_client::validate_provider_base_url(&raw_base_url)
        {
            tracing::error!(error = %e, "Gemini provider base_url failed validation");
        }

        // Normalize URL: strip trailing slashes and /v1 suffix
        // (user may have /v1 from switching between OpenAI/Anthropic protocols)
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        let model = model_override.unwrap_or_else(|| config.default_model());

        // Always use the streaming endpoint
        format!("{base_url}/v1beta/models/{model}:streamGenerateContent")
    }

    /// Convert `UnifiedMessages` to Gemini Contents
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub(super) fn convert_messages(messages: &[UnifiedMessage]) -> Vec<Content> {
        let mut result = Vec::new();
        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    // rust-doctor-disable-next-line unnecessary-allocation
                    let mut parts = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                // rust-doctor-disable-next-line excessive-clone
                                parts.push(Part::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::Image { data, mime_type } => {
                                parts.push(Part::InlineData {
                                    inline_data: crate::providers::gemini::InlineData {
                                        // rust-doctor-disable-next-line excessive-clone
                                        mime_type: mime_type.clone(),
                                        // rust-doctor-disable-next-line excessive-clone
                                        data: data.clone(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }
                    // Gemini rejects empty text parts ("empty text parameter",
                    // HTTP 400), so a message that carried no convertible
                    // blocks is skipped entirely rather than padded with a
                    // placeholder. Mirrors hermes/pi's empty-parts guard.
                    if parts.is_empty() {
                        continue;
                    }
                    result.push(Content {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
                UnifiedMessage::Assistant { content } => {
                    // rust-doctor-disable-next-line unnecessary-allocation
                    let mut parts = Vec::new();
                    for block in content {
                        match block {
                            crate::providers::message::ContentBlock::Text { text, .. } => {
                                // rust-doctor-disable-next-line excessive-clone
                                parts.push(Part::Text { text: text.clone() });
                            }
                            crate::providers::message::ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                                thought_signature,
                            } => {
                                parts.push(Part::FunctionCall {
                                    function_call: crate::providers::gemini::GeminiFunctionCall {
                                        // rust-doctor-disable-next-line excessive-clone
                                        name: name.clone(),
                                        // rust-doctor-disable-next-line excessive-clone
                                        args: arguments.clone(),
                                        // Replay the id so the assistant's functionCall
                                        // and the matching functionResponse stay paired
                                        // (required for Gemini 3 native tool-call ids).
                                        // rust-doctor-disable-next-line excessive-clone
                                        id: Some(id.clone()),
                                    },
                                    // Replay Gemini 3's thoughtSignature verbatim so
                                    // the model's reasoning chain stays intact across
                                    // turns. `None` (other providers / older Gemini)
                                    // is omitted from the wire.
                                    // rust-doctor-disable-next-line excessive-clone
                                    thought_signature: thought_signature.clone(),
                                });
                            }
                            _ => {}
                        }
                    }
                    // Same empty-parts guard as the user arm: an assistant
                    // message whose blocks were all dropped (e.g. a pure
                    // Thinking turn) must not become an empty text part.
                    if parts.is_empty() {
                        continue;
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
                            // rust-doctor-disable-next-line excessive-clone
                            value.clone()
                        }
                        _ => {
                            let output = content
                                .iter()
                                .filter_map(|b| match b {
                                    crate::providers::message::ContentBlock::Text {
                                        text, ..
                                    } => {
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
                            serde_json::json!({ "result": output })
                        }
                    };
                    let part = Part::FunctionResponse {
                        function_response: crate::providers::gemini::GeminiFunctionResponse {
                            // rust-doctor-disable-next-line excessive-clone
                            name: tool_name.clone(),
                            response,
                            // rust-doctor-disable-next-line excessive-clone
                            id: Some(tool_call_id.clone()),
                        },
                    };
                    // Parallel function calling: Gemini requires the number of
                    // functionResponse parts in the user turn to match the
                    // functionCall parts of the preceding model turn, so
                    // consecutive tool results merge into one user Content
                    // (mirrors pi/openclaw). A response-only user turn is
                    // recognizable by its parts being all FunctionResponse.
                    match result.last_mut() {
                        Some(prev)
                            if prev.role.as_deref() == Some("user")
                                && prev
                                    .parts
                                    .iter()
                                    .all(|p| matches!(p, Part::FunctionResponse { .. })) =>
                        {
                            prev.parts.push(part);
                        }
                        _ => result.push(Content {
                            role: Some("user".to_string()),
                            parts: vec![part],
                        }),
                    }
                }
            }
        }
        // Defensive: every message was skipped by the empty-parts guard but the
        // caller did pass messages — send a minimal non-empty user turn instead
        // of an empty `contents` array (which Gemini also rejects).
        if result.is_empty() && !messages.is_empty() {
            result.push(Content {
                role: Some("user".to_string()),
                parts: vec![Part::Text {
                    text: " ".to_string(),
                }],
            });
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

    /// Map `ThinkLevel` to Gemini `ThinkingConfig`.
    ///
    /// - Gemini 2.5 models → `thinkingBudget` (integer)
    /// - All others (Gemini 3+) → `thinkingLevel` (enum)
    ///
    /// # `Off` must be sent, not omitted
    ///
    /// This used to `return None` for `Off`, emitting no `thinkingConfig` at all.
    /// But Gemini 2.5 Flash **thinks by default** — omitting the config does not
    /// disable thinking, it accepts the model's default budget, which then bills
    /// as output tokens. "Thinking off" therefore bought thinking and charged for
    /// it. The documented disable is an explicit `thinkingBudget: 0`.
    ///
    /// The Gemini 3+ `thinkingLevel` enum has no "off" member, so `Off` floors to
    /// its cheapest level instead — the same semantics `clamp_effort` applies on
    /// the OpenAI side (disable where the family can, else the least reasoning it
    /// offers). No API constant is invented here.
    ///
    /// `Off` is distinct from `think_level: None`, which never reaches this
    /// function and leaves the provider on its own default.
    pub(super) fn map_think_level(level: &ThinkLevel, model: &str) -> Option<ThinkingConfig> {
        // Gemini 2.5 models use thinkingBudget; all others use thinkingLevel
        let use_budget = model.contains("gemini-2.5");
        if use_budget {
            let budget = match level {
                ThinkLevel::Off => 0,
                ThinkLevel::Minimal => 500,
                ThinkLevel::Low => 1000,
                ThinkLevel::Medium => 2000,
                ThinkLevel::High => 4000,
                ThinkLevel::XHigh => 8000,
            };
            Some(ThinkingConfig {
                thinking_budget: Some(budget),
                thinking_level: None,
                // Asking for thoughts we just disabled is incoherent, and on a
                // zero budget there are none to include.
                include_thoughts: Some(*level != ThinkLevel::Off),
            })
        } else {
            let level_str = match level {
                ThinkLevel::Off | ThinkLevel::Minimal => "MINIMAL",
                ThinkLevel::Low => "LOW",
                ThinkLevel::Medium => "MEDIUM",
                ThinkLevel::High | ThinkLevel::XHigh => "HIGH",
            };
            Some(ThinkingConfig {
                thinking_budget: None,
                thinking_level: Some(level_str.into()),
                include_thoughts: Some(*level != ThinkLevel::Off),
            })
        }
    }
}

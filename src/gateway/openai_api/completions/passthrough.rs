//! Passthrough path — direct LLM proxy via `HttpProvider`.

use crate::sync_primitives::Arc;

use axum::body::Body;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;

use crate::gateway::openai_api::auth::ApiError;
use crate::gateway::openai_api::state::OpenAiApiState;
use crate::gateway::openai_api::stream;
use crate::gateway::openai_api::types::{
    ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, Usage,
};
use crate::providers::adapter::{RequestPayload, ToolChoice};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use crate::tool_metadata::{ToolCategory, ToolDefinition};
use serde_json::{json, Value};

/// Convert OpenAI-format tool definitions to internal `ToolDefinition`.
#[must_use]
pub fn convert_openai_tools(tools: &[Value]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .filter_map(|t| {
            let func = t.get("function")?;
            Some(ToolDefinition {
                name: func.get("name")?.as_str()?.to_string(),
                description: func
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                parameters: func.get("parameters").cloned().unwrap_or(json!({})),
                requires_confirmation: false,
                category: ToolCategory::Custom,
                strict: func
                    .get("strict")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Convert OpenAI-format `tool_choice` value to internal `ToolChoice`.
#[must_use]
pub fn convert_tool_choice(choice: &Value) -> Option<ToolChoice> {
    match choice {
        Value::String(s) => match s.as_str() {
            "auto" => Some(ToolChoice::Auto),
            "none" => Some(ToolChoice::None),
            "required" => Some(ToolChoice::Required),
            _ => None,
        },
        Value::Object(obj) => obj
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|name| ToolChoice::Specific(name.to_string())),
        _ => None,
    }
}

/// Convert `ChatMessage` list to `Vec<UnifiedMessage>`.
///
/// System messages are excluded here — they are handled separately by
/// [`extract_system_prompt`]. Only "user" and "assistant" roles are mapped.
fn convert_messages(messages: &[ChatMessage]) -> Vec<UnifiedMessage> {
    messages
        .iter()
        .filter_map(|msg| {
            let content_text = msg.content.as_deref().unwrap_or("");
            match msg.role.as_str() {
                "user" => Some(UnifiedMessage::User {
                    content: vec![ContentBlock::Text {
                        text: content_text.to_string(),
                        cache_control: None,
                    }],
                }),
                "assistant" => {
                    let mut content = Vec::new();
                    if !content_text.is_empty() {
                        content.push(ContentBlock::Text {
                            text: content_text.to_string(),
                            cache_control: None,
                        });
                    }
                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            if let (Some(id), Some(func)) =
                                (tc.get("id").and_then(|v| v.as_str()), tc.get("function"))
                            {
                                content.push(ContentBlock::ToolCall {
                                    thought_signature: None,
                                    id: id.to_string(),
                                    name: func
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    arguments: func
                                        .get("arguments")
                                        .and_then(|a| a.as_str())
                                        .and_then(|s| serde_json::from_str(s).ok())
                                        .unwrap_or(json!({})),
                                });
                            }
                        }
                    }
                    Some(UnifiedMessage::Assistant { content })
                }
                "tool" => {
                    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                    Some(UnifiedMessage::ToolResult {
                        tool_call_id,
                        tool_name: String::new(),
                        content: vec![ContentBlock::Text {
                            text: content_text.to_string(),
                            cache_control: None,
                        }],
                        is_error: false,
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// Extract the first system message's content as the system prompt.
fn extract_system_prompt(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.role == "system")
        .and_then(|m| m.content.clone())
}

/// Handle a passthrough (non-agent) completion request.
///
/// Looks up the model in `state.provider_map`, converts messages to the
/// unified format, and either streams SSE or returns a single JSON response.
pub async fn handle(
    state: Arc<OpenAiApiState>,
    req: ChatCompletionRequest,
) -> Result<Response, ApiError> {
    // Look up provider by model name
    let provider = state
        .provider_map
        .get(&req.model)
        .ok_or_else(|| ApiError::NotFound(format!("Model '{}' not found", req.model)))?
        .clone();

    // Convert messages
    let unified_messages = convert_messages(&req.messages);
    let system_prompt = extract_system_prompt(&req.messages);

    // Build payload
    let tool_defs = req.tools.as_ref().map(|t| convert_openai_tools(t));
    let tool_choice = req.tool_choice.as_ref().and_then(convert_tool_choice);

    let payload = RequestPayload {
        messages: &unified_messages,
        system_prompt: system_prompt.as_deref(),
        system_blocks: None,
        tools: tool_defs.as_deref(),
        think_level: None,
        temperature: req.temperature.map(|t| t as f32),
        max_tokens: req.max_tokens,
        tool_choice,
        model: None,
        metadata: None,
    };

    let is_streaming = req.stream.unwrap_or(false);
    let include_usage = req
        .stream_options
        .as_ref()
        .and_then(|o| o.include_usage)
        .unwrap_or(false);

    if is_streaming {
        // Streaming path
        let delta_stream = provider
            .stream_raw(payload)
            .await
            .map_err(|e| ApiError::BadGateway(format!("Provider stream error: {e}")))?;

        let sse_stream = stream::provider_deltas_to_sse(delta_stream, req.model, include_usage);

        let body = Body::from_stream(sse_stream.map(Ok::<_, std::convert::Infallible>));
        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .map_err(|e| ApiError::BadGateway(format!("failed to build response: {e}")))?
            .into_response())
    } else {
        // Non-streaming path
        let response = provider
            .process(payload)
            .await
            .map_err(|e| ApiError::BadGateway(format!("Provider error: {e}")))?;

        let usage = response.usage.map(|u| Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        });

        let tool_calls = if response.tool_calls.is_empty() {
            None
        } else {
            Some(
                response
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments.to_string()
                            }
                        })
                    })
                    .collect::<Vec<_>>(),
            )
        };

        let finish_reason = if tool_calls.is_some() {
            "tool_calls"
        } else {
            "stop"
        };

        let completion = ChatCompletionResponse {
            id: stream::completion_id(),
            object: "chat.completion".to_string(),
            created: stream::now_timestamp(),
            model: req.model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: response.text,
                    tool_calls,
                    tool_call_id: None,
                },
                finish_reason: Some(finish_reason.to_string()),
                delta: None,
            }],
            usage,
        };

        Ok(Json(completion).into_response())
    }
}

//! Passthrough path — direct LLM proxy via HttpProvider.

use std::sync::Arc;

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
use crate::providers::adapter::RequestPayload;
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;

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
                    }],
                }),
                "assistant" => Some(UnifiedMessage::Assistant {
                    content: vec![ContentBlock::Text {
                        text: content_text.to_string(),
                    }],
                }),
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
    let payload = RequestPayload {
        messages: &unified_messages,
        system_prompt: system_prompt.as_deref(),
        tools: None,
        think_level: None,
        temperature: req.temperature.map(|t| t as f32),
        max_tokens: req.max_tokens,
        tool_choice: None,
    };

    let is_streaming = req.stream.unwrap_or(false);

    if is_streaming {
        // Streaming path
        let delta_stream = provider
            .stream_raw(payload)
            .await
            .map_err(|e| ApiError::BadGateway(format!("Provider stream error: {e}")))?;

        let sse_stream = stream::provider_deltas_to_sse(delta_stream, req.model);

        let body = Body::from_stream(sse_stream.map(Ok::<_, std::convert::Infallible>));
        Ok(Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .unwrap()
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
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
                delta: None,
            }],
            usage,
        };

        Ok(Json(completion).into_response())
    }
}

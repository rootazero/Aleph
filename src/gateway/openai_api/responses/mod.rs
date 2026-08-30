//! POST /v1/responses — `OpenAI` Responses API passthrough.

pub mod sse;

use crate::sync_primitives::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde_json::json;

use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;

use super::auth::{extract_bearer_token, ApiError};
use super::completions::passthrough::{convert_openai_tools, convert_tool_choice};
use super::state::OpenAiApiState;
use super::stream;
use super::types::{ResponsesInput, ResponsesRequest, ResponsesResponse, ResponsesUsage};

/// Extract text from a Responses API message `content` field.
///
/// The content can be either a plain string or an array of content parts.
/// For arrays, we look for items with `type: "input_text"` and extract their `text`.
fn extract_content_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                if item.get("type")?.as_str()? == "input_text" {
                    item.get("text")?.as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Convert Responses API input to unified messages.
fn convert_input(input: &ResponsesInput) -> Vec<UnifiedMessage> {
    match input {
        ResponsesInput::Text(s) => vec![UnifiedMessage::user(s.clone())],
        ResponsesInput::Messages(msgs) => msgs
            .iter()
            .filter_map(|msg| {
                let text = msg
                    .content
                    .as_ref()
                    .map(extract_content_text)
                    .unwrap_or_default();
                match msg.role.as_str() {
                    "user" => Some(UnifiedMessage::user(text)),
                    "assistant" => Some(UnifiedMessage::assistant(text)),
                    _ => None,
                }
            })
            .collect(),
    }
}

/// Handle a `POST /v1/responses` request.
pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<ResponsesRequest>,
) -> Result<Response, ApiError> {
    // Auth check — same pattern as completions/embeddings
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = extract_bearer_token(auth_header)
        .ok_or_else(|| ApiError::Unauthorized("Missing or invalid Authorization header".into()))?;
    if let Some(expected) = (state.api_token)() {
        if !crate::security::secret_equal(Some(token), Some(expected.as_str())) {
            return Err(ApiError::Unauthorized("Invalid API key".into()));
        }
    }

    // Provider lookup
    let provider = state
        .provider_map
        .get(&req.model)
        .ok_or_else(|| ApiError::NotFound(format!("Model '{}' not found", req.model)))?
        .clone();

    // Convert input
    let unified_messages = convert_input(&req.input);
    let system_prompt = req.instructions.clone();

    // Convert tools and tool_choice from OpenAI format
    let tool_defs = req.tools.as_ref().map(|t| convert_openai_tools(t));
    let tool_choice = req.tool_choice.as_ref().and_then(convert_tool_choice);

    // Build RequestPayload
    let payload = RequestPayload {
        messages: &unified_messages,
        system_prompt: system_prompt.as_deref(),
        system_blocks: None,
        tools: tool_defs.as_deref(),
        think_level: None,
        temperature: req.temperature.map(|t| t as f32),
        max_tokens: req.max_output_tokens,
        tool_choice,
        model: None,
        metadata: None,
    };

    let is_streaming = req.stream.unwrap_or(false);

    if is_streaming {
        // Streaming path
        let delta_stream = provider
            .stream_raw(payload)
            .await
            .map_err(|e| ApiError::BadGateway(format!("Provider stream error: {e}")))?;

        let sse_stream = sse::provider_deltas_to_responses_sse(delta_stream, req.model);

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

        // Build output items
        let mut output: Vec<serde_json::Value> = Vec::new();

        if let Some(ref text) = response.text {
            output.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": text,
                }]
            }));
        }

        for tc in &response.tool_calls {
            output.push(json!({
                "type": "function_call",
                "id": tc.id,
                "name": tc.name,
                "arguments": tc.arguments.to_string(),
            }));
        }

        let usage = response.usage.map_or(
            ResponsesUsage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            },
            |u| ResponsesUsage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                total_tokens: u.input_tokens + u.output_tokens,
            },
        );

        let resp = ResponsesResponse {
            id: format!("resp-{}", uuid::Uuid::new_v4()),
            object: "response".to_string(),
            created_at: stream::now_timestamp(),
            status: "completed".to_string(),
            model: req.model,
            output,
            usage,
        };

        Ok(Json(resp).into_response())
    }
}

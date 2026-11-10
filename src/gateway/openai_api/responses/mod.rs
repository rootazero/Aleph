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

use crate::providers::adapter::{RequestPayload, ToolChoice};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use crate::tool_metadata::{ToolCategory, ToolDefinition};

use super::auth::{authorize, ApiError};
use super::state::OpenAiApiState;
use super::stream;
use super::types::{
    ResponsesInput, ResponsesInputItem, ResponsesRequest, ResponsesResponse,
    ResponsesToolInputItem, ResponsesUsage,
};

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
                if matches!(item.get("type")?.as_str()?, "input_text" | "output_text") {
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
fn convert_input(input: &ResponsesInput) -> Result<Vec<UnifiedMessage>, ApiError> {
    match input {
        ResponsesInput::Text(s) => Ok(vec![UnifiedMessage::user(s.clone())]),
        ResponsesInput::Messages(msgs) => msgs
            .iter()
            .map(|msg| match msg {
                ResponsesInputItem::Message(msg) => {
                    let text = msg.content.as_ref().map(extract_content_text).unwrap_or_default();
                    match msg.role.as_str() {
                        "user" => Ok(UnifiedMessage::user(text)),
                        "assistant" => Ok(UnifiedMessage::assistant(text)),
                        role => Err(ApiError::BadRequest(format!("Unsupported Responses input role: {role}; use instructions for system context"))),
                    }
                }
                ResponsesInputItem::Tool(ResponsesToolInputItem::FunctionCall { call_id, name, arguments }) => {
                    let arguments = serde_json::from_str(arguments).map_err(|e| ApiError::BadRequest(format!("Invalid function_call arguments: {e}")))?;
                    Ok(UnifiedMessage::Assistant { content: vec![ContentBlock::ToolCall {
                        thought_signature: None, id: call_id.clone(), name: name.clone(), arguments,
                    }] })
                }
                ResponsesInputItem::Tool(ResponsesToolInputItem::FunctionCallOutput { call_id, output }) => Ok(UnifiedMessage::ToolResult {
                    tool_call_id: call_id.clone(), tool_name: String::new(),
                    content: vec![ContentBlock::Text { text: output.clone(), cache_control: None }], is_error: false,
                }),
            })
            .collect(),
    }
}

/// Responses uses flat function definitions; do not parse them as Chat wire objects.
fn convert_responses_tools(tools: &[serde_json::Value]) -> Result<Vec<ToolDefinition>, ApiError> {
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(|v| v.as_str()) != Some("function") {
                return Err(ApiError::BadRequest(
                    "Only function tools are supported by this Responses endpoint".into(),
                ));
            }
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Function tool requires a name".into()))?;
            Ok(ToolDefinition::new(
                name.to_string(),
                tool.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tool.get("parameters").cloned().unwrap_or(json!({})),
                ToolCategory::Custom,
            )
            .with_strict(
                tool.get("strict")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            ))
        })
        .collect()
}

fn convert_responses_tool_choice(choice: &serde_json::Value) -> Result<ToolChoice, ApiError> {
    match choice.as_str() {
        Some("auto") => Ok(ToolChoice::Auto),
        Some("none") => Ok(ToolChoice::None),
        Some("required") => Ok(ToolChoice::Required),
        _ if choice.get("type").and_then(|v| v.as_str()) == Some("function") => choice
            .get("name")
            .and_then(|v| v.as_str())
            .map(|name| ToolChoice::Specific(name.to_string()))
            .ok_or_else(|| ApiError::BadRequest("Function tool_choice requires a name".into())),
        _ => Err(ApiError::BadRequest(
            "Unsupported Responses tool_choice".into(),
        )),
    }
}

/// Both transports retain the provider id as item identity and correlation id.
fn function_call_item(id: &str, name: &str, arguments: &str) -> serde_json::Value {
    json!({"type": "function_call", "id": id, "call_id": id, "name": name, "arguments": arguments})
}

#[cfg(test)]
mod gx6_tests {
    use super::*;
    use crate::providers::adapter::ToolChoice;
    use crate::providers::message::ContentBlock;

    #[test]
    fn flattened_tools_and_choices_convert_without_the_chat_wire_parser() {
        let tools = convert_responses_tools(&[json!({
            "type": "function", "name": "lookup", "parameters": {"type": "object"}, "strict": true
        })])
        .unwrap();
        assert_eq!(tools[0].name, "lookup");
        assert_eq!(tools[0].parameters, json!({"type": "object"}));
        assert!(tools[0].strict);
        assert!(
            matches!(convert_responses_tool_choice(&json!({"type":"function","name":"lookup"})).unwrap(), ToolChoice::Specific(name) if name == "lookup")
        );
        for choice in ["auto", "none", "required"] {
            assert!(convert_responses_tool_choice(&json!(choice)).is_ok());
        }
        assert!(convert_responses_tools(&[json!({"type":"web_search_preview"})]).is_err());
        assert!(convert_responses_tool_choice(&json!({"type":"allowed_tools"})).is_err());
    }

    #[test]
    fn function_call_output_round_trips_with_the_call_id_not_the_item_id() {
        let input: ResponsesInput = serde_json::from_value(json!([
            {"role":"user","content":"lookup"},
            {"type":"function_call","id":"fc_item","call_id":"c1","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","call_id":"c1","output":"ok"}
        ]))
        .unwrap();
        let messages = convert_input(&input).unwrap();
        assert!(
            matches!(&messages[1], UnifiedMessage::Assistant { content } if matches!(&content[0], ContentBlock::ToolCall { id, .. } if id == "c1"))
        );
        assert!(
            matches!(&messages[2], UnifiedMessage::ToolResult { tool_call_id, .. } if tool_call_id == "c1")
        );
        let item = function_call_item("c1", "lookup", "{}");
        assert_eq!(item["id"], "c1");
        assert_eq!(item["call_id"], "c1");
        let replay: ResponsesInput = serde_json::from_value(json!([item])).unwrap();
        assert!(matches!(
            &convert_input(&replay).unwrap()[0],
            UnifiedMessage::Assistant { .. }
        ));
    }
}

/// Handle a `POST /v1/responses` request.
pub async fn handle(
    State(state): State<Arc<OpenAiApiState>>,
    headers: HeaderMap,
    Json(req): Json<ResponsesRequest>,
) -> Result<Response, ApiError> {
    // Auth check — same pattern as completions/embeddings
    authorize(&state, &headers)?;

    // Provider lookup
    let provider = state
        .provider_map
        .get(&req.model)
        .ok_or_else(|| ApiError::NotFound(format!("Model '{}' not found", req.model)))?
        .clone();

    // Convert input
    let unified_messages = convert_input(&req.input)?;
    let system_prompt = req.instructions.clone();

    let tool_defs = req
        .tools
        .as_deref()
        .map(convert_responses_tools)
        .transpose()?;
    let tool_choice = req
        .tool_choice
        .as_ref()
        .map(convert_responses_tool_choice)
        .transpose()?;

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
            output.push(function_call_item(
                &tc.id,
                &tc.name,
                &tc.arguments.to_string(),
            ));
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

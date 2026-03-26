//! Agent completions path — routes `aleph/*` models through ExecutionAdapter.
//!
//! Translates between OpenAI Chat Completions format and the internal
//! agent execution pipeline, using an `EventEmitter` that converts
//! `StreamEvent`s into SSE frames.

use std::collections::HashMap;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tokio::sync::mpsc;

use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::media::PendingMedia;
use crate::gateway::openai_api::auth::ApiError;
use crate::gateway::openai_api::state::OpenAiApiState;
use crate::gateway::openai_api::stream::{self, SSE_DONE};
use crate::gateway::openai_api::types::{
    ChatChoice, ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
    Delta, DeltaFunction, DeltaToolCall, StreamChoice, Usage,
};
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;

// =============================================================================
// SseEventEmitter — translates StreamEvent → OpenAI SSE frames
// =============================================================================

/// An [`EventEmitter`] that serialises [`StreamEvent`]s as OpenAI-compatible
/// SSE frames and sends them through an mpsc channel.
struct SseEventEmitter {
    tx: mpsc::Sender<String>,
    completion_id: String,
    model: String,
    created: u64,
    seq: std::sync::atomic::AtomicU64,
}

impl SseEventEmitter {
    fn new(tx: mpsc::Sender<String>, completion_id: String, model: String, created: u64) -> Self {
        Self {
            tx,
            completion_id,
            model,
            created,
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Build a `ChatCompletionChunk` with the given delta and optional finish reason.
    fn make_chunk(
        &self,
        delta: Delta,
        finish_reason: Option<String>,
    ) -> ChatCompletionChunk {
        ChatCompletionChunk {
            id: self.completion_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![StreamChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage: None,
        }
    }
}

#[async_trait]
impl EventEmitter for SseEventEmitter {
    fn next_seq(&self) -> u64 {
        self.seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let frame: Option<String> = match event {
            StreamEvent::ResponseChunk { content, .. } => {
                let chunk = self.make_chunk(
                    Delta {
                        content: Some(content),
                        role: None,
                        tool_calls: None,
                    },
                    None,
                );
                Some(stream::sse_data(&chunk))
            }

            StreamEvent::ToolStart {
                tool_name,
                tool_id,
                params,
                ..
            } => {
                let arguments = serde_json::to_string(&params).unwrap_or_default();
                let chunk = self.make_chunk(
                    Delta {
                        content: None,
                        role: None,
                        tool_calls: Some(vec![DeltaToolCall {
                            index: 0,
                            id: Some(tool_id),
                            r#type: Some("function".to_string()),
                            function: Some(DeltaFunction {
                                name: Some(tool_name),
                                arguments: Some(arguments),
                            }),
                        }]),
                    },
                    None,
                );
                Some(stream::sse_data(&chunk))
            }

            StreamEvent::RunComplete { summary, .. } => {
                // Emit a final chunk with finish_reason "stop", then [DONE]
                let chunk = self.make_chunk(
                    Delta {
                        content: None,
                        role: None,
                        tool_calls: None,
                    },
                    Some("stop".to_string()),
                );
                let usage_chunk = ChatCompletionChunk {
                    usage: Some(Usage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        total_tokens: summary.total_tokens as u32,
                    }),
                    ..self.make_chunk(
                        Delta {
                            content: None,
                            role: None,
                            tool_calls: None,
                        },
                        Some("stop".to_string()),
                    )
                };
                let frame = format!(
                    "{}{}{}",
                    stream::sse_data(&chunk),
                    stream::sse_data(&usage_chunk),
                    SSE_DONE
                );
                // Send directly — we combine stop + usage + DONE
                self.tx
                    .send(frame)
                    .await
                    .map_err(|_| EventEmitError::ChannelClosed)?;
                return Ok(());
            }

            StreamEvent::RunError { error, .. } => {
                let error_json = serde_json::json!({
                    "error": {
                        "message": error,
                        "type": "server_error",
                    }
                });
                let frame = format!("data: {error_json}\n\n{SSE_DONE}");
                self.tx
                    .send(frame)
                    .await
                    .map_err(|_| EventEmitError::ChannelClosed)?;
                return Ok(());
            }

            // Suppress non-content events
            StreamEvent::Reasoning { .. } => None,
            StreamEvent::ReasoningBlock { .. } => None,
            StreamEvent::ToolEnd { .. } => None,
            StreamEvent::ToolUpdate { .. } => None,
            StreamEvent::RunAccepted { .. } => None,
            StreamEvent::AskUser { .. } => None,
            StreamEvent::UncertaintySignal { .. } => None,
            StreamEvent::SessionUpdated { .. } => None,
        };

        if let Some(data) = frame {
            self.tx
                .send(data)
                .await
                .map_err(|_| EventEmitError::ChannelClosed)?;
        }

        Ok(())
    }
}

// =============================================================================
// CollectingEmitter — collects events for non-streaming responses
// =============================================================================

/// An [`EventEmitter`] that collects all events into a vector for building
/// a single non-streaming response.
struct CollectingEmitter {
    events: tokio::sync::Mutex<Vec<StreamEvent>>,
    seq: std::sync::atomic::AtomicU64,
}

impl CollectingEmitter {
    fn new() -> Self {
        Self {
            events: tokio::sync::Mutex::new(Vec::new()),
            seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    async fn take_events(&self) -> Vec<StreamEvent> {
        std::mem::take(&mut *self.events.lock().await)
    }
}

#[async_trait]
impl EventEmitter for CollectingEmitter {
    fn next_seq(&self) -> u64 {
        self.seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.events.lock().await.push(event);
        Ok(())
    }
}

// =============================================================================
// handle() — main agent completions handler
// =============================================================================

/// Handle an agent-mode completion request (`model: "aleph/..."`)
pub async fn handle(
    state: Arc<OpenAiApiState>,
    headers: &HeaderMap,
    req: ChatCompletionRequest,
) -> Result<Response, ApiError> {
    // 1. Get execution adapter
    let adapter = state
        .execution_adapter
        .as_ref()
        .ok_or_else(|| {
            ApiError::ServiceUnavailable("Execution adapter not available".into())
        })?
        .clone();

    // 2. Get agent registry
    let registry = state
        .agent_registry
        .as_ref()
        .ok_or_else(|| {
            ApiError::ServiceUnavailable("Agent registry not available".into())
        })?
        .clone();

    // 3. Parse agent_id from model name: "aleph/iris" → "iris", "aleph/default" → default
    let suffix = req
        .model
        .strip_prefix("aleph/")
        .unwrap_or("default");

    let agent = if suffix == "default" {
        registry.get_default().await
    } else {
        registry.get(suffix).await
    }
    .ok_or_else(|| ApiError::NotFound(format!("Agent '{}' not found", suffix)))?;

    // 4. Peer ID from header or fallback
    let peer_id = headers
        .get("x-aleph-user")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("openai-api-client")
        .to_string();

    // 5. Extract input: last user message content
    let input = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default();

    // 6. Build session key and run request
    let agent_id = agent.id().to_string();
    let session_key = SessionKey::PerPeer {
        agent_id: agent_id.clone(),
        peer_id: peer_id.clone(),
        epoch: 0,
    };

    let run_id = uuid::Uuid::new_v4().to_string();
    let pending_media: PendingMedia = Arc::new(std::sync::Mutex::new(Vec::new()));

    let run_request = RunRequest {
        run_id: run_id.clone(),
        input,
        session_key,
        timeout_secs: None,
        metadata: HashMap::new(),
        attachments: Vec::new(),
        pending_media,
    };

    let is_streaming = req.stream.unwrap_or(false);

    if is_streaming {
        handle_streaming(adapter, agent, run_request, req.model).await
    } else {
        handle_non_streaming(adapter, agent, run_request, req.model).await
    }
}

/// Streaming response — spawn execution in background, return SSE body.
async fn handle_streaming(
    adapter: Arc<dyn ExecutionAdapter>,
    agent: Arc<crate::gateway::agent_instance::AgentInstance>,
    run_request: RunRequest,
    model: String,
) -> Result<Response, ApiError> {
    let (tx, mut rx) = mpsc::channel::<String>(256);

    let cid = stream::completion_id();
    let created = stream::now_timestamp();

    // Send initial role chunk
    let initial_chunk = ChatCompletionChunk {
        id: cid.clone(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.clone(),
        choices: vec![StreamChoice {
            index: 0,
            delta: Delta {
                content: None,
                role: Some("assistant".to_string()),
                tool_calls: None,
            },
            finish_reason: None,
        }],
        usage: None,
    };
    // Best-effort send of initial chunk
    let _ = tx.send(stream::sse_data(&initial_chunk)).await;

    let emitter = Arc::new(SseEventEmitter::new(tx, cid, model.clone(), created));

    // Spawn execution in background
    tokio::spawn(async move {
        if let Err(e) = adapter.execute(run_request, agent, emitter.clone()).await {
            let error_json = serde_json::json!({
                "error": {
                    "message": e.to_string(),
                    "type": "server_error",
                }
            });
            let frame = format!("data: {error_json}\n\n{SSE_DONE}");
            let _ = emitter.tx.send(frame).await;
        }
    });

    // Build SSE body from receiver
    let body_stream = async_stream::stream! {
        while let Some(frame) = rx.recv().await {
            yield Ok::<_, std::convert::Infallible>(frame);
        }
    };

    let body = Body::from_stream(body_stream);
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(body)
        .unwrap()
        .into_response())
}

/// Non-streaming response — execute, collect events, return JSON.
async fn handle_non_streaming(
    adapter: Arc<dyn ExecutionAdapter>,
    agent: Arc<crate::gateway::agent_instance::AgentInstance>,
    run_request: RunRequest,
    model: String,
) -> Result<Response, ApiError> {
    let emitter = Arc::new(CollectingEmitter::new());

    adapter
        .execute(run_request, agent, emitter.clone())
        .await
        .map_err(|e| ApiError::BadGateway(format!("Agent execution error: {e}")))?;

    let events = emitter.take_events().await;

    // Accumulate content from ResponseChunk events
    let mut content = String::new();
    let mut total_tokens: u64 = 0;

    for event in &events {
        match event {
            StreamEvent::ResponseChunk {
                content: chunk, ..
            } => {
                content.push_str(chunk);
            }
            StreamEvent::RunComplete { summary, .. } => {
                total_tokens = summary.total_tokens;
            }
            _ => {}
        }
    }

    let response = ChatCompletionResponse {
        id: stream::completion_id(),
        object: "chat.completion".to_string(),
        created: stream::now_timestamp(),
        model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                tool_calls: None,
            },
            finish_reason: Some("stop".to_string()),
            delta: None,
        }],
        usage: Some(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: total_tokens as u32,
        }),
    };

    Ok(Json(response).into_response())
}

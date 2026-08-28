//! Agent completions path — routes `aleph/*` models through `ExecutionAdapter`.
//!
//! Translates between `OpenAI` Chat Completions format and the internal
//! agent execution pipeline, using an `EventEmitter` that converts
//! `StreamEvent`s into SSE frames.

use std::collections::HashMap;

use crate::sync_primitives::{Arc, Mutex};
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
    seq: crate::sync_primitives::AtomicU64,
    tool_tracker: Mutex<stream::ToolCallTracker>,
}

impl SseEventEmitter {
    fn new(tx: mpsc::Sender<String>, completion_id: String, model: String, created: u64) -> Self {
        Self {
            tx,
            completion_id,
            model,
            created,
            seq: crate::sync_primitives::AtomicU64::new(0),
            tool_tracker: Mutex::new(stream::ToolCallTracker::default()),
        }
    }

    /// Build a `ChatCompletionChunk` with the given delta and optional finish reason.
    fn make_chunk(&self, delta: Delta, finish_reason: Option<String>) -> ChatCompletionChunk {
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
        self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let frame: Option<String> = match event {
            StreamEvent::ResponseChunk { delta, .. } => {
                let chunk = self.make_chunk(
                    Delta {
                        content: Some(delta),
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
                let idx = self
                    .tool_tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .index_for(&tool_id);
                let arguments = match &params {
                    serde_json::Value::String(s) => s.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                let chunk = self.make_chunk(
                    Delta {
                        content: None,
                        role: None,
                        tool_calls: Some(vec![DeltaToolCall {
                            index: idx,
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
                // Single final chunk with finish_reason + usage + [DONE]
                let chunk = ChatCompletionChunk {
                    usage: if summary.total_tokens > 0 {
                        Some(Usage {
                            prompt_tokens: 0,
                            completion_tokens: 0,
                            total_tokens: u32::try_from(summary.total_tokens).unwrap_or(u32::MAX),
                        })
                    } else {
                        None
                    },
                    ..self.make_chunk(
                        Delta {
                            content: None,
                            role: None,
                            tool_calls: None,
                        },
                        Some("stop".to_string()),
                    )
                };
                let frame = format!("{}{}", stream::sse_data(&chunk), SSE_DONE);
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
            StreamEvent::AgentTrace { .. } => None,
            StreamEvent::RunAccepted { .. } => None,
            // The OpenAI Chat Completions wire format has no queueing
            // concept, so a queued run produces no chunk on this surface —
            // deliberate suppression, not a missing producer. Panel is the
            // one client that renders this frame today (`ChatPhase::Queued`).
            StreamEvent::RunQueued { .. } => None,
            StreamEvent::AskUser { .. } => None,
            StreamEvent::UncertaintySignal { .. } => None,
            StreamEvent::ModelResolved { .. } => None,
            StreamEvent::ContextGauge { .. } => None,
            StreamEvent::RunRetrying { .. } => None,
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
        self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
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
        .ok_or_else(|| ApiError::ServiceUnavailable("Execution adapter not available".into()))?
        .clone();

    // 2. Get agent registry
    let registry = state
        .agent_registry
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Agent registry not available".into()))?
        .clone();

    // 3. Parse agent_id from model name: "aleph/iris" → "iris", "aleph/default" → default
    let suffix = req.model.strip_prefix("aleph/").unwrap_or("default");

    let agent = if suffix == "default" {
        registry.get_default().await
    } else {
        registry.get(suffix).await
    }
    .ok_or_else(|| ApiError::NotFound(format!("Agent '{suffix}' not found")))?;

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

    // 6. Build session key.
    // OpenAI stateless semantics: when `messages[]` carries multi-turn
    // history, the client is authoritative. Route through a fresh ephemeral
    // session so the persisted peer session does not leak prior server-side
    // context into the reply. Single-message requests keep the stable peer
    // session for continuity across successive calls.
    let agent_id = agent.id().to_string();
    let session_key = if req.messages.len() > 1 {
        SessionKey::Ephemeral {
            agent_id: agent_id.clone(),
            ephemeral_id: uuid::Uuid::new_v4().to_string(),
        }
    } else {
        SessionKey::peer(&agent_id, &peer_id)
    };

    // Seed the ephemeral session with the client-supplied prior history so
    // the agent sees the conversation the client intended.
    // Task 7a: emit via SessionService instead of direct messages-table write.
    if req.messages.len() > 1 {
        agent.ensure_session(&session_key).await;
        for msg in &req.messages[..req.messages.len() - 1] {
            if let Some(content) = &msg.content {
                let turn_id = uuid::Uuid::new_v4();
                let at = crate::session::events::now_ms();
                let mc = crate::session::events::MessageContent {
                    text: content.clone(),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                };
                let event = match msg.role.as_str() {
                    "user" => crate::session::events::SessionEvent::UserMessage {
                        turn_id,
                        content: mc,
                        at,
                        synthetic: false,
                        author_user_id: None,
                    },
                    "assistant" => crate::session::events::SessionEvent::AssistantMessage {
                        turn_id,
                        content: mc,
                        // Client-supplied history from an OpenAI-compat request —
                        // we did not make this call and were not billed for it.
                        usage: None,
                        at,
                    },
                    "system" => crate::session::events::SessionEvent::SystemMessage {
                        turn_id,
                        content: content.clone(),
                        at,
                    },
                    _ => continue,
                };
                if let Some(svc) = crate::session::service::global_session_service() {
                    let _ = svc.emit_event(&session_key, event).await;
                } else {
                    // Same class as the main-path producers (Task 7a): this is
                    // the sole writer for client-supplied history on the
                    // OpenAI-compat path. No handle means this replayed
                    // message never reaches `messages`.
                    tracing::warn!(
                        session_key = %session_key,
                        role = msg.role.as_str(),
                        "session/service capability absent; dropped a replayed history message — see `aleph doctor`"
                    );
                }
            }
        }
    }

    let run_id = uuid::Uuid::new_v4().to_string();
    let pending_media: PendingMedia = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // OpenAI-compat shim is for third-party SDKs (LangChain, LiteLLM, plain
    // `openai.ChatCompletion.create(...)`), none of which know about
    // Aleph's project picker. Clients that want project-scoped execution
    // should use the JSON-RPC `chat.send` endpoint (which carries
    // `project_root`) instead of the OpenAI shim.
    let run_request = RunRequest {
        run_id: run_id.clone(),
        input,
        session_key,
        timeout_secs: None,
        metadata: busy_input_metadata(),
        attachments: Vec::new(),
        pending_media,
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
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
        .map_err(|e| ApiError::BadGateway(format!("failed to build response: {e}")))?
        .into_response())
}

/// Busy-input policy for the OpenAI-compat surface.
///
/// A synchronous request/response call cannot be answered by somebody else's
/// run, so it must never be steered. The engine's default when the key is
/// absent is `Steer`, and this surface sent no metadata at all — while the
/// single-message session key is *stable* (`SessionKey::peer`), so every client
/// that omits `x-aleph-user` shares one. Two overlapping calls therefore folded
/// the second one's text into the first one's run and answered it with HTTP 200
/// and an empty completion: a successful-looking reply to a question that had
/// silently been given to someone else.
///
/// # On the queue-vs-parallel trade-off (reviewed; intentional)
///
/// The audit suggested keying each request on a per-request UUID so two
/// in-flight calls with the same `x-aleph-user` run in PARALLEL instead of
/// queuing. That is deliberately NOT done here: the stable peer key is what
/// gives a single-message client conversational continuity across successive
/// calls (the session keeps its transcript), and a per-request UUID would
/// silently drop that — every call would start a fresh, empty session. The
/// queue is the price of keeping BOTH correctness (no cross-run text
/// folding) and continuity. A client that genuinely wants parallel,
/// independent sessions opts in explicitly by sending a DISTINCT
/// `x-aleph-user` value per request (e.g. a UUID) — the peer id is read
/// from that header, so the parallel-session behavior is already available
/// to any client that asks for it, without changing the safe default for
/// the single-threaded caller the default serves.
fn busy_input_metadata() -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert(
        crate::gateway::execution_engine::BUSY_INPUT_MODE_KEY.to_string(),
        "queue".to_string(),
    );
    metadata
}

#[cfg(test)]
mod busy_input_tests {
    use super::busy_input_metadata;
    use crate::gateway::execution_engine::BusyInputMode;

    /// Asserts what the engine actually resolves, not that a key was written:
    /// the wire spelling and the default both live in `BusyInputMode`.
    #[test]
    fn an_openai_compat_call_is_queued_never_steered() {
        assert!(matches!(
            BusyInputMode::from_metadata(&busy_input_metadata()),
            BusyInputMode::Queue
        ));
        // The bug this guards: absent metadata resolves to Steer.
        assert!(matches!(
            BusyInputMode::from_metadata(&std::collections::HashMap::new()),
            BusyInputMode::Steer
        ));
    }
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
            StreamEvent::ResponseChunk { delta, .. } => {
                content.push_str(delta);
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
                tool_call_id: None,
            },
            finish_reason: Some("stop".to_string()),
            delta: None,
        }],
        usage: if total_tokens > 0 {
            Some(Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: u32::try_from(total_tokens).unwrap_or(u32::MAX),
            })
        } else {
            None
        },
    };

    Ok(Json(response).into_response())
}

// =============================================================================
// Tests — OpenAI-compat SSE regression guards (Task 6)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::RunSummary;
    use tokio::sync::mpsc;

    fn emitter(tx: mpsc::Sender<String>) -> SseEventEmitter {
        SseEventEmitter::new(tx, "cmpl-test".to_string(), "test-model".to_string(), 0)
    }

    /// The SSE surface forwards a ResponseChunk's `delta` verbatim as the
    /// OpenAI `choices[].delta.content`. Guards against re-introducing a raw
    /// `content` alias read or dropping the delta (G1 regression guard). The
    /// upstream drain (Task 3) is what strips `<think>`; this surface only
    /// forwards, so we feed already-clean text and assert verbatim forwarding.
    #[tokio::test]
    async fn sse_forwards_response_chunk_delta_as_content() {
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let em = emitter(tx);
        em.emit(StreamEvent::ResponseChunk {
            run_id: "r".to_string(),
            seq: 0,
            delta: "visible answer".to_string(),
            full_text: "visible answer".to_string(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        })
        .await
        .expect("emit ok");
        drop(em); // close tx so rx.recv() terminates
        let frame = rx.recv().await.expect("one SSE frame");
        assert!(
            frame.contains("visible answer"),
            "delta must be forwarded as SSE content: {frame:?}"
        );
    }

    /// Reasoning is never forwarded to the OpenAI SSE surface — the reasoning
    /// leak guard for this surface (G4). Emitting a Reasoning event produces
    /// no SSE frame at all.
    #[tokio::test]
    async fn sse_suppresses_reasoning_events() {
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let em = emitter(tx);
        em.emit(StreamEvent::Reasoning {
            run_id: "r".to_string(),
            seq: 0,
            content: "SECRET internal reasoning".to_string(),
            is_complete: false,
        })
        .await
        .expect("emit ok");
        drop(em);
        assert!(
            rx.recv().await.is_none(),
            "reasoning must not reach the SSE surface"
        );
    }

    /// The terminal RunComplete frame carries finish_reason=stop and NO text —
    /// summary.final_response must never leak into the SSE stream (the text was
    /// already streamed as clean deltas).
    #[tokio::test]
    async fn sse_run_complete_carries_no_summary_text() {
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let em = emitter(tx);
        em.emit(StreamEvent::RunComplete {
            run_id: "r".to_string(),
            seq: 0,
            summary: RunSummary {
                final_response: Some("SHOULD_NOT_LEAK".to_string()),
                ..Default::default()
            },
            total_duration_ms: 0,
        })
        .await
        .expect("emit ok");
        drop(em);
        let frame = rx.recv().await.expect("final SSE frame");
        assert!(
            frame.contains("stop"),
            "finish_reason=stop present: {frame:?}"
        );
        assert!(
            !frame.contains("SHOULD_NOT_LEAK"),
            "summary text must not leak into the SSE terminal frame: {frame:?}"
        );
    }
}

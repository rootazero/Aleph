//! Public helpers exposed for the `gateway_chat_*` integration tests.
//!
//! `run_dispatch_and_drain` is the minimum viable surface that Task 4c's
//! 6 integration tests exercise. It composes:
//!   1. `Orchestrator::dispatch` with a caller-built `FlowRequest`.
//!   2. A drain task that forwards `FlowStreamEvent`s through
//!      `event_drain::emit_flow_event` to the Gateway emitter.
//!   3. Await of the completion oneshot + mapping of `FlowOutcome` to the
//!      user-visible response string (including i18n `ErrLoopExhausted`
//!      parity with the retiring `run_agent_loop` branch).
//!
//! The production `run_agent_loop` wraps this helper inside the provider
//! fallback retry loop; this module is the inner per-attempt dispatch body.

use crate::sync_primitives::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::event_drain::{emit_flow_event, DrainState};
use super::ExecutionError;
use crate::gateway::event_emitter::EventEmitter;
use crate::gateway::i18n::{self, Locale, Msg};
use crate::orchestrator::{
    FlowError, FlowHistoryTurn, FlowInput, FlowOutcome, FlowRequest, FlowStreamEvent, Orchestrator,
};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::session::events::MessageContent;

/// Convert a `Vec<UnifiedMessage>` loop-history into an `orchestrator::FlowInput::History`.
///
/// The harness's `seed_session` replays each turn as the corresponding session
/// event before emitting the trailing user prompt. Only text content survives
/// this conversion today — multimodal blocks (images, tool_calls) are carried
/// separately via the `Multimodal(Vec<MessageContent>)` variant.
///
/// `prompt` is the fresh user message that triggered this run.
pub fn history_to_flow_input(history: Vec<UnifiedMessage>, prompt: String) -> FlowInput {
    let turns: Vec<FlowHistoryTurn> = history
        .into_iter()
        .filter_map(|msg| match msg {
            UnifiedMessage::User { content } => Some(FlowHistoryTurn::User(MessageContent {
                text: extract_text(&content),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            })),
            UnifiedMessage::Assistant { content } => {
                Some(FlowHistoryTurn::Assistant(MessageContent {
                    text: extract_text(&content),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                }))
            }
            // ToolResult messages are part of the Think→Act cadence; the
            // harness re-emits them from its own session log on replay so we
            // do not reproject them into history turns here.
            _ => None,
        })
        .collect();
    FlowInput::History { turns, prompt }
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Outcome classification for the dispatch path. Lets the outer `run_agent_loop`
/// retry loop distinguish transient provider failures (eligible for fallback
/// retry) from terminal errors.
#[derive(Debug)]
pub enum DispatchFailure {
    /// Transient provider failure. Caller may retry with a different provider.
    Transient { provider: String, message: String },
    /// Flow-level cancellation (caller or deadline).
    Cancelled,
    /// Any non-transient dispatch failure.
    Fatal(String),
}

impl std::fmt::Display for DispatchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient { provider, message } => {
                write!(f, "transient harness error ({provider}): {message}")
            }
            Self::Cancelled => write!(f, "flow dispatch cancelled"),
            Self::Fatal(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<DispatchFailure> for ExecutionError {
    fn from(err: DispatchFailure) -> Self {
        match err {
            DispatchFailure::Cancelled => ExecutionError::Cancelled,
            other => ExecutionError::Orchestrator(other.to_string()),
        }
    }
}

/// Dispatch one chat turn through the Orchestrator and drain events to the
/// supplied emitter. Returns the final user-visible response string.
///
/// # Behaviour
/// * Calls `orchestrator.dispatch(req)` and spawns a drain task consuming
///   the event stream.
/// * Awaits the completion oneshot; maps `FlowError` to `ExecutionError::Orchestrator`
///   (callers classify `FlowError::Transient` vs `Internal` separately for
///   the outer retry loop).
/// * Maps `FlowOutcome.hit_limit` + empty `final_text` to i18n
///   `ErrLoopExhausted`; otherwise returns `final_text`.
///
/// # Cancellation
/// `cancel_token.cancel()` is forwarded to the orchestrator by the caller via
/// the `FlowHandle.cancel` token the dispatch returns. When the cancel arrives
/// the harness returns `FlowError::Cancelled` which surfaces here as
/// `ExecutionError::Orchestrator("flow: flow dispatch cancelled")`.
pub async fn run_dispatch_and_drain(
    orchestrator: Arc<Orchestrator>,
    req: FlowRequest,
    emitter: Arc<dyn EventEmitter>,
    run_id: &str,
    cancel_token: CancellationToken,
    locale: Locale,
) -> Result<String, ExecutionError> {
    run_dispatch_and_drain_classified(orchestrator, req, emitter, run_id, cancel_token, locale)
        .await
        .map_err(ExecutionError::from)
}

/// Same as `run_dispatch_and_drain` but returns a classified `DispatchFailure`
/// so the outer retry loop can distinguish retryable transient errors.
/// Used by `run_agent_loop` in production; integration tests use the simpler
/// `run_dispatch_and_drain` wrapper above.
pub async fn run_dispatch_and_drain_classified(
    orchestrator: Arc<Orchestrator>,
    req: FlowRequest,
    emitter: Arc<dyn EventEmitter>,
    run_id: &str,
    cancel_token: CancellationToken,
    locale: Locale,
) -> Result<String, DispatchFailure> {
    // 1. Dispatch.
    let handle = orchestrator.dispatch(req).await.map_err(map_flow_error)?;

    // Wire the caller's cancel token to the orchestrator's handle token so
    // `cancel_token.cancel()` propagates into the harness run.
    let handle_cancel = handle.cancel.clone();
    let propagate = tokio::spawn({
        let outer = cancel_token.clone();
        async move {
            outer.cancelled().await;
            handle_cancel.cancel();
        }
    });

    // 2. Drain task.
    let drain_state = Arc::new(Mutex::new(DrainState::default()));
    let drain = tokio::spawn({
        let emitter = emitter.clone();
        let run_id = run_id.to_string();
        let drain_state = drain_state.clone();
        let cancel = cancel_token.clone();
        let mut events = handle.events;
        async move {
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                match events.recv().await {
                    Ok(event) => {
                        let is_complete = matches!(event, FlowStreamEvent::Complete(_));
                        if let Err(e) =
                            emit_flow_event(event, &emitter, &run_id, &drain_state).await
                        {
                            warn!(run_id = %run_id, error = %e, "emit_flow_event failed");
                        }
                        if is_complete {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, run_id = %run_id, "orchestrator stream lagged; dropping frames");
                    }
                }
            }
        }
    });

    // 3. Await completion.
    let outcome: FlowOutcome = handle
        .completion
        .await
        .map_err(|e| DispatchFailure::Fatal(format!("completion dropped: {e}")))?
        .map_err(map_flow_error)?;

    // 4. Let the drain task finish forwarding any in-flight events.
    let _ = drain.await;
    propagate.abort();

    // 5. Map `hit_limit` → i18n `ErrLoopExhausted` (parity with run_agent_loop).
    let response = if outcome.hit_limit && outcome.final_text.is_empty() {
        i18n::t(
            Msg::ErrLoopExhausted {
                iterations: outcome.iterations as usize,
                tool_calls: outcome.tool_calls_made as usize,
            },
            locale,
        )
    } else {
        outcome.final_text
    };
    Ok(response)
}

fn map_flow_error(err: FlowError) -> DispatchFailure {
    match err {
        FlowError::Cancelled => DispatchFailure::Cancelled,
        FlowError::Transient { provider, message } => {
            DispatchFailure::Transient { provider, message }
        }
        other => DispatchFailure::Fatal(format!("flow: {other}")),
    }
}

//! Public helpers exposed for the `gateway_chat_*` integration tests.
//!
//! `run_dispatch_and_drain` is the minimum viable surface that Task 4c's
//! 6 integration tests exercise. It composes:
//!   1. `Orchestrator::dispatch` with a caller-built `FlowRequest`.
//!   2. A drain task that forwards `FlowStreamEvent`s through
//!      `event_drain::emit_flow_event` to the Gateway emitter.
//!   3. Await of the completion oneshot + mapping of `FlowOutcome` to the
//!      user-visible response string (loop-halt branches go through
//!      `i18n::render_loop_halt`, which dispatches on `terminate_reason`).
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
use crate::gateway::i18n::{self, Locale};
use crate::orchestrator::{
    FlowError, FlowHistoryTurn, FlowInput, FlowOutcome, FlowRequest, FlowStreamEvent, Orchestrator,
};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::session::events::MessageContent;

/// What a completed run is worth remembering, captured from the authoritative
/// [`FlowOutcome`] so the gateway can persist it onto the assistant message and
/// the session row. The Panel re-projects the occupancy onto its gauge when a
/// session is reloaded from history — the gauge no longer depends on a live
/// `run_complete` event surviving in memory.
#[derive(Debug, Clone)]
pub struct RunContextOccupancy {
    /// Current window occupancy (provider-reported prompt tokens of the last call).
    pub context_tokens: u32,
    /// The model's authoritative context window (core's lookup, honoring overrides).
    pub context_window: u32,
    /// Run-cumulative token total — rides along for the gauge tooltip.
    pub total_tokens: u64,
    /// Prompt tokens this run actually spent, for the session's cumulative
    /// counters. Distinct from `context_tokens`, which is an occupancy SNAPSHOT
    /// of the last call, not a sum.
    pub input_tokens: u32,
    /// Completion tokens this run actually spent.
    pub output_tokens: u32,
    /// This run's cost in USD, or `None` when the price table could not produce
    /// one (unknown provider/model). `None` is NOT zero: the session's cumulative
    /// total accumulates only what it can actually price, so an unpriced run
    /// leaves the total honest-but-incomplete rather than silently understated by
    /// a fabricated $0.00.
    pub cost_usd: Option<f64>,
    /// Model that served this run, and the provider that served it — the same
    /// pair the cost estimate is keyed on. Recorded onto `sessions.model` /
    /// `sessions.model_provider`, whose only writer was the dead
    /// `LlmCallStarted` event and which therefore read back NULL forever.
    pub model: Option<String>,
    /// Provider id behind [`model`](Self::model).
    pub model_provider: Option<String>,
}

/// Convert a `Vec<UnifiedMessage>` loop-history into an `orchestrator::FlowInput::History`.
///
/// The harness's `seed_session` replays each turn as the corresponding session
/// event before emitting the trailing user prompt. Only text content survives
/// this conversion — a turn carrying attachments goes through
/// `FlowInput::Multimodal` instead, which seeds the new user message with its
/// image blocks (see `run_loop::inner`).
///
/// `prompt` is the fresh user message that triggered this run.
#[must_use]
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
            DispatchFailure::Cancelled => Self::Cancelled,
            other => Self::Orchestrator(other.to_string()),
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
/// * Maps `FlowOutcome.hit_limit` + empty `final_text` through
///   `i18n::render_loop_halt(terminate_reason, ...)`; otherwise returns
///   `final_text`.
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
    // Integration-test wrapper: occupancy persistence is the production loop's
    // concern, so discard it through a throwaway slot.
    let occupancy = std::sync::Mutex::new(None);
    run_dispatch_and_drain_classified(
        orchestrator,
        req,
        emitter,
        run_id,
        cancel_token,
        locale,
        &occupancy,
    )
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
    occupancy_out: &std::sync::Mutex<Option<RunContextOccupancy>>,
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

    // 2. Drain task. Resolves to `true` when the terminal `Complete` event
    //    was forwarded to the emitter, so step 4 can emit a safety-net
    //    `RunComplete` in the rare case the broadcast channel lagged past
    //    the `Complete` frame (or emitting it failed).
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
                    return false;
                }
                match events.recv().await {
                    Ok(event) => {
                        let is_complete = matches!(event, FlowStreamEvent::Complete(_));
                        match emit_flow_event(event, &emitter, &run_id, &drain_state).await {
                            Ok(()) => {
                                if is_complete {
                                    return true;
                                }
                            }
                            Err(e) => {
                                warn!(run_id = %run_id, error = %e, "emit_flow_event failed");
                                if is_complete {
                                    return false;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return false,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, run_id = %run_id, "orchestrator stream lagged; dropping frames");
                    }
                }
            }
        }
    });

    // 3. Await completion. The propagate task must be reaped on the error
    //    paths too: it waits on a cancel token that is never cancelled for a
    //    Transient/Fatal failure, so an early `?` return here would leak one
    //    task (pinned forever on `cancelled()`) per failed attempt.
    let outcome: FlowOutcome = match handle.completion.await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => {
            propagate.abort();
            return Err(map_flow_error(e));
        }
        Err(e) => {
            propagate.abort();
            return Err(DispatchFailure::Fatal(format!("completion dropped: {e}")));
        }
    };

    // Capture what this run is worth remembering: the authoritative
    // context-window occupancy (so the gauge survives a session reload) and the
    // tokens + USD it spent (so the session's cumulative counters are real).
    //
    // The gauge fields require a real LLM call AND a resolved window; the spend
    // fields only require that tokens moved. A run that spent tokens without a
    // usable window still records its spend — otherwise the session cost would
    // quietly skip it. A run that did nothing at all leaves the slot untouched,
    // so the gauge stays hidden. On a provider-fallback retry the last
    // successful attempt overwrites the slot.
    let spent = outcome.token_breakdown != crate::orchestrator::dispatch::TokenBreakdown::default();
    if (outcome.context_tokens > 0 && outcome.context_window > 0) || spent {
        if let Ok(mut slot) = occupancy_out.lock() {
            *slot = Some(RunContextOccupancy {
                context_tokens: outcome.context_tokens,
                context_window: outcome.context_window,
                total_tokens: u64::from(outcome.total_tokens),
                input_tokens: outcome.token_breakdown.input,
                output_tokens: outcome.token_breakdown.output,
                // `CostStatus::Unknown` ⇒ no estimate exists. Record `None`, not
                // 0.0 — see the field doc.
                cost_usd: outcome
                    .estimated_cost
                    .as_ref()
                    .and_then(|c| match c.status {
                        crate::pricing::CostStatus::Complete
                        | crate::pricing::CostStatus::PartialMissingPrice => Some(c.usd),
                        crate::pricing::CostStatus::Unknown => None,
                    }),
                model: outcome.serving_model.clone(),
                model_provider: outcome.serving_provider.clone(),
            });
        }
    }

    // 4. Let the drain task finish forwarding any in-flight events. The
    //    drain is the single source of the terminal `RunComplete` (the
    //    degenerate duplicate previously emitted by `ExecutionEngine::
    //    execute` is gone); if the drain missed the `Complete` frame,
    //    re-emit it here from the authoritative `FlowOutcome` so channel
    //    and Panel consumers still observe the run end — with the full
    //    enriched summary instead of an all-zeros placeholder.
    let complete_forwarded = drain.await.unwrap_or(false);
    propagate.abort();
    if !complete_forwarded {
        // No plan: this fallback fires precisely when the drain never saw the
        // `Complete` frame, so its latched execution list is unavailable here.
        let summary = super::event_drain::build_run_summary(&outcome, None);
        let seq = emitter.next_seq();
        // Run-skeleton event → `warn` on send failure, matching the emit-tier
        // policy the trait's default `emit_run_complete` applies (decorative
        // Reasoning/Tool* events stay at `debug`). This second `RunComplete`
        // producer used to discard the error, so the one path where the
        // terminal frame is *most* likely to be missed failed invisibly.
        if let Err(e) = emitter
            .emit(crate::gateway::event_emitter::StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq,
                summary,
                total_duration_ms: outcome.duration_ms,
            })
            .await
        {
            tracing::warn!(
                run_id,
                error = %e,
                "failed to emit the drain-fallback RunComplete stream event"
            );
        }
    }

    // 5. Map `hit_limit` → i18n loop-halt message, dispatched on the rich
    //    `terminate_reason` so the user gets root-cause-specific advice
    //    instead of a one-size-fits-all "raise max_iterations" blob.
    let response = if outcome.hit_limit && outcome.final_text.is_empty() {
        i18n::render_loop_halt(
            &outcome.terminate_reason,
            outcome.iterations as usize,
            outcome.tool_calls_made as usize,
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

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

use std::time::Duration;

use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::event_drain::{
    emit_flow_event, emit_held_complete, hold_complete, DrainState, HeldComplete,
};
use super::ExecutionError;
use crate::gateway::event_emitter::EventEmitter;
use crate::gateway::i18n::{self, Locale};
use crate::orchestrator::{
    FlowError, FlowHistoryTurn, FlowInput, FlowOutcome, FlowRequest, FlowStreamEvent, Orchestrator,
};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::session::events::MessageContent;

/// How long the `completion dropped` arm waits for the drain to hand back the
/// terminal frame it prepared.
///
/// Only that arm is bounded. The two ordinary arms join the drain after the
/// flow task has finished, so its event channel is on its way closed; this one
/// runs precisely when the task died without answering, and a broadcast sender
/// held somewhere else would otherwise park the join — and with it the run's
/// last word — indefinitely.
const TERMINAL_DRAIN_GRACE: Duration = Duration::from_secs(2);

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

/// The `ExecutionError` a dispatch failure presents as.
///
/// Split out from the `From` impl below because the pre-outcome receipt path
/// (`emit_error_run_complete`) only holds a borrow, and a second `match` there
/// would be a second answer to "which receipt does this failure earn" — free to
/// drift from this one.
fn execution_error_for(err: &DispatchFailure) -> ExecutionError {
    match err {
        DispatchFailure::Cancelled => ExecutionError::Cancelled,
        other => ExecutionError::Orchestrator(other.to_string()),
    }
}

impl From<DispatchFailure> for ExecutionError {
    fn from(err: DispatchFailure) -> Self {
        execution_error_for(&err)
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
/// the harness returns `FlowError::Cancelled`, which `map_flow_error` turns
/// into `DispatchFailure::Cancelled` and `execution_error_for` into
/// `ExecutionError::Cancelled` — NOT the `Orchestrator(..)` string this line
/// used to claim, which mattered because `Cancelled` is the one arm that earns
/// its own receipt (`ReceiptKind::Cancelled`) rather than the generic one.
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
        // Never withholds the terminal frame: this wrapper has no retry loop
        // above it, so an attempt of its is always the run's last word.
        false,
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
    may_retry: bool,
) -> Result<String, DispatchFailure> {
    // Session this run's provider dials are witnessed under. Captured before
    // `req` moves into the dispatch; the drain below reads the witness at the
    // terminal frame, and the second copy serves the path where the drain never
    // saw that frame.
    // rust-doctor-disable-next-line excessive-clone
    let witness_session = req.session_hint.clone();
    // rust-doctor-disable-next-line excessive-clone
    let witness_session_for_fallback = witness_session.clone();

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

    // 2. Drain task. Resolves to the *prepared but unsent* terminal frame
    //    (`Some`) when the harness broadcast `Complete`, and to `None` when it
    //    never did — so step 4 can emit a safety-net `RunComplete` in the rare
    //    case the broadcast channel lagged past that frame.
    //
    //    It stops short of sending because the decision is not the drain's to
    //    make: a failed attempt the outer loop is about to retry must not
    //    broadcast a terminal frame at all, and that classification does not
    //    exist until `handle.completion` has resolved below. See
    //    [`HeldComplete`].
    let drain_state = Arc::new(Mutex::new(DrainState::default()));
    let drain: tokio::task::JoinHandle<Option<HeldComplete>> = tokio::spawn({
        let emitter = emitter.clone();
        let run_id = run_id.to_string();
        let drain_state = drain_state.clone();
        let cancel = cancel_token.clone();
        let mut events = handle.events;
        async move {
            loop {
                if cancel.is_cancelled() {
                    return None;
                }
                match events.recv().await {
                    Ok(FlowStreamEvent::Complete(outcome)) => {
                        // The route correction has to precede the terminal
                        // frame, not follow the dispatch: channel replies append
                        // their fallback notice while flushing on `Complete`, so
                        // a `ModelResolved` emitted after this point sets
                        // `fallback_info` that nothing will ever read.
                        emit_route_correction(&emitter, &run_id, witness_session.as_deref()).await;
                        return match hold_complete(&emitter, &run_id, &drain_state, outcome).await {
                            Ok(held) => Some(held),
                            Err(e) => {
                                warn!(run_id = %run_id, error = %e, "terminal flush failed");
                                None
                            }
                        };
                    }
                    Ok(event) => {
                        if let Err(e) =
                            emit_flow_event(event, &emitter, &run_id, &drain_state).await
                        {
                            warn!(run_id = %run_id, error = %e, "emit_flow_event failed");
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
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
            // Two shapes reach here and only one of them needs a synthesized
            // terminal frame.
            //
            // A run that failed *inside* the harness loop has already settled:
            // `AgentHarnessRunner::run` reads the terminate reason / token
            // breakdown / cost / tool timeline on both arms and broadcasts
            // `FlowStreamEvent::Complete` before returning the error, so the
            // drain forwarded a `RunComplete` carrying the real numbers.
            // Synthesizing a second one here would overwrite that with zeros
            // for every consumer that takes the first terminal frame it sees
            // (`reply_emitter::streaming::run_complete_handled`).
            //
            // A run that failed *before* the loop started (unknown agent /
            // flow, cancelled at admission) never constructed a
            // `BroadcastCallback`, so the drain only exits when the channel
            // closes and no terminal event ever arrived. That one still needs
            // the safety net, or channel/Panel consumers hang on a run end
            // that never comes. `complete_forwarded` is the discriminator —
            // the drain reports whether it actually forwarded the frame.
            let held = drain.await.ok().flatten();
            propagate.abort();
            emit_route_correction(&emitter, run_id, witness_session_for_fallback.as_deref()).await;
            // Both halves of this line were fixed independently and both are
            // load-bearing: the `held` guard stops a synthetic zeroed frame
            // from overwriting the real one the drain prepared, and the receipt
            // text stops the flattened internal chain from being pushed to a
            // bound channel as the assistant's answer. Dropping either one
            // restores a shipped defect.
            let failure = map_flow_error(e);
            // The third guard, and the newest: an attempt the caller is about
            // to retry says nothing terminal at all. Its numbers are real but
            // they are not the run's, and the next attempt's frame is — a
            // client keeping the last terminal frame it saw would otherwise
            // report the run as whatever the final, instant, zeroed retry did.
            let superseded_by_a_retry =
                may_retry && matches!(failure, DispatchFailure::Transient { .. });
            if !superseded_by_a_retry {
                match held {
                    Some(held) => {
                        if let Err(err) = emit_held_complete(&emitter, run_id, &held).await {
                            warn!(run_id, error = %err, "failed to emit the held RunComplete");
                        }
                    }
                    None => emit_error_run_complete(&emitter, run_id, &failure, locale).await,
                }
            }
            return Err(failure);
        }
        Err(e) => {
            // The flow task died without answering (panic / abort). The drain
            // may still have prepared a terminal frame; nothing else will send
            // it, and a client that never sees one waits forever. Not gated on
            // `may_retry`: a dropped completion is not something the outer loop
            // classifies as transient, so this attempt is the run's last word.
            // Why the settle below is bounded — and why only here — is in
            // `settle_dropped_completion`'s doc, next to the code that does it.
            //
            // Reaped before that grace window rather than after it: this task
            // forwards an outer cancel into a flow task that is already gone,
            // and the drain reads the outer token itself.
            propagate.abort();
            let failure = DispatchFailure::Fatal(format!("completion dropped: {e}"));
            settle_dropped_completion(
                &emitter,
                run_id,
                drain,
                &failure,
                locale,
                TERMINAL_DRAIN_GRACE,
            )
            .await;
            return Err(failure);
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
    let held = drain.await.ok().flatten();
    propagate.abort();
    if let Some(held) = held {
        // A successful attempt always speaks: `may_retry` is irrelevant here
        // because there is nothing left to supersede it.
        if let Err(err) = emit_held_complete(&emitter, run_id, &held).await {
            warn!(run_id, error = %err, "failed to emit the held RunComplete");
        }
    } else {
        // The drain never reached its correction either, so make it here — still
        // ahead of the terminal frame, which is the ordering channel replies
        // depend on. A no-op if the drain already took the witness.
        emit_route_correction(&emitter, run_id, witness_session_for_fallback.as_deref()).await;
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

/// Say the run's last word after the flow task died without answering.
///
/// Split out of the arm that calls it because the arm itself is unreachable
/// from a test: `run_dispatch_and_drain_classified` takes a concrete
/// `Arc<Orchestrator>`, and reaching `handle.completion.await == Err(_)`
/// requires a flow task that panics or is aborted after dispatch — so the
/// grace window below shipped on source reasoning alone, with no test that had
/// ever entered this branch. As a function over the drain handle it is
/// directly answerable: hand it a join that never returns and assert a
/// terminal frame comes out anyway.
///
/// **Why bounded, and only here.** The two ordinary arms join the drain
/// knowing the flow task finished, so its event channel is on its way closed.
/// This one runs precisely when it did *not* finish, and nothing proves the
/// broadcast sender went with it — an unbounded join would park the run's last
/// word forever. On expiry the drain task stays detached, which is what this
/// arm did before the frame was withheld at all.
///
/// `grace` is a parameter rather than a read of [`TERMINAL_DRAIN_GRACE`] so a
/// test can state the boundary it is asserting instead of inheriting it.
async fn settle_dropped_completion(
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    drain: tokio::task::JoinHandle<Option<HeldComplete>>,
    failure: &DispatchFailure,
    locale: Locale,
    grace: Duration,
) {
    let held = match tokio::time::timeout(grace, drain).await {
        Ok(joined) => joined.ok().flatten(),
        Err(_) => None,
    };
    match held {
        Some(held) => {
            if let Err(err) = emit_held_complete(emitter, run_id, &held).await {
                warn!(run_id, error = %err, "failed to emit the held RunComplete");
            }
        }
        None => emit_error_run_complete(emitter, run_id, failure, locale).await,
    }
}

/// Emit the `ModelResolved` correction, if this run's provider dials ended up
/// somewhere other than where the walk set out for.
///
/// The run-start `ModelResolved` states what was *asked for*; it is emitted
/// before anything has been dialed, so it cannot know about a migration. This is
/// the other half: the failover walk records what actually answered
/// ([`route_witness`](crate::providers::route_witness)) and this turns that into
/// the `is_fallback` flag every user-visible fallback notice gates on — Panel,
/// TUI, CLI and the channel reply line.
///
/// Silent when nothing deviated, which is the overwhelmingly common case: all
/// four surfaces render nothing on the happy path, and announcing the model on
/// every run would be noise.
///
/// Taking (not reading) the witness is deliberate — it keeps the map bounded and
/// stops one run's migration from being re-announced by the next.
async fn emit_route_correction(
    emitter: &Arc<dyn EventEmitter>,
    run_id: &str,
    session: Option<&str>,
) {
    let Some(session) = session else {
        return;
    };
    let Some(witness) = crate::providers::route_witness::take(session) else {
        return;
    };
    let Some(model_info) = correction_for(&witness) else {
        return;
    };
    tracing::info!(
        run_id,
        original = %witness.first.label(),
        served_provider = %witness.served.provider,
        served_model = %witness.served.label(),
        "run was served by a fallback endpoint"
    );
    if let Err(e) = emitter
        .emit(crate::gateway::event_emitter::StreamEvent::ModelResolved {
            run_id: run_id.to_string(),
            model_info,
        })
        .await
    {
        tracing::warn!(
            run_id,
            error = %e,
            "failed to emit the fallback ModelResolved correction"
        );
    }
}

/// Turn a route witness into the `ModelInfo` the correction carries, or `None`
/// when the run was served by what it set out for.
///
/// Split out of [`emit_route_correction`] so the decision is testable without an
/// emitter: this is the sole producer of `is_fallback: true` in the whole
/// system, and every user-visible fallback notice hangs off it.
fn correction_for(
    witness: &crate::providers::route_witness::RouteWitness,
) -> Option<crate::providers::health::ModelInfo> {
    if !witness.deviated() {
        return None;
    }
    Some(crate::providers::health::ModelInfo {
        model: witness.served.label(),
        // rust-doctor-disable-next-line excessive-clone
        provider: witness.served.provider.clone(),
        is_fallback: true,
        original_model: Some(witness.first.label()),
    })
}

/// Emit a terminal `RunComplete` stream event for a run that failed before the
/// harness produced a `FlowOutcome` — an unknown agent/flow, a session
/// conflict, a cancel at admission. Those never reach the harness loop, so
/// nothing settled and no terminal frame was broadcast; without this one,
/// channel/Panel consumers hang on a run end that never comes.
///
/// A run that failed *inside* the loop does not come here: the bridge settles
/// on both arms and the drain forwards the real frame (see the
/// `complete_forwarded` discriminator at the `Ok(Err(..))` arm above).
///
/// # Why the text comes from `user_receipt` and not from `{err}`
///
/// `summary.final_response` is not a log line — `OriginFanoutEmitter` reads it
/// off this very event and pushes it to the bound channel as the run's answer,
/// with only `sanitize_final_response` (a `<think>` stripper) in the way. This
/// used to format the error directly, so a channel-bound `goal`/`loop`
/// continuation that failed pre-outcome received
/// `"Run failed before completion: flow: internal dispatch error: …"` as the
/// assistant's reply — precisely the flattened internal chain
/// [`ExecutionError::user_receipt`] calls itself the single source of truth for
/// suppressing, and `mod.rs` claims never reaches a user surface. Reaching it
/// is not exotic either: `FlowError::Cancelled` resolves here, so every
/// cancelled channel-bound run took that path. It was also hardcoded English on
/// a path that already carries a `Locale`.
///
/// The synthetic `RunComplete` itself stays: `OriginFanoutEmitter` mirrors only
/// `RunComplete` (`_ => None` for everything else), so dropping the text would
/// leave channel-bound runs silent instead of merely terse.
async fn emit_error_run_complete(
    emitter: &Arc<dyn crate::gateway::event_emitter::EventEmitter>,
    run_id: &str,
    failure: &DispatchFailure,
    locale: Locale,
) {
    let summary = pre_outcome_summary(failure, locale);
    let seq = emitter.next_seq();
    if let Err(e) = emitter
        .emit(crate::gateway::event_emitter::StreamEvent::RunComplete {
            run_id: run_id.to_string(),
            seq,
            summary,
            total_duration_ms: 0,
        })
        .await
    {
        tracing::warn!(
            run_id,
            error = %e,
            "failed to emit the error-path RunComplete stream event"
        );
    }
}

/// The summary a run that died before producing any outcome reports.
///
/// Split out of [`emit_error_run_complete`] so the one field that must not be
/// guessed is testable without an emitter.
///
/// `terminate_reason` is deliberately cleared. `build_run_summary` derives it
/// from the `FlowOutcome`, and `FlowOutcome::default()` carries
/// `TerminateReason::Completed` — so the synthesized frame used to tell four
/// live renderers (`reply_emitter::streaming::cap_notice_for`, the TUI run
/// footer, `aleph exec`, `aleph watch`) that a run which never started had
/// *completed*. All four treat a missing reason as "say nothing", which is the
/// only honest answer here: this path knows the run failed and knows nothing
/// about how it terminated.
/// The summary a pre-outcome failure reports, and the two things it must not
/// get wrong.
///
/// **`terminate_reason` must be `None`.** `build_run_summary` derives it from
/// the `FlowOutcome`, and `FlowOutcome::default()` carries
/// `TerminateReason::Completed` — so the synthesized frame used to tell four
/// live renderers that a run which never started had *completed*, and all four
/// read `"completed"` as "nothing worth saying".
///
/// **`final_response` must be the localized receipt, not `{err}`.** This field
/// is not a log line: `OriginFanoutEmitter` reads it off this very event and
/// pushes it to the bound channel as the run's ANSWER, with only a `<think>`
/// stripper in the way. Formatting the error directly sent a channel-bound
/// `goal`/`loop` continuation `"Run failed before completion: flow: internal
/// dispatch error: …"` as the assistant's reply — the flattened internal chain
/// [`ExecutionError::user_receipt`] calls itself the single source of truth for
/// suppressing. `FlowError::Cancelled` resolves here, so every cancelled
/// channel-bound run took that path, in hardcoded English on a path that
/// already carries a `Locale`.
///
/// The two were fixed on separate branches within days of each other and the
/// merge kept both; either one alone leaves a shipped defect.
fn pre_outcome_summary(
    failure: &DispatchFailure,
    locale: Locale,
) -> crate::gateway::event_emitter::RunSummary {
    let outcome = crate::orchestrator::dispatch::FlowOutcome::default();
    let mut summary = super::event_drain::build_run_summary(&outcome, None);
    summary.terminate_reason = None;
    summary.final_response = Some(execution_error_for(failure).user_receipt(locale).1);
    summary
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

#[cfg(test)]
mod route_correction_tests {
    use super::correction_for;
    use crate::providers::route_witness::{Dialed, RouteWitness};

    fn witness(first: Dialed, served: Dialed) -> RouteWitness {
        RouteWitness { first, served }
    }

    #[test]
    fn a_run_served_by_its_first_choice_produces_no_correction() {
        // The happy path, and the overwhelmingly common one. Emitting here
        // would announce the model on every single run across four surfaces.
        let w = witness(
            Dialed::new("openai", Some("gpt-5".into())),
            Dialed::new("openai", Some("gpt-5".into())),
        );
        assert!(correction_for(&w).is_none());
    }

    #[test]
    fn a_provider_migration_names_both_ends() {
        let w = witness(
            Dialed::new("openai", Some("gpt-5".into())),
            Dialed::new("anthropic", Some("claude-sonnet-5".into())),
        );
        let info = correction_for(&w).expect("a migration must produce a correction");
        assert!(info.is_fallback, "this is the only producer of the flag");
        assert_eq!(info.provider, "anthropic");
        assert_eq!(info.model, "claude-sonnet-5");
        assert_eq!(info.original_model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn a_sibling_model_migration_keeps_the_provider_and_still_reports() {
        // Same endpoint, different model — the user still did not get what the
        // walk set out for, so the notice is warranted.
        let w = witness(
            Dialed::new("openai", Some("gpt-5".into())),
            Dialed::new("openai", Some("gpt-5-mini".into())),
        );
        let info = correction_for(&w).expect("a sibling-model migration is a deviation");
        assert_eq!(info.provider, "openai");
        assert_eq!(info.model, "gpt-5-mini");
        assert_eq!(info.original_model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn a_default_model_endpoint_is_labelled_not_left_blank() {
        // An empty model list on a slot means "let the provider choose". There
        // is no model id to name, and naming the requested one would be a lie —
        // so the label says what actually happened instead of rendering an
        // empty string into the notice.
        let w = witness(
            Dialed::new("openai", Some("gpt-5".into())),
            Dialed::new("ollama", None),
        );
        let info = correction_for(&w).expect("deviation");
        assert_eq!(info.model, "ollama's default model");
        assert_eq!(info.original_model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn the_same_model_on_another_provider_still_reports() {
        // Two providers serving the same model id (e.g. openai + azure). The
        // model labels match, so the three renderers that guard on
        // `original != model` drop the arrow — but the notice must still fire,
        // because the endpoint changed.
        let w = witness(
            Dialed::new("openai", Some("gpt-4o".into())),
            Dialed::new("azure", Some("gpt-4o".into())),
        );
        let info = correction_for(&w).expect("a provider change is a deviation");
        assert_eq!(info.provider, "azure");
        assert_eq!(info.original_model.as_deref(), Some("gpt-4o"));
    }
}

#[cfg(test)]
mod pre_outcome_receipt_tests {
    use super::{emit_error_run_complete, execution_error_for, DispatchFailure};
    use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter, StreamEvent};
    use crate::gateway::i18n::Locale;
    use std::sync::Arc;

    async fn final_response_for(failure: &DispatchFailure, locale: Locale) -> String {
        let inner = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = Arc::clone(&inner) as Arc<dyn EventEmitter>;
        emit_error_run_complete(&emitter, "run-1", failure, locale).await;
        let events = inner.events().await;
        let StreamEvent::RunComplete { summary, .. } = &events[0] else {
            panic!("expected a synthetic RunComplete, got {:?}", events[0]);
        };
        summary.final_response.clone().expect(
            "the synthetic RunComplete must carry text — OriginFanoutEmitter \
                     mirrors only this variant, so an empty one leaves the channel silent",
        )
    }

    /// `summary.final_response` on this path is what a bound channel receives as
    /// the assistant's answer, so it must be the localized receipt — not the
    /// flattened internal chain.
    ///
    /// The expectation is DERIVED from `ExecutionError::user_receipt`, the
    /// method whose own doc calls itself the single source of truth for
    /// user-facing failure text, rather than restating a string here.
    #[tokio::test]
    async fn a_pre_outcome_failure_delivers_the_localized_receipt() {
        let failure = DispatchFailure::Fatal(
            "flow: internal dispatch error: harness: llm error: boom".to_string(),
        );
        for locale in [Locale::En, Locale::Zh] {
            let text = final_response_for(&failure, locale).await;
            assert_eq!(
                text,
                execution_error_for(&failure).user_receipt(locale).1,
                "the pre-outcome RunComplete must carry the same receipt every other \
                 failure surface renders"
            );
            assert!(
                !text.contains("flow: "),
                "the flattened internal chain reached a channel as the run's answer: {text}"
            );
            assert!(
                !text.contains("Run failed before completion"),
                "hardcoded English on a path that carries a Locale: {text}"
            );
        }
    }

    /// Cancellation is the cheap, common way onto this path — `FlowError::Cancelled`
    /// resolves as `Ok(Err(..))` — so it gets its own assertion that the user is
    /// told they cancelled rather than shown `"flow dispatch cancelled"`.
    #[tokio::test]
    async fn a_cancelled_dispatch_reads_as_cancelled_not_as_an_internal_string() {
        let text = final_response_for(&DispatchFailure::Cancelled, Locale::En).await;
        assert_eq!(
            text,
            execution_error_for(&DispatchFailure::Cancelled)
                .user_receipt(Locale::En)
                .1
        );
        assert!(
            !text.contains("dispatch"),
            "internal wording leaked: {text}"
        );
    }
}

/// main's half of the same function, kept verbatim in intent: the frame must
/// not claim a `terminate_reason` it cannot know, and clearing it must not also
/// silence the error. Rewritten only where the signature moved from
/// `&FlowError` to `(&DispatchFailure, Locale)`.
#[cfg(test)]
mod pre_outcome_summary_tests {
    use super::{execution_error_for, pre_outcome_summary, DispatchFailure};
    use crate::gateway::i18n::Locale;

    /// The one field this path must not guess.
    ///
    /// `build_run_summary` derives `terminate_reason` from the `FlowOutcome`,
    /// and `FlowOutcome::default()` carries `TerminateReason::Completed` — so
    /// the synthesized frame used to report a run that never started as having
    /// *completed*, to four live renderers that all treat `"completed"` as
    /// "nothing worth saying". Drop the `= None` line and this goes red here.
    #[test]
    fn a_run_that_never_started_claims_no_terminate_reason() {
        let s = pre_outcome_summary(
            &DispatchFailure::Fatal("flow: unknown agent: ghost".into()),
            Locale::En,
        );
        assert!(
            s.terminate_reason.is_none(),
            "a pre-outcome failure knows the run failed and nothing about how \
             it terminated; got {:?}",
            s.terminate_reason
        );
        assert!(
            s.terminate_detail.is_none(),
            "the granular cap label has no meaning without a reason"
        );
    }

    /// The other half: clearing the reason must not also silence the frame.
    /// Without this, "make the frame honest" and "make the frame empty" look
    /// identical in the suite.
    ///
    /// ⚠️ **This assertion was `text.contains("ghost")` on `main` and is
    /// deliberately weaker now — the merge of two branches that each fixed a
    /// different defect in this one function had to decide who
    /// `final_response` is FOR, and it is not the operator.**
    ///
    /// `OriginFanoutEmitter` mirrors `RunComplete` and pushes this exact string
    /// to the bound channel as the assistant's ANSWER, with only a `<think>`
    /// stripper in the way. Naming the specific cause there means a Telegram
    /// user reading `"Run failed before completion: flow: internal dispatch
    /// error: …"` in the voice of the assistant — the flattened internal chain
    /// `ExecutionError::user_receipt` exists to suppress. So the specific cause
    /// leaves this field and the frame keeps a localized receipt.
    ///
    /// **The cause is not lost, it moved to the surfaces that are for it**: the
    /// `Err(DispatchFailure::Fatal("flow: unknown agent: ghost"))` this function
    /// runs alongside is what the caller returns, what `tracing` records and
    /// what the trace store persists. What this test now pins is main's actual
    /// concern — the frame is not EMPTY — plus the property that replaced the
    /// stronger one: it is not the internal chain either.
    #[test]
    fn the_synthesized_frame_is_neither_empty_nor_the_internal_chain() {
        let s = pre_outcome_summary(
            &DispatchFailure::Fatal("flow: unknown agent: ghost".into()),
            Locale::En,
        );
        let text = s.final_response.expect(
            "the fallback frame must carry text — OriginFanoutEmitter \
                     mirrors only this variant, so an empty one leaves the \
                     channel silent",
        );
        assert!(
            !text.trim().is_empty(),
            "clearing terminate_reason must not also empty the frame"
        );
        assert!(
            !text.contains("ghost") && !text.contains("flow:"),
            "the internal chain reached a field that is rendered to the user as \
             the assistant's reply: {text}"
        );
        assert_eq!(
            text,
            execution_error_for(&DispatchFailure::Fatal("flow: unknown agent: ghost".into()))
                .user_receipt(Locale::En)
                .1,
            "the frame must carry the SAME receipt every other user-facing \
             failure path renders, not a second wording of it"
        );
    }
}

#[cfg(test)]
mod dropped_completion_tests {
    use super::{settle_dropped_completion, DispatchFailure, HeldComplete};
    use crate::gateway::event_emitter::{CollectingEventEmitter, EventEmitter, StreamEvent};
    use crate::gateway::i18n::Locale;
    use std::sync::Arc;
    use std::time::Duration;

    const GRACE: Duration = Duration::from_secs(2);

    /// A real `HeldComplete`, built through the production path rather than a
    /// test-only constructor, so what these tests emit is what the drain emits.
    async fn prepared_frame(iterations: u32) -> HeldComplete {
        let sink = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = Arc::clone(&sink) as Arc<dyn EventEmitter>;
        let state = Arc::new(tokio::sync::Mutex::new(
            crate::gateway::execution_engine::event_drain::DrainState::default(),
        ));
        let outcome = crate::orchestrator::FlowOutcome {
            iterations,
            ..Default::default()
        };
        crate::gateway::execution_engine::event_drain::hold_complete(
            &emitter, "run-1", &state, outcome,
        )
        .await
        .expect("hold_complete")
    }

    fn failure() -> DispatchFailure {
        DispatchFailure::Fatal("completion dropped: channel closed".to_string())
    }

    /// The frame the drain prepared is the one the client gets — the numbers a
    /// run really spent, not a synthesized zero.
    #[tokio::test]
    async fn a_prepared_frame_is_what_reaches_the_client() {
        let sink = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = Arc::clone(&sink) as Arc<dyn EventEmitter>;
        let held = prepared_frame(7).await;
        let drain = tokio::spawn(async move { Some(held) });

        settle_dropped_completion(&emitter, "run-1", drain, &failure(), Locale::En, GRACE).await;

        let events = sink.events().await;
        let completes: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::RunComplete { summary, .. } => Some(summary),
                _ => None,
            })
            .collect();
        assert_eq!(completes.len(), 1, "exactly one terminal frame");
        assert_eq!(
            completes[0].loops, 7,
            "the prepared frame's real numbers must survive, not be replaced by \
             the synthesized pre-outcome receipt, which reports zero"
        );
    }

    /// The property the grace window exists for, asserted as an effect.
    ///
    /// The drain here never returns — the shape a live broadcast sender
    /// outliving a dead flow task produces. Before the bound, this parked the
    /// run's last word forever and the client waited for a terminal frame that
    /// never came. Time is paused, so the two seconds are virtual and the
    /// elapsed assertion below is what separates "the timeout fired" from "the
    /// join happened to return immediately" — without it this test would pass
    /// against a `settle` that ignored `grace` entirely.
    #[tokio::test(start_paused = true)]
    async fn a_drain_that_never_returns_still_produces_a_terminal_frame() {
        let sink = Arc::new(CollectingEventEmitter::new());
        let emitter: Arc<dyn EventEmitter> = Arc::clone(&sink) as Arc<dyn EventEmitter>;
        let drain = tokio::spawn(std::future::pending::<Option<HeldComplete>>());

        let started = tokio::time::Instant::now();
        // Bounded by the test as well as by the code under test. The mutation
        // this test exists to catch is "the grace window is gone", and an
        // unbounded `settle` under a `pending` drain would hang the whole
        // suite rather than fail — the one failure mode a test run cannot
        // report. Paused time makes the outer budget virtual, so the mutant
        // fails here in milliseconds instead of parking 17k tests.
        let settled = tokio::time::timeout(
            GRACE * 10,
            settle_dropped_completion(&emitter, "run-1", drain, &failure(), Locale::En, GRACE),
        )
        .await;
        assert!(
            settled.is_ok(),
            "settle parked on a drain that never returns — the grace window is \
             not doing anything"
        );
        let waited = started.elapsed();

        assert!(
            waited >= GRACE,
            "the join must be given the full grace window before giving up, waited {waited:?}"
        );
        let events = sink.events().await;
        let summary = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::RunComplete { summary, .. } => Some(summary),
                _ => None,
            })
            .expect("a run whose completion was dropped still owes the client a terminal frame");
        assert!(
            summary.terminate_reason.is_none(),
            "the synthesized receipt must not claim a terminate reason it cannot know; \
             `FlowOutcome::default()` carries `Completed` and four renderers read that \
             as 'nothing worth saying'"
        );
    }
}

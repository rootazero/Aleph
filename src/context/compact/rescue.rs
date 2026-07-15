//! Reactive-compaction rescue — recovery for an LLM call that hit the model's
//! context window (claude-code parity, query.ts:1092-1162; codex `mid-turn
//! auto-compact` parity).
//!
//! Relocated from `src/harness/agent/think.rs` (R10 harness diet). It belongs
//! here because it is **mechanism, not cognition**: the decision to compact is
//! entirely encoded in `llm_retry::classify`'s `RetryVerdict::CompactAndRetry`
//! verdict, which the providers layer produces from the provider's own error
//! string. Nothing below inspects message content or picks a strategy — it
//! shrinks the prompt with the already-wired compactor and re-issues the same
//! call. That is why moving it out of the loop does not turn A2's "let the model
//! see and self-heal an error" into "the harness picks a recovery strategy":
//! there is no strategy to pick.
//!
//! # The seam
//!
//! The algorithm needs five things from whoever is running the turn: issue an
//! LLM call raced against cancellation, note a rescue attempt for the trace,
//! mark the run's terminate reason when the rescue is spent, account a
//! discarded-but-billed response's tokens, and reserve the one-shot rescue slot.
//! Those are the only handles on private run state ([`RescueHost`]); everything
//! else the algorithm needs is data ([`RescueCx`]).
//!
//! The trait is defined **here** and implemented by the harness, not the other
//! way round (P4 dependency inversion): `src/context/` names no harness path
//! anywhere — grep the harness module path across this directory and the count
//! is zero, which is what keeps this layer usable by any turn driver. That is
//! also why [`RescueHost::Fatal`] is an associated type: the harness's error
//! enum is never named here, only its `From<AlephError>` conversion.

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;
use crate::context::compact::directive::compact_to_fit_and_note;
use crate::error::AlephError;
use crate::providers::adapter::{ProviderResponse, StopReason};
use crate::providers::llm_retry::{classify, RetryVerdict};
use crate::providers::message::UnifiedMessage;
use crate::session::service::SessionId;
use crate::sync_primitives::Arc;
use crate::tool_metadata::ToolDefinition;

/// Hard cap on reactive-compaction rescue attempts per run. The classifier
/// yielded `CompactAndRetry { token_gap }` and the LLM compactor ran on the
/// local message vec; one retry is enough — repeated overflows after
/// summarisation mean the input is fundamentally too large rather than a
/// recoverable burst. Mirrors claude-code's "already attempted" single-shot
/// guard (query.ts:1092).
///
/// The cap is the *policy*; the slot itself is per-run state owned by the host
/// (see [`RescueHost::reserve_rescue_slot`], whose `compare_exchange` is what
/// keeps two concurrent paths from both rescuing).
pub const MAX_REACTIVE_COMPACT_ATTEMPTS: u32 = 1;

/// Everything the rescue needs that is plain data — the in-flight turn's prompt
/// surroundings plus the two context-layer collaborators. All borrowed: the
/// rescue never outlives the turn that built it.
///
/// `started` is the turn's own clock: the rescue's retries must race against the
/// SAME turn budget as the primary call, so this is constructed *after* the
/// caller takes its `Instant`, never before.
pub struct RescueCx<'a> {
    pub session_id: &'a SessionId,
    /// The resolved system prompt (`""` when none is wired) — used for the
    /// budget's pressure re-estimates, not for the request payload.
    pub system_prompt: &'a str,
    pub tools: Option<&'a [ToolDefinition]>,
    pub budget_tool_tokens: usize,
    pub started: std::time::Instant,
    pub compactor: Option<&'a ContextCompactor>,
    pub budget: Option<&'a Arc<Mutex<ContextBudget>>>,
}

/// The turn driver's side of the rescue: the handles on private run state the
/// algorithm cannot own itself (a token counter, a trace sink, a terminate
/// reason, and the one-shot rescue slot).
#[async_trait::async_trait]
pub trait RescueHost: Sync {
    /// The driver's fatal-error type. Only `From<AlephError>` is required, so
    /// this module never names the harness's error enum.
    type Fatal: From<AlephError> + Send;

    /// Issue one LLM call with `messages`, raced against cancellation and the
    /// driver's per-turn timeout. Outer `Err` is driver-fatal (cancelled /
    /// stalled); inner `Err` is the provider's own error.
    async fn call_llm(
        &self,
        cx: &RescueCx<'_>,
        messages: &[UnifiedMessage],
        parent_cancel: &CancellationToken,
    ) -> Result<Result<ProviderResponse, AlephError>, Self::Fatal>;

    /// Atomically claim the run's single rescue slot. `true` at most
    /// [`MAX_REACTIVE_COMPACT_ATTEMPTS`] times per run; a `compare_exchange`, so
    /// two concurrent paths can never both rescue.
    fn reserve_rescue_slot(&self) -> bool;

    /// Fold a discarded response's billed tokens into the run totals. Every
    /// response this module drops was still a real round-trip the provider
    /// billed, and each must be accounted exactly once.
    fn account_discarded_tokens(&self, response: &ProviderResponse);

    /// Record one rescue attempt (trace/observability only).
    fn note_rescue_attempt(&self, token_gap: Option<usize>, succeeded: bool);

    /// The rescue is spent and the error is about to surface — set the run's
    /// terminate reason accordingly.
    fn mark_rescue_exhausted(&self);
}

/// Drain a `ContextWindowExceeded` terminal state via reactive compaction.
///
/// `model_context_window_exceeded` means the *context window* filled
/// mid-generation (distinct from the output-token cap). Each pass counts the
/// overflowed call's billed tokens, synthesizes the overflow-marker error
/// (`llm_retry::classify` maps it to `CompactAndRetry`), and routes through
/// [`try_reactive_compact_and_retry`] — reusing its one-shot cap, trace, and
/// budget plumbing.
///
/// The loop is finite because [`reactive_fit_and_retry`] never returns
/// `Ok(overflow)`: every path out of the rescue either yields a
/// non-`ContextWindowExceeded` response or an `Err` that `?` propagates. Do not
/// add an arm that returns a still-overflowing response — that is the one change
/// that would make this spin.
///
/// `response` / `response_was_streamed` are `&mut` so the caller's surviving
/// state is updated in place. Clearing `response_was_streamed` is load-bearing:
/// the rescue replaces the response through the NON-streaming path, so the
/// caller must re-enable its one-shot delta emit or the rescued text never
/// reaches live stream consumers. No-op (zero LLM cost) when `response` is not
/// in the overflow state.
pub async fn drain_context_overflow<H: RescueHost>(
    host: &H,
    response: &mut ProviderResponse,
    response_was_streamed: &mut bool,
    cx: &RescueCx<'_>,
    messages: &mut Vec<UnifiedMessage>,
    parent_cancel: &CancellationToken,
) -> Result<(), H::Fatal> {
    while matches!(response.stop_reason, StopReason::ContextWindowExceeded) {
        // The overflowed call still billed input plus the partial output;
        // count it before the retry replaces `response`.
        host.account_discarded_tokens(response);
        // The rescue below replaces `response` via the non-streaming path.
        *response_was_streamed = false;
        tracing::warn!(
            session_id = ?cx.session_id,
            "provider stopped with model_context_window_exceeded; \
             routing to reactive compaction",
        );
        let overflow_err = AlephError::ProviderError {
            message: "model_context_window_exceeded: provider stopped because \
                      the context window is full"
                .to_string(),
            suggestion: None,
        };
        *response =
            try_reactive_compact_and_retry(host, overflow_err, cx, messages, parent_cancel).await?;
    }
    Ok(())
}

/// Reactive-compaction rescue (Phase A).
///
/// When the LLM call returned a provider error, classify it via
/// [`crate::providers::llm_retry::classify`]. If the verdict is
/// `RetryVerdict::CompactAndRetry { token_gap }`, run `compactor.compact()` on
/// the in-flight `messages` vec and retry the LLM call ONCE. On any other
/// verdict the original error passes through untouched, so a caller sees
/// identical pre-rescue semantics.
///
/// When the compactor is not wired, the rescue cap is already exhausted, the
/// compactor itself fails, or the retried call still overflows, fall through to
/// [`reactive_fit_and_retry`]'s deterministic floor — a full context window must
/// not end the run (never-break).
pub async fn try_reactive_compact_and_retry<H: RescueHost>(
    host: &H,
    primary_err: AlephError,
    cx: &RescueCx<'_>,
    messages: &mut Vec<UnifiedMessage>,
    parent_cancel: &CancellationToken,
) -> Result<ProviderResponse, H::Fatal> {
    // 1. Classify. Anything that isn't `CompactAndRetry` is a clean
    //    pass-through — preserve the original error.
    let token_gap = match classify(&primary_err.to_string()) {
        RetryVerdict::CompactAndRetry { token_gap } => token_gap,
        _ => return Err(primary_err.into()),
    };

    // 2. The compactor must be wired AND we must still have a rescue slot.
    let Some(compactor) = cx.compactor else {
        // No LLM compactor wired — fall back to the deterministic floor + one
        // retry instead of hard-stopping. A full context window must not end
        // the run (never-break).
        return reactive_fit_and_retry(host, primary_err, cx, messages, parent_cancel, token_gap)
            .await;
    };
    if !host.reserve_rescue_slot() {
        // Rescue cap reached — the LLM-compaction budget for this run is spent.
        // Fall back to the deterministic floor + one retry instead of
        // hard-stopping (never-break).
        tracing::warn!(
            session_id = ?cx.session_id,
            MAX_REACTIVE_COMPACT_ATTEMPTS,
            "reactive-compaction rescue cap reached; flooring to fit and retrying once",
        );
        return reactive_fit_and_retry(host, primary_err, cx, messages, parent_cancel, token_gap)
            .await;
    }

    // 3. Run the compactor on the in-flight message vec. Failure here is
    //    fail-soft: the original provider error is what the user needs to see,
    //    the compactor's own error is secondary noise.
    tracing::warn!(
        session_id = ?cx.session_id,
        ?token_gap,
        "provider hit context overflow; running reactive compaction",
    );
    let session_id_str = cx.session_id.to_string();
    if let Err(e) = compactor
        .compact(messages, 0, Some(session_id_str.as_str()))
        .await
    {
        // LLM compaction failed — fall back to the deterministic floor + one
        // retry instead of hard-stopping (never-break). `reactive_fit_and_retry`
        // re-runs `compact_to_fit`, whose floor guarantees fit even when the
        // summariser is down.
        tracing::warn!(
            session_id = ?cx.session_id,
            error = %e,
            "reactive compactor failed; flooring to fit and retrying once",
        );
        return reactive_fit_and_retry(host, primary_err, cx, messages, parent_cancel, token_gap)
            .await;
    }

    // 3a. Refresh the budget's `last_pressure` snapshot to the compacted message
    //     vec. `before_turn` snapshotted the *pre*-compaction prompt; without
    //     this refresh the post-retry `observe_actual_usage` calibration would
    //     divide the surviving response's real `prompt_tokens_total` (compacted)
    //     by the stale uncompacted estimate, injecting a spurious shrink into the
    //     EWMA and corrupting every later compaction decision this run. Mirrors
    //     the `CompactAndContinue` path's `note_compaction_effect` call.
    if let Some(budget) = cx.budget {
        budget.lock().await.note_compaction_effect(
            messages,
            cx.system_prompt,
            cx.budget_tool_tokens,
        );
    }

    // 4. Retry the LLM call once with the summarised history.
    match host
        .call_llm(cx, messages.as_slice(), parent_cancel)
        .await?
    {
        Ok(resp) => {
            host.note_rescue_attempt(token_gap, true);
            Ok(resp)
        }
        Err(retry_err) => {
            // I1: if the retry ALSO failed with a context-overflow error (the
            // OpenAI-compatible-proxy shape that surfaces overflow as an `Err`
            // rather than a `ContextWindowExceeded` stop_reason), the LLM
            // summary alone wasn't enough — fall back to the deterministic
            // floor + one more retry before giving up (never-break). Only a
            // genuine non-context error surfaces immediately. Loop-safe:
            // `reactive_fit_and_retry` floors once and retries once, never
            // returning `Ok(overflow)`, so the caller's drain loop terminates.
            if matches!(
                classify(&retry_err.to_string()),
                RetryVerdict::CompactAndRetry { .. }
            ) {
                return reactive_fit_and_retry(
                    host,
                    retry_err,
                    cx,
                    messages,
                    parent_cancel,
                    token_gap,
                )
                .await;
            }
            tracing::warn!(
                session_id = ?cx.session_id,
                error = %retry_err,
                "reactive-compaction retry failed with a non-context error; surfacing",
            );
            host.note_rescue_attempt(token_gap, false);
            host.mark_rescue_exhausted();
            Err(retry_err.into())
        }
    }
}

/// Reactive-overflow fallback: floor the in-flight prompt to fit the model
/// window DETERMINISTICALLY (`compact_to_fit_and_note` with
/// `use_llm_compactor: false` — the LLM summariser was already tried on the main
/// path), then retry the provider ONCE. Backs the no-compactor / cap-exhausted /
/// compact-failed exits AND the still-overflow retry error (I1) — a full context
/// window must not end the run. Only a non-overflow response is success; a
/// still-overflowing OR erroring retry surfaces honestly (the truncated prompt
/// didn't fit ⇒ pathological config, not "context full", which the floor already
/// resolved). Loop-safe: never returns `Ok(overflow)`, so
/// [`drain_context_overflow`]'s loop always terminates.
async fn reactive_fit_and_retry<H: RescueHost>(
    host: &H,
    primary_err: AlephError,
    cx: &RescueCx<'_>,
    messages: &mut Vec<UnifiedMessage>,
    parent_cancel: &CancellationToken,
    token_gap: Option<usize>,
) -> Result<ProviderResponse, H::Fatal> {
    // 1. Compact to fit — DETERMINISTIC floor only (`use_llm_compactor: false`).
    // The LLM summariser was already attempted on the main reactive path before
    // we got here, so re-running it would waste a call and soften the reactive
    // rescue cap; the floor alone guarantees fit.
    compact_to_fit_and_note(
        cx.budget,
        cx.compactor,
        cx.session_id,
        messages,
        cx.system_prompt,
        cx.budget_tool_tokens,
        false,
    )
    .await;

    // 2. Retry the provider once with the fitted prompt.
    match host
        .call_llm(cx, messages.as_slice(), parent_cancel)
        .await?
    {
        Ok(resp) if !matches!(resp.stop_reason, StopReason::ContextWindowExceeded) => {
            host.note_rescue_attempt(token_gap, true);
            Ok(resp)
        }
        // Either the truncated prompt STILL overflows (pathological: configured
        // window wider than the provider's real window) or the retry errored
        // outright. Both surface honestly instead of looping — the still-overflow
        // response carries the *original* error, the erroring retry its own.
        // Loop-safe: always Err, so the caller's `?` breaks the drain.
        other => {
            // A still-overflow response is billed like any other (usage rides the
            // same `message_delta` frame) and is dropped here — account it first,
            // exactly as every other discard point does.
            if let Ok(still_overflow) = &other {
                host.account_discarded_tokens(still_overflow);
            }
            host.note_rescue_attempt(token_gap, false);
            host.mark_rescue_exhausted();
            Err(other.err().unwrap_or(primary_err).into())
        }
    }
}

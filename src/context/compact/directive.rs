//! Compaction-directive dispatch — the single entry the harness think loop
//! calls to apply a `LoopDirective` (`CompactAndContinue` / `CompactToFit` /
//! `SplitSession`) to the in-flight prompt.
//!
//! Relocated verbatim from `src/harness/agent/think.rs` (R10 harness diet):
//! all compaction policy lives in `context::compact`; the loop only forwards
//! its dependencies and branches on the returned [`DirectiveOutcome`].

use crate::context::budget::{ContextBudget, LoopDirective};
use crate::context::compact::compactor::ContextCompactor;
use crate::providers::message::UnifiedMessage;
use crate::session::epoch_registrar::SessionEpochRegistrar;
use crate::session::events::SessionEventRecord;
use crate::session::service::{SessionId, SessionService};
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;

/// Outcome of applying a compaction directive to the in-flight prompt.
pub enum DirectiveOutcome {
    /// Pressure handled (or nothing to do) — caller falls through to the LLM call.
    FellThrough,
    /// Session split succeeded — caller must return Continue with this child id.
    SplitTo(SessionId),
}

/// Compact the in-flight prompt down to fit the model's context window, then
/// return so the caller can fall through to the normal LLM call. Shared by the
/// `CompactToFit` directive and the `SplitSession` fail-soft path — both must
/// reduce pressure and continue, never hard-stop (the never-break guarantee).
#[allow(clippy::too_many_arguments)]
pub async fn compact_to_fit_and_note(
    budget: Option<&Arc<Mutex<ContextBudget>>>,
    compactor: Option<&ContextCompactor>,
    session_id: &SessionId,
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    budget_tool_tokens: usize,
    use_llm_compactor: bool,
) {
    let Some(budget) = budget else {
        return;
    };
    let mut guard = budget.lock().await;
    let session_key_str = session_id.to_key_string();
    // Reactive fallback passes `false`: the LLM summariser was already tried
    // on the main path (and the reactive rescue cap governs it), so re-invoking
    // it here wastes a call and softens the cap. Proactive callers pass `true`
    // for a full compact (LLM summary → floor).
    let compactor = if use_llm_compactor { compactor } else { None };
    crate::context::compact::fit::compact_to_fit(
        compactor,
        &guard,
        messages,
        system_prompt,
        budget_tool_tokens,
        Some(session_key_str.as_str()),
    )
    .await;
    // Refresh `last_pressure` to the fitted prompt so a later
    // `observe_actual_usage` calibration divides the real (post-fit)
    // prompt_tokens by the post-fit estimate — not the stale pre-fit snapshot
    // `before_turn` took. Mirrors the `CompactAndContinue` path's
    // `note_compaction_effect` call and the reactive path's 3a refresh;
    // omitting it injects a spurious shrink into the tokenizer-ratio EWMA that
    // degrades every later compaction decision this run.
    guard.note_compaction_effect(messages.as_slice(), system_prompt, budget_tool_tokens);
}

/// Apply a budget directive to the in-flight prompt. Returns
/// [`DirectiveOutcome::SplitTo`] only when a `SplitSession` directive
/// succeeded; every other path (including directives this function does not
/// handle) falls through so the caller proceeds to the normal LLM call.
#[allow(clippy::too_many_arguments)]
pub async fn apply_budget_directive(
    directive: &LoopDirective,
    compactor: Option<&ContextCompactor>,
    budget: Option<&Arc<Mutex<ContextBudget>>>,
    registrar: Option<&std::sync::Arc<dyn SessionEpochRegistrar>>,
    session: &dyn SessionService,
    session_id: &SessionId,
    messages: &mut Vec<UnifiedMessage>,
    events: &[SessionEventRecord],
    tail_start: usize,
    system_prompt: &str,
    budget_tool_tokens: usize,
) -> DirectiveOutcome {
    // 2c. Compact when directive calls for it and a compactor is wired.
    if matches!(directive, LoopDirective::CompactAndContinue) {
        if let Some(compactor) = compactor {
            // `fresh_tail = 0` lets the compactor fall back to its own
            // config default (matches Task 6 spec). The session id enables
            // the compactor's zero-API-cost reuse of hierarchical session
            // summaries when a memory backend is wired.
            let session_key_str = session_id.to_key_string();
            match compactor
                .compact(messages, 0, Some(session_key_str.as_str()))
                .await
            {
                Ok(_) => {
                    // Re-arm the circuit breaker only when this compaction
                    // actually reduced pressure. An ineffective compaction
                    // leaves the breaker counting, so a thrashing run still
                    // escalates to `FinalReply` (hermes anti-thrash).
                    if let Some(budget) = budget {
                        budget.lock().await.note_compaction_effect(
                            messages,
                            system_prompt,
                            budget_tool_tokens,
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        ?session_id,
                        ?e,
                        "context compactor failed; continuing with uncompacted messages",
                    );
                }
            }
        }
    }

    // 2c-fit. `CompactToFit` directive — critical pressure (or a split cap
    // reached). Compact aggressively down to the model's window, then FALL
    // THROUGH to the normal LLM call. A run must never end just because
    // context filled (never-break guarantee). R10-safe: mechanical dispatch
    // to the external fit helper; no policy, no completion judgement.
    if matches!(directive, LoopDirective::CompactToFit) {
        compact_to_fit_and_note(
            budget,
            compactor,
            session_id,
            messages,
            system_prompt,
            budget_tool_tokens,
            true,
        )
        .await;
        // no early return — control falls through to the LLM call below.
    }

    // 2c-split. `SplitSession` directive — attempt compaction-driven session
    // split. On success, return `TurnState::Continue` with the child session
    // id so `run()` can rebind `current_session`. On failure or when the
    // registrar/compactor is not wired, fall back to compact-to-fit and
    // continue (never-break). R10-safe: mechanical dispatch to
    // `perform_session_split` (lives outside the harness); no intent
    // classification, no new heuristic.
    if matches!(directive, LoopDirective::SplitSession) {
        let split_child = match (compactor, registrar) {
            (Some(compactor), Some(registrar)) => {
                match crate::context::compact::session_split::perform_session_split(
                    session,
                    registrar.as_ref(),
                    compactor,
                    session_id,
                    events,
                    tail_start,
                )
                .await
                {
                    Ok(outcome) => {
                        if let Some(budget) = budget {
                            budget.lock().await.record_split();
                        }
                        tracing::info!(
                            ?session_id,
                            child = ?outcome.child_session_id,
                            "session split: continuing run in child session",
                        );
                        Some(outcome.child_session_id)
                    }
                    Err(e) => {
                        tracing::warn!(
                            ?session_id,
                            %e,
                            "session split failed; falling back to compact-to-fit",
                        );
                        None
                    }
                }
            }
            _ => None, // compactor or registrar not wired — fall back to compact-to-fit
        };

        if let Some(child) = split_child {
            // Continue the run in the child; run() rebinds current_session.
            return DirectiveOutcome::SplitTo(child);
        }
        // Fail-soft: registrar/compactor unavailable or the split failed.
        // Rather than hard-stop (the old ContextBudgetExhausted + grace),
        // compact to fit and FALL THROUGH to the normal LLM call — the
        // never-break guarantee. R10-safe: same mechanical fit helper as the
        // `CompactToFit` arm; no policy, no error-recovery strategy choice.
        compact_to_fit_and_note(
            budget,
            compactor,
            session_id,
            messages,
            system_prompt,
            budget_tool_tokens,
            true,
        )
        .await;
        // no early return — control falls through to the LLM call below.
    }

    DirectiveOutcome::FellThrough
}

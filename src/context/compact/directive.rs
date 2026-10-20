//! Compaction-directive dispatch — the single entry the harness think loop
//! calls to apply a `LoopDirective` (`CompactAndContinue` / `CompactToFit` /
//! `SplitSession`) to the in-flight prompt.
//!
//! Relocated verbatim from `src/harness/agent/think.rs` (R10 harness diet):
//! all compaction policy lives in `context::compact`; the loop only forwards
//! its dependencies and branches on the returned [`DirectiveOutcome`].

use crate::context::budget::{ContextBudget, LoopDirective};
use crate::context::compact::compactor::ContextCompactor;
use crate::context::fabric::runtime::ContextFabricError;
use crate::context::fabric::{AuthenticatedCaller, ContextFabric};
use crate::providers::message::UnifiedMessage;
use crate::session::epoch_registrar::SessionEpochRegistrar;
use crate::session::events::SessionEventRecord;
use crate::session::service::{SessionId, SessionService};
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Fresh-tail fallback when context budgeting is disabled.
const NO_BUDGET_FRESH_TAIL: usize = 6;

/// Prepare pressure and invoke preflight without holding the budget lock across
/// any handler await. Cancellation is fatal; other errors preserve native
/// fail-soft turn semantics.
#[allow(clippy::too_many_arguments)]
pub async fn preflight(
    fabric: Option<&ContextFabric>,
    budget: Option<&Arc<Mutex<ContextBudget>>>,
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    budget_tool_tokens: usize,
    transient_tail: usize,
    session_id: &SessionId,
    cancel: &CancellationToken,
) -> Result<(), ContextFabricError> {
    if cancel.is_cancelled() {
        return Err(ContextFabricError::Cancelled);
    }
    let result: Result<(), ContextFabricError> = async {
        // 2a. Preflight cheap passes (hermes-inspired). Run BEFORE the budget
        // check so token-saving transforms — tool_result pruning + historical
        // image stripping — happen even if the compactor's side-channel LLM
        // call later fails. No LLM cost in this step.
        if let Some(pipeline) = fabric {
            // Compute the REAL pressure snapshot (read-only via `peek_pressure`
            // — no calibration or circuit-breaker mutation) so the pipeline's
            // preventive-band gate (`PreflightPipeline::min_pressure_ratio`,
            // wired from `ContextBudgetConfig::preventive_floor`, default 0.60)
            // fires the lossy cheap passes only when the context is actually
            // filling up. The old `ratio: 1.0` placeholder kept the gate
            // permanently open, so the passes ran on every turn even on a
            // near-empty context — contradicting the "calm runs pay nothing"
            // contract.
            // `fresh_tail` comes from the same `ContextBudget` the compactor
            // consults, keeping the protected-tail invariant intact. Falls back
            // to a max-pressure placeholder + `CompactorConfig::default` tail of
            // 6 only when no budget is wired (degenerate config), preserving the
            // previous always-on behaviour there.
            let (pressure, fresh_tail) = match budget {
                Some(budget) => {
                    let guard = budget.lock().await;
                    let pressure = guard.peek_pressure(messages, system_prompt, budget_tool_tokens);
                    (pressure, guard.fresh_tail_count())
                }
                None => (
                    crate::context::budget::ContextPressure {
                        used_tokens: 0,
                        budget_tokens: 0,
                        ratio: 1.0,
                        overhead_tokens: 0,
                        available_for_messages: 0,
                    },
                    NO_BUDGET_FRESH_TAIL,
                ),
            };
            // `+ transient_tail`: the protected tail is a count of PERSISTED
            // messages, but this vector ends with up to four `<system-reminder>`
            // nudges plus the recall message — five synthetic entries that could
            // consume a six-message guard, letting the cheap passes rewrite a
            // message the model saw one turn ago. Adding the count back makes the
            // cut `persisted_len - fresh_tail`, which is also independent of how
            // many nudges happened to fire, so the boundary stops jittering near
            // a quantum edge (FEATURE_LOCATOR §2.18 ledger item 2).
            let caller = AuthenticatedCaller::from_host_boundary(session_id.to_key_string())?;
            let outcome = pipeline
                .preflight(
                    messages,
                    &pressure,
                    fresh_tail + transient_tail,
                    &caller,
                    Some(cancel),
                )
                .await?;
            let freed = outcome.tokens_freed;
            if freed > 0 {
                tracing::debug!(
                    ?session_id,
                    tokens_freed = freed,
                    "preflight cheap passes saved tokens",
                );
            }
        }
        Ok(())
    }
    .await;
    match result {
        Err(ContextFabricError::Cancelled) => Err(ContextFabricError::Cancelled),
        Err(e) => {
            tracing::warn!(?session_id, error = %e, "context preflight failed; continuing");
            Ok(())
        }
        Ok(()) => Ok(()),
    }
}

/// Authenticate exclusively from the host's resolved session, never a plugin
/// argument or a separately selected registry owner.
pub(crate) async fn compact_for_session(
    fabric: &ContextFabric,
    messages: &mut Vec<UnifiedMessage>,
    fresh_tail: usize,
    transient_tail: usize,
    session_id: &SessionId,
    cancel: &CancellationToken,
) -> Result<crate::context::fabric::runtime::CompactionOutcome, ContextFabricError> {
    let session_key = session_id.to_key_string();
    let caller = AuthenticatedCaller::from_host_boundary(session_key.as_str())?;
    fabric
        .compact(
            messages,
            fresh_tail,
            transient_tail,
            Some(&session_key),
            &caller,
            Some(cancel),
        )
        .await
}

/// Outcome of applying a compaction directive to the in-flight prompt.
pub enum DirectiveOutcome {
    /// Pressure handled (or nothing to do) — caller falls through to the LLM call.
    FellThrough,
    /// The request was cancelled; never fall through to the LLM.
    Cancelled,
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
    fabric: Option<&ContextFabric>,
    session_id: &SessionId,
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    budget_tool_tokens: usize,
    transient_tail: usize,
    use_llm_compactor: bool,
    cancel: &CancellationToken,
) -> Result<(), ContextFabricError> {
    if cancel.is_cancelled() {
        return Err(ContextFabricError::Cancelled);
    }
    let Some(budget) = budget else {
        return Ok(());
    };
    // Reactive fallback passes `false`: the LLM summariser was already tried
    // on the main path (and the reactive rescue cap governs it), so re-invoking
    // it here wastes a call and softens the cap. Proactive callers pass `true`
    // for a full compact (LLM summary → floor).
    let fabric = if use_llm_compactor { fabric } else { None };
    crate::context::compact::fit::compact_to_fit(
        fabric,
        budget,
        messages,
        system_prompt,
        budget_tool_tokens,
        transient_tail,
        session_id,
        cancel,
    )
    .await?;
    // Refresh `last_pressure` to the fitted prompt so a later
    // `observe_actual_usage` calibration divides the real (post-fit)
    // prompt_tokens by the post-fit estimate — not the stale pre-fit snapshot
    // `before_turn` took. Mirrors the `CompactAndContinue` path's
    // `note_compaction_effect` call and the reactive path's 3a refresh;
    // omitting it injects a spurious shrink into the tokenizer-ratio EWMA that
    // degrades every later compaction decision this run.
    budget.lock().await.note_compaction_effect(
        messages.as_slice(),
        system_prompt,
        budget_tool_tokens,
    );
    Ok(())
}

/// Apply a budget directive to the in-flight prompt. Returns
/// [`DirectiveOutcome::SplitTo`] only when a `SplitSession` directive
/// succeeded; every other path (including directives this function does not
/// handle) falls through so the caller proceeds to the normal LLM call.
#[allow(clippy::too_many_arguments)]
pub async fn apply_budget_directive(
    directive: &LoopDirective,
    compactor: Option<&ContextCompactor>,
    fabric: Option<&ContextFabric>,
    budget: Option<&Arc<Mutex<ContextBudget>>>,
    registrar: Option<&std::sync::Arc<dyn SessionEpochRegistrar>>,
    session: &dyn SessionService,
    session_id: &SessionId,
    messages: &mut Vec<UnifiedMessage>,
    events: &[SessionEventRecord],
    tail_start: usize,
    system_prompt: &str,
    budget_tool_tokens: usize,
    // How many messages at the end of `messages` are synthetic (nudges +
    // recall) rather than persisted turns. Required, not defaulted: it is the
    // difference between protecting the last six turns and protecting one.
    transient_tail: usize,
    cancel: &CancellationToken,
) -> DirectiveOutcome {
    if cancel.is_cancelled() {
        return DirectiveOutcome::Cancelled;
    }
    // 2c. Compact when directive calls for it and a compactor is wired.
    match directive {
        LoopDirective::CompactAndContinue => {
            if let Some(fabric) = fabric {
                let fresh_tail = match budget {
                    Some(budget) => budget.lock().await.fresh_tail_count(),
                    None => NO_BUDGET_FRESH_TAIL,
                };
                match compact_for_session(
                    fabric,
                    messages,
                    fresh_tail,
                    transient_tail,
                    session_id,
                    cancel,
                )
                .await
                {
                    Ok(outcome) => {
                        let result = outcome.result;
                        // `strategy_used` had exactly one reader — the
                        // watchdog's reset gate — and this, its only production
                        // call site, threw it away with `Ok(_)`. So the one
                        // carrier that distinguishes "the LLM summarized this
                        // window" from "we kept the first line of every
                        // message" reached no operator surface at all. Say it
                        // once, here, where the compaction actually happened.
                        if matches!(
                            result.strategy_used,
                            crate::context::compact::compactor::CompactStrategy::DeterministicTruncation
                        ) {
                            tracing::warn!(
                                target: "context_budget",
                                ?session_id,
                                "compaction degraded to deterministic truncation — the \
                                 side-channel summarizer is not producing summaries; \
                                 check [context_budget] / the aux model on this provider"
                            );
                        }
                        // Structured telemetry for the SUCCESS paths too
                        // (codex's compaction analytics parity: reason /
                        // strategy / tokens before→after on every compaction,
                        // not only on degradation). This is also the first
                        // consumer of `CompactResult.tokens_before/after` on
                        // the automatic path — the manual `/compact` RPC
                        // already reported them, the automatic path dropped
                        // them on the floor. `0/0` with a non-LLM strategy is
                        // expected: CacheReuse and SessionMemoryReuse never
                        // measured the window (that is exactly what makes
                        // them zero-cost), and `Skipped` names its reason in
                        // the strategy itself.
                        tracing::info!(
                            target: "context_budget",
                            ?session_id,
                            strategy = ?result.strategy_used,
                            tokens_before = result.tokens_before,
                            tokens_after = result.tokens_after,
                            "compaction applied",
                        );
                        if let Some(budget) = budget {
                            budget.lock().await.note_compaction_effect(
                                messages,
                                system_prompt,
                                budget_tool_tokens,
                            );
                        }
                    }
                    Err(ContextFabricError::Cancelled) => return DirectiveOutcome::Cancelled,
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
        LoopDirective::CompactToFit => {
            if let Err(e) = compact_to_fit_and_note(
                budget,
                fabric,
                session_id,
                messages,
                system_prompt,
                budget_tool_tokens,
                transient_tail,
                true,
                cancel,
            )
            .await
            {
                if matches!(e, ContextFabricError::Cancelled) {
                    return DirectiveOutcome::Cancelled;
                }
                tracing::warn!(error = %e, "compact-to-fit failed; continuing");
            }
        }
        LoopDirective::SplitSession => {
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
                                "session split refused or failed; falling back to compact-to-fit",
                            );
                            None
                        }
                    }
                }
                _ => None,
            };

            if let Some(child) = split_child {
                return DirectiveOutcome::SplitTo(child);
            }
            if let Err(e) = compact_to_fit_and_note(
                budget,
                fabric,
                session_id,
                messages,
                system_prompt,
                budget_tool_tokens,
                transient_tail,
                true,
                cancel,
            )
            .await
            {
                if matches!(e, ContextFabricError::Cancelled) {
                    return DirectiveOutcome::Cancelled;
                }
                tracing::warn!(error = %e, "compact-to-fit failed; continuing");
            }
        }
        LoopDirective::Continue => {}
    }

    if cancel.is_cancelled() {
        DirectiveOutcome::Cancelled
    } else {
        DirectiveOutcome::FellThrough
    }
}

#[cfg(test)]
#[path = "directive_tests.rs"]
mod tests;

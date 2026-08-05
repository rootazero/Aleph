//! Advisor fan-out: parallel consultation with per-advisor timeout,
//! fail-soft degradation, and trace-event emission. Extracted from
//! `provider.rs::process()` (round-2 refactor R1) so the facade stays an
//! orchestration skeleton.

use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};

use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use crate::providers::adapter::{RequestPayload, TokenUsage};
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::Arc;

use super::advisor_health::CallOutcome;
use super::prompts::AdvisorOutcome;
use super::provider::AdvisorSlot;

/// One advisor's full fan-out result: display outcome + accounting + the
/// structural error (None on success). The error channel feeds the
/// `MoaAdvisor.error` trace field (round-2 B2); `health` feeds the run-scoped
/// breaker (round-6 G1).
pub(crate) struct AdvisorResult {
    pub outcome: AdvisorOutcome,
    pub usage: Option<TokenUsage>,
    pub error: Option<String>,
    pub health: CallOutcome,
}

impl AdvisorResult {
    /// Whether this slot was actually consulted (i.e. the breaker let it
    /// through). Drives `MoaAdvisorSpend.advisor_count`, which is documented
    /// as *consulted* — unlike the `i/n` display count, which stays the total
    /// slot count so advisor numbering never shifts.
    pub(crate) fn consulted(&self) -> bool {
        self.health != CallOutcome::Skipped
    }
}

/// Parallel fan-out, per-advisor timeout, fail-soft.
///
/// INDEX-ALIGNMENT INVARIANT: exactly one `AdvisorResult` per entry of
/// `advisors`, in slot order. `MoaProvider::spend_event` indexes
/// `self.advisors[idx]` off this vector's enumeration, and
/// `AdvisorHealth::record` folds it back slot-by-slot — so a slot the breaker
/// skipped yields a synthetic result, never a filtered-out entry. The
/// consultations are driven by a `FuturesUnordered` (completion order) while
/// the RETURN vector is written by slot index, so the invariant holds
/// independently of who finishes first.
///
/// Each `MoaAdvisor` trace event is emitted THE MOMENT that advisor lands,
/// not after the whole fan-out. Advisors differ wildly in latency — a local
/// 7B answers in seconds next to a reasoning model taking a minute — and
/// under the default `per_iteration` cadence the old batch-at-the-end
/// emission meant the panel and the TUI went dark for the full
/// `advisor_timeout_secs` on *every tool step*, then printed everything at
/// once. hermes needed a poll loop (`_run_references_parallel`'s
/// `progress_callback` over `_futures_wait`) for the same effect; a stream of
/// futures gets it with no polling and no extra task.
///
/// Never fails the turn: an error, timeout or breaker skip degrades to a
/// labelled note in `outcome.text`.
pub(crate) async fn run_fan_out(
    advisors: &[AdvisorSlot],
    view: &[UnifiedMessage],
    system_prompt: &str,
    skip_reasons: &[Option<String>],
    timeout: Duration,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    sink: Option<&Arc<dyn TraceSink>>,
) -> Vec<AdvisorResult> {
    let futures = advisors.iter().enumerate().map(|(idx, slot)| {
        let skip = skip_reasons.get(idx).and_then(Option::as_deref);
        let label = slot.label.as_str();
        async move {
            if let Some(reason) = skip {
                return (
                    idx,
                    AdvisorOutcome::unavailable(label, &format!("[skipped: {reason}]")),
                    None,
                    Some(format!("skipped: {reason}")),
                    CallOutcome::Skipped,
                );
            }
            let advisor_payload = RequestPayload::new(view)
                .with_system(Some(system_prompt))
                .with_temperature(temperature)
                .with_max_tokens(max_tokens);
            match tokio::time::timeout(timeout, slot.chain.process(advisor_payload)).await {
                Ok(Ok(resp)) => {
                    let advice = resp
                        .text
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .map(str::to_string);
                    let usage = resp.usage;
                    // The CALL succeeded either way, so health must not take a
                    // strike (the slot is reachable and answering) — but an
                    // empty body is not advice, and numbering it as one asks
                    // the aggregator to act on a blank block.
                    let outcome = advice.map_or_else(
                        || AdvisorOutcome::unavailable(label, "[empty response]"),
                        |text| AdvisorOutcome::advice(label, text),
                    );
                    (idx, outcome, usage, None::<String>, CallOutcome::Ok)
                }
                Ok(Err(e)) => (
                    idx,
                    AdvisorOutcome::unavailable(label, &format!("[failed: {e}]")),
                    None,
                    // The trace/panel channel keeps the FULL error — only the
                    // prompt-bound copy inside `outcome` is clamped.
                    Some(e.to_string()),
                    CallOutcome::failed(&e),
                ),
                Err(_) => (
                    idx,
                    AdvisorOutcome::unavailable(
                        label,
                        &format!("[timeout after {}s]", timeout.as_secs()),
                    ),
                    None,
                    Some(format!("timeout after {}s", timeout.as_secs())),
                    CallOutcome::timed_out(),
                ),
            }
        }
    });

    let count = advisors.len();
    let mut slots: Vec<Option<AdvisorResult>> = Vec::with_capacity(count);
    slots.resize_with(count, || None);

    let mut pending: FuturesUnordered<_> = futures.collect();
    while let Some((idx, outcome, usage, error, health)) = pending.next().await {
        let result = AdvisorResult {
            outcome,
            usage,
            error,
            health,
        };
        // Live, in completion order: `index` stays the SLOT number so the
        // `i/n` a user reads never shifts, and `count` stays the total slot
        // count (deliberately different from `MoaAdvisorSpend.advisor_count`,
        // which counts only slots actually consulted).
        if let Some(s) = sink {
            s.on_trace(&LoopTraceEvent::MoaAdvisor {
                index: idx + 1,
                count,
                // rust-doctor-disable-next-line excessive-clone
                label: result.outcome.label.clone(),
                // rust-doctor-disable-next-line excessive-clone
                text: result.outcome.text.clone(),
                // rust-doctor-disable-next-line excessive-clone
                error: result.error.clone(),
            });
        }
        if let Some(cell) = slots.get_mut(idx) {
            *cell = Some(result);
        }
    }

    // Every future carries a distinct `idx < count` and the stream yields all
    // of them, so each cell is written exactly once — the flatten below cannot
    // shorten the vector without breaking the index-alignment invariant.
    let results: Vec<AdvisorResult> = slots.into_iter().flatten().collect();
    debug_assert_eq!(
        results.len(),
        count,
        "MoA fan-out must return exactly one result per advisor slot"
    );
    results
}

/// Close a completed fan-out with the aggregating marker (MISS-only display
/// path), called from `process()`. The per-advisor events already fired inside
/// [`run_fan_out`], one per completion.
///
/// Always `cached: false` here — a HIT reusing the previous fan-out's advice
/// emits its own lightweight `MoaAggregating { cached: true }` from
/// `process()` directly (round-2 B4), never through this MISS-only path.
///
/// `advisor_count` here (matching the `i/n` display) is the TOTAL slot count,
/// breaker-skipped slots included, so advisor numbering is stable across
/// iterations. It deliberately differs from `MoaAdvisorSpend.advisor_count`,
/// which counts only the slots actually CONSULTED — see
/// [`AdvisorResult::consulted`]. Keep the two apart.
pub(crate) fn emit_aggregating_event(
    sink: &Option<Arc<dyn TraceSink>>,
    advisor_count: usize,
    aggregator_label: &str,
) {
    if let Some(s) = sink {
        s.on_trace(&LoopTraceEvent::MoaAggregating {
            aggregator: aggregator_label.to_string(),
            advisor_count,
            cached: false,
        });
    }
}

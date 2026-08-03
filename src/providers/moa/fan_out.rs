//! Advisor fan-out: parallel consultation with per-advisor timeout,
//! fail-soft degradation, and trace-event emission. Extracted from
//! `provider.rs::process()` (round-2 refactor R1) so the facade stays an
//! orchestration skeleton.

use std::time::Duration;

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
/// skipped yields a synthetic result, never a filtered-out entry.
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
) -> Vec<AdvisorResult> {
    let futures = advisors.iter().enumerate().map(|(idx, slot)| {
        let skip = skip_reasons.get(idx).and_then(Option::as_deref);
        let label = slot.label.as_str();
        async move {
            if let Some(reason) = skip {
                return (
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
                    (outcome, usage, None::<String>, CallOutcome::Ok)
                }
                Ok(Err(e)) => (
                    AdvisorOutcome::unavailable(label, &format!("[failed: {e}]")),
                    None,
                    // The trace/panel channel keeps the FULL error — only the
                    // prompt-bound copy inside `outcome` is clamped.
                    Some(e.to_string()),
                    CallOutcome::failed(&e),
                ),
                Err(_) => (
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
    let results = futures::future::join_all(futures).await;

    results
        .into_iter()
        .map(|(outcome, usage, error, health)| AdvisorResult {
            outcome,
            usage,
            error,
            health,
        })
        .collect()
}

/// Emit the per-advisor trace events + aggregating marker for a completed
/// fan-out (MISS-only display path), called from `process()`. Advisor events
/// carry `AdvisorResult.error` (round-2 B2); the aggregating event is always
/// `cached: false` here — a HIT reusing the previous fan-out's advice emits
/// its own lightweight `MoaAggregating { cached: true }` from `process()`
/// directly (round-2 B4), never through this MISS-only path.
///
/// `count` here (the `i/n` display and `MoaAggregating.advisor_count`) is the
/// TOTAL slot count, breaker-skipped slots included, so advisor numbering is
/// stable across iterations. It deliberately differs from
/// `MoaAdvisorSpend.advisor_count`, which counts only the slots actually
/// CONSULTED — see [`AdvisorResult::consulted`]. Keep the two apart.
pub(crate) fn emit_fanout_events(
    sink: &Option<Arc<dyn TraceSink>>,
    results: &[AdvisorResult],
    aggregator_label: &str,
) {
    let count = results.len();
    for (idx, r) in results.iter().enumerate() {
        if let Some(s) = sink {
            s.on_trace(&LoopTraceEvent::MoaAdvisor {
                index: idx + 1,
                count,
                // rust-doctor-disable-next-line excessive-clone
                label: r.outcome.label.clone(),
                // rust-doctor-disable-next-line excessive-clone
                text: r.outcome.text.clone(),
                // rust-doctor-disable-next-line excessive-clone
                error: r.error.clone(),
            });
        }
    }
    if let Some(s) = sink {
        s.on_trace(&LoopTraceEvent::MoaAggregating {
            aggregator: aggregator_label.to_string(),
            advisor_count: count,
            cached: false,
        });
    }
}

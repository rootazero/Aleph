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

use super::prompts::{AdvisorOutcome, ADVISOR_SYSTEM_PROMPT};
use super::provider::AdvisorSlot;

/// One advisor's full fan-out result: display outcome + accounting + the
/// structural error (None on success). The error channel feeds the
/// `MoaAdvisor.error` trace field (round-2 B2).
pub(crate) struct AdvisorResult {
    pub outcome: AdvisorOutcome,
    pub usage: Option<TokenUsage>,
    pub error: Option<String>,
}

/// Parallel fan-out, per-advisor timeout, fail-soft. Result order is stable
/// (preset slot order). Never fails the turn: an advisor error/timeout
/// degrades to a labelled note in `outcome.text`.
pub(crate) async fn run_fan_out(
    advisors: &[AdvisorSlot],
    view: &[UnifiedMessage],
    timeout: Duration,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Vec<AdvisorResult> {
    let futures = advisors.iter().map(|slot| async move {
        let advisor_payload = RequestPayload::new(view)
            .with_system(Some(ADVISOR_SYSTEM_PROMPT))
            .with_temperature(temperature)
            .with_max_tokens(max_tokens);
        match tokio::time::timeout(timeout, slot.chain.process(advisor_payload)).await {
            Ok(Ok(resp)) => {
                let text = resp
                    .text
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "(empty response)".to_string());
                (text, resp.usage, None::<String>)
            }
            Ok(Err(e)) => (format!("[failed: {e}]"), None, Some(e.to_string())),
            Err(_) => (
                format!("[timeout after {}s]", timeout.as_secs()),
                None,
                Some(format!("timeout after {}s", timeout.as_secs())),
            ),
        }
    });
    let results = futures::future::join_all(futures).await;

    results
        .into_iter()
        .enumerate()
        .map(|(idx, (text, usage, error))| AdvisorResult {
            outcome: AdvisorOutcome {
                label: advisors[idx].label.clone(),
                text,
            },
            usage,
            error,
        })
        .collect()
}

/// Emit the per-advisor trace events + aggregating marker for a completed
/// fan-out (MISS-only display path), called from `process()`. Byte-identical
/// to the pre-extraction inline emission. Task 3 (B1/B2/B4) will change this
/// signature/internals to also carry `AdvisorResult.error` and a `cached`
/// flag.
pub(crate) fn emit_fanout_events(
    sink: &Option<Arc<dyn TraceSink>>,
    _advisors: &[AdvisorSlot],
    results: &[AdvisorResult],
    aggregator_label: &str,
) {
    let count = results.len();
    for (idx, r) in results.iter().enumerate() {
        if let Some(s) = sink {
            s.on_trace(&LoopTraceEvent::MoaAdvisor {
                index: idx + 1,
                count,
                label: r.outcome.label.clone(),
                text: r.outcome.text.clone(),
            });
        }
    }
    if let Some(s) = sink {
        s.on_trace(&LoopTraceEvent::MoaAggregating {
            aggregator: aggregator_label.to_string(),
            advisor_count: count,
        });
    }
}

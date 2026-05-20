//! Per-run cost estimation against a static price table.
//!
//! Hermes-agent surfaces `estimated_cost_usd` + `cost_status` on every turn
//! summary. Aleph mirrors that signal with an opt-in module: callers supply
//! the cumulative [`TokenBreakdown`] for the run, plus the provider + model
//! identifiers; the table returns a USD figure annotated with how confident
//! the estimate is. Unknown models degrade to [`CostStatus::Unknown`] without
//! poisoning the rest of the outcome — pricing is best-effort, never a gate.
//!
//! The price table is intentionally inline (no network lookup, no config
//! file). Prices drift; we accept that and let operators upgrade Aleph to
//! pick up new entries. The alternative — pulling live rates — would import
//! a HTTP dependency for a low-signal feature, violating R3 (Core
//! Minimalism).
//!
//! # Phase note
//!
//! Stage 1 (this file) provides type definitions + an `estimate()` entry
//! point that returns `CostStatus::Unknown` for every model. The actual
//! table is filled in by the follow-up `Cost — src/pricing.rs static price
//! table` task. Splitting the work lets P1 (FlowOutcome expansion) ship
//! without blocking on table research.

use serde::{Deserialize, Serialize};

use crate::orchestrator::dispatch::TokenBreakdown;

/// Estimated USD cost for a single agent run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostEstimate {
    /// Cost in USD. `0.0` when [`status`] is `Unknown`.
    pub usd: f64,
    /// Confidence band — see [`CostStatus`].
    pub status: CostStatus,
    /// Provider identifier the table was queried with (e.g. `"anthropic"`).
    pub provider: String,
    /// Model identifier the table was queried with (e.g. `"claude-sonnet-4-6"`).
    pub model: String,
}

impl CostEstimate {
    /// Construct the "no entry found" estimate. Used by [`estimate`] when
    /// either provider or model misses the table.
    pub fn unknown(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            usd: 0.0,
            status: CostStatus::Unknown,
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Confidence band attached to every [`CostEstimate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CostStatus {
    /// All token components hit a populated rate.
    Complete,
    /// Some components (e.g. cache_creation) lacked a rate; the figure is a
    /// lower bound.
    PartialMissingPrice,
    /// Provider or model not in the table.
    Unknown,
}

/// Estimate the cost of a run given its accumulated token breakdown.
///
/// Phase-stub: returns `CostStatus::Unknown` for every input. The follow-up
/// pricing task wires the actual table.
pub fn estimate(
    provider: &str,
    model: &str,
    _breakdown: &TokenBreakdown,
) -> CostEstimate {
    CostEstimate::unknown(provider, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_provider_returns_zero_usd_unknown_status() {
        let breakdown = TokenBreakdown::default();
        let est = estimate("nonexistent-provider", "nonexistent-model", &breakdown);
        assert_eq!(est.usd, 0.0);
        assert_eq!(est.status, CostStatus::Unknown);
        assert_eq!(est.provider, "nonexistent-provider");
        assert_eq!(est.model, "nonexistent-model");
    }

    #[test]
    fn cost_estimate_roundtrips_through_serde() {
        let est = CostEstimate {
            usd: 1.23,
            status: CostStatus::Complete,
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };
        let json = serde_json::to_string(&est).expect("serialize");
        let back: CostEstimate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(est, back);
    }
}

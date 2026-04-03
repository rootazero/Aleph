//! Foundation types for the compaction orchestration system.
//!
//! Defines the core traits and data types used by all compaction strategies:
//! `PressureLevel`, `CompactionContext`, `CompactionResult`,
//! `CompactionStrategy`, and `PostCompactCleanup`.

use std::future::Future;
use std::pin::Pin;

use crate::agent_loop::ContextPressure;
use crate::providers::message::UnifiedMessage;

// =============================================================================
// PressureLevel
// =============================================================================

/// Discrete pressure level derived from the context window fill ratio.
///
/// Variants are ordered from least to most severe so that `<` / `>` comparisons
/// work as expected (Calm < Preventive < … < Critical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureLevel {
    /// < 60% — no action needed.
    Calm,
    /// 60–70% — start preventive compaction.
    Preventive,
    /// 70–80% — warn and compact aggressively.
    Warning,
    /// 80–85% — high urgency, escalate strategy.
    High,
    /// ≥ 85% — critical; force final reply after compaction.
    Critical,
}

impl PressureLevel {
    /// Derive a `PressureLevel` from a fill ratio in `[0.0, 1.0+]`.
    pub fn from_ratio(ratio: f64) -> Self {
        if ratio >= 0.85 {
            Self::Critical
        } else if ratio >= 0.80 {
            Self::High
        } else if ratio >= 0.70 {
            Self::Warning
        } else if ratio >= 0.60 {
            Self::Preventive
        } else {
            Self::Calm
        }
    }
}

// =============================================================================
// TokenEstimate
// =============================================================================

/// Estimated savings that a compaction strategy expects to free.
#[derive(Debug, Clone, Copy)]
pub struct TokenEstimate {
    /// Number of tokens the strategy expects to free.
    pub estimated_savings: usize,
    /// Confidence in the estimate, in `[0.0, 1.0]`.
    pub confidence: f32,
}

// =============================================================================
// CompactionContext
// =============================================================================

/// Runtime context handed to a `CompactionStrategy` before execution.
#[derive(Debug, Clone)]
pub struct CompactionContext {
    /// Current conversation messages available for compaction.
    pub messages: Vec<UnifiedMessage>,
    /// Raw pressure snapshot from the context budget.
    pub pressure: ContextPressure,
    /// Discrete pressure level derived from `pressure.ratio`.
    pub pressure_level: PressureLevel,
    /// Characters-per-token ratio used for token estimation.
    pub token_estimate_ratio: f64,
    /// Number of recent messages to leave untouched (the "fresh tail").
    pub fresh_tail_count: usize,
}

// =============================================================================
// CompactionResult
// =============================================================================

/// Outcome produced by a `CompactionStrategy` after it runs.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Estimated number of tokens freed by this compaction pass.
    pub freed_tokens: usize,
    /// Number of messages compacted (removed, merged, or summarised).
    pub compacted_count: usize,
    /// Human-readable name of the strategy that produced this result.
    pub strategy_name: String,
    /// Pressure level before compaction.
    pub pressure_before: PressureLevel,
    /// Pressure level after compaction (caller updates this).
    pub pressure_after: PressureLevel,
}

impl CompactionResult {
    /// Returns `true` if compaction actually reduced the pressure level.
    pub fn pressure_reduced(&self) -> bool {
        self.pressure_after < self.pressure_before
    }
}

// =============================================================================
// CompactionStrategy
// =============================================================================

/// A pluggable compaction strategy that can inspect context pressure and
/// selectively compact messages to free token budget.
///
/// # Object Safety
///
/// `execute` returns a boxed future so that implementations remain
/// object-safe and can be stored as `Box<dyn CompactionStrategy>`.
pub trait CompactionStrategy: Send + Sync {
    /// Short, stable name for logging and metrics (e.g. `"micro_compactor"`).
    fn name(&self) -> &str;

    /// Estimate how many tokens this strategy could free given the current
    /// context, without actually performing any mutations.
    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate;

    /// Execute the compaction strategy and return a result describing what
    /// was freed.  The implementation must not modify `ctx` in place;
    /// instead, callers replace the message list with the returned messages
    /// embedded in `CompactionResult` (or a separate out-parameter in
    /// concrete orchestrators).
    ///
    /// Returns `Err` only for unrecoverable internal failures; a strategy
    /// that simply found nothing to compact should return `Ok` with
    /// `freed_tokens == 0`.
    fn execute<'a>(
        &'a self,
        ctx: &'a CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>>;

    /// Whether this strategy is applicable given the current pressure level.
    /// Callers use this to skip strategies that are not relevant.
    fn is_applicable(&self, level: PressureLevel) -> bool;
}

// =============================================================================
// PostCompactCleanup
// =============================================================================

/// A hook that runs after a compaction pass completes.
///
/// Multiple hooks can be registered; they are executed in ascending
/// `cleanup_order()` order so that deterministic sequencing is preserved.
pub trait PostCompactCleanup: Send + Sync {
    /// Execution order relative to other cleanup hooks (lower = earlier).
    fn cleanup_order(&self) -> u32;

    /// Called once per successful compaction pass with the aggregated result.
    fn on_compact_complete(&self, result: &CompactionResult);
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_level_ordering() {
        assert!(PressureLevel::Calm < PressureLevel::Preventive);
        assert!(PressureLevel::Preventive < PressureLevel::Warning);
        assert!(PressureLevel::Warning < PressureLevel::High);
        assert!(PressureLevel::High < PressureLevel::Critical);
    }

    #[test]
    fn pressure_level_from_ratio() {
        assert_eq!(PressureLevel::from_ratio(0.50), PressureLevel::Calm);
        assert_eq!(PressureLevel::from_ratio(0.65), PressureLevel::Preventive);
        assert_eq!(PressureLevel::from_ratio(0.75), PressureLevel::Warning);
        assert_eq!(PressureLevel::from_ratio(0.82), PressureLevel::High);
        assert_eq!(PressureLevel::from_ratio(0.90), PressureLevel::Critical);
    }

    #[test]
    fn compaction_result_reports_success() {
        let reduced = CompactionResult {
            freed_tokens: 500,
            compacted_count: 3,
            strategy_name: "test_strategy".to_string(),
            pressure_before: PressureLevel::Warning,
            pressure_after: PressureLevel::Calm,
        };
        assert!(reduced.pressure_reduced());

        let not_reduced = CompactionResult {
            freed_tokens: 10,
            compacted_count: 1,
            strategy_name: "test_strategy".to_string(),
            pressure_before: PressureLevel::Warning,
            pressure_after: PressureLevel::Warning,
        };
        assert!(!not_reduced.pressure_reduced());

        let worsened = CompactionResult {
            freed_tokens: 0,
            compacted_count: 0,
            strategy_name: "test_strategy".to_string(),
            pressure_before: PressureLevel::Calm,
            pressure_after: PressureLevel::High,
        };
        assert!(!worsened.pressure_reduced());
    }
}

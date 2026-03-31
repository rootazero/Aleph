//! Context Budget — pressure sensing, compaction circuit breaker, and diminishing returns detection.
//!
//! This module replaces the old `ToolCompactorConfig` with a richer abstraction
//! that tracks context window pressure across turns and issues directives to the
//! agent loop (compact, force final reply, or stop on diminishing returns).

pub mod pressure;
pub mod pipeline;
pub mod diagnostics;

use crate::memory::session_compactor::context_window::{estimate_tokens, estimate_total_tokens};
use crate::providers::message::UnifiedMessage;
use super::tool::ToolDefinition;

// =============================================================================
// ContextPressure
// =============================================================================

/// Snapshot of context window utilization at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextPressure {
    /// Estimated tokens currently consumed by messages.
    pub used_tokens: usize,
    /// Total token budget for the model.
    pub budget_tokens: usize,
    /// Ratio of used / budget (0.0 .. 1.0+).
    pub ratio: f64,
}

impl ContextPressure {
    pub(crate) fn compute(
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
        token_budget: u64,
        ratio: f64,
    ) -> Self {
        let prompt_tokens = estimate_tokens(system_prompt, ratio);
        let tool_tokens: usize = tool_defs
            .iter()
            .map(|td| {
                estimate_tokens(&td.name, ratio)
                    + estimate_tokens(&td.description, ratio)
                    + estimate_tokens(&td.parameters.to_string(), ratio)
            })
            .sum();
        let msg_tokens = estimate_total_tokens(messages, ratio);
        let used = prompt_tokens + tool_tokens + msg_tokens;
        let budget = token_budget as usize;
        Self {
            used_tokens: used,
            budget_tokens: budget,
            ratio: if budget == 0 {
                1.0
            } else {
                used as f64 / budget as f64
            },
        }
    }
}

// =============================================================================
// LoopDirective
// =============================================================================

/// Directive issued by the context budget to the agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDirective {
    /// Context is within budget — proceed normally.
    Continue,
    /// Context exceeds warning threshold — compact tool results before the next LLM call.
    CompactAndContinue,
    /// Context is critically full — force compaction, inject a system notice, skip tools.
    FinalReply,
    /// Diminishing returns detected — inject a notice and stop tool execution.
    StopDiminishing,
}

// =============================================================================
// TurnMetrics
// =============================================================================

/// Metrics collected after each turn for diminishing returns detection.
#[derive(Debug, Clone)]
pub struct TurnMetrics {
    /// Number of output tokens the LLM produced this turn.
    pub output_tokens: usize,
    /// Number of tool calls the LLM requested this turn.
    pub tool_calls: usize,
    /// Whether the turn was considered productive (tools ran without errors).
    pub productive: bool,
}

// =============================================================================
// ContextBudgetConfig
// =============================================================================

/// Configuration for constructing a `ContextBudget`.
#[derive(Debug, Clone)]
pub struct ContextBudgetConfig {
    /// Total token budget for the model context window.
    pub token_budget: u64,
    /// Fraction of budget at which compaction triggers (e.g. 0.70).
    pub warning_threshold: f64,
    /// Fraction of budget at which we force a final reply (e.g. 0.85).
    pub critical_threshold: f64,
    /// Characters-per-token ratio for estimation.
    pub token_estimate_ratio: f64,
    /// Number of recent messages to leave untouched during compaction.
    pub fresh_tail_count: usize,
    /// Max consecutive compaction attempts before circuit breaker trips.
    pub circuit_breaker_max: usize,
    /// Window size for diminishing returns detection.
    pub diminishing_window: usize,
    /// Minimum total output tokens in the window to be considered productive.
    pub diminishing_threshold: usize,
}

// =============================================================================
// CompactionCircuitBreaker
// =============================================================================

/// Tracks consecutive compaction attempts. If compaction keeps firing without
/// the pressure dropping, we escalate to FinalReply instead of looping forever.
#[derive(Debug)]
struct CompactionCircuitBreaker {
    max_consecutive: usize,
    consecutive_count: usize,
}

impl CompactionCircuitBreaker {
    fn new(max: usize) -> Self {
        Self {
            max_consecutive: max,
            consecutive_count: 0,
        }
    }

    /// Record that compaction was triggered. Returns true if the breaker has tripped.
    fn record_compaction(&mut self) -> bool {
        self.consecutive_count += 1;
        self.consecutive_count >= self.max_consecutive
    }

    /// Reset the counter (called when pressure drops below warning, or after
    /// a compaction that actually reduced pressure).
    fn reset(&mut self) {
        self.consecutive_count = 0;
    }

    /// Record that a compaction succeeded in reducing pressure.
    /// Resets the counter so the breaker re-arms.
    fn record_success(&mut self) {
        self.consecutive_count = 0;
    }
}

// =============================================================================
// DiminishingReturnsDetector
// =============================================================================

/// Sliding window detector for unproductive turns.
///
/// Uses a bounded `VecDeque` to avoid unbounded memory growth in long loops.
#[derive(Debug)]
struct DiminishingReturnsDetector {
    window_size: usize,
    threshold: usize,
    history: std::collections::VecDeque<TurnMetrics>,
}

impl DiminishingReturnsDetector {
    fn new(window_size: usize, threshold: usize) -> Self {
        Self {
            window_size,
            threshold,
            history: std::collections::VecDeque::with_capacity(window_size),
        }
    }

    /// Record a turn's metrics and return true if diminishing returns detected.
    fn record(&mut self, metrics: TurnMetrics) -> bool {
        if self.history.len() >= self.window_size {
            self.history.pop_front();
        }
        self.history.push_back(metrics);
        if self.history.len() < self.window_size {
            return false;
        }
        let total_output: usize = self.history.iter().map(|m| m.output_tokens).sum();
        let any_productive = self.history.iter().any(|m| m.productive);
        // Diminishing if: no productive turns AND total output below threshold
        !any_productive && total_output < self.threshold
    }
}

// =============================================================================
// ContextBudget
// =============================================================================

/// Orchestrator that combines pressure sensing, circuit breaking, and
/// diminishing returns detection to issue directives to the agent loop.
#[derive(Debug)]
pub struct ContextBudget {
    token_budget: u64,
    warning_threshold: f64,
    critical_threshold: f64,
    token_estimate_ratio: f64,
    fresh_tail_count: usize,
    circuit_breaker: CompactionCircuitBreaker,
    diminishing: DiminishingReturnsDetector,
}

impl ContextBudget {
    /// Create a new context budget from configuration.
    pub fn new(config: &ContextBudgetConfig) -> Self {
        Self {
            token_budget: config.token_budget,
            warning_threshold: config.warning_threshold,
            critical_threshold: config.critical_threshold,
            token_estimate_ratio: config.token_estimate_ratio,
            fresh_tail_count: config.fresh_tail_count,
            circuit_breaker: CompactionCircuitBreaker::new(config.circuit_breaker_max),
            diminishing: DiminishingReturnsDetector::new(
                config.diminishing_window,
                config.diminishing_threshold,
            ),
        }
    }

    /// Total token budget.
    pub fn token_budget(&self) -> u64 {
        self.token_budget
    }

    /// Characters-per-token ratio.
    pub fn token_estimate_ratio(&self) -> f64 {
        self.token_estimate_ratio
    }

    /// Warning threshold fraction.
    pub fn warning_threshold(&self) -> f64 {
        self.warning_threshold
    }

    /// Fresh tail count for compaction.
    pub fn fresh_tail_count(&self) -> usize {
        self.fresh_tail_count
    }

    /// Evaluate context pressure before a turn and return a directive.
    pub fn before_turn(
        &mut self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[ToolDefinition],
    ) -> LoopDirective {
        let pressure = ContextPressure::compute(
            messages,
            system_prompt,
            tool_defs,
            self.token_budget,
            self.token_estimate_ratio,
        );

        if pressure.ratio >= self.critical_threshold {
            // Critical — force final reply regardless of circuit breaker
            tracing::warn!(
                target: "context_budget",
                used = pressure.used_tokens,
                budget = pressure.budget_tokens,
                ratio = pressure.ratio,
                "Critical context pressure — forcing final reply"
            );
            return LoopDirective::FinalReply;
        }

        if pressure.ratio >= self.warning_threshold {
            // Warning — compact, but check circuit breaker
            if self.circuit_breaker.record_compaction() {
                tracing::warn!(
                    target: "context_budget",
                    "Compaction circuit breaker tripped — escalating to FinalReply"
                );
                return LoopDirective::FinalReply;
            }
            tracing::info!(
                target: "context_budget",
                used = pressure.used_tokens,
                budget = pressure.budget_tokens,
                ratio = pressure.ratio,
                "Warning context pressure — requesting compaction"
            );
            return LoopDirective::CompactAndContinue;
        }

        // Under threshold — reset circuit breaker
        self.circuit_breaker.reset();
        LoopDirective::Continue
    }

    /// Notify that a compaction succeeded in reducing pressure.
    /// Resets the circuit breaker so it re-arms for future compaction attempts.
    pub fn notify_compaction_success(&mut self) {
        self.circuit_breaker.record_success();
    }

    /// Record post-turn metrics and return a directive if diminishing returns detected.
    pub fn after_turn(&mut self, metrics: TurnMetrics) -> LoopDirective {
        if self.diminishing.record(metrics) {
            tracing::warn!(
                target: "context_budget",
                "Diminishing returns detected — requesting stop"
            );
            return LoopDirective::StopDiminishing;
        }
        LoopDirective::Continue
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ContextBudgetConfig {
        ContextBudgetConfig {
            token_budget: 10_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
        }
    }

    #[test]
    fn test_context_pressure_compute() {
        let msgs = vec![UnifiedMessage::user("Hello world")];
        let pressure = ContextPressure::compute(&msgs, "system", &[], 1000, 3.5);
        assert!(pressure.ratio < 1.0);
        assert!(pressure.used_tokens > 0);
        assert_eq!(pressure.budget_tokens, 1000);
    }

    #[test]
    fn test_loop_directive_continue_under_threshold() {
        let config = default_config();
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("short")];
        let directive = budget.before_turn(&msgs, "sys", &[]);
        assert_eq!(directive, LoopDirective::Continue);
    }

    #[test]
    fn test_circuit_breaker_trips() {
        let mut cb = CompactionCircuitBreaker::new(3);
        assert!(!cb.record_compaction());
        assert!(!cb.record_compaction());
        assert!(cb.record_compaction()); // 3rd time trips
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_compaction();
        cb.record_compaction();
        cb.reset();
        assert!(!cb.record_compaction()); // reset, so starts from 1
    }

    #[test]
    fn test_diminishing_returns_not_enough_history() {
        let mut dr = DiminishingReturnsDetector::new(4, 500);
        // Only 2 unproductive turns — not enough for window of 4
        assert!(!dr.record(TurnMetrics { output_tokens: 10, tool_calls: 1, productive: false }));
        assert!(!dr.record(TurnMetrics { output_tokens: 10, tool_calls: 1, productive: false }));
    }

    #[test]
    fn test_diminishing_returns_triggers() {
        let mut dr = DiminishingReturnsDetector::new(4, 500);
        for _ in 0..4 {
            dr.record(TurnMetrics {
                output_tokens: 50,
                tool_calls: 1,
                productive: false,
            });
        }
        // Window of 4 unproductive turns with 200 total tokens < 500 threshold
        // The 4th call already returned the result, let's check with a 5th
        let triggered = dr.record(TurnMetrics {
            output_tokens: 50,
            tool_calls: 1,
            productive: false,
        });
        assert!(triggered);
    }

    #[test]
    fn test_diminishing_returns_productive_resets() {
        let mut dr = DiminishingReturnsDetector::new(4, 500);
        for _ in 0..3 {
            dr.record(TurnMetrics {
                output_tokens: 10,
                tool_calls: 1,
                productive: false,
            });
        }
        // One productive turn in window prevents triggering
        let triggered = dr.record(TurnMetrics {
            output_tokens: 10,
            tool_calls: 1,
            productive: true,
        });
        assert!(!triggered);
    }

    #[test]
    fn test_after_turn_diminishing() {
        let config = ContextBudgetConfig {
            diminishing_window: 2,
            diminishing_threshold: 100,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        budget.after_turn(TurnMetrics { output_tokens: 10, tool_calls: 1, productive: false });
        let directive = budget.after_turn(TurnMetrics { output_tokens: 10, tool_calls: 1, productive: false });
        assert_eq!(directive, LoopDirective::StopDiminishing);
    }

    // --- before_turn pressure path tests ---

    #[test]
    fn test_before_turn_warning_returns_compact() {
        // token_estimate_ratio=1.0 → 1 char = 1 token, budget=1000
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // 750 chars = 75% usage → Warning zone
        let msgs = vec![UnifiedMessage::user("x".repeat(750))];
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::CompactAndContinue);
    }

    #[test]
    fn test_before_turn_critical_returns_final_reply() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // 900 chars = 90% usage → Critical zone
        let msgs = vec![UnifiedMessage::user("x".repeat(900))];
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::FinalReply);
    }

    #[test]
    fn test_before_turn_circuit_breaker_escalates_to_final_reply() {
        let config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            circuit_breaker_max: 2,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        // 750 chars = Warning zone, stays in warning for 2+ turns → breaker trips
        let msgs = vec![UnifiedMessage::user("x".repeat(750))];
        let d1 = budget.before_turn(&msgs, "", &[]);
        assert_eq!(d1, LoopDirective::CompactAndContinue);
        let d2 = budget.before_turn(&msgs, "", &[]);
        assert_eq!(d2, LoopDirective::FinalReply); // breaker tripped on 2nd attempt
    }

    #[test]
    fn test_before_turn_zero_budget_is_critical() {
        let config = ContextBudgetConfig {
            token_budget: 0,
            ..default_config()
        };
        let mut budget = ContextBudget::new(&config);
        let msgs = vec![UnifiedMessage::user("hello")];
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::FinalReply);
    }
}

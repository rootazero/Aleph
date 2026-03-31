//! Context budget management for the agent loop.
//!
//! Provides multi-tier pressure sensing, compaction circuit breaking,
//! and per-turn diminishing returns detection. The [`ContextBudget`]
//! orchestrator returns a [`LoopDirective`] each turn to guide the
//! main loop's control flow.

use std::collections::VecDeque;

use crate::memory::session_compactor::context_window::estimate_tokens;
use crate::providers::message::UnifiedMessage;

// =============================================================================
// ContextPressure + LoopDirective
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPressure {
    Normal,
    Warning,
    Critical,
}

fn evaluate_pressure(
    used_tokens: usize,
    budget: usize,
    warning_threshold: f64,
    critical_threshold: f64,
) -> ContextPressure {
    if budget == 0 {
        return ContextPressure::Critical;
    }
    let ratio = used_tokens as f64 / budget as f64;
    if ratio >= critical_threshold {
        ContextPressure::Critical
    } else if ratio >= warning_threshold {
        ContextPressure::Warning
    } else {
        ContextPressure::Normal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDirective {
    Continue,
    CompactAndContinue,
    FinalReply,
    StopDiminishing,
}

// =============================================================================
// CompactionCircuitBreaker
// =============================================================================

#[derive(Debug)]
struct CompactionCircuitBreaker {
    consecutive_failures: u32,
    max_failures: u32,
    tripped: bool,
}

impl CompactionCircuitBreaker {
    fn new(max_failures: u32) -> Self {
        Self {
            consecutive_failures: 0,
            max_failures,
            tripped: false,
        }
    }

    fn is_tripped(&self) -> bool {
        self.tripped
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.max_failures {
            self.tripped = true;
            tracing::warn!(
                target: "context_budget",
                failures = self.consecutive_failures,
                "Circuit breaker tripped — switching to deterministic compaction"
            );
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.tripped = false;
    }
}

// =============================================================================
// TurnMetrics + DiminishingReturnsDetector
// =============================================================================

#[derive(Debug, Clone)]
pub struct TurnMetrics {
    pub output_tokens: usize,
    pub tool_calls: usize,
    pub productive: bool,
}

#[derive(Debug)]
struct DiminishingReturnsDetector {
    window: VecDeque<TurnMetrics>,
    window_size: usize,
    low_output_threshold: usize,
}

impl DiminishingReturnsDetector {
    fn new(window_size: usize, low_output_threshold: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size),
            window_size,
            low_output_threshold,
        }
    }

    fn record(&mut self, metrics: TurnMetrics) {
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        self.window.push_back(metrics);
    }

    fn is_diminishing(&self) -> bool {
        if self.window.len() < self.window_size {
            return false;
        }
        let total_output: usize = self.window.iter().map(|m| m.output_tokens).sum();
        let avg_output = total_output / self.window.len();
        let unproductive_count = self.window.iter().filter(|m| !m.productive).count();
        let unproductive_ratio = unproductive_count as f64 / self.window.len() as f64;
        avg_output < self.low_output_threshold && unproductive_ratio >= 0.75
    }
}

// =============================================================================
// ContextBudgetConfig + ContextBudget
// =============================================================================

#[derive(Debug, Clone)]
pub struct ContextBudgetConfig {
    pub token_budget: u64,
    pub warning_threshold: f64,
    pub critical_threshold: f64,
    pub token_estimate_ratio: f64,
    pub fresh_tail_count: usize,
    pub circuit_breaker_max: u32,
    pub diminishing_window: usize,
    pub diminishing_threshold: usize,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            token_budget: 500_000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 6,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
        }
    }
}

#[derive(Debug)]
pub struct ContextBudget {
    token_budget: usize,
    warning_threshold: f64,
    critical_threshold: f64,
    token_estimate_ratio: f64,
    fresh_tail_count: usize,
    circuit_breaker: CompactionCircuitBreaker,
    diminishing: DiminishingReturnsDetector,
    pressure: ContextPressure,
}

impl ContextBudget {
    pub fn new(config: &ContextBudgetConfig) -> Self {
        Self {
            token_budget: config.token_budget as usize,
            warning_threshold: config.warning_threshold,
            critical_threshold: config.critical_threshold,
            token_estimate_ratio: config.token_estimate_ratio,
            fresh_tail_count: config.fresh_tail_count,
            circuit_breaker: CompactionCircuitBreaker::new(config.circuit_breaker_max),
            diminishing: DiminishingReturnsDetector::new(
                config.diminishing_window,
                config.diminishing_threshold,
            ),
            pressure: ContextPressure::Normal,
        }
    }

    pub fn before_turn(
        &mut self,
        messages: &[UnifiedMessage],
        system_prompt: &str,
        tool_defs: &[crate::agent_loop::tool::ToolDefinition],
    ) -> LoopDirective {
        let ratio = self.token_estimate_ratio;
        let msg_tokens: usize = messages
            .iter()
            .map(|m| estimate_tokens(&m.text_content(), ratio))
            .sum();
        let prompt_tokens = estimate_tokens(system_prompt, ratio);
        let tool_tokens: usize = tool_defs
            .iter()
            .map(|td| {
                estimate_tokens(&td.name, ratio)
                    + estimate_tokens(&td.description, ratio)
                    + estimate_tokens(&td.parameters.to_string(), ratio)
            })
            .sum();
        let total = msg_tokens + prompt_tokens + tool_tokens;

        self.pressure = evaluate_pressure(
            total,
            self.token_budget,
            self.warning_threshold,
            self.critical_threshold,
        );

        match self.pressure {
            ContextPressure::Normal => LoopDirective::Continue,
            ContextPressure::Warning => {
                tracing::warn!(
                    target: "context_budget",
                    total_tokens = total,
                    budget = self.token_budget,
                    "Context pressure: Warning — triggering compaction"
                );
                LoopDirective::CompactAndContinue
            }
            ContextPressure::Critical => {
                tracing::warn!(
                    target: "context_budget",
                    total_tokens = total,
                    budget = self.token_budget,
                    "Context pressure: Critical — requesting final reply"
                );
                LoopDirective::FinalReply
            }
        }
    }

    pub fn after_turn(&mut self, metrics: TurnMetrics) -> LoopDirective {
        self.diminishing.record(metrics);
        if self.diminishing.is_diminishing() {
            tracing::warn!(
                target: "context_budget",
                "Diminishing returns detected — recommending stop"
            );
            LoopDirective::StopDiminishing
        } else {
            LoopDirective::Continue
        }
    }

    pub fn on_compaction_success(&mut self) {
        self.circuit_breaker.record_success();
    }

    pub fn on_compaction_failure(&mut self) {
        self.circuit_breaker.record_failure();
    }

    pub fn is_compaction_tripped(&self) -> bool {
        self.circuit_breaker.is_tripped()
    }

    pub fn pressure(&self) -> ContextPressure {
        self.pressure
    }

    pub fn token_estimate_ratio(&self) -> f64 {
        self.token_estimate_ratio
    }

    pub fn fresh_tail_count(&self) -> usize {
        self.fresh_tail_count
    }

    pub fn token_budget(&self) -> u64 {
        self.token_budget as u64
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Pressure tests
    // -------------------------------------------------------------------------

    #[test]
    fn pressure_normal_when_under_warning() {
        assert_eq!(
            evaluate_pressure(5000, 10000, 0.70, 0.85),
            ContextPressure::Normal
        );
    }

    #[test]
    fn pressure_warning_at_boundary() {
        assert_eq!(
            evaluate_pressure(7000, 10000, 0.70, 0.85),
            ContextPressure::Warning
        );
    }

    #[test]
    fn pressure_warning_between_thresholds() {
        assert_eq!(
            evaluate_pressure(8000, 10000, 0.70, 0.85),
            ContextPressure::Warning
        );
    }

    #[test]
    fn pressure_critical_at_boundary() {
        assert_eq!(
            evaluate_pressure(8500, 10000, 0.70, 0.85),
            ContextPressure::Critical
        );
    }

    #[test]
    fn pressure_critical_above() {
        assert_eq!(
            evaluate_pressure(9500, 10000, 0.70, 0.85),
            ContextPressure::Critical
        );
    }

    // -------------------------------------------------------------------------
    // Circuit breaker tests
    // -------------------------------------------------------------------------

    #[test]
    fn circuit_breaker_closed_initially() {
        let cb = CompactionCircuitBreaker::new(3);
        assert!(!cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_trips_after_max_failures() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_tripped());
        cb.record_failure();
        assert!(cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_tripped());
        cb.record_success();
        assert!(!cb.is_tripped());
    }

    #[test]
    fn circuit_breaker_stays_tripped_after_more_failures() {
        let mut cb = CompactionCircuitBreaker::new(3);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_tripped());
        cb.record_failure();
        assert!(cb.is_tripped());
    }

    // -------------------------------------------------------------------------
    // Diminishing returns tests
    // -------------------------------------------------------------------------

    #[test]
    fn diminishing_not_detected_when_window_not_full() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        d.record(TurnMetrics {
            output_tokens: 10,
            tool_calls: 0,
            productive: false,
        });
        d.record(TurnMetrics {
            output_tokens: 10,
            tool_calls: 0,
            productive: false,
        });
        assert!(!d.is_diminishing());
    }

    #[test]
    fn diminishing_detected_when_all_turns_low_and_unproductive() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        for _ in 0..4 {
            d.record(TurnMetrics {
                output_tokens: 50,
                tool_calls: 0,
                productive: false,
            });
        }
        assert!(d.is_diminishing());
    }

    #[test]
    fn diminishing_not_detected_when_productive() {
        // 50% productive — below the 75% unproductive threshold
        let mut d = DiminishingReturnsDetector::new(4, 500);
        for i in 0..4 {
            d.record(TurnMetrics {
                output_tokens: 50,
                tool_calls: 0,
                productive: i % 2 == 0,
            });
        }
        assert!(!d.is_diminishing());
    }

    #[test]
    fn diminishing_not_detected_when_high_output() {
        // avg 1000 > threshold 500
        let mut d = DiminishingReturnsDetector::new(4, 500);
        for _ in 0..4 {
            d.record(TurnMetrics {
                output_tokens: 1000,
                tool_calls: 0,
                productive: false,
            });
        }
        assert!(!d.is_diminishing());
    }

    #[test]
    fn diminishing_sliding_window_evicts_old() {
        let mut d = DiminishingReturnsDetector::new(4, 500);
        // Fill with low unproductive turns
        for _ in 0..4 {
            d.record(TurnMetrics {
                output_tokens: 10,
                tool_calls: 0,
                productive: false,
            });
        }
        assert!(d.is_diminishing());

        // Push 2 productive high-output turns — evicts 2 old ones
        for _ in 0..2 {
            d.record(TurnMetrics {
                output_tokens: 2000,
                tool_calls: 3,
                productive: true,
            });
        }
        // Now window has 2 low/unproductive + 2 high/productive
        // unproductive_ratio = 0.50 < 0.75, so not diminishing
        assert!(!d.is_diminishing());
    }

    // -------------------------------------------------------------------------
    // ContextBudget integration tests
    // -------------------------------------------------------------------------

    fn make_config(budget: u64) -> ContextBudgetConfig {
        ContextBudgetConfig {
            token_budget: budget,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 1.0,
            fresh_tail_count: 6,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
        }
    }

    fn make_messages(total_chars: usize) -> Vec<UnifiedMessage> {
        // With ratio 1.0, estimate_tokens returns chars.len() / 1 (rounded)
        // so we create a single message with the desired character count.
        let text = "x".repeat(total_chars);
        vec![UnifiedMessage::user(text)]
    }

    #[test]
    fn before_turn_returns_continue_when_normal() {
        let config = make_config(10000);
        let mut budget = ContextBudget::new(&config);
        let msgs = make_messages(5000);
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::Continue);
        assert_eq!(budget.pressure(), ContextPressure::Normal);
    }

    #[test]
    fn before_turn_returns_compact_when_warning() {
        let config = make_config(10000);
        let mut budget = ContextBudget::new(&config);
        let msgs = make_messages(7500);
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::CompactAndContinue);
        assert_eq!(budget.pressure(), ContextPressure::Warning);
    }

    #[test]
    fn before_turn_returns_final_reply_when_critical() {
        let config = make_config(10000);
        let mut budget = ContextBudget::new(&config);
        let msgs = make_messages(9000);
        let directive = budget.before_turn(&msgs, "", &[]);
        assert_eq!(directive, LoopDirective::FinalReply);
        assert_eq!(budget.pressure(), ContextPressure::Critical);
    }

    #[test]
    fn after_turn_returns_stop_when_diminishing() {
        let config = make_config(10000);
        let mut budget = ContextBudget::new(&config);
        for _ in 0..4 {
            let directive = budget.after_turn(TurnMetrics {
                output_tokens: 50,
                tool_calls: 0,
                productive: false,
            });
            // Only the 4th call should trigger StopDiminishing
            if budget.diminishing.window.len() < 4 {
                assert_eq!(directive, LoopDirective::Continue);
            }
        }
        // After 4 unproductive low-output turns
        let directive = budget.after_turn(TurnMetrics {
            output_tokens: 50,
            tool_calls: 0,
            productive: false,
        });
        assert_eq!(directive, LoopDirective::StopDiminishing);
    }

    #[test]
    fn circuit_breaker_exposed_via_budget() {
        let config = make_config(10000);
        let mut budget = ContextBudget::new(&config);
        assert!(!budget.is_compaction_tripped());

        // 3 failures → tripped
        budget.on_compaction_failure();
        budget.on_compaction_failure();
        budget.on_compaction_failure();
        assert!(budget.is_compaction_tripped());

        // success → reset
        budget.on_compaction_success();
        assert!(!budget.is_compaction_tripped());
    }
}

//! Budget management for POE execution.
//!
//! This module provides entropy-based budget tracking for POE execution cycles.
//! It monitors token consumption, attempt counts, and entropy trends to determine
//! when to stop retrying or when progress has stalled.
//!
//! # Key Concepts
//!
//! - **Entropy History**: Tracks distance scores from each attempt to measure progress
//! - **Stuck Detection**: Identifies when the system is making no meaningful progress
//! - **Budget Exhaustion**: Tracks when resource limits (attempts/tokens) are reached
//!
//! # Example
//!
//! ```rust
//! use alephcore::poe::budget::{PoeBudget, BudgetStatus};
//!
//! let mut budget = PoeBudget::new(5, 100_000);
//!
//! // Record attempts with decreasing distance scores (improvement)
//! budget.record_attempt(1000, 0.8);
//! budget.record_attempt(1500, 0.5);
//! budget.record_attempt(2000, 0.2);
//!
//! assert_eq!(budget.status(), BudgetStatus::Improving);
//! assert!(!budget.exhausted());
//! assert!(!budget.is_stuck(3));
//! ```

use serde::{Deserialize, Serialize};

// ============================================================================
// Budget Status
// ============================================================================

/// Status of the POE budget based on entropy trends and resource consumption.
///
/// This enum helps the POE manager decide whether to continue retrying,
/// switch strategies, or give up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetStatus {
    /// Entropy is decreasing - making progress toward the goal
    Improving,

    /// Entropy is stable - neither improving nor degrading significantly
    Stable,

    /// Entropy is increasing - getting further from the goal
    Degrading,

    /// No progress over multiple attempts - system is stuck in a loop
    Stuck,

    /// Budget limits (attempts or tokens) have been reached
    Exhausted,
}

impl BudgetStatus {
    /// Returns a human-readable description of the status.
    pub fn description(&self) -> &'static str {
        match self {
            BudgetStatus::Improving => "Making progress toward goal",
            BudgetStatus::Stable => "Progress is stable",
            BudgetStatus::Degrading => "Getting further from goal",
            BudgetStatus::Stuck => "No progress detected",
            BudgetStatus::Exhausted => "Budget limits reached",
        }
    }

    /// Returns true if this status suggests continuing execution.
    pub fn should_continue(&self) -> bool {
        matches!(self, BudgetStatus::Improving | BudgetStatus::Stable)
    }
}

// ============================================================================
// POE Budget
// ============================================================================

/// Entropy-based budget manager for POE execution.
///
/// Tracks resource consumption (attempts, tokens) and monitors entropy trends
/// to detect when the system is making progress, stuck, or exhausted.
///
/// # Entropy Tracking
///
/// Distance scores from validation are recorded in `entropy_history`. A decreasing
/// trend indicates improvement (getting closer to the goal), while an increasing
/// or flat trend indicates degradation or stagnation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeBudget {
    /// Maximum number of attempts allowed
    pub max_attempts: u8,

    /// Current attempt number (0-indexed)
    pub current_attempt: u8,

    /// Maximum tokens that can be consumed
    pub max_tokens: u32,

    /// Total tokens consumed so far
    pub tokens_used: u32,

    /// Distance scores from each attempt (entropy history)
    /// Lower is better (0.0 = perfect, 1.0 = complete failure)
    pub entropy_history: Vec<f32>,
}

impl PoeBudget {
    /// Create a new budget with the specified limits.
    ///
    /// # Arguments
    ///
    /// * `max_attempts` - Maximum number of retry attempts allowed
    /// * `max_tokens` - Maximum total tokens that can be consumed
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::poe::budget::PoeBudget;
    ///
    /// let budget = PoeBudget::new(5, 100_000);
    /// assert_eq!(budget.remaining_attempts(), 5);
    /// assert_eq!(budget.remaining_tokens(), 100_000);
    /// ```
    pub fn new(max_attempts: u8, max_tokens: u32) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            max_tokens,
            tokens_used: 0,
            entropy_history: Vec::new(),
        }
    }

    /// Check if the budget is exhausted.
    ///
    /// Returns `true` if either:
    /// - The number of attempts has reached or exceeded the maximum
    /// - The tokens used have reached or exceeded the maximum
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::poe::budget::PoeBudget;
    ///
    /// let mut budget = PoeBudget::new(2, 1000);
    /// assert!(!budget.exhausted());
    ///
    /// budget.record_attempt(500, 0.5);
    /// budget.record_attempt(500, 0.3);
    /// assert!(budget.exhausted()); // max_attempts reached
    /// ```
    pub fn exhausted(&self) -> bool {
        self.current_attempt >= self.max_attempts || self.tokens_used >= self.max_tokens
    }

    /// Check if the system is stuck (no entropy reduction over the last N attempts).
    ///
    /// The system is considered stuck if:
    /// - There are at least `window` entries in entropy history
    /// - The entropy trend over the window is non-negative (no improvement)
    ///
    /// # Arguments
    ///
    /// * `window` - Number of recent attempts to consider
    ///
    /// # Returns
    ///
    /// `true` if the system shows no progress over the specified window
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::poe::budget::PoeBudget;
    ///
    /// let mut budget = PoeBudget::new(10, 100_000);
    ///
    /// // Record flat entropy (no progress)
    /// budget.record_attempt(1000, 0.5);
    /// budget.record_attempt(1000, 0.5);
    /// budget.record_attempt(1000, 0.5);
    ///
    /// assert!(budget.is_stuck(3));
    /// ```
    pub fn is_stuck(&self, window: usize) -> bool {
        if self.entropy_history.len() < window || window < 2 {
            return false;
        }

        // Get the last `window` entropy values
        let recent: Vec<f32> = self.entropy_history
            .iter()
            .rev()
            .take(window)
            .copied()
            .collect();

        // Calculate variance to detect oscillation/plateau
        let mean: f32 = recent.iter().sum::<f32>() / recent.len() as f32;
        let variance: f32 = recent.iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f32>() / recent.len() as f32;

        // Also check that we're not clearly improving
        let trend = self.entropy_trend(window);

        // Stuck if:
        // 1. Low variance (oscillating around same value), AND
        // 2. Not clearly improving (trend not significantly negative)
        const VARIANCE_THRESHOLD: f32 = 0.01; // Low variance = stuck
        const IMPROVING_THRESHOLD: f32 = -0.05; // Clear improvement threshold

        variance < VARIANCE_THRESHOLD && trend >= IMPROVING_THRESHOLD
    }

    /// Record an attempt with its token consumption and distance score.
    ///
    /// # Arguments
    ///
    /// * `tokens` - Number of tokens consumed in this attempt
    /// * `distance_score` - Distance from success (0.0 = perfect, 1.0 = failure)
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::poe::budget::PoeBudget;
    ///
    /// let mut budget = PoeBudget::new(5, 100_000);
    /// budget.record_attempt(1500, 0.7);
    ///
    /// assert_eq!(budget.current_attempt, 1);
    /// assert_eq!(budget.tokens_used, 1500);
    /// assert_eq!(budget.entropy_history, vec![0.7]);
    /// ```
    pub fn record_attempt(&mut self, tokens: u32, distance_score: f32) {
        self.current_attempt = self.current_attempt.saturating_add(1);
        self.tokens_used = self.tokens_used.saturating_add(tokens);
        self.entropy_history.push(distance_score.clamp(0.0, 1.0));
    }

    /// Get the number of remaining attempts.
    ///
    /// # Returns
    ///
    /// The number of attempts remaining before exhaustion
    pub fn remaining_attempts(&self) -> u8 {
        self.max_attempts.saturating_sub(self.current_attempt)
    }

    /// Get the number of remaining tokens.
    ///
    /// # Returns
    ///
    /// The number of tokens remaining before exhaustion
    pub fn remaining_tokens(&self) -> u32 {
        self.max_tokens.saturating_sub(self.tokens_used)
    }

    /// Calculate the entropy trend over the last N attempts.
    ///
    /// Uses linear regression to determine the slope of entropy over time.
    ///
    /// # Arguments
    ///
    /// * `window` - Number of recent attempts to consider
    ///
    /// # Returns
    ///
    /// - Negative value: Entropy is decreasing (improving)
    /// - Zero: Entropy is stable
    /// - Positive value: Entropy is increasing (degrading)
    ///
    /// Returns 0.0 if there are fewer than 2 data points in the window.
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::poe::budget::PoeBudget;
    ///
    /// let mut budget = PoeBudget::new(10, 100_000);
    ///
    /// // Improving trend (entropy decreasing)
    /// budget.record_attempt(1000, 0.8);
    /// budget.record_attempt(1000, 0.5);
    /// budget.record_attempt(1000, 0.2);
    ///
    /// let trend = budget.entropy_trend(3);
    /// assert!(trend < 0.0); // Negative means improving
    /// ```
    pub fn entropy_trend(&self, window: usize) -> f32 {
        let n = self.entropy_history.len();

        if n < 2 || window < 2 {
            return 0.0;
        }

        // Get the last `window` entries (or all if fewer)
        let actual_window = window.min(n);
        let start = n.saturating_sub(actual_window);
        let values = &self.entropy_history[start..];

        // Linear regression: calculate slope
        // slope = (n * sum(xy) - sum(x) * sum(y)) / (n * sum(x^2) - sum(x)^2)
        let n_f = values.len() as f32;

        let mut sum_x: f32 = 0.0;
        let mut sum_y: f32 = 0.0;
        let mut sum_xy: f32 = 0.0;
        let mut sum_x2: f32 = 0.0;

        for (i, &y) in values.iter().enumerate() {
            let x = i as f32;
            sum_x += x;
            sum_y += y;
            sum_xy += x * y;
            sum_x2 += x * x;
        }

        let denominator = n_f * sum_x2 - sum_x * sum_x;

        if denominator.abs() < f32::EPSILON {
            // Degenerate case (all x values are the same, which shouldn't happen)
            return 0.0;
        }

        (n_f * sum_xy - sum_x * sum_y) / denominator
    }

    /// Get the current budget status based on entropy trends and resource usage.
    ///
    /// # Returns
    ///
    /// - `Exhausted`: If budget limits have been reached
    /// - `Stuck`: If no progress over the last 3 attempts
    /// - `Degrading`: If entropy trend is positive (getting worse)
    /// - `Improving`: If entropy trend is negative (getting better)
    /// - `Stable`: Otherwise (small or no change)
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::poe::budget::{PoeBudget, BudgetStatus};
    ///
    /// let mut budget = PoeBudget::new(5, 100_000);
    /// budget.record_attempt(1000, 0.8);
    /// budget.record_attempt(1000, 0.5);
    ///
    /// assert_eq!(budget.status(), BudgetStatus::Improving);
    /// ```
    pub fn status(&self) -> BudgetStatus {
        // Check exhaustion first
        if self.exhausted() {
            return BudgetStatus::Exhausted;
        }

        // Check if stuck (using default window of 3)
        const STUCK_WINDOW: usize = 3;
        if self.is_stuck(STUCK_WINDOW) {
            return BudgetStatus::Stuck;
        }

        // Analyze entropy trend
        if self.entropy_history.len() < 2 {
            // Not enough data to determine trend
            return BudgetStatus::Stable;
        }

        let trend = self.entropy_trend(self.entropy_history.len());

        // Thresholds for determining status
        const IMPROVING_THRESHOLD: f32 = -0.05;
        const DEGRADING_THRESHOLD: f32 = 0.05;

        if trend < IMPROVING_THRESHOLD {
            BudgetStatus::Improving
        } else if trend > DEGRADING_THRESHOLD {
            BudgetStatus::Degrading
        } else {
            BudgetStatus::Stable
        }
    }

    /// Get the best (lowest) distance score achieved so far.
    ///
    /// # Returns
    ///
    /// The minimum distance score, or `None` if no attempts have been made.
    pub fn best_score(&self) -> Option<f32> {
        self.entropy_history
            .iter()
            .copied()
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Get the most recent distance score.
    ///
    /// # Returns
    ///
    /// The last recorded distance score, or `None` if no attempts have been made.
    pub fn latest_score(&self) -> Option<f32> {
        self.entropy_history.last().copied()
    }

    /// Calculate the average distance score across all attempts.
    ///
    /// # Returns
    ///
    /// The mean distance score, or `None` if no attempts have been made.
    pub fn average_score(&self) -> Option<f32> {
        if self.entropy_history.is_empty() {
            return None;
        }

        let sum: f32 = self.entropy_history.iter().sum();
        Some(sum / self.entropy_history.len() as f32)
    }

    /// Reset the budget to its initial state while keeping the same limits.
    ///
    /// This clears:
    /// - Current attempt counter
    /// - Tokens used
    /// - Entropy history
    pub fn reset(&mut self) {
        self.current_attempt = 0;
        self.tokens_used = 0;
        self.entropy_history.clear();
    }
}

impl Default for PoeBudget {
    /// Create a budget with default values.
    ///
    /// Defaults:
    /// - max_attempts: 5
    /// - max_tokens: 100,000
    fn default() -> Self {
        Self {
            max_attempts: 5,
            current_attempt: 0,
            max_tokens: 100_000,
            tokens_used: 0,
            entropy_history: Vec::new(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_new() {
        let budget = PoeBudget::new(10, 50_000);

        assert_eq!(budget.max_attempts, 10);
        assert_eq!(budget.current_attempt, 0);
        assert_eq!(budget.max_tokens, 50_000);
        assert_eq!(budget.tokens_used, 0);
        assert!(budget.entropy_history.is_empty());
    }

    #[test]
    fn test_budget_default() {
        let budget = PoeBudget::default();

        assert_eq!(budget.max_attempts, 5);
        assert_eq!(budget.max_tokens, 100_000);
        assert_eq!(budget.current_attempt, 0);
        assert_eq!(budget.tokens_used, 0);
    }

    #[test]
    fn test_budget_exhausted_by_attempts() {
        let mut budget = PoeBudget::new(2, 100_000);

        assert!(!budget.exhausted());

        budget.record_attempt(100, 0.5);
        assert!(!budget.exhausted());

        budget.record_attempt(100, 0.4);
        assert!(budget.exhausted()); // 2 attempts reached
    }

    #[test]
    fn test_budget_exhausted_by_tokens() {
        let mut budget = PoeBudget::new(10, 1000);

        assert!(!budget.exhausted());

        budget.record_attempt(500, 0.5);
        assert!(!budget.exhausted());

        budget.record_attempt(500, 0.4);
        assert!(budget.exhausted()); // 1000 tokens reached

        // Also test exceeding
        let mut budget2 = PoeBudget::new(10, 1000);
        budget2.record_attempt(1500, 0.5);
        assert!(budget2.exhausted()); // Exceeded in one attempt
    }

    #[test]
    fn test_is_stuck_detects_no_progress() {
        let mut budget = PoeBudget::new(10, 100_000);

        // Not enough data
        budget.record_attempt(1000, 0.5);
        assert!(!budget.is_stuck(3));

        // Still not enough
        budget.record_attempt(1000, 0.5);
        assert!(!budget.is_stuck(3));

        // Now we have 3 flat values - should be stuck
        budget.record_attempt(1000, 0.5);
        assert!(budget.is_stuck(3));

        // Test with slight oscillation (stuck - not making meaningful progress)
        // Note: Increasing entropy is "Degrading", not "Stuck"
        // "Stuck" specifically means oscillating without progress
        let mut budget2 = PoeBudget::new(10, 100_000);
        budget2.record_attempt(1000, 0.50);
        budget2.record_attempt(1000, 0.51);
        budget2.record_attempt(1000, 0.49);
        assert!(budget2.is_stuck(3));
    }

    #[test]
    fn test_is_stuck_with_improvement() {
        let mut budget = PoeBudget::new(10, 100_000);

        // Decreasing entropy - making progress
        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.5);
        budget.record_attempt(1000, 0.2);

        assert!(!budget.is_stuck(3));
    }

    #[test]
    fn test_entropy_trend() {
        // Test improving trend (negative slope)
        let mut budget = PoeBudget::new(10, 100_000);
        budget.record_attempt(1000, 1.0);
        budget.record_attempt(1000, 0.5);
        budget.record_attempt(1000, 0.0);

        let trend = budget.entropy_trend(3);
        assert!(trend < 0.0, "Expected negative trend, got {}", trend);

        // Test degrading trend (positive slope)
        let mut budget2 = PoeBudget::new(10, 100_000);
        budget2.record_attempt(1000, 0.0);
        budget2.record_attempt(1000, 0.5);
        budget2.record_attempt(1000, 1.0);

        let trend2 = budget2.entropy_trend(3);
        assert!(trend2 > 0.0, "Expected positive trend, got {}", trend2);

        // Test stable trend (zero slope)
        let mut budget3 = PoeBudget::new(10, 100_000);
        budget3.record_attempt(1000, 0.5);
        budget3.record_attempt(1000, 0.5);
        budget3.record_attempt(1000, 0.5);

        let trend3 = budget3.entropy_trend(3);
        assert!(
            trend3.abs() < 0.01,
            "Expected near-zero trend, got {}",
            trend3
        );
    }

    #[test]
    fn test_entropy_trend_window() {
        let mut budget = PoeBudget::new(10, 100_000);

        // Early history: improving
        budget.record_attempt(1000, 0.9);
        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.7);

        // Recent history: degrading
        budget.record_attempt(1000, 0.7);
        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.9);

        // Full window should be near zero (improved then degraded)
        // Last 3 should show degrading
        let trend_last_3 = budget.entropy_trend(3);
        assert!(
            trend_last_3 > 0.0,
            "Expected positive trend for last 3, got {}",
            trend_last_3
        );

        // First 3 should show improving
        let trend_first_3 = {
            let mut temp = PoeBudget::new(10, 100_000);
            temp.entropy_history = budget.entropy_history[0..3].to_vec();
            temp.entropy_trend(3)
        };
        assert!(
            trend_first_3 < 0.0,
            "Expected negative trend for first 3, got {}",
            trend_first_3
        );
    }

    #[test]
    fn test_remaining_resources() {
        let mut budget = PoeBudget::new(5, 10_000);

        assert_eq!(budget.remaining_attempts(), 5);
        assert_eq!(budget.remaining_tokens(), 10_000);

        budget.record_attempt(3000, 0.5);
        assert_eq!(budget.remaining_attempts(), 4);
        assert_eq!(budget.remaining_tokens(), 7000);

        budget.record_attempt(5000, 0.3);
        assert_eq!(budget.remaining_attempts(), 3);
        assert_eq!(budget.remaining_tokens(), 2000);
    }

    #[test]
    fn test_status_exhausted() {
        let mut budget = PoeBudget::new(2, 100_000);
        budget.record_attempt(1000, 0.5);
        budget.record_attempt(1000, 0.4);

        assert_eq!(budget.status(), BudgetStatus::Exhausted);
    }

    #[test]
    fn test_status_stuck() {
        let mut budget = PoeBudget::new(10, 100_000);
        budget.record_attempt(1000, 0.5);
        budget.record_attempt(1000, 0.5);
        budget.record_attempt(1000, 0.5);

        assert_eq!(budget.status(), BudgetStatus::Stuck);
    }

    #[test]
    fn test_status_improving() {
        let mut budget = PoeBudget::new(10, 100_000);
        budget.record_attempt(1000, 0.9);
        budget.record_attempt(1000, 0.6);
        budget.record_attempt(1000, 0.3);

        assert_eq!(budget.status(), BudgetStatus::Improving);
    }

    #[test]
    fn test_status_degrading() {
        let mut budget = PoeBudget::new(10, 100_000);
        budget.record_attempt(1000, 0.3);
        budget.record_attempt(1000, 0.6);
        budget.record_attempt(1000, 0.9);

        assert_eq!(budget.status(), BudgetStatus::Degrading);
    }

    #[test]
    fn test_status_stable() {
        let mut budget = PoeBudget::new(10, 100_000);

        // Not enough data - should be stable
        budget.record_attempt(1000, 0.5);
        assert_eq!(budget.status(), BudgetStatus::Stable);
    }

    #[test]
    fn test_record_attempt_clamps_distance() {
        let mut budget = PoeBudget::new(5, 100_000);

        budget.record_attempt(100, -0.5); // Should clamp to 0.0
        budget.record_attempt(100, 1.5); // Should clamp to 1.0

        assert_eq!(budget.entropy_history[0], 0.0);
        assert_eq!(budget.entropy_history[1], 1.0);
    }

    #[test]
    fn test_best_score() {
        let mut budget = PoeBudget::new(10, 100_000);

        assert!(budget.best_score().is_none());

        budget.record_attempt(1000, 0.8);
        budget.record_attempt(1000, 0.3);
        budget.record_attempt(1000, 0.5);

        assert_eq!(budget.best_score(), Some(0.3));
    }

    #[test]
    fn test_latest_score() {
        let mut budget = PoeBudget::new(10, 100_000);

        assert!(budget.latest_score().is_none());

        budget.record_attempt(1000, 0.8);
        assert_eq!(budget.latest_score(), Some(0.8));

        budget.record_attempt(1000, 0.3);
        assert_eq!(budget.latest_score(), Some(0.3));
    }

    #[test]
    fn test_average_score() {
        let mut budget = PoeBudget::new(10, 100_000);

        assert!(budget.average_score().is_none());

        budget.record_attempt(1000, 0.2);
        budget.record_attempt(1000, 0.4);
        budget.record_attempt(1000, 0.6);

        let avg = budget.average_score().unwrap();
        assert!((avg - 0.4).abs() < 0.001);
    }

    #[test]
    fn test_reset() {
        let mut budget = PoeBudget::new(5, 10_000);

        budget.record_attempt(1000, 0.5);
        budget.record_attempt(2000, 0.3);

        assert_eq!(budget.current_attempt, 2);
        assert_eq!(budget.tokens_used, 3000);
        assert_eq!(budget.entropy_history.len(), 2);

        budget.reset();

        assert_eq!(budget.current_attempt, 0);
        assert_eq!(budget.tokens_used, 0);
        assert!(budget.entropy_history.is_empty());
        assert_eq!(budget.max_attempts, 5); // Limits unchanged
        assert_eq!(budget.max_tokens, 10_000);
    }

    #[test]
    fn test_budget_status_description() {
        assert_eq!(
            BudgetStatus::Improving.description(),
            "Making progress toward goal"
        );
        assert_eq!(BudgetStatus::Stable.description(), "Progress is stable");
        assert_eq!(
            BudgetStatus::Degrading.description(),
            "Getting further from goal"
        );
        assert_eq!(BudgetStatus::Stuck.description(), "No progress detected");
        assert_eq!(BudgetStatus::Exhausted.description(), "Budget limits reached");
    }

    #[test]
    fn test_budget_status_should_continue() {
        assert!(BudgetStatus::Improving.should_continue());
        assert!(BudgetStatus::Stable.should_continue());
        assert!(!BudgetStatus::Degrading.should_continue());
        assert!(!BudgetStatus::Stuck.should_continue());
        assert!(!BudgetStatus::Exhausted.should_continue());
    }

    #[test]
    fn test_saturating_arithmetic() {
        let mut budget = PoeBudget::new(u8::MAX, u32::MAX);

        // Should not overflow
        budget.record_attempt(u32::MAX, 0.5);
        assert_eq!(budget.current_attempt, 1);
        assert_eq!(budget.tokens_used, u32::MAX);

        budget.record_attempt(1000, 0.5);
        assert_eq!(budget.tokens_used, u32::MAX); // Saturated
    }

    #[test]
    fn test_serialization() {
        let mut budget = PoeBudget::new(5, 100_000);
        budget.record_attempt(1000, 0.5);
        budget.record_attempt(2000, 0.3);

        let json = serde_json::to_string(&budget).unwrap();
        let deserialized: PoeBudget = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.max_attempts, budget.max_attempts);
        assert_eq!(deserialized.current_attempt, budget.current_attempt);
        assert_eq!(deserialized.max_tokens, budget.max_tokens);
        assert_eq!(deserialized.tokens_used, budget.tokens_used);
        assert_eq!(deserialized.entropy_history, budget.entropy_history);
    }
}

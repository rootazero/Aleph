//! Property-based tests for PoeBudget invariants and arithmetic safety.
//!
//! Uses proptest to verify:
//! - Serde roundtrip preserves all fields
//! - Fresh budgets are never exhausted
//! - Budget exhaustion after max_attempts
//! - should_continue is false when exhausted
//! - Saturating arithmetic for tokens_used
//! - Entropy history scores are clamped to [0.0, 1.0]
//! - Strictly decreasing entropy produces negative trend
//! - BudgetStatus::Exhausted implies should_continue is false

use proptest::prelude::*;

use super::budget::{BudgetStatus, PoeBudget};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary PoeBudget with recorded attempts.
fn arb_poe_budget() -> impl Strategy<Value = PoeBudget> {
    (
        1..=u8::MAX,           // max_attempts (at least 1)
        1..=u32::MAX,          // max_tokens (at least 1)
        prop::collection::vec(
            (0u32..10_000u32, -2.0f32..3.0f32),   // (tokens, raw_distance_score)
            0..10,
        ),
    )
        .prop_map(|(max_attempts, max_tokens, attempts)| {
            let mut budget = PoeBudget::new(max_attempts, max_tokens);
            for (tokens, distance) in attempts {
                budget.record_attempt(tokens, distance);
            }
            budget
        })
}

/// Generate a fresh (no attempts) PoeBudget.
fn arb_fresh_budget() -> impl Strategy<Value = PoeBudget> {
    (1..=u8::MAX, 1..=u32::MAX).prop_map(|(max_attempts, max_tokens)| {
        PoeBudget::new(max_attempts, max_tokens)
    })
}

/// Generate a strictly decreasing sequence of f32 values in [0.0, 1.0].
/// Produces at least `min_len` values.
fn arb_strictly_decreasing(min_len: usize) -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(0.0f32..1.0f32, min_len..=10).prop_map(|mut vals| {
        // Sort descending and add spacing to guarantee strictly decreasing
        vals.sort_by(|a, b| b.partial_cmp(a).unwrap());
        // Deduplicate by nudging equal neighbours down slightly
        for i in 1..vals.len() {
            if vals[i] >= vals[i - 1] {
                vals[i] = (vals[i - 1] - 0.01).max(0.0);
            }
        }
        vals
    })
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    // ---- 1. Serde roundtrip ------------------------------------------------

    /// PoeBudget: serialize then deserialize preserves all fields.
    #[test]
    fn poe_budget_serde_roundtrip(budget in arb_poe_budget()) {
        let json_str = serde_json::to_string(&budget).unwrap();
        let parsed: PoeBudget = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(parsed.max_attempts, budget.max_attempts);
        prop_assert_eq!(parsed.current_attempt, budget.current_attempt);
        prop_assert_eq!(parsed.max_tokens, budget.max_tokens);
        prop_assert_eq!(parsed.tokens_used, budget.tokens_used);
        prop_assert_eq!(parsed.entropy_history.len(), budget.entropy_history.len());
        for (a, b) in parsed.entropy_history.iter().zip(budget.entropy_history.iter()) {
            prop_assert!((a - b).abs() < f32::EPSILON, "entropy mismatch: {} vs {}", a, b);
        }
    }

    // ---- 2. Fresh budget is never exhausted --------------------------------

    /// A freshly created budget (no attempts recorded) is never exhausted.
    #[test]
    fn fresh_budget_never_exhausted(budget in arb_fresh_budget()) {
        prop_assert!(
            !budget.exhausted(),
            "Fresh budget with max_attempts={} max_tokens={} should not be exhausted",
            budget.max_attempts, budget.max_tokens
        );
    }

    // ---- 3. After max_attempts, budget is exhausted ------------------------

    /// Recording exactly max_attempts attempts exhausts the budget.
    #[test]
    fn exhausted_after_max_attempts(max_attempts in 1u8..=20u8, max_tokens in 100_000u32..=u32::MAX) {
        let mut budget = PoeBudget::new(max_attempts, max_tokens);
        for i in 0..max_attempts {
            // Use small tokens to avoid token-based exhaustion
            budget.record_attempt(1, 0.5);
            if i + 1 < max_attempts {
                prop_assert!(
                    !budget.exhausted(),
                    "Budget exhausted prematurely at attempt {}",
                    i + 1
                );
            }
        }
        prop_assert!(
            budget.exhausted(),
            "Budget should be exhausted after {} attempts",
            max_attempts
        );
    }

    // ---- 4. should_continue is false when exhausted ------------------------

    /// When the budget is exhausted, status().should_continue() returns false.
    #[test]
    fn should_continue_false_when_exhausted(max_attempts in 1u8..=10u8) {
        let mut budget = PoeBudget::new(max_attempts, u32::MAX);
        for _ in 0..max_attempts {
            budget.record_attempt(1, 0.5);
        }
        prop_assert!(budget.exhausted());
        prop_assert_eq!(budget.status(), BudgetStatus::Exhausted);
        prop_assert!(!budget.status().should_continue());
    }

    // ---- 5. tokens_used never overflows (saturating arithmetic) ------------

    /// Adding tokens never causes u32 overflow; uses saturating addition.
    #[test]
    fn tokens_used_never_overflows(
        initial_tokens in 0u32..=u32::MAX,
        add_tokens in prop::collection::vec(0u32..=u32::MAX, 1..5),
    ) {
        let mut budget = PoeBudget::new(u8::MAX, u32::MAX);
        budget.tokens_used = initial_tokens;
        // Manually advance to avoid attempt-exhaustion affecting the loop
        budget.max_attempts = u8::MAX;

        for t in &add_tokens {
            budget.record_attempt(*t, 0.5);
        }

        // tokens_used must be <= u32::MAX (cannot overflow)
        prop_assert!(budget.tokens_used <= u32::MAX);

        // If the sum would have overflowed, it should saturate at u32::MAX
        let expected: u64 = initial_tokens as u64 + add_tokens.iter().map(|t| *t as u64).sum::<u64>();
        if expected > u32::MAX as u64 {
            prop_assert_eq!(budget.tokens_used, u32::MAX);
        }
    }

    // ---- 6. Entropy history scores are clamped to [0.0, 1.0] ---------------

    /// All scores recorded via record_attempt are clamped to [0.0, 1.0].
    #[test]
    fn entropy_scores_clamped(
        raw_scores in prop::collection::vec(-10.0f32..10.0f32, 1..20),
    ) {
        let mut budget = PoeBudget::new(u8::MAX, u32::MAX);

        for score in &raw_scores {
            budget.record_attempt(1, *score);
        }

        for (i, clamped) in budget.entropy_history.iter().enumerate() {
            prop_assert!(
                *clamped >= 0.0 && *clamped <= 1.0,
                "entropy_history[{}] = {} is out of [0.0, 1.0] (raw input was {})",
                i, clamped, raw_scores[i]
            );
        }
    }

    // ---- 7. Strictly decreasing entropy → negative trend -------------------

    /// When entropy scores are strictly decreasing, entropy_trend returns negative.
    #[test]
    fn strictly_decreasing_entropy_negative_trend(
        scores in arb_strictly_decreasing(3),
    ) {
        // Only test if we have a meaningful decreasing sequence
        // (all values might collapse to the same after clamping)
        let first = scores.first().copied().unwrap_or(0.0);
        let last = scores.last().copied().unwrap_or(0.0);

        // Skip degenerate cases where the spread is too small
        prop_assume!(first - last > 0.05);

        let mut budget = PoeBudget::new(u8::MAX, u32::MAX);
        for score in &scores {
            budget.record_attempt(1, *score);
        }

        let trend = budget.entropy_trend(scores.len());
        prop_assert!(
            trend < 0.0,
            "Expected negative trend for decreasing scores {:?}, got {}",
            scores, trend
        );
    }

    // ---- 8. BudgetStatus::Exhausted → should_continue is false -------------

    /// The Exhausted variant always returns false for should_continue.
    #[test]
    fn exhausted_status_never_continues(_dummy in 0..100u32) {
        // This is a constant property, but proptest runs it many times
        prop_assert!(!BudgetStatus::Exhausted.should_continue());
    }

    // ---- Bonus: remaining_attempts is consistent ---------------------------

    /// remaining_attempts + current_attempt == max_attempts (before saturation).
    #[test]
    fn remaining_attempts_consistent(
        max_attempts in 1u8..=50u8,
        n_attempts in 0u8..=50u8,
    ) {
        let actual_attempts = n_attempts.min(max_attempts);
        let mut budget = PoeBudget::new(max_attempts, u32::MAX);
        for _ in 0..actual_attempts {
            budget.record_attempt(1, 0.5);
        }

        prop_assert_eq!(
            budget.remaining_attempts() + budget.current_attempt,
            budget.max_attempts,
            "remaining + current should equal max"
        );
    }

    // ---- Bonus: remaining_tokens is consistent -----------------------------

    /// remaining_tokens + tokens_used == max_tokens (when not saturated).
    #[test]
    fn remaining_tokens_consistent(
        max_tokens in 1000u32..=1_000_000u32,
        token_uses in prop::collection::vec(1u32..100u32, 0..10),
    ) {
        let mut budget = PoeBudget::new(u8::MAX, max_tokens);
        for t in &token_uses {
            budget.record_attempt(*t, 0.5);
        }

        // Only check if we haven't saturated
        let total_used: u64 = token_uses.iter().map(|t| *t as u64).sum();
        if total_used <= max_tokens as u64 {
            prop_assert_eq!(
                budget.remaining_tokens() + budget.tokens_used,
                budget.max_tokens,
                "remaining + used should equal max"
            );
        }
    }
}

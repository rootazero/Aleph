//! Property tests for LoopState invariants
//!
//! Tests:
//! 1. step_count equals steps.len() after record_step
//! 2. total_tokens is cumulative sum of step tokens_used
//! 3. compressed_until_step never exceeds steps.len()
//! 4. recent_steps returns at most N steps

use proptest::prelude::*;

use super::decision::{Action, ActionResult, Decision};
use super::state::{LoopState, LoopStep, RequestContext, Thinking};

// ============================================================================
// Strategies
// ============================================================================

/// Generate a LoopStep with arbitrary tokens_used
fn arb_loop_step(step_id: usize) -> impl Strategy<Value = LoopStep> {
    (1..500usize, 10..5000u64).prop_map(move |(tokens, duration)| LoopStep {
        step_id,
        observation_summary: format!("observation_{}", step_id),
        thinking: Thinking {
            reasoning: Some(format!("reasoning for step {}", step_id)),
            decision: Decision::Complete {
                summary: "done".to_string(),
            },
            structured: None,
            tokens_used: None,
        },
        action: Action::Completion {
            summary: "done".to_string(),
        },
        result: ActionResult::Completed,
        tokens_used: tokens,
        duration_ms: duration,
    })
}

/// Generate a vector of LoopSteps with sequential step_ids
fn arb_loop_steps(max_len: usize) -> impl Strategy<Value = Vec<LoopStep>> {
    proptest::collection::vec(1..500usize, 0..=max_len).prop_flat_map(|token_counts| {
        let steps: Vec<LoopStep> = token_counts
            .into_iter()
            .enumerate()
            .map(|(i, tokens)| LoopStep {
                step_id: i,
                observation_summary: format!("obs_{}", i),
                thinking: Thinking {
                    reasoning: None,
                    decision: Decision::Complete {
                        summary: "done".to_string(),
                    },
                    structured: None,
                    tokens_used: None,
                },
                action: Action::Completion {
                    summary: "done".to_string(),
                },
                result: ActionResult::Completed,
                tokens_used: tokens,
                duration_ms: 100,
            })
            .collect();
        Just(steps)
    })
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    /// Invariant: after calling record_step N times, step_count == steps.len() == N
    #[test]
    fn step_count_equals_steps_len(steps in arb_loop_steps(20)) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        prop_assert_eq!(
            state.step_count,
            state.steps.len(),
            "step_count should equal steps.len()"
        );
        prop_assert_eq!(
            state.step_count,
            steps.len(),
            "step_count should equal number of recorded steps"
        );
    }

    /// Invariant: total_tokens equals the cumulative sum of all step tokens_used
    #[test]
    fn total_tokens_is_cumulative_sum(steps in arb_loop_steps(20)) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        let expected_total: usize = steps.iter().map(|s| s.tokens_used).sum();
        prop_assert_eq!(
            state.total_tokens,
            expected_total,
            "total_tokens should equal sum of all step tokens_used"
        );
    }

    /// Invariant: compressed_until_step never exceeds steps.len()
    ///
    /// After apply_compression, compressed_until_step is bounded by the
    /// value passed in, but we verify the invariant holds after operations.
    #[test]
    fn compressed_until_step_bounded(
        steps in arb_loop_steps(20),
        compress_at in 0..25usize,
    ) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        // Apply compression at a bounded index
        let bounded_compress = compress_at.min(state.steps.len());
        state.apply_compression("compressed summary".to_string(), bounded_compress);

        prop_assert!(
            state.compressed_until_step <= state.steps.len(),
            "compressed_until_step ({}) should not exceed steps.len() ({})",
            state.compressed_until_step,
            state.steps.len()
        );
    }

    /// Invariant: recent_steps returns at most window_size steps
    #[test]
    fn recent_steps_at_most_n(
        steps in arb_loop_steps(20),
        window_size in 1..25usize,
    ) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        let recent = state.recent_steps(window_size);
        prop_assert!(
            recent.len() <= window_size,
            "recent_steps({}) returned {} steps, expected at most {}",
            window_size,
            recent.len(),
            window_size
        );
    }

    /// Invariant: recent_steps returns all steps when window >= steps.len()
    #[test]
    fn recent_steps_returns_all_when_window_large(steps in arb_loop_steps(15)) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        let step_count = steps.len();
        for step in &steps {
            state.record_step(step.clone());
        }

        // Use a window larger than step count
        let recent = state.recent_steps(step_count + 10);
        prop_assert_eq!(
            recent.len(),
            step_count,
            "When window >= steps.len(), recent_steps should return all steps"
        );
    }

    /// Invariant: recent_steps returns the LAST N steps (most recent)
    #[test]
    fn recent_steps_returns_last_n(steps in arb_loop_steps(20)) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        if !steps.is_empty() {
            let window = 3.min(steps.len());
            let recent = state.recent_steps(window);
            let expected_first_id = steps.len() - window;

            prop_assert_eq!(
                recent[0].step_id,
                expected_first_id,
                "First recent step should be step_id={}, got {}",
                expected_first_id,
                recent[0].step_id
            );

            // Verify sequential step_ids
            for (i, step) in recent.iter().enumerate() {
                prop_assert_eq!(
                    step.step_id,
                    expected_first_id + i,
                    "Step {} should have step_id={}, got {}",
                    i,
                    expected_first_id + i,
                    step.step_id
                );
            }
        }
    }

    /// Invariant: needs_compression is true iff uncompressed steps > threshold
    #[test]
    fn needs_compression_consistent(
        steps in arb_loop_steps(20),
        threshold in 1..15usize,
        compress_at in 0..10usize,
    ) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        let bounded_compress = compress_at.min(state.steps.len());
        state.apply_compression("summary".to_string(), bounded_compress);

        let uncompressed = state.steps.len() - state.compressed_until_step;
        let needs = state.needs_compression(threshold);

        prop_assert_eq!(
            needs,
            uncompressed > threshold,
            "needs_compression({}) should be {} (uncompressed={}, compressed_until={})",
            threshold,
            uncompressed > threshold,
            uncompressed,
            state.compressed_until_step
        );
    }

    /// Invariant: last_result returns the result of the most recently recorded step
    #[test]
    fn last_result_matches_last_step(steps in arb_loop_steps(20)) {
        let mut state = LoopState::new(
            "test-session".to_string(),
            "test request".to_string(),
            RequestContext::empty(),
        );

        for step in &steps {
            state.record_step(step.clone());
        }

        if steps.is_empty() {
            prop_assert!(state.last_result().is_none());
        } else {
            let last = state.last_result().unwrap();
            prop_assert_eq!(
                last,
                &steps.last().unwrap().result,
                "last_result should match the result of the last recorded step"
            );
        }
    }

    /// Invariant: new state starts with zero counts
    #[test]
    fn new_state_is_clean(
        session_id in "[a-z]{1,20}",
        request in "[a-zA-Z ]{1,50}",
    ) {
        let state = LoopState::new(
            session_id.clone(),
            request.clone(),
            RequestContext::empty(),
        );

        prop_assert_eq!(state.session_id, session_id);
        prop_assert_eq!(state.original_request, request);
        prop_assert_eq!(state.step_count, 0);
        prop_assert_eq!(state.total_tokens, 0);
        prop_assert!(state.steps.is_empty());
        prop_assert_eq!(state.compressed_until_step, 0);
        prop_assert!(state.history_summary.is_empty());
        prop_assert!(state.poe_hint.is_none());
    }

    /// POE hint: set then take returns the hint, second take returns None
    #[test]
    fn poe_hint_set_and_take(hint in "[a-zA-Z ]{1,50}") {
        let mut state = LoopState::new(
            "test".to_string(),
            "request".to_string(),
            RequestContext::empty(),
        );

        state.set_poe_hint(hint.clone());
        let taken = state.take_poe_hint();
        prop_assert_eq!(taken.as_deref(), Some(hint.as_str()));

        let taken_again = state.take_poe_hint();
        prop_assert!(taken_again.is_none(), "Second take should return None");
    }
}

//! Property-based tests for POE types serde roundtrip and invariants.
//!
//! Uses proptest to verify:
//! - ValidationRule serde roundtrip
//! - Verdict serde roundtrip
//! - Verdict distance_score is bounded to [0.0, 1.0]
//! - WorkerState serde roundtrip
//! - SoftMetric weight and threshold clamping
//! - PoeOutcome::Success with passed=true → is_success()
//! - PoeOutcome::BudgetExhausted → not is_success()

use proptest::prelude::*;
use std::path::PathBuf;

use super::types::{
    JudgeTarget, ModelTier, PoeOutcome, SoftMetric, ValidationRule, Verdict, WorkerState,
};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary PathBuf.
fn arb_pathbuf() -> impl Strategy<Value = PathBuf> {
    "[a-zA-Z0-9/_.-]{1,40}".prop_map(PathBuf::from)
}

/// Generate an arbitrary ModelTier.
fn arb_model_tier() -> impl Strategy<Value = ModelTier> {
    prop_oneof![
        Just(ModelTier::LocalFast),
        Just(ModelTier::CloudFast),
        Just(ModelTier::CloudSmart),
        Just(ModelTier::CloudDeep),
    ]
}

/// Generate an arbitrary JudgeTarget.
fn arb_judge_target() -> impl Strategy<Value = JudgeTarget> {
    prop_oneof![
        arb_pathbuf().prop_map(JudgeTarget::File),
        "[a-zA-Z0-9 ]{1,50}".prop_map(|s| JudgeTarget::Content(s)),
        ("[a-zA-Z0-9_]{1,20}", prop::collection::vec("[a-zA-Z0-9_-]{1,10}".prop_map(String::from), 0..3))
            .prop_map(|(cmd, args)| JudgeTarget::CommandOutput { cmd, args }),
    ]
}

/// Generate an arbitrary ValidationRule.
fn arb_validation_rule() -> impl Strategy<Value = ValidationRule> {
    prop_oneof![
        // FileExists
        arb_pathbuf().prop_map(|path| ValidationRule::FileExists { path }),
        // FileNotExists
        arb_pathbuf().prop_map(|path| ValidationRule::FileNotExists { path }),
        // FileContains
        (arb_pathbuf(), "[a-zA-Z0-9.*+?]{1,20}")
            .prop_map(|(path, pattern)| ValidationRule::FileContains { path, pattern }),
        // FileNotContains
        (arb_pathbuf(), "[a-zA-Z0-9.*+?]{1,20}")
            .prop_map(|(path, pattern)| ValidationRule::FileNotContains { path, pattern }),
        // DirStructureMatch
        (arb_pathbuf(), "[a-zA-Z0-9/, .]{1,30}")
            .prop_map(|(root, expected)| ValidationRule::DirStructureMatch { root, expected }),
        // CommandPasses
        (
            "[a-zA-Z0-9_]{1,15}",
            prop::collection::vec("[a-zA-Z0-9_-]{1,10}".prop_map(String::from), 0..3),
            1000u64..120_000u64,
        )
            .prop_map(|(cmd, args, timeout_ms)| ValidationRule::CommandPasses { cmd, args, timeout_ms }),
        // CommandOutputContains
        (
            "[a-zA-Z0-9_]{1,15}",
            prop::collection::vec("[a-zA-Z0-9_-]{1,10}".prop_map(String::from), 0..3),
            "[a-zA-Z0-9]{1,20}",
            1000u64..120_000u64,
        )
            .prop_map(|(cmd, args, pattern, timeout_ms)| {
                ValidationRule::CommandOutputContains { cmd, args, pattern, timeout_ms }
            }),
        // JsonSchemaValid
        (arb_pathbuf(), "[a-zA-Z0-9{}:\" ]{1,30}")
            .prop_map(|(path, schema)| ValidationRule::JsonSchemaValid { path, schema }),
        // SemanticCheck
        (arb_judge_target(), "[a-zA-Z0-9 ]{1,30}", "[a-zA-Z0-9 ]{1,30}", arb_model_tier())
            .prop_map(|(target, prompt, passing_criteria, model_tier)| {
                ValidationRule::SemanticCheck { target, prompt, passing_criteria, model_tier }
            }),
    ]
}

/// Generate an arbitrary Verdict.
fn arb_verdict() -> impl Strategy<Value = Verdict> {
    (
        any::<bool>(),                              // passed
        -2.0f32..3.0f32,                            // raw distance_score (will be clamped)
        "[a-zA-Z0-9 ]{1,40}",                      // reason
        proptest::option::of("[a-zA-Z0-9 ]{1,40}"), // suggestion
    )
        .prop_map(|(passed, raw_distance, reason, suggestion)| {
            let mut verdict = if passed {
                Verdict::success(reason)
            } else {
                Verdict::failure(reason)
            };
            verdict = verdict.with_distance_score(raw_distance);
            if let Some(s) = suggestion {
                verdict = verdict.with_suggestion(s);
            }
            verdict
        })
}

/// Generate an arbitrary WorkerState.
fn arb_worker_state() -> impl Strategy<Value = WorkerState> {
    prop_oneof![
        "[a-zA-Z0-9 ]{1,40}".prop_map(|summary| WorkerState::Completed { summary }),
        "[a-zA-Z0-9 ]{1,40}".prop_map(|reason| WorkerState::Failed { reason }),
        "[a-zA-Z0-9 ?]{1,40}".prop_map(|question| WorkerState::NeedsInput { question }),
    ]
}

/// Generate an arbitrary SoftMetric with raw (unclamped) weight and threshold.
fn arb_soft_metric() -> impl Strategy<Value = (SoftMetric, f32, f32)> {
    (
        arb_validation_rule(),
        -2.0f32..3.0f32, // raw weight
        -2.0f32..3.0f32, // raw threshold
    )
        .prop_map(|(rule, raw_weight, raw_threshold)| {
            let metric = SoftMetric::new(rule)
                .with_weight(raw_weight)
                .with_threshold(raw_threshold);
            (metric, raw_weight, raw_threshold)
        })
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    // ---- 1. ValidationRule serde roundtrip ----------------------------------

    /// ValidationRule: serialize then deserialize preserves the variant and fields.
    #[test]
    fn validation_rule_serde_roundtrip(rule in arb_validation_rule()) {
        let json_str = serde_json::to_string(&rule).unwrap();
        let parsed: ValidationRule = serde_json::from_str(&json_str).unwrap();

        // Re-serialize and compare JSON strings (since ValidationRule doesn't impl PartialEq)
        let json_str2 = serde_json::to_string(&parsed).unwrap();
        prop_assert_eq!(
            &json_str, &json_str2,
            "ValidationRule roundtrip mismatch"
        );
    }

    // ---- 2. Verdict serde roundtrip ----------------------------------------

    /// Verdict: serialize then deserialize preserves all fields.
    #[test]
    fn verdict_serde_roundtrip(verdict in arb_verdict()) {
        let json_str = serde_json::to_string(&verdict).unwrap();
        let parsed: Verdict = serde_json::from_str(&json_str).unwrap();

        prop_assert_eq!(parsed.passed, verdict.passed);
        prop_assert!(
            (parsed.distance_score - verdict.distance_score).abs() < f32::EPSILON,
            "distance_score mismatch: {} vs {}",
            parsed.distance_score, verdict.distance_score
        );
        prop_assert_eq!(&parsed.reason, &verdict.reason);
        prop_assert_eq!(&parsed.suggestion, &verdict.suggestion);
    }

    // ---- 3. Verdict distance_score bounded ---------------------------------

    /// Verdict's distance_score is always clamped to [0.0, 1.0] after with_distance_score.
    #[test]
    fn verdict_distance_score_bounded(raw_score in -10.0f32..10.0f32) {
        let verdict = Verdict::success("test").with_distance_score(raw_score);
        prop_assert!(
            verdict.distance_score >= 0.0 && verdict.distance_score <= 1.0,
            "distance_score {} out of [0.0, 1.0] (raw: {})",
            verdict.distance_score, raw_score
        );
    }

    // ---- 4. WorkerState serde roundtrip ------------------------------------

    /// WorkerState: serialize then deserialize preserves the variant and fields.
    #[test]
    fn worker_state_serde_roundtrip(state in arb_worker_state()) {
        let json_str = serde_json::to_string(&state).unwrap();
        let parsed: WorkerState = serde_json::from_str(&json_str).unwrap();

        // Re-serialize and compare JSON strings (WorkerState doesn't impl PartialEq)
        let json_str2 = serde_json::to_string(&parsed).unwrap();
        prop_assert_eq!(
            &json_str, &json_str2,
            "WorkerState roundtrip mismatch"
        );
    }

    // ---- 5. SoftMetric weight and threshold clamped ------------------------

    /// SoftMetric's weight and threshold are clamped to [0.0, 1.0].
    #[test]
    fn soft_metric_weight_threshold_clamped(
        (metric, _raw_weight, _raw_threshold) in arb_soft_metric(),
    ) {
        prop_assert!(
            metric.weight >= 0.0 && metric.weight <= 1.0,
            "weight {} out of [0.0, 1.0]",
            metric.weight
        );
        prop_assert!(
            metric.threshold >= 0.0 && metric.threshold <= 1.0,
            "threshold {} out of [0.0, 1.0]",
            metric.threshold
        );
    }

    // ---- 6. PoeOutcome::Success with passed=true → is_success() ------------

    /// PoeOutcome::Success wrapping a Verdict with passed=true is_success().
    #[test]
    fn poe_outcome_success_with_passed_true_is_success(
        reason in "[a-zA-Z0-9 ]{1,30}",
        score in 0.0f32..1.0f32,
    ) {
        let verdict = Verdict::success(reason).with_distance_score(score);
        // Verify the Verdict itself has passed=true
        prop_assert!(verdict.passed);

        let outcome = PoeOutcome::success(verdict, "");
        prop_assert!(
            outcome.is_success(),
            "PoeOutcome::Success with passed=true should be is_success()"
        );
    }

    // ---- 7. PoeOutcome::BudgetExhausted → not is_success() -----------------

    /// PoeOutcome::BudgetExhausted is never is_success().
    #[test]
    fn poe_outcome_budget_exhausted_not_success(
        attempts in 0u8..=u8::MAX,
        error in "[a-zA-Z0-9 ]{1,30}",
    ) {
        let outcome = PoeOutcome::budget_exhausted(attempts, error);
        prop_assert!(
            !outcome.is_success(),
            "PoeOutcome::BudgetExhausted should not be is_success()"
        );
    }

    // ---- Bonus: PoeOutcome::StrategySwitch → not is_success() --------------

    /// PoeOutcome::StrategySwitch is never is_success().
    #[test]
    fn poe_outcome_strategy_switch_not_success(
        reason in "[a-zA-Z0-9 ]{1,30}",
        suggestion in "[a-zA-Z0-9 ]{1,30}",
    ) {
        let outcome = PoeOutcome::strategy_switch(reason, suggestion);
        prop_assert!(
            !outcome.is_success(),
            "PoeOutcome::StrategySwitch should not be is_success()"
        );
    }

    // ---- Bonus: PoeOutcome::Success with passed=false → not is_success() ---

    /// PoeOutcome::Success wrapping a Verdict with passed=false is NOT is_success().
    #[test]
    fn poe_outcome_success_with_passed_false_not_success(
        reason in "[a-zA-Z0-9 ]{1,30}",
    ) {
        let verdict = Verdict::failure(reason);
        prop_assert!(!verdict.passed);

        let outcome = PoeOutcome::success(verdict, "");
        prop_assert!(
            !outcome.is_success(),
            "PoeOutcome::Success with passed=false should NOT be is_success()"
        );
    }

    // ---- Bonus: SoftMetric serde roundtrip ---------------------------------

    /// SoftMetric: serialize then deserialize preserves fields.
    #[test]
    fn soft_metric_serde_roundtrip(
        (metric, _rw, _rt) in arb_soft_metric(),
    ) {
        let json_str = serde_json::to_string(&metric).unwrap();
        let parsed: SoftMetric = serde_json::from_str(&json_str).unwrap();

        prop_assert!(
            (parsed.weight - metric.weight).abs() < f32::EPSILON,
            "weight mismatch: {} vs {}",
            parsed.weight, metric.weight
        );
        prop_assert!(
            (parsed.threshold - metric.threshold).abs() < f32::EPSILON,
            "threshold mismatch: {} vs {}",
            parsed.threshold, metric.threshold
        );

        // Verify rule survived by re-serializing
        let json_str2 = serde_json::to_string(&parsed).unwrap();
        prop_assert_eq!(&json_str, &json_str2, "SoftMetric roundtrip mismatch");
    }

    // ---- Bonus: PoeOutcome serde roundtrip ---------------------------------

    /// PoeOutcome serde roundtrip for all variants.
    #[test]
    fn poe_outcome_serde_roundtrip(
        variant in prop_oneof![
            arb_verdict().prop_map(|v| PoeOutcome::Success { verdict: v, worker_summary: String::new() }),
            ("[a-zA-Z0-9 ]{1,20}", "[a-zA-Z0-9 ]{1,20}")
                .prop_map(|(r, s)| PoeOutcome::StrategySwitch { reason: r, suggestion: s }),
            (0u8..=255u8, "[a-zA-Z0-9 ]{1,20}")
                .prop_map(|(a, e)| PoeOutcome::BudgetExhausted { attempts: a, last_error: e }),
        ]
    ) {
        let json_str = serde_json::to_string(&variant).unwrap();
        let parsed: PoeOutcome = serde_json::from_str(&json_str).unwrap();

        // Verify is_success() is consistent after roundtrip
        prop_assert_eq!(
            parsed.is_success(), variant.is_success(),
            "is_success() diverged after roundtrip"
        );

        // Verify JSON roundtrip
        let json_str2 = serde_json::to_string(&parsed).unwrap();
        prop_assert_eq!(&json_str, &json_str2, "PoeOutcome roundtrip mismatch");
    }
}

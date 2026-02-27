//! Integration tests for POE trust evaluation and contract auto-approval.
//!
//! Verifies the full trust loop: events -> TrustProjector -> poe_trust_scores ->
//! ExperienceTrustEvaluator -> AutoApprovalDecision.

use std::path::PathBuf;
use std::sync::Arc;

use alephcore::poe::{
    PoeEvent, PoeEventEnvelope, PoeOutcomeKind, SuccessManifest, TrustProjector, ValidationRule,
};
use alephcore::poe::trust::{
    AlwaysRequireSignature, ExperienceTrustEvaluator, TrustContext, TrustEvaluator,
    WhitelistTrustEvaluator,
};
use alephcore::resilience::database::{StateDatabase, TrustScoreRow};

/// Helper: create a StateDatabase backed by a temp directory.
///
/// Returns (db, _temp_dir). The TempDir must be kept alive for the
/// duration of the test to prevent the directory from being deleted.
fn make_db() -> (Arc<StateDatabase>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    let db_path = tmp.path().join("test_trust.db");
    let db = StateDatabase::new(db_path).expect("Failed to create StateDatabase");
    (Arc::new(db), tmp)
}

#[tokio::test]
async fn trust_projector_builds_trust_over_time() {
    let (db, _tmp) = make_db();
    let projector = TrustProjector::new(db.clone());

    // Simulate 5 successful executions for the same task pattern
    for i in 0..5u32 {
        let envelope = PoeEventEnvelope::new(
            "poe-create-rust-file".into(),
            i,
            PoeEvent::OutcomeRecorded {
                task_id: "poe-create-rust-file".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 1000,
                duration_ms: 500,
                best_distance: 0.05,
            },
            None,
        );

        let handled = projector.handle(&envelope).await.unwrap();
        assert!(handled);
    }

    // Verify trust score is 1.0 (5/5 = 100%)
    let score: TrustScoreRow = db
        .get_trust_score("poe-create-rust-file")
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(score.total_executions, 5);
    assert_eq!(score.successful_executions, 5);
    assert_eq!(score.trust_score, 1.0);
}

#[tokio::test]
async fn trust_auto_approval_with_history() {
    let (db, _tmp) = make_db();

    // Build trust history: 10 successful executions
    for _ in 0..10 {
        let _: f32 = db
            .upsert_trust_score("poe-create-rust-file", true)
            .await
            .unwrap();
    }

    // Query the trust score
    let score_row: TrustScoreRow = db
        .get_trust_score("poe-create-rust-file")
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(score_row.total_executions, 10);
    assert_eq!(score_row.trust_score, 1.0);

    // Create evaluator and evaluate
    let evaluator = ExperienceTrustEvaluator::new()
        .with_min_success_rate(0.95)
        .with_min_executions(5);

    let manifest = SuccessManifest::new("poe-create-rust-file", "Create a Rust source file")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("src/lib.rs"),
        });

    let context = TrustContext::new()
        .with_pattern_id("poe-create-rust-file")
        .with_crystallized_skill()
        .with_history(score_row.trust_score, score_row.total_executions);

    let decision = evaluator.evaluate(&manifest, &context);
    assert!(
        decision.can_auto_approve(),
        "Should auto-approve with 10 successful executions and 100% success rate"
    );
}

#[tokio::test]
async fn trust_requires_signature_for_new_pattern() {
    let evaluator = ExperienceTrustEvaluator::new()
        .with_min_success_rate(0.95)
        .with_min_executions(5);

    // Task with a non-whitelisted constraint (FileNotExists is not in the safe list)
    let manifest = SuccessManifest::new("unknown-pattern", "Do something new")
        .with_hard_constraint(ValidationRule::FileNotExists {
            path: PathBuf::from("should_not_exist.txt"),
        });

    // No history at all -- falls back to WhitelistTrustEvaluator which rejects
    // unsafe constraints
    let context = TrustContext::new().with_pattern_id("unknown-pattern");

    let decision = evaluator.evaluate(&manifest, &context);
    assert!(
        decision.requires_signature(),
        "Unknown pattern with unsafe constraint should require signature"
    );
}

#[tokio::test]
async fn trust_requires_signature_with_low_success_rate() {
    let (db, _tmp) = make_db();

    // Build mixed history: 3 success, 7 failures
    for _ in 0..3 {
        let _: f32 = db
            .upsert_trust_score("flaky-pattern", true)
            .await
            .unwrap();
    }
    for _ in 0..7 {
        let _: f32 = db
            .upsert_trust_score("flaky-pattern", false)
            .await
            .unwrap();
    }

    let score_row: TrustScoreRow = db
        .get_trust_score("flaky-pattern")
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(score_row.total_executions, 10);
    assert!((score_row.trust_score - 0.3).abs() < 0.01);

    let evaluator = ExperienceTrustEvaluator::new()
        .with_min_success_rate(0.95)
        .with_min_executions(5);

    // Use a non-whitelisted constraint so the fallback also rejects
    let manifest = SuccessManifest::new("flaky-pattern", "Unreliable task")
        .with_hard_constraint(ValidationRule::FileNotExists {
            path: PathBuf::from("should_not_exist.txt"),
        });

    let context = TrustContext::new()
        .with_pattern_id("flaky-pattern")
        .with_crystallized_skill() // Even marked as crystallized
        .with_history(score_row.trust_score, score_row.total_executions);

    let decision = evaluator.evaluate(&manifest, &context);
    // 30% success rate is below 95% threshold -- experience path rejects.
    // FileNotExists is not in the whitelist safe list -- fallback also rejects.
    assert!(
        decision.requires_signature(),
        "Low success rate with unsafe constraint should require signature"
    );
}

#[tokio::test]
async fn trust_projector_handles_mixed_outcomes() {
    let (db, _tmp) = make_db();
    let projector = TrustProjector::new(db.clone());

    // Success
    projector
        .handle(&PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 1000,
                duration_ms: 500,
                best_distance: 0.05,
            },
            None,
        ))
        .await
        .unwrap();

    // Failure
    projector
        .handle(&PoeEventEnvelope::new(
            "task-1".into(),
            1,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::BudgetExhausted,
                attempts: 5,
                total_tokens: 50000,
                duration_ms: 30000,
                best_distance: 0.8,
            },
            None,
        ))
        .await
        .unwrap();

    let score: TrustScoreRow = db
        .get_trust_score("task-1")
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(score.total_executions, 2);
    assert_eq!(score.successful_executions, 1);
    assert!((score.trust_score - 0.5).abs() < 0.01);
}

#[tokio::test]
async fn always_require_signature_ignores_perfect_history() {
    let evaluator = AlwaysRequireSignature::new();
    let manifest = SuccessManifest::new("any-task", "Any objective");
    let context = TrustContext::new()
        .with_crystallized_skill()
        .with_history(1.0, 100); // Perfect history

    let decision = evaluator.evaluate(&manifest, &context);
    assert!(
        decision.requires_signature(),
        "AlwaysRequireSignature should always require signature regardless of history"
    );
}

#[tokio::test]
async fn whitelist_evaluator_approves_safe_constraints() {
    let evaluator = WhitelistTrustEvaluator::new();

    let manifest = SuccessManifest::new("safe-task", "Check a file").with_hard_constraint(
        ValidationRule::FileExists {
            path: PathBuf::from("test.txt"),
        },
    );

    let context = TrustContext::new().with_file_count(1);

    let decision = evaluator.evaluate(&manifest, &context);
    assert!(
        decision.can_auto_approve(),
        "Simple FileExists constraint should be auto-approved by whitelist"
    );
}

/// End-to-end: events flow through TrustProjector, scores are read back,
/// and ExperienceTrustEvaluator makes the correct auto-approval decision.
#[tokio::test]
async fn full_round_trip_events_to_auto_approval() {
    let (db, _tmp) = make_db();
    let projector = TrustProjector::new(db.clone());

    let pattern = "poe-write-unit-test";

    // Phase 1: Accumulate 8 successful outcomes via TrustProjector
    for seq in 0..8u32 {
        let envelope = PoeEventEnvelope::new(
            pattern.into(),
            seq,
            PoeEvent::OutcomeRecorded {
                task_id: pattern.into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 2000,
                duration_ms: 1000,
                best_distance: 0.02,
            },
            None,
        );
        projector.handle(&envelope).await.unwrap();
    }

    // Phase 2: Read trust scores from DB
    let score_row: TrustScoreRow = db
        .get_trust_score(pattern)
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(score_row.total_executions, 8);
    assert_eq!(score_row.trust_score, 1.0);

    // Phase 3: Build TrustContext from score_row (mimicking PoeContractService logic)
    let trust_score = score_row.trust_score;
    let total_executions = score_row.total_executions;

    let mut context = TrustContext::new()
        .with_pattern_id(pattern)
        .with_file_count(1)
        .with_history(trust_score, total_executions);

    // Enrichment: mark as crystallized if high trust + enough executions
    if trust_score >= 0.9 && total_executions >= 5 {
        context = context.with_crystallized_skill();
    }

    // Phase 4: Evaluate
    let evaluator = ExperienceTrustEvaluator::new()
        .with_min_success_rate(0.95)
        .with_min_executions(5);

    let manifest = SuccessManifest::new(pattern, "Write a unit test file")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("tests/my_test.rs"),
        });

    let decision = evaluator.evaluate(&manifest, &context);
    assert!(
        decision.can_auto_approve(),
        "Full round-trip should yield auto-approval for trusted crystallized skill"
    );
    assert!(
        decision.reason().contains("Crystallized skill"),
        "Decision reason should mention crystallized skill, got: {}",
        decision.reason()
    );
}

/// After a degradation event the trust score drops and auto-approval is revoked.
#[tokio::test]
async fn trust_degrades_after_failures() {
    let (db, _tmp) = make_db();
    let projector = TrustProjector::new(db.clone());

    let pattern = "poe-deploy-service";

    // 5 successes -> trust 1.0
    for seq in 0..5u32 {
        projector
            .handle(&PoeEventEnvelope::new(
                pattern.into(),
                seq,
                PoeEvent::OutcomeRecorded {
                    task_id: pattern.into(),
                    outcome: PoeOutcomeKind::Success,
                    attempts: 1,
                    total_tokens: 1000,
                    duration_ms: 500,
                    best_distance: 0.05,
                },
                None,
            ))
            .await
            .unwrap();
    }

    let mid_score: TrustScoreRow = db
        .get_trust_score(pattern)
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(mid_score.trust_score, 1.0);

    // 5 failures -> trust drops to 5/10 = 0.5
    for seq in 5..10u32 {
        projector
            .handle(&PoeEventEnvelope::new(
                pattern.into(),
                seq,
                PoeEvent::OutcomeRecorded {
                    task_id: pattern.into(),
                    outcome: PoeOutcomeKind::BudgetExhausted,
                    attempts: 5,
                    total_tokens: 50000,
                    duration_ms: 30000,
                    best_distance: 0.9,
                },
                None,
            ))
            .await
            .unwrap();
    }

    let final_score: TrustScoreRow = db
        .get_trust_score(pattern)
        .await
        .unwrap()
        .expect("Trust score should exist");
    assert_eq!(final_score.total_executions, 10);
    assert!((final_score.trust_score - 0.5).abs() < 0.01);

    // Even with crystallized_skill flag, 50% success rate should NOT pass
    // the experience evaluator (threshold is 95%)
    let evaluator = ExperienceTrustEvaluator::new()
        .with_min_success_rate(0.95)
        .with_min_executions(5);

    let manifest = SuccessManifest::new(pattern, "Deploy service")
        .with_hard_constraint(ValidationRule::FileNotExists {
            path: PathBuf::from("lockfile.tmp"),
        });

    let context = TrustContext::new()
        .with_pattern_id(pattern)
        .with_crystallized_skill()
        .with_history(final_score.trust_score, final_score.total_executions);

    let decision = evaluator.evaluate(&manifest, &context);
    // Experience path fails (0.5 < 0.95), fallback whitelist rejects FileNotExists
    assert!(
        decision.requires_signature(),
        "Degraded trust should require signature, got: {}",
        decision.reason()
    );
}

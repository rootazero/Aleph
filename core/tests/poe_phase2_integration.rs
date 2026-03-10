//! Integration tests for POE Phase 2+3: Recursive POE, Memory Decay, Phase 3 Interfaces.
//!
//! End-to-end tests covering:
//! 1. DecompositionDetector complex manifest identification
//! 2. DecayCalculator weight convergence
//! 3. Memory decay over time
//! 4. ExecutionEnvironment with HostEnvironment
//! 5. ValidatorRole default behavior
//! 6. Full decay pipeline (tracker → calculator → filtered store)

use alephcore::poe::{
    DecayFilteredStore, ExperienceStore, InMemoryExperienceStore, PoeExperience,
    SuccessManifest, ValidatorRole, ValidationRule,
};
use alephcore::poe::decomposition::detector::{DecompositionAdvice, DecompositionDetector};
use alephcore::poe::execution_env::host::HostEnvironment;
use alephcore::poe::execution_env::ExecutionEnvironment;
use alephcore::poe::memory_decay::decay::{DecayCalculator, DecayConfig};
use alephcore::poe::memory_decay::reuse_tracker::{InMemoryReuseTracker, ReuseRecord};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Test 1: DecompositionDetector correctly identifies complex manifests
// ============================================================================

#[test]
fn test_decomposition_detector_complex_manifest_end_to_end() {
    // A manifest with 9 file constraints across 3 directories
    let mut manifest = SuccessManifest::new("integration-decomp", "Build full feature");
    for dir in &["src/api", "tests/unit", "config/settings"] {
        for i in 0..3 {
            manifest.hard_constraints.push(ValidationRule::FileExists {
                path: PathBuf::from(format!("{}/file{}.rs", dir, i)),
            });
        }
    }
    assert_eq!(manifest.hard_constraints.len(), 9);

    let advice = DecompositionDetector::analyze(&manifest);
    assert!(
        advice.should_decompose(),
        "9 constraints across 3 dirs should trigger decomposition"
    );

    if let DecompositionAdvice::Decompose {
        sub_objectives,
        reason,
    } = advice
    {
        assert_eq!(sub_objectives.len(), 3, "Should split into 3 sub-tasks");
        assert!(reason.contains("9 constraints"));
        assert!(reason.contains("3 directories"));
    } else {
        panic!("Expected Decompose advice");
    }

    // A simple manifest should NOT decompose
    let simple = SuccessManifest::new("simple", "Add one file")
        .with_hard_constraint(ValidationRule::FileExists {
            path: PathBuf::from("src/main.rs"),
        });
    assert!(!DecompositionDetector::analyze(&simple).should_decompose());
}

// ============================================================================
// Test 2: DecayCalculator produces expected weights
// ============================================================================

#[test]
fn test_decay_calculator_weight_convergence() {
    let config = DecayConfig::default();

    // Fresh experience with perfect performance: weight ~1.0
    let fresh_weight = DecayCalculator::effective_weight(
        DecayCalculator::performance_factor(5, 5, config.min_reuses_for_decay),
        DecayCalculator::drift_factor(0, 10), // no drift
        DecayCalculator::time_factor(0.0, config.time_half_life_days),
    );
    assert!(
        (fresh_weight - 1.0).abs() < 0.01,
        "Fresh successful experience should have weight ~1.0, got {}",
        fresh_weight
    );

    // Old experience with poor performance: weight should be very low
    let stale_weight = DecayCalculator::effective_weight(
        DecayCalculator::performance_factor(1, 5, config.min_reuses_for_decay),
        DecayCalculator::drift_factor(8, 10), // 80% drift
        DecayCalculator::time_factor(360.0, config.time_half_life_days), // 4 half-lives
    );
    assert!(
        stale_weight < 0.05,
        "Old failing experience should have very low weight, got {}",
        stale_weight
    );

    // Should be archived
    assert!(DecayCalculator::should_archive(stale_weight, &config));
    // Fresh should NOT be archived
    assert!(!DecayCalculator::should_archive(fresh_weight, &config));
}

// ============================================================================
// Test 3: Memory decay formula convergence over time
// ============================================================================

#[test]
fn test_memory_decay_convergence_over_time() {
    let half_life = 90;

    // Track weight at intervals
    let intervals = [0.0, 45.0, 90.0, 180.0, 360.0, 720.0];
    let mut prev_weight = f32::MAX;

    for &days in &intervals {
        let time_factor = DecayCalculator::time_factor(days, half_life);
        let weight = DecayCalculator::effective_weight(1.0, 1.0, time_factor);

        assert!(
            weight <= prev_weight,
            "Weight should monotonically decrease: {} at day {} >= {} at previous",
            weight,
            days,
            prev_weight
        );
        assert!(weight > 0.0, "Weight should never reach exactly 0.0");
        prev_weight = weight;
    }

    // At half-life, weight should be ~0.5
    let at_half_life = DecayCalculator::time_factor(90.0, half_life);
    assert!(
        (at_half_life - 0.5).abs() < 0.01,
        "At half-life, time factor should be ~0.5, got {}",
        at_half_life
    );

    // At 2x half-life, weight should be ~0.25
    let at_double = DecayCalculator::time_factor(180.0, half_life);
    assert!(
        (at_double - 0.25).abs() < 0.02,
        "At 2x half-life, time factor should be ~0.25, got {}",
        at_double
    );
}

// ============================================================================
// Test 4: ExecutionEnvironment works with HostEnvironment
// ============================================================================

#[tokio::test]
async fn test_execution_environment_host_integration() {
    let env = HostEnvironment::new();

    // Name check
    assert_eq!(env.name(), "host");

    // Execute a simple command
    let output = env
        .execute_command("echo", &["integration_test".to_string()], 5000, None)
        .await
        .expect("echo should succeed");

    assert_eq!(output.exit_code, 0);
    assert!(output.stdout.contains("integration_test"));
    assert!(output.duration_ms < 5000);

    // Execute a failing command
    let output = env
        .execute_command("false", &[], 5000, None)
        .await
        .expect("false should execute without error");

    assert_ne!(output.exit_code, 0, "false should have non-zero exit code");
}

// ============================================================================
// Test 5: ValidatorRole default behavior
// ============================================================================

#[test]
fn test_validator_role_defaults_and_serialization() {
    // Default is NormalCritic
    let role = ValidatorRole::default();
    assert_eq!(role, ValidatorRole::NormalCritic);

    // Serialization round-trip
    let json = serde_json::to_string(&role).unwrap();
    let deserialized: ValidatorRole = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, ValidatorRole::NormalCritic);

    // AdversarialCritic round-trip
    let adversarial = ValidatorRole::AdversarialCritic;
    let json2 = serde_json::to_string(&adversarial).unwrap();
    let deserialized2: ValidatorRole = serde_json::from_str(&json2).unwrap();
    assert_eq!(deserialized2, ValidatorRole::AdversarialCritic);
}

// ============================================================================
// Test 6: Full decay pipeline (tracker → calculator → filtered store)
// ============================================================================

#[tokio::test]
async fn test_full_decay_pipeline_integration() {
    let store = InMemoryExperienceStore::new();
    let tracker = Arc::new(RwLock::new(InMemoryReuseTracker::new()));
    let config = DecayConfig::default();
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Insert a fresh, well-performing experience
    let good_exp = PoeExperience {
        id: "good-1".into(),
        task_id: "task-good".into(),
        objective: "A good experience".into(),
        pattern_id: "pattern-a".into(),
        tool_sequence_json: "[]".into(),
        parameter_mapping: None,
        satisfaction: 0.9,
        distance_score: 0.1,
        attempts: 1,
        duration_ms: 500,
        created_at: now_ms, // just created
    };
    store
        .insert(good_exp, &[1.0, 0.0, 0.0])
        .await
        .unwrap();

    // Insert an old, poorly-performing experience
    let bad_exp = PoeExperience {
        id: "bad-1".into(),
        task_id: "task-bad".into(),
        objective: "A bad experience".into(),
        pattern_id: "pattern-b".into(),
        tool_sequence_json: "[]".into(),
        parameter_mapping: None,
        satisfaction: 0.2,
        distance_score: 0.8,
        attempts: 5,
        duration_ms: 10000,
        created_at: now_ms - (400 * 24 * 60 * 60 * 1000), // 400 days old
    };
    store
        .insert(bad_exp, &[0.9, 0.1, 0.0])
        .await
        .unwrap();

    // Record failures for the bad experience
    {
        let mut t = tracker.write().await;
        for i in 0..5 {
            t.record_reuse(ReuseRecord {
                experience_id: "bad-1".into(),
                reused_at: now_ms - (i * 1000),
                led_to_success: false,
                task_id: format!("task-fail-{}", i),
            });
        }
        // Record successes for the good experience
        for i in 0..3 {
            t.record_reuse(ReuseRecord {
                experience_id: "good-1".into(),
                reused_at: now_ms - (i * 1000),
                led_to_success: true,
                task_id: format!("task-success-{}", i),
            });
        }
    }

    // Create the decay-filtered store
    let filtered = DecayFilteredStore::new(store, config, tracker);

    // Search with embedding close to both experiences
    let results = filtered
        .weighted_search(&[1.0, 0.0, 0.0], 10, 0.0)
        .await
        .unwrap();

    // Good experience should be present
    assert!(
        results.iter().any(|(exp, _, _)| exp.id == "good-1"),
        "Good experience should be in results"
    );

    // Bad experience should be filtered out (old + all failures → archived)
    assert!(
        !results.iter().any(|(exp, _, _)| exp.id == "bad-1"),
        "Bad experience should be filtered out (archived)"
    );

    // Verify the good experience has a high effective weight
    if let Some((_, _similarity, weight)) = results.iter().find(|(exp, _, _)| exp.id == "good-1") {
        assert!(
            *weight > 0.5,
            "Good experience should have weight > 0.5, got {}",
            weight
        );
    }
}

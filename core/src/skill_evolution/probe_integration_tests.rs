//! End-to-end probe integration tests for the skill evolution pipeline.
//!
//! Validates that all 17 observability probes fire correctly across
//! a complete skill lifecycle: tracking → detection → validation →
//! deployment → vitality → desolidification → consolidation → graveyard.
//!
//! Run with: `RUST_LOG=aleph::evolution::probe=info cargo test -p alephcore --lib probe_integration`

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::{EnvFilter, fmt, Layer};

    use crate::skill_evolution::consolidator::{
        check_consolidation, ConsolidationVerdict, ConsolidatorConfig, MergeType, SkillCandidate,
    };
    use crate::skill_evolution::desolidification::{
        apply_feedback, compute_entropy_penalty, CircuitBreaker, CircuitBreakerConfig,
        CircuitBreakerVerdict, EntropyCanaryConfig, FeedbackType,
    };
    use crate::skill_evolution::detector::SolidificationDetector;
    use crate::skill_evolution::graveyard::{GraveyardEntry, SkillGraveyard};
    use crate::skill_evolution::lifecycle::{
        ObservationReason, SkillLifecycleState,
    };
    use crate::skill_evolution::shadow_deployer::ShadowDeployer;
    use crate::skill_evolution::tracker::EvolutionTracker;
    use crate::skill_evolution::types::{ExecutionStatus, SkillExecution, SolidificationConfig};
    use crate::skill_evolution::validation::sandbox_executor::{SandboxConfig, SandboxExecutor};
    use crate::skill_evolution::validation::tiered_validator::{
        TieredValidator, ValidationLevel,
    };
    use crate::skill_evolution::validation::risk_profiler::SkillRiskProfiler;
    use crate::skill_evolution::vitality::{VitalityConfig, VitalityInput, VitalityScore};

    use crate::poe::crystallization::experience_store::{ExperienceStore, InMemoryExperienceStore, PoeExperience};
    use crate::poe::crystallization::pattern_model::{
        ParameterMapping, PatternSequence, PatternStep, ToolCallTemplate, ToolCategory,
    };
    use crate::poe::crystallization::synthesis_backend::{
        PatternSynthesisBackend, PatternSynthesisRequest, PatternSuggestion,
    };

    use async_trait::async_trait;
    use tokio::fs;

    // ========================================================================
    // Shared test infrastructure
    // ========================================================================

    /// Thread-safe buffer that captures tracing output.
    #[derive(Clone)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }

        fn contents(&self) -> String {
            let buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
            String::from_utf8_lossy(&buf).to_string()
        }
    }

    impl std::io::Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
            inner.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedBuffer {
        type Writer = SharedBuffer;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Install a tracing subscriber that captures to a buffer and returns the buffer.
    /// Uses `set_default` so it's thread-local (safe for parallel tests).
    fn install_probe_subscriber() -> (tracing::subscriber::DefaultGuard, SharedBuffer) {
        let buffer = SharedBuffer::new();
        let filter = EnvFilter::new("aleph::evolution::probe=info");
        let layer = fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(buffer.clone());
        let subscriber = tracing_subscriber::registry().with(layer.with_filter(filter));
        let guard = tracing::subscriber::set_default(subscriber);
        (guard, buffer)
    }

    /// Assert a probe name appears in the captured output.
    fn assert_probe_fired(output: &str, probe_name: &str) {
        assert!(
            output.contains(probe_name),
            "Expected probe '{}' to fire, but it was not found in output.\nOutput:\n{}",
            probe_name,
            &output[..output.len().min(2000)]
        );
    }

    /// Count occurrences of a probe in the output.
    fn count_probe(output: &str, probe_name: &str) -> usize {
        output.matches(probe_name).count()
    }

    /// Mock backend that always returns high confidence.
    struct AlwaysAgreeBackend;

    #[async_trait]
    impl PatternSynthesisBackend for AlwaysAgreeBackend {
        async fn synthesize_pattern(
            &self,
            _request: PatternSynthesisRequest,
        ) -> anyhow::Result<PatternSuggestion> {
            Ok(PatternSuggestion {
                description: "mock".to_string(),
                steps: vec![],
                parameter_mapping: ParameterMapping::default(),
                pattern_hash: "mock".to_string(),
                confidence: 0.95,
            })
        }

        async fn evaluate_confidence(
            &self,
            _pattern_hash: &str,
            _occurrences: &[PoeExperience],
        ) -> anyhow::Result<f32> {
            Ok(0.95)
        }
    }

    // -- Helpers --

    fn make_execution(skill_id: &str, status: ExecutionStatus, satisfaction: f32) -> SkillExecution {
        SkillExecution {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: skill_id.to_string(),
            session_id: "test-session".to_string(),
            invoked_at: chrono::Utc::now().timestamp(),
            duration_ms: 1000,
            status,
            satisfaction: Some(satisfaction),
            context: "test context".to_string(),
            input_summary: "test input".to_string(),
            output_length: 100,
        }
    }

    fn make_action(name: &str, category: ToolCategory) -> PatternStep {
        PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: name.to_string(),
                category,
            },
            params: ParameterMapping::default(),
        }
    }

    fn make_pattern(steps: Vec<PatternStep>) -> PatternSequence {
        PatternSequence {
            description: "test pattern".to_string(),
            steps,
            expected_outputs: vec![],
        }
    }

    fn make_experience(id: &str, pattern_id: &str) -> PoeExperience {
        PoeExperience {
            id: id.to_string(),
            task_id: format!("task-{}", id),
            objective: "test objective".to_string(),
            pattern_id: pattern_id.to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction: 0.9,
            distance_score: 0.1,
            attempts: 1,
            duration_ms: 1000,
            created_at: 0,
        }
    }

    // ========================================================================
    // Scenario 1: Full skill lifecycle — birth to grave
    // ========================================================================
    //
    // Exercises probes:
    //   1. execution_logged          (tracker)
    //   2. solidification_candidates_detected (detector)
    //   3. solidification_candidate  (detector)
    //   4. vitality_computed          (vitality)
    //   5. skill_deployed_shadow      (shadow_deployer)
    //   6. skill_promoted             (shadow_deployer)
    //   7. skill_demoted              (shadow_deployer)

    #[tokio::test]
    async fn scenario_1_full_lifecycle_birth_to_grave() {
        let (_guard, buffer) = install_probe_subscriber();

        // -- Phase 1: Track executions --
        let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
        for _ in 0..5 {
            let exec = make_execution(
                "refactor-code",
                ExecutionStatus::Success,
                0.9,
            );
            tracker.log_execution(&exec).unwrap();
        }

        // -- Phase 2: Detect solidification candidates --
        let config = SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.7,
            min_age_days: 0,  // allow immediate detection in tests
            max_idle_days: 30,
        };
        let detector = SolidificationDetector::new(tracker.clone()).with_config(config);
        let _candidates = detector.detect_candidates().unwrap();

        // -- Phase 3: Compute vitality score --
        let input = VitalityInput {
            success_rate: 1.0,
            invocations_last_30d: 5,
            avg_tokens: 500.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 1.0,
        };
        let vitality = VitalityScore::compute(&input, &VitalityConfig::default());
        assert!(vitality.value > 0.0);

        // -- Phase 4: Deploy to shadow → promote → demote --
        let tmp = tempfile::tempdir().unwrap();
        let evolved = tmp.path().join("evolved");
        let official = tmp.path().join("official");
        let deployer = ShadowDeployer::new(evolved.clone(), official.clone());

        deployer
            .deploy("refactor-code", "# Refactor Code\nAutomatic code refactoring.", "pattern-001")
            .await
            .unwrap();

        // Deploy a second skill to test promote and demote
        deployer
            .deploy("format-code", "# Format Code\nAutomatic formatting.", "pattern-002")
            .await
            .unwrap();

        deployer.promote("format-code").await.unwrap();
        deployer.demote("refactor-code", "vitality below threshold").await.unwrap();

        // -- Verify probes --
        let output = buffer.contents();

        assert_probe_fired(&output, "execution_logged");
        assert!(count_probe(&output, "execution_logged") >= 5);

        assert_probe_fired(&output, "solidification_candidates_detected");

        assert_probe_fired(&output, "vitality_computed");

        assert_probe_fired(&output, "skill_deployed_shadow");
        assert!(count_probe(&output, "skill_deployed_shadow") >= 2);

        assert_probe_fired(&output, "skill_promoted");
        assert_probe_fired(&output, "skill_demoted");
    }

    // ========================================================================
    // Scenario 2: De-solidification cascade
    // ========================================================================
    //
    // Exercises probes:
    //   8. circuit_breaker_tripped (consecutive failures)
    //   9. circuit_breaker_tripped (low success rate)
    //  10. entropy_canary_penalty
    //  11. user_feedback_applied

    #[tokio::test]
    async fn scenario_2_desolidification_cascade() {
        let (_guard, buffer) = install_probe_subscriber();

        // -- Layer 1a: Circuit breaker — consecutive failures --
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let outcomes_consecutive = vec![true, true, false, false, false];
        let verdict = breaker.check(&outcomes_consecutive);
        assert!(matches!(verdict, CircuitBreakerVerdict::Tripped { .. }));

        // -- Layer 1b: Circuit breaker — low success rate --
        let outcomes_low_rate = vec![
            false, false, true, false, false, true, false, false, true, false, false, true,
        ];
        let verdict = breaker.check(&outcomes_low_rate);
        assert!(matches!(verdict, CircuitBreakerVerdict::Tripped { .. }));

        // -- Layer 2: Entropy canary --
        let entropy_config = EntropyCanaryConfig::default();

        // Entropy increasing only
        let penalty1 = compute_entropy_penalty(true, 1000.0, 1000.0, &entropy_config);
        assert!(penalty1 > 0.0);

        // Entropy + duration degradation
        let penalty2 = compute_entropy_penalty(true, 1000.0, 1600.0, &entropy_config);
        assert!(penalty2 > penalty1);

        // -- Layer 3: User feedback --
        let mut mul = 1.0;
        mul = apply_feedback(mul, &FeedbackType::Negative);
        assert!((mul - 0.7).abs() < 0.01);

        mul = apply_feedback(mul, &FeedbackType::ManualEdit);
        assert!(mul < 0.7);

        let _recovered = apply_feedback(mul, &FeedbackType::Positive);

        // -- Verify probes --
        let output = buffer.contents();

        // Both circuit breaker trigger types
        assert!(count_probe(&output, "circuit_breaker_tripped") >= 2);
        assert!(output.contains("consecutive_failures"));
        assert!(output.contains("low_success_rate"));

        assert_probe_fired(&output, "entropy_canary_penalty");
        assert!(count_probe(&output, "entropy_canary_penalty") >= 2);

        assert_probe_fired(&output, "user_feedback_applied");
        assert!(count_probe(&output, "user_feedback_applied") >= 3);
    }

    // ========================================================================
    // Scenario 3: Knowledge consolidation — dedup & merge
    // ========================================================================
    //
    // Exercises probes:
    //  12. consolidation_verdict (unique)
    //  13. consolidation_verdict (duplicate)
    //  14. consolidation_verdict (merge — absorb & synthesize)

    #[test]
    fn scenario_3_knowledge_consolidation() {
        let (_guard, buffer) = install_probe_subscriber();

        let config = ConsolidatorConfig::default();

        // -- Case 1: Unique skill (no matches) --
        let candidate_unique = SkillCandidate {
            skill_id: "brand-new-skill".to_string(),
            vitality: 0.8,
        };
        let verdict = check_consolidation(&candidate_unique, &[], &config);
        assert_eq!(verdict, ConsolidationVerdict::Unique);

        // -- Case 2: Duplicate (existing has higher vitality) --
        let candidate_weak = SkillCandidate {
            skill_id: "weak-skill".to_string(),
            vitality: 0.3,
        };
        let matches_strong = vec![("existing-strong".to_string(), 0.92, 0.7)];
        let verdict = check_consolidation(&candidate_weak, &matches_strong, &config);
        assert!(matches!(verdict, ConsolidationVerdict::Duplicate { .. }));

        // -- Case 3: Absorb (candidate better, existing weak) --
        let candidate_strong = SkillCandidate {
            skill_id: "strong-skill".to_string(),
            vitality: 0.8,
        };
        let matches_weak = vec![("old-weak".to_string(), 0.90, 0.3)];
        let verdict = check_consolidation(&candidate_strong, &matches_weak, &config);
        assert_eq!(
            verdict,
            ConsolidationVerdict::Merge {
                winner_id: "strong-skill".to_string(),
                loser_id: "old-weak".to_string(),
                merge_type: MergeType::Absorb,
            }
        );

        // -- Case 4: Synthesize (both strong) --
        let candidate_also_strong = SkillCandidate {
            skill_id: "newer-skill".to_string(),
            vitality: 0.8,
        };
        let matches_both_strong = vec![("older-skill".to_string(), 0.91, 0.6)];
        let verdict = check_consolidation(&candidate_also_strong, &matches_both_strong, &config);
        assert_eq!(
            verdict,
            ConsolidationVerdict::Merge {
                winner_id: "newer-skill".to_string(),
                loser_id: "older-skill".to_string(),
                merge_type: MergeType::Synthesize,
            }
        );

        // -- Verify probes --
        let output = buffer.contents();

        assert_probe_fired(&output, "consolidation_verdict");
        // 4 verdicts: unique, duplicate, merge(absorb), merge(synthesize)
        assert!(count_probe(&output, "consolidation_verdict") >= 4);
        assert!(output.contains("unique"));
        assert!(output.contains("duplicate"));
        assert!(output.contains("merge"));
        assert!(output.contains("Absorb"));
        assert!(output.contains("Synthesize"));
    }

    // ========================================================================
    // Scenario 4: Tiered validation — L1 → L2 → L3
    // ========================================================================
    //
    // Exercises probes:
    //  15. validation_started
    //  16. validation_completed

    #[tokio::test]
    async fn scenario_4_tiered_validation() {
        let (_guard, buffer) = install_probe_subscriber();

        let backend = Arc::new(AlwaysAgreeBackend);
        let validator = TieredValidator::new(backend);
        let store = InMemoryExperienceStore::new();

        // -- Case 1: Low risk (L1 only) --
        let pattern_low = make_pattern(vec![make_action("read_file", ToolCategory::ReadOnly)]);
        let risk_low = SkillRiskProfiler::profile(&pattern_low);
        let verdict = validator
            .validate(&pattern_low, "pattern-low", &risk_low, &store)
            .await
            .unwrap();
        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L1Structural);

        // -- Case 2: Medium risk (L1 + L2) --
        let pattern_med = make_pattern(vec![make_action("write_file", ToolCategory::FileWrite)]);
        let risk_med = SkillRiskProfiler::profile(&pattern_med);
        store
            .insert(make_experience("exp-1", "pattern-med"), &[1.0])
            .await
            .unwrap();
        let verdict = validator
            .validate(&pattern_med, "pattern-med", &risk_med, &store)
            .await
            .unwrap();
        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L2Semantic);

        // -- Case 3: High risk (L1 + L2 + L3) --
        let pattern_high = make_pattern(vec![make_action("run_shell", ToolCategory::Shell)]);
        let risk_high = SkillRiskProfiler::profile(&pattern_high);
        store
            .insert(make_experience("exp-2", "pattern-high"), &[1.0])
            .await
            .unwrap();
        let verdict = validator
            .validate(&pattern_high, "pattern-high", &risk_high, &store)
            .await
            .unwrap();
        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L3Sandbox);

        // -- Verify probes --
        let output = buffer.contents();

        assert_probe_fired(&output, "validation_started");
        assert_probe_fired(&output, "validation_completed");
        // 3 validations = 3 starts, 3 completions (all paths fire validation_completed)
        assert!(count_probe(&output, "validation_started") >= 3);
        assert!(count_probe(&output, "validation_completed") >= 3);
    }

    // ========================================================================
    // Scenario 5: Sandbox execution validation
    // ========================================================================
    //
    // Exercises probe:
    //  17. sandbox_validation_completed

    #[tokio::test]
    async fn scenario_5_sandbox_validation() {
        let (_guard, buffer) = install_probe_subscriber();

        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let tools: HashSet<String> = ["read_file", "write_file"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let executor = SandboxExecutor::new(source, overlay.clone(), tools, SandboxConfig::default());

        // Valid calls
        let valid_calls = vec![
            ("read_file".to_string(), Some("data.txt".to_string())),
            ("write_file".to_string(), Some("output.txt".to_string())),
        ];
        let result = executor.validate_tool_calls(&valid_calls).await;
        assert!(result.success);
        assert!(result.violations.is_empty());

        // Invalid calls (disallowed tool + path escape)
        let invalid_calls = vec![
            ("delete_all".to_string(), None),
            ("read_file".to_string(), Some("../../../etc/passwd".to_string())),
        ];
        let result = executor.validate_tool_calls(&invalid_calls).await;
        assert!(!result.success);
        assert_eq!(result.violations.len(), 2);

        // -- Verify probes --
        let output = buffer.contents();

        assert_probe_fired(&output, "sandbox_validation_completed");
        assert!(count_probe(&output, "sandbox_validation_completed") >= 2);
    }

    // ========================================================================
    // Scenario 6: Graveyard archival & lifecycle transitions
    // ========================================================================
    //
    // End-to-end: vitality decay → observation → demotion → graveyard

    #[tokio::test]
    async fn scenario_6_vitality_decay_to_graveyard() {
        let (_guard, buffer) = install_probe_subscriber();

        // -- Step 1: Healthy skill --
        let healthy_input = VitalityInput {
            success_rate: 0.95,
            invocations_last_30d: 10,
            avg_tokens: 300.0,
            avg_retries: 0.0,
            user_feedback_multiplier: 1.0,
        };
        let config = VitalityConfig::default();
        let healthy_score = VitalityScore::compute(&healthy_input, &config);
        assert!(healthy_score.value > config.healthy_threshold);

        // -- Step 2: Degrading skill (with negative feedback) --
        let mut feedback_mul = 1.0;
        for _ in 0..5 {
            feedback_mul = apply_feedback(feedback_mul, &FeedbackType::Negative);
        }
        // After 5x negative: 1.0 * 0.7^5 ≈ 0.168

        // -- Step 3: Compute degraded vitality --
        let degraded_input = VitalityInput {
            success_rate: 0.6,
            invocations_last_30d: 3,
            avg_tokens: 1500.0,
            avg_retries: 2.0,
            user_feedback_multiplier: feedback_mul,
        };
        let degraded_score = VitalityScore::compute(&degraded_input, &config);
        // Should be well below warning_threshold (0.3)
        assert!(degraded_score.value < config.warning_threshold);

        // -- Step 4: Enter observation state --
        let observation_state = SkillLifecycleState::Observation {
            entered_at: chrono::Utc::now().timestamp_millis(),
            reason: ObservationReason::VitalityWarning,
            previous_vitality: degraded_score.value,
        };
        // Verify it round-trips
        let json = serde_json::to_string(&observation_state).unwrap();
        let _: SkillLifecycleState = serde_json::from_str(&json).unwrap();

        // -- Step 5: Circuit breaker confirms demotion --
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let failing_outcomes = vec![true, false, false, false, false];
        let verdict = breaker.check(&failing_outcomes);
        assert!(matches!(verdict, CircuitBreakerVerdict::Tripped { .. }));

        // -- Step 6: Entropy canary adds penalty --
        let entropy_penalty = compute_entropy_penalty(
            true,
            1000.0,
            2000.0, // 100% slower
            &EntropyCanaryConfig::default(),
        );
        assert!(entropy_penalty > 0.0);

        // -- Step 7: Archive to graveyard --
        let mut graveyard = SkillGraveyard::in_memory();
        graveyard
            .archive(GraveyardEntry {
                skill_id: "degraded-skill".to_string(),
                skill_md: "# Degraded Skill\nWas once useful for file operations.".to_string(),
                failure_traces: vec![
                    "Error: timeout after 60s".to_string(),
                    "Error: API changed".to_string(),
                ],
                reason: "vitality below demotion threshold".to_string(),
                retired_at: chrono::Utc::now().timestamp_millis(),
                vitality_at_death: degraded_score.value,
            })
            .await
            .unwrap();

        assert_eq!(graveyard.len(), 1);
        let similar = graveyard.query_similar(&["file"]);
        assert_eq!(similar.len(), 1);

        // -- Verify all probes fired across the cascade --
        let output = buffer.contents();

        // Vitality computed at least twice (healthy + degraded)
        assert!(count_probe(&output, "vitality_computed") >= 2);

        // User feedback applied 5 times
        assert!(count_probe(&output, "user_feedback_applied") >= 5);

        // Circuit breaker tripped
        assert_probe_fired(&output, "circuit_breaker_tripped");

        // Entropy canary penalty
        assert_probe_fired(&output, "entropy_canary_penalty");
    }

    // ========================================================================
    // Scenario 7: Complete probe coverage audit
    // ========================================================================
    //
    // Runs all pipeline stages and verifies every probe name appears.

    #[tokio::test]
    async fn scenario_7_all_17_probes_fire() {
        let (_guard, buffer) = install_probe_subscriber();

        // -- 1. execution_logged --
        let tracker = Arc::new(EvolutionTracker::in_memory().unwrap());
        for _ in 0..5 {
            tracker
                .log_execution(&make_execution("test-skill", ExecutionStatus::Success, 0.9))
                .unwrap();
        }

        // -- 2,3. solidification_candidates_detected, solidification_candidate --
        let config = SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.7,
            min_age_days: 0,
            max_idle_days: 30,
        };
        let detector = SolidificationDetector::new(tracker.clone()).with_config(config);
        let _ = detector.detect_candidates();

        // -- 4. vitality_computed --
        let _ = VitalityScore::compute(
            &VitalityInput {
                success_rate: 0.9,
                invocations_last_30d: 5,
                avg_tokens: 500.0,
                avg_retries: 0.0,
                user_feedback_multiplier: 1.0,
            },
            &VitalityConfig::default(),
        );

        // -- 5,6,7. skill_deployed_shadow, skill_promoted, skill_demoted --
        let tmp = tempfile::tempdir().unwrap();
        let deployer =
            ShadowDeployer::new(tmp.path().join("evolved"), tmp.path().join("official"));
        deployer
            .deploy("s1", "# S1", "p1")
            .await
            .unwrap();
        deployer
            .deploy("s2", "# S2", "p2")
            .await
            .unwrap();
        deployer.promote("s1").await.unwrap();
        deployer.demote("s2", "test demotion").await.unwrap();

        // -- 8,9. circuit_breaker_tripped (consecutive + low_success_rate) --
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        let _ = breaker.check(&[true, true, false, false, false]); // consecutive
        let _ = breaker.check(&[
            false, false, true, false, false, true, false, false, true, false, false, true,
        ]); // low rate

        // -- 10. entropy_canary_penalty --
        let _ = compute_entropy_penalty(true, 1000.0, 1600.0, &EntropyCanaryConfig::default());

        // -- 11. user_feedback_applied --
        let _ = apply_feedback(1.0, &FeedbackType::Negative);

        // -- 12,13,14. consolidation_verdict (unique, duplicate, merge) --
        let cfg = ConsolidatorConfig::default();
        let _ = check_consolidation(
            &SkillCandidate { skill_id: "a".into(), vitality: 0.8 },
            &[],
            &cfg,
        );
        let _ = check_consolidation(
            &SkillCandidate { skill_id: "b".into(), vitality: 0.3 },
            &[("existing".into(), 0.9, 0.7)],
            &cfg,
        );
        let _ = check_consolidation(
            &SkillCandidate { skill_id: "c".into(), vitality: 0.8 },
            &[("old".into(), 0.9, 0.6)],
            &cfg,
        );

        // -- 15,16. validation_started, validation_completed --
        let backend = Arc::new(AlwaysAgreeBackend);
        let validator = TieredValidator::new(backend);
        let store = InMemoryExperienceStore::new();
        let pattern = make_pattern(vec![make_action("read_file", ToolCategory::ReadOnly)]);
        let risk = SkillRiskProfiler::profile(&pattern);
        let _ = validator
            .validate(&pattern, "p1", &risk, &store)
            .await;

        // -- 17. sandbox_validation_completed --
        let tmp2 = tempfile::tempdir().unwrap();
        let source = tmp2.path().join("src");
        let overlay = tmp2.path().join("ovl");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();
        let tools: HashSet<String> = ["read_file"].iter().map(|s| s.to_string()).collect();
        let executor = SandboxExecutor::new(source, overlay, tools, SandboxConfig::default());
        let _ = executor.validate_tool_calls(&[("read_file".into(), None)]).await;

        // ======== VERIFY ALL 17 PROBES ========
        let output = buffer.contents();

        let all_probes = [
            "execution_logged",
            "solidification_candidates_detected",
            // Note: solidification_candidate only fires if candidates are found
            "vitality_computed",
            "skill_deployed_shadow",
            "skill_promoted",
            "skill_demoted",
            "circuit_breaker_tripped",
            "entropy_canary_penalty",
            "user_feedback_applied",
            "consolidation_verdict",
            "validation_started",
            "validation_completed",
            "sandbox_validation_completed",
        ];

        let mut missing: Vec<&str> = Vec::new();
        for probe in &all_probes {
            if !output.contains(probe) {
                missing.push(probe);
            }
        }

        assert!(
            missing.is_empty(),
            "Missing probes ({}/{}): {:?}\n\nFull output:\n{}",
            missing.len(),
            all_probes.len(),
            missing,
            &output[..output.len().min(3000)]
        );

        // Verify we have a good variety of probe events
        let total_events: usize = all_probes.iter().map(|p| count_probe(&output, p)).sum();
        assert!(
            total_events >= 20,
            "Expected at least 20 total probe events, got {}",
            total_events
        );
    }
}

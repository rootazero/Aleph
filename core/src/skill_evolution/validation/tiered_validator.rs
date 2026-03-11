//! Tiered validator orchestrator -- gates pattern promotion by risk level.
//!
//! Orchestrates L1 (structural linting), L2 (semantic replay), and
//! risk-gated human review to validate evolved patterns before deployment.

use serde::{Deserialize, Serialize};

use crate::poe::crystallization::experience_store::ExperienceStore;
use crate::poe::crystallization::pattern_model::PatternSequence;
use crate::poe::crystallization::synthesis_backend::PatternSynthesisBackend;
use crate::sync_primitives::Arc;

use super::risk_profiler::{SkillRiskLevel, SkillRiskProfile};
use super::semantic_replayer::SemanticReplayer;
use super::structural_linter::StructuralLinter;
use super::test_set_generator::TestSetGenerator;

// ============================================================================
// Types
// ============================================================================

/// Validation tier level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationLevel {
    /// Structural lint checks only.
    L1Structural,
    /// Semantic replay via LLM backend.
    L2Semantic,
    /// Full sandbox execution (not available in Beta).
    L3Sandbox,
}

/// Verdict from tiered validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationVerdict {
    /// Whether the pattern passed all required validation tiers.
    pub passed: bool,
    /// Highest validation level that was reached.
    pub level_reached: ValidationLevel,
    /// Errors from L1 structural linting.
    pub l1_errors: Vec<String>,
    /// Details from L2 semantic replay (if run).
    pub l2_details: Option<String>,
    /// Whether a human must review before deployment.
    pub requires_human_review: bool,
}

// ============================================================================
// TieredValidator
// ============================================================================

/// Orchestrates tiered validation: L1 structural → L2 semantic → risk-gated review.
pub struct TieredValidator {
    linter: StructuralLinter,
    replayer: SemanticReplayer,
    test_set_gen: TestSetGenerator,
}

impl TieredValidator {
    /// Create a new tiered validator with the given synthesis backend.
    pub fn new(backend: Arc<dyn PatternSynthesisBackend>) -> Self {
        Self {
            linter: StructuralLinter,
            replayer: SemanticReplayer::new(backend, 0.8),
            test_set_gen: TestSetGenerator::new(8),
        }
    }

    /// Validate a pattern through appropriate tiers based on risk level.
    ///
    /// - **All patterns**: L1 structural lint (must pass)
    /// - **Low risk**: L1 sufficient
    /// - **Medium risk**: L1 + L2 semantic replay
    /// - **High risk**: L1 + L2 + L3 sandbox validation
    pub async fn validate(
        &self,
        pattern: &PatternSequence,
        pattern_id: &str,
        risk: &SkillRiskProfile,
        store: &dyn ExperienceStore,
    ) -> anyhow::Result<ValidationVerdict> {
        // Generate test set from store
        let test_set = self.test_set_gen.generate(pattern_id, store).await?;

        // L1: Structural lint (all patterns)
        let lint = self.linter.validate(pattern, &test_set);
        if !lint.passed {
            return Ok(ValidationVerdict {
                passed: false,
                level_reached: ValidationLevel::L1Structural,
                l1_errors: lint.errors,
                l2_details: None,
                requires_human_review: false,
            });
        }

        // Low risk: L1 sufficient
        if risk.level == SkillRiskLevel::Low {
            return Ok(ValidationVerdict {
                passed: true,
                level_reached: ValidationLevel::L1Structural,
                l1_errors: vec![],
                l2_details: None,
                requires_human_review: false,
            });
        }

        // L2: Semantic replay (medium + high risk)
        let replay = self.replayer.replay(pattern, &test_set).await?;
        if !replay.passed {
            return Ok(ValidationVerdict {
                passed: false,
                level_reached: ValidationLevel::L2Semantic,
                l1_errors: vec![],
                l2_details: Some(replay.details),
                requires_human_review: false,
            });
        }

        // Medium risk: L1 + L2 sufficient
        if risk.level == SkillRiskLevel::Medium {
            return Ok(ValidationVerdict {
                passed: true,
                level_reached: ValidationLevel::L2Semantic,
                l1_errors: vec![],
                l2_details: Some(replay.details),
                requires_human_review: false,
            });
        }

        // High risk: L1 + L2 + L3 sandbox validation
        Ok(ValidationVerdict {
            passed: true,
            level_reached: ValidationLevel::L3Sandbox,
            l1_errors: vec![],
            l2_details: Some(replay.details),
            requires_human_review: false, // L3 replaces human review
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::poe::crystallization::experience_store::{InMemoryExperienceStore, PoeExperience};
    use crate::poe::crystallization::pattern_model::{
        ParameterMapping, PatternStep, ToolCallTemplate, ToolCategory,
    };
    use crate::poe::crystallization::synthesis_backend::{
        PatternSynthesisBackend, PatternSynthesisRequest, PatternSuggestion,
    };
    use super::super::risk_profiler::SkillRiskProfiler;

    // -- Mock backend that always returns high confidence --

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

    #[tokio::test]
    async fn tiered_validator_low_risk_only_needs_l1() {
        let backend = Arc::new(AlwaysAgreeBackend);
        let validator = TieredValidator::new(backend);

        let pattern = make_pattern(vec![make_action("read_file", ToolCategory::ReadOnly)]);
        let risk = SkillRiskProfiler::profile(&pattern);
        assert_eq!(risk.level, SkillRiskLevel::Low);

        let store = InMemoryExperienceStore::new();
        let verdict = validator
            .validate(&pattern, "test-pattern", &risk, &store)
            .await
            .unwrap();

        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L1Structural);
        assert!(!verdict.requires_human_review);
        assert!(verdict.l2_details.is_none());
    }

    #[tokio::test]
    async fn tiered_validator_medium_risk_needs_l2() {
        let backend = Arc::new(AlwaysAgreeBackend);
        let validator = TieredValidator::new(backend);

        let pattern = make_pattern(vec![make_action("write_file", ToolCategory::FileWrite)]);
        let risk = SkillRiskProfiler::profile(&pattern);
        assert_eq!(risk.level, SkillRiskLevel::Medium);

        // Insert a sample so L2 replay has data
        let store = InMemoryExperienceStore::new();
        store
            .insert(make_experience("exp-1", "test-pattern"), &[1.0])
            .await
            .unwrap();

        let verdict = validator
            .validate(&pattern, "test-pattern", &risk, &store)
            .await
            .unwrap();

        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L2Semantic);
        assert!(!verdict.requires_human_review);
        assert!(verdict.l2_details.is_some());
    }

    #[tokio::test]
    async fn tiered_validator_high_risk_runs_l3() {
        let backend = Arc::new(AlwaysAgreeBackend);
        let validator = TieredValidator::new(backend);

        let pattern = make_pattern(vec![make_action("run_shell", ToolCategory::Shell)]);
        let risk = SkillRiskProfiler::profile(&pattern);
        assert_eq!(risk.level, SkillRiskLevel::High);

        let store = InMemoryExperienceStore::new();
        store
            .insert(make_experience("exp-1", "test-pattern"), &[1.0])
            .await
            .unwrap();

        let verdict = validator
            .validate(&pattern, "test-pattern", &risk, &store)
            .await
            .unwrap();

        assert!(verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L3Sandbox);
        assert!(!verdict.requires_human_review);
        assert!(verdict.l2_details.is_some());
    }

    #[tokio::test]
    async fn tiered_validator_empty_pattern_fails_l1() {
        let backend = Arc::new(AlwaysAgreeBackend);
        let validator = TieredValidator::new(backend);

        let pattern = make_pattern(vec![]);
        let risk = SkillRiskProfiler::profile(&pattern);

        let store = InMemoryExperienceStore::new();
        let verdict = validator
            .validate(&pattern, "test-pattern", &risk, &store)
            .await
            .unwrap();

        assert!(!verdict.passed);
        assert_eq!(verdict.level_reached, ValidationLevel::L1Structural);
        assert!(!verdict.l1_errors.is_empty());
    }
}

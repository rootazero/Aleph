//! Semantic replayer (L2) -- validates patterns via confidence-based replay.
//!
//! Uses a `PatternSynthesisBackend` to evaluate how well a pattern matches
//! each test sample, computing pass rates against a similarity threshold.

use crate::poe::crystallization::synthesis_backend::PatternSynthesisBackend;
use crate::sync_primitives::Arc;

use super::test_set_generator::ValidationTestSet;
use crate::poe::crystallization::pattern_model::PatternSequence;

// ============================================================================
// Types
// ============================================================================

/// Result of semantic replay validation.
#[derive(Debug, Clone)]
pub struct ReplayResult {
    pub passed: bool,
    pub avg_similarity: f64,
    pub passed_count: usize,
    pub total_count: usize,
    pub details: String,
}

// ============================================================================
// SemanticReplayer
// ============================================================================

/// Validates patterns by replaying test samples through an LLM backend (L2).
pub struct SemanticReplayer {
    backend: Arc<dyn PatternSynthesisBackend>,
    similarity_threshold: f64,
    pass_rate_threshold: f32,
}

impl SemanticReplayer {
    /// Create a new replayer with the given backend and similarity threshold.
    pub fn new(backend: Arc<dyn PatternSynthesisBackend>, similarity_threshold: f64) -> Self {
        Self {
            backend,
            similarity_threshold,
            pass_rate_threshold: 0.8,
        }
    }

    /// Replay all test samples against the pattern and evaluate confidence.
    pub async fn replay(
        &self,
        pattern: &PatternSequence,
        test_set: &ValidationTestSet,
    ) -> anyhow::Result<ReplayResult> {
        if test_set.samples.is_empty() {
            return Ok(ReplayResult {
                passed: true,
                avg_similarity: 0.0,
                passed_count: 0,
                total_count: 0,
                details: "no samples".to_string(),
            });
        }

        let mut total_similarity = 0.0_f64;
        let mut passed_count = 0_usize;

        for sample in &test_set.samples {
            let confidence = self
                .backend
                .evaluate_confidence(&pattern.description, &[sample.experience.clone()])
                .await?;

            let similarity = confidence as f64;
            total_similarity += similarity;

            if similarity >= self.similarity_threshold {
                passed_count += 1;
            }
        }

        let total = test_set.samples.len();
        let avg_similarity = total_similarity / total as f64;
        let pass_rate = passed_count as f32 / total as f32;
        let passed = pass_rate >= self.pass_rate_threshold;

        Ok(ReplayResult {
            passed,
            avg_similarity,
            passed_count,
            total_count: total,
            details: format!(
                "pass_rate={:.2} ({}/{}), avg_similarity={:.3}",
                pass_rate, passed_count, total, avg_similarity
            ),
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
    use crate::poe::crystallization::experience_store::PoeExperience;
    use crate::poe::crystallization::pattern_model::{
        ParameterMapping, PatternStep, ToolCallTemplate, ToolCategory,
    };
    use crate::poe::crystallization::synthesis_backend::{
        PatternSynthesisBackend, PatternSynthesisRequest, PatternSuggestion,
    };
    use super::super::test_set_generator::{SampleSource, TestSample};

    struct MockBackend {
        confidence: f32,
    }

    #[async_trait]
    impl PatternSynthesisBackend for MockBackend {
        async fn synthesize_pattern(
            &self,
            _request: PatternSynthesisRequest,
        ) -> anyhow::Result<PatternSuggestion> {
            Ok(PatternSuggestion {
                description: "mock".to_string(),
                steps: vec![],
                parameter_mapping: ParameterMapping::default(),
                pattern_hash: "mock".to_string(),
                confidence: self.confidence,
            })
        }

        async fn evaluate_confidence(
            &self,
            _pattern_hash: &str,
            _occurrences: &[PoeExperience],
        ) -> anyhow::Result<f32> {
            Ok(self.confidence)
        }
    }

    fn make_test_set(count: usize) -> ValidationTestSet {
        let samples = (0..count)
            .map(|i| TestSample {
                experience: PoeExperience {
                    id: format!("exp-{}", i),
                    task_id: format!("task-{}", i),
                    objective: "test".to_string(),
                    pattern_id: "test-pattern".to_string(),
                    tool_sequence_json: "[]".to_string(),
                    parameter_mapping: None,
                    satisfaction: 0.9,
                    distance_score: 0.1,
                    attempts: 1,
                    duration_ms: 1000,
                    created_at: 0,
                },
                source: SampleSource::ClusterRepresentative,
            })
            .collect();
        ValidationTestSet { samples }
    }

    fn make_pattern() -> PatternSequence {
        PatternSequence {
            description: "test pattern".to_string(),
            steps: vec![PatternStep::Action {
                tool_call: ToolCallTemplate {
                    tool_name: "read".to_string(),
                    category: ToolCategory::ReadOnly,
                },
                params: ParameterMapping::default(),
            }],
            expected_outputs: vec![],
        }
    }

    #[tokio::test]
    async fn high_confidence_passes() {
        let backend = Arc::new(MockBackend { confidence: 0.95 });
        let replayer = SemanticReplayer::new(backend, 0.8);
        let pattern = make_pattern();
        let test_set = make_test_set(5);

        let result = replayer.replay(&pattern, &test_set).await.unwrap();
        assert!(result.passed);
        assert_eq!(result.passed_count, 5);
        assert_eq!(result.total_count, 5);
        assert!(result.avg_similarity > 0.9);
    }

    #[tokio::test]
    async fn low_confidence_fails() {
        let backend = Arc::new(MockBackend { confidence: 0.3 });
        let replayer = SemanticReplayer::new(backend, 0.8);
        let pattern = make_pattern();
        let test_set = make_test_set(5);

        let result = replayer.replay(&pattern, &test_set).await.unwrap();
        assert!(!result.passed);
        assert_eq!(result.passed_count, 0);
        assert_eq!(result.total_count, 5);
    }
}

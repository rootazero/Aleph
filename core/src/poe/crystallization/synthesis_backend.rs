//! Backend trait for LLM-powered pattern synthesis.
//!
//! Provides the dependency-inverted interface that `PatternExtractor` uses
//! to request pattern generation from an LLM provider. Concrete implementations
//! live outside `core` (e.g., an OpenAI-backed synthesizer), keeping the core
//! free of provider-specific dependencies (R3).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::experience_store::PoeExperience;
use super::pattern_model::{ParameterMapping, PatternStep};

// ============================================================================
// Request / Response Types
// ============================================================================

/// A single tool-sequence execution trace used as input for synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSequenceTrace {
    /// JSON-encoded tool call sequence
    pub tool_sequence_json: String,
    /// User satisfaction score (0.0..=1.0)
    pub satisfaction: f32,
    /// Total execution time in milliseconds
    pub duration_ms: u64,
    /// Number of execution attempts
    pub attempts: u8,
}

/// Request payload for pattern synthesis (internal only, not serialized).
#[derive(Debug, Clone)]
pub struct PatternSynthesisRequest {
    /// High-level objective the pattern should achieve
    pub objective: String,
    /// Historical tool-sequence traces for this objective
    pub tool_sequences: Vec<ToolSequenceTrace>,
    /// Optional environment context (OS, shell, locale, etc.)
    pub env_context: Option<String>,
    /// Hashes of patterns already known, to avoid duplicates
    pub existing_patterns: Vec<String>,
}

/// A synthesized pattern suggestion returned by the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternSuggestion {
    /// Human-readable description of what the pattern does
    pub description: String,
    /// Ordered steps that constitute the pattern
    pub steps: Vec<PatternStep>,
    /// Variable bindings for tool parameters
    pub parameter_mapping: ParameterMapping,
    /// Content-addressable hash for deduplication
    pub pattern_hash: String,
    /// Backend confidence in the suggestion (0.0..=1.0)
    pub confidence: f32,
}

// ============================================================================
// PatternSynthesisBackend Trait
// ============================================================================

/// Dependency-inverted interface for LLM-powered pattern synthesis.
///
/// The `PatternExtractor` holds an `Arc<dyn PatternSynthesisBackend>` and
/// delegates the actual LLM interaction to the concrete implementation,
/// keeping `core` free of provider-specific code.
#[async_trait]
pub trait PatternSynthesisBackend: Send + Sync {
    /// Synthesize a reusable pattern from observed tool-sequence traces.
    async fn synthesize_pattern(
        &self,
        request: PatternSynthesisRequest,
    ) -> anyhow::Result<PatternSuggestion>;

    /// Re-evaluate confidence for an existing pattern given new occurrences.
    async fn evaluate_confidence(
        &self,
        pattern_hash: &str,
        occurrences: &[PoeExperience],
    ) -> anyhow::Result<f32>;
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pattern_model::{
        ParameterMapping, PatternStep, ToolCallTemplate, ToolCategory,
    };

    // -- Mock backend --------------------------------------------------------

    struct MockBackend;

    #[async_trait]
    impl PatternSynthesisBackend for MockBackend {
        async fn synthesize_pattern(
            &self,
            request: PatternSynthesisRequest,
        ) -> anyhow::Result<PatternSuggestion> {
            Ok(PatternSuggestion {
                description: format!("Pattern for: {}", request.objective),
                steps: vec![PatternStep::Action {
                    tool_call: ToolCallTemplate {
                        tool_name: "mock_tool".to_string(),
                        category: ToolCategory::ReadOnly,
                    },
                    params: ParameterMapping::default(),
                }],
                parameter_mapping: ParameterMapping::default(),
                pattern_hash: "mock-hash-001".to_string(),
                confidence: 0.85,
            })
        }

        async fn evaluate_confidence(
            &self,
            _pattern_hash: &str,
            occurrences: &[PoeExperience],
        ) -> anyhow::Result<f32> {
            // Simple heuristic: more occurrences => higher confidence
            let base = 0.5_f32;
            let bonus = (occurrences.len() as f32 * 0.1).min(0.5);
            Ok(base + bonus)
        }
    }

    // -- Tests ---------------------------------------------------------------

    #[tokio::test]
    async fn test_mock_backend_synthesize() {
        let backend = MockBackend;
        let request = PatternSynthesisRequest {
            objective: "compile project".to_string(),
            tool_sequences: vec![ToolSequenceTrace {
                tool_sequence_json: r#"["cargo build"]"#.to_string(),
                satisfaction: 0.9,
                duration_ms: 1200,
                attempts: 1,
            }],
            env_context: None,
            existing_patterns: vec![],
        };

        let suggestion = backend.synthesize_pattern(request).await.unwrap();
        assert_eq!(suggestion.description, "Pattern for: compile project");
        assert_eq!(suggestion.pattern_hash, "mock-hash-001");
        assert!((suggestion.confidence - 0.85).abs() < f32::EPSILON);
        assert_eq!(suggestion.steps.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_backend_evaluate_confidence() {
        let backend = MockBackend;

        let make_exp = |id: &str| PoeExperience {
            id: id.to_string(),
            task_id: "t1".to_string(),
            objective: "test".to_string(),
            pattern_id: "poe-test".to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction: 0.8,
            distance_score: 0.2,
            attempts: 1,
            duration_ms: 100,
            created_at: 0,
        };

        // 0 occurrences => base confidence
        let conf = backend
            .evaluate_confidence("hash", &[])
            .await
            .unwrap();
        assert!((conf - 0.5).abs() < f32::EPSILON);

        // 3 occurrences => 0.5 + 0.3 = 0.8
        let exps = vec![make_exp("a"), make_exp("b"), make_exp("c")];
        let conf = backend
            .evaluate_confidence("hash", &exps)
            .await
            .unwrap();
        assert!((conf - 0.8).abs() < f32::EPSILON);
    }
}

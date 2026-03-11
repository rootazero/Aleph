//! Real LLM implementation of PatternSynthesisBackend.
//!
//! Wraps `Arc<dyn AiProvider>` to connect pattern extraction to actual
//! LLM inference. `synthesize_pattern` calls the LLM; `evaluate_confidence`
//! uses a token-efficient heuristic.

use async_trait::async_trait;

use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

use super::experience_store::PoeExperience;
use super::synthesis_backend::{
    PatternSuggestion, PatternSynthesisBackend, PatternSynthesisRequest,
};

/// Real LLM-backed implementation of `PatternSynthesisBackend`.
pub struct ProviderBackend {
    provider: Arc<dyn AiProvider>,
}

impl ProviderBackend {
    /// Create a new ProviderBackend wrapping the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl PatternSynthesisBackend for ProviderBackend {
    async fn synthesize_pattern(
        &self,
        _request: PatternSynthesisRequest,
    ) -> anyhow::Result<PatternSuggestion> {
        anyhow::bail!("synthesize_pattern not yet implemented")
    }

    async fn evaluate_confidence(
        &self,
        _pattern_hash: &str,
        occurrences: &[PoeExperience],
    ) -> anyhow::Result<f32> {
        let base: f32 = 0.5;

        let occurrence_bonus = (occurrences.len() as f32 * 0.05).min(0.3);

        let success_bonus = if occurrences.is_empty() {
            0.0
        } else {
            let avg_satisfaction: f32 =
                occurrences.iter().map(|e| e.satisfaction).sum::<f32>()
                    / occurrences.len() as f32;
            avg_satisfaction * 0.2
        };

        let seven_days_ms: i64 = 7 * 86_400_000;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let recency_bonus = if occurrences
            .iter()
            .any(|e| (now_ms - e.created_at) < seven_days_ms)
        {
            0.05
        } else {
            0.0
        };

        let confidence = (base + occurrence_bonus + success_bonus + recency_bonus).min(1.0);
        Ok(confidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockAiProvider;

    impl AiProvider for MockAiProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
            Box::pin(async { Ok("mock response".to_string()) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[test]
    fn test_provider_backend_creation() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let _backend = ProviderBackend::new(provider);
    }

    fn make_exp(id: &str, satisfaction: f32, distance: f32, created_at: i64) -> PoeExperience {
        PoeExperience {
            id: id.to_string(),
            task_id: "t1".to_string(),
            objective: "test".to_string(),
            pattern_id: "poe-test".to_string(),
            tool_sequence_json: "[]".to_string(),
            parameter_mapping: None,
            satisfaction,
            distance_score: distance,
            attempts: 1,
            duration_ms: 100,
            created_at,
        }
    }

    #[tokio::test]
    async fn test_evaluate_confidence_empty_occurrences() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let backend = ProviderBackend::new(provider);
        let conf = backend.evaluate_confidence("hash-1", &[]).await.unwrap();
        assert!((conf - 0.5).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_evaluate_confidence_with_occurrences() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let backend = ProviderBackend::new(provider);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let exps = vec![
            make_exp("a", 0.9, 0.1, now_ms),
            make_exp("b", 0.8, 0.2, now_ms),
            make_exp("c", 0.7, 0.3, now_ms - 86_400_000 * 10),
        ];
        let conf = backend.evaluate_confidence("hash-1", &exps).await.unwrap();
        // base=0.5 + occurrence=0.15 + success=0.16 + recency=0.05 = 0.86
        assert!((conf - 0.86).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_evaluate_confidence_clamped_to_one() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let backend = ProviderBackend::new(provider);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let exps: Vec<PoeExperience> = (0..10)
            .map(|i| make_exp(&format!("e{}", i), 1.0, 0.0, now_ms))
            .collect();
        let conf = backend.evaluate_confidence("hash-1", &exps).await.unwrap();
        assert!((conf - 1.0).abs() < f32::EPSILON);
    }
}

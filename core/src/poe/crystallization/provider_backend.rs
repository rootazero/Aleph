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

use serde_json;

/// Real LLM-backed implementation of `PatternSynthesisBackend`.
pub struct ProviderBackend {
    provider: Arc<dyn AiProvider>,
}

impl ProviderBackend {
    /// Create a new ProviderBackend wrapping the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Build the system prompt instructing the LLM to output PatternSuggestion JSON.
    fn build_system_prompt() -> String {
        r#"You are an expert at analyzing tool execution traces and extracting reusable patterns.

Given a set of tool-sequence traces for a common objective, synthesize a single reusable pattern.

Output ONLY a JSON object matching this schema:
{
  "description": "What this pattern does (1-2 sentences)",
  "steps": [PatternStep],
  "parameter_mapping": { "variables": {} },
  "pattern_hash": "content-hash-string",
  "confidence": 0.0-1.0
}

PatternStep is one of:
- {"step_type": "Action", "tool_call": {"tool_name": "...", "category": "ReadOnly|FileWrite|Shell|Network|CrossPlugin|Destructive"}, "params": {"variables": {}}}
- {"step_type": "Conditional", "predicate": {"type": "Semantic", "value": "condition text"}, "then_steps": [...], "else_steps": [...]}
- {"step_type": "Loop", "predicate": {"type": "Semantic", "value": "condition"}, "body": [...], "max_iterations": N}
- {"step_type": "SubPattern", "pattern_id": "existing-pattern-id"}

Output ONLY the JSON, no markdown fences, no explanation."#
            .to_string()
    }

    /// Build the user prompt from the synthesis request fields.
    fn build_user_prompt(request: &PatternSynthesisRequest) -> String {
        let traces_json = serde_json::to_string_pretty(&request.tool_sequences)
            .unwrap_or_else(|_| "[]".to_string());
        let env = request.env_context.as_deref().unwrap_or("not provided");
        let existing = if request.existing_patterns.is_empty() {
            "none".to_string()
        } else {
            request.existing_patterns.join(", ")
        };
        format!(
            "Objective: {objective}\n\nTool Sequence Traces:\n{traces}\n\nEnvironment: {env}\n\nExisting patterns (avoid duplicates): {existing}",
            objective = request.objective,
            traces = traces_json,
            env = env,
            existing = existing,
        )
    }
}

#[async_trait]
impl PatternSynthesisBackend for ProviderBackend {
    async fn synthesize_pattern(
        &self,
        request: PatternSynthesisRequest,
    ) -> anyhow::Result<PatternSuggestion> {
        let system_prompt = Self::build_system_prompt();
        let user_prompt = Self::build_user_prompt(&request);

        let response = self
            .provider
            .process(&user_prompt, Some(&system_prompt))
            .await
            .map_err(|e| anyhow::anyhow!("LLM call failed: {}", e))?;

        // Try robust JSON extraction first (handles ```json blocks)
        if let Some(json_value) =
            crate::utils::json_extract::extract_json_robust(&response)
        {
            let suggestion: PatternSuggestion = serde_json::from_value(json_value)
                .map_err(|e| {
                    anyhow::anyhow!("Failed to parse PatternSuggestion: {}", e)
                })?;
            return Ok(suggestion);
        }

        // Fallback: try direct parse
        let suggestion: PatternSuggestion =
            serde_json::from_str(&response).map_err(|e| {
                anyhow::anyhow!(
                    "LLM response is not valid PatternSuggestion JSON: {}",
                    e
                )
            })?;
        Ok(suggestion)
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

    struct JsonMockProvider {
        response: String,
    }

    impl AiProvider for JsonMockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>,
        > {
            let resp = self.response.clone();
            Box::pin(async move { Ok(resp) })
        }

        fn name(&self) -> &str {
            "json-mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    struct ErrorMockProvider;

    impl AiProvider for ErrorMockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>,
        > {
            Box::pin(async {
                Err(crate::error::AlephError::ProviderError {
                    message: "connection refused".to_string(),
                    suggestion: None,
                })
            })
        }

        fn name(&self) -> &str {
            "error-mock"
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

    #[tokio::test]
    async fn test_synthesize_pattern_success() {
        let json_response = serde_json::json!({
            "description": "Compile and test Rust project",
            "steps": [{
                "step_type": "Action",
                "tool_call": { "tool_name": "cargo", "category": "Shell" },
                "params": { "variables": {} }
            }],
            "parameter_mapping": { "variables": {} },
            "pattern_hash": "abc123",
            "confidence": 0.9
        });
        let provider: Arc<dyn AiProvider> =
            Arc::new(JsonMockProvider { response: json_response.to_string() });
        let backend = ProviderBackend::new(provider);
        let request = PatternSynthesisRequest {
            objective: "Build the project".to_string(),
            tool_sequences: vec![],
            env_context: None,
            existing_patterns: vec![],
        };
        let result = backend.synthesize_pattern(request).await;
        assert!(result.is_ok());
        let suggestion = result.unwrap();
        assert_eq!(suggestion.description, "Compile and test Rust project");
        assert_eq!(suggestion.pattern_hash, "abc123");
    }

    #[tokio::test]
    async fn test_synthesize_pattern_invalid_json() {
        let provider: Arc<dyn AiProvider> =
            Arc::new(JsonMockProvider { response: "Not JSON".to_string() });
        let backend = ProviderBackend::new(provider);
        let request = PatternSynthesisRequest {
            objective: "Build".to_string(),
            tool_sequences: vec![],
            env_context: None,
            existing_patterns: vec![],
        };
        assert!(backend.synthesize_pattern(request).await.is_err());
    }

    #[tokio::test]
    async fn test_synthesize_pattern_provider_error() {
        let provider: Arc<dyn AiProvider> = Arc::new(ErrorMockProvider);
        let backend = ProviderBackend::new(provider);
        let request = PatternSynthesisRequest {
            objective: "Build".to_string(),
            tool_sequences: vec![],
            env_context: None,
            existing_patterns: vec![],
        };
        assert!(backend.synthesize_pattern(request).await.is_err());
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

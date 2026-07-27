use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::store::sqlite::routing_experience::{
    ModelAggregate, RoutingExperienceRow, RoutingNeighbor,
};
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::orchestrator::dispatch::TokenBreakdown;

pub const DEFAULT_ROUTING_RETENTION_CAP: usize = 5000;

/// Zero-judgment feedback surface — every field is a raw fact (§5.2). No
/// `success: bool`, no `quality_score`, no `user_re_steer`, no `consecutive_errors`.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingOutcome {
    pub iterations: u32,
    pub tool_calls_made: u32,
    pub terminate_reason: String,
    pub token_breakdown: TokenBreakdown,
    pub estimated_cost: Option<f64>,
    pub duration_ms: u64,
    pub tool_error_count: u32,
    pub tool_call_total: u32,
}

pub struct RoutingExperienceStore {
    backend: Arc<SqliteMemoryBackend>,
    embedder: Arc<dyn EmbeddingProvider>,
    retention_cap: usize,
}

impl RoutingExperienceStore {
    #[must_use]
    pub fn new(backend: Arc<SqliteMemoryBackend>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            backend,
            embedder,
            retention_cap: DEFAULT_ROUTING_RETENTION_CAP,
        }
    }

    pub async fn embed_task(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        self.embedder.embed(text).await
    }

    pub async fn record(
        &self,
        agent_id: &str,
        model_id: &str,
        provider_id: &str,
        task_emb: &[f32],
        outcome: &RoutingOutcome,
    ) -> Result<(), AlephError> {
        let dim = self.embedder.dimensions() as u32;
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let row = RoutingExperienceRow {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            model_id: model_id.to_string(),
            provider_id: provider_id.to_string(),
            terminate_reason: outcome.terminate_reason.clone(),
            iterations: outcome.iterations as i64,
            tool_calls: outcome.tool_calls_made as i64,
            tool_error_count: outcome.tool_error_count as i64,
            tool_call_total: outcome.tool_call_total as i64,
            tok_input: outcome.token_breakdown.input as i64,
            tok_output: outcome.token_breakdown.output as i64,
            tok_cache_read: outcome.token_breakdown.cache_read as i64,
            tok_cache_creation: outcome.token_breakdown.cache_creation as i64,
            tok_reasoning: outcome.token_breakdown.reasoning as i64,
            estimated_cost: outcome.estimated_cost,
            duration_ms: outcome.duration_ms as i64,
            // The two context-pressure columns are written as 0 and stay in the
            // schema for row compatibility. They were `RoutingOutcome` fields,
            // which made them look producible — but the only production producer
            // (`observer::outcome_from_session_completed`) hardcoded 0 because
            // `LoopTraceEvent::SessionCompleted` carries no context-pressure
            // fact, and no renderer read them back. Removing them from the type
            // stops the struct from advertising a value nothing can supply;
            // filling them for real means putting the fact on the trace event
            // first, which is a harness change.
            context_tokens: 0,
            context_window: 0,
            created_at,
        };
        self.backend
            .record_routing_experience(&row, task_emb, dim)?;
        self.backend
            .prune_routing_experiences(agent_id, dim, self.retention_cap)?;
        Ok(())
    }

    pub async fn recall(
        &self,
        agent_id: &str,
        task_emb: &[f32],
        k: usize,
    ) -> Result<Vec<RoutingNeighbor>, AlephError> {
        let dim = self.embedder.dimensions() as u32;
        self.backend
            .recall_routing_experience(task_emb, dim, agent_id, k)
    }

    /// Per-(model, provider) lifetime aggregate for one agent (VESR v1.1 a).
    /// Raw facts only — the recall block renders these for the LLM to weigh.
    pub async fn aggregate_by_model(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ModelAggregate>, AlephError> {
        self.backend
            .aggregate_routing_experiences_by_model(agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-fac-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    struct StubEmbedder {
        dim: usize,
        vec: Vec<f32>,
    }
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> {
            Ok(self.vec.clone())
        }
        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| self.vec.clone()).collect())
        }
        fn dimensions(&self) -> usize {
            self.dim
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    #[tokio::test]
    async fn facade_record_then_recall_roundtrip() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder {
            dim: 768,
            vec: emb(1.0),
        });
        let store = RoutingExperienceStore::new(backend, embedder);
        let outcome = RoutingOutcome {
            iterations: 2,
            tool_calls_made: 1,
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            token_breakdown: TokenBreakdown::default(),
            estimated_cost: None,
            duration_ms: 10,
            tool_error_count: 0,
            tool_call_total: 1,
        };
        store
            .record("a", "MODEL_X", "PROV_Y", &emb(1.0), &outcome)
            .await
            .unwrap();
        let got = store.recall("a", &emb(0.0), 5).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model_id, "MODEL_X");
        assert_eq!(got[0].provider_id, "PROV_Y");
        assert_eq!(got[0].iterations, 2);
    }
}

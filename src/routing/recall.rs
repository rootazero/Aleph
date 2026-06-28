//! Run-start recall for VESR (Verified-Experience Self-Routing).
//!
//! [`RoutingRecall::build_routing_experience_message`] is invoked ONCE per run
//! at the orchestrator run-start (Task 4c wires it). It embeds the user query,
//! backfills [`RoutingAttribution::task_emb`] for record/recall symmetry (§8 D6),
//! recalls k-NN neighbors, marks unavailable-provider entries (O4: mark, not
//! filter), and fence-wraps via [`wrap_memory_context`]. Returns `None` on
//! cold-start or empty recall — the caller skips injection.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::ProviderConfig;
use crate::error::AlephError;
use crate::gateway::security::SharedTokenManager;
use crate::memory::assembler::context_block::wrap_memory_context;
use crate::memory::store::sqlite::routing_experience::RoutingNeighbor;

use super::experience_store::RoutingExperienceStore;
use super::RoutingAttribution;

/// Currently-configured predicate over a provider id.
pub type ProviderAvailability = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Build the availability gate from boot config + vault. Lives in the lib so it
/// can call the `pub(crate)` `resolve_vault_secret`; the binary calls only this
/// `pub` constructor. Same gate semantics as `list_models::provider_configured`.
#[must_use]
pub fn provider_availability_from_config(
    providers: HashMap<String, ProviderConfig>,
    token_manager: Option<Arc<SharedTokenManager>>,
) -> ProviderAvailability {
    Arc::new(move |provider: &str| {
        if providers.get(provider).and_then(|c| c.api_key.as_ref()).is_some() {
            return true;
        }
        match &token_manager {
            Some(tm) => {
                crate::gateway::handlers::resolve_vault_secret(&format!("ai:{provider}"), tm)
                    .is_some()
            }
            None => false,
        }
    })
}

pub struct RoutingRecall {
    store: Arc<RoutingExperienceStore>,
    availability: ProviderAvailability,
    k: usize,
}

impl RoutingRecall {
    #[must_use]
    pub fn new(store: Arc<RoutingExperienceStore>, availability: ProviderAvailability) -> Self {
        Self { store, availability, k: 5 }
    }

    pub async fn build_routing_experience_message(
        &self,
        user_query: &str,
        agent_id: &str,
        _available_tokens: Option<u32>,
        attribution: &RoutingAttribution,
    ) -> Result<Option<String>, AlephError> {
        // Embed once; backfill attribution so the observer attributes with the
        // SAME key recall queried with (§8 D6).
        let task_emb = self.store.embed_task(user_query).await?;
        let _ = attribution.task_emb.set(task_emb.clone());

        let neighbors = self.store.recall(agent_id, &task_emb, self.k).await?;
        if neighbors.is_empty() {
            return Ok(None); // cold start: behave exactly like today's blind selection (D1)
        }
        let rendered = render_neighbors(&neighbors, &self.availability);
        if rendered.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(wrap_memory_context(&rendered)))
    }
}

fn render_neighbors(neighbors: &[RoutingNeighbor], availability: &ProviderAvailability) -> String {
    let mut out = String::new();
    out.push_str(
        "Verified routing experience from semantically similar past tasks (raw observations, \
         NOT a recommendation — weigh them yourself; discount far/old/low-sample entries):\n",
    );
    for n in neighbors {
        let avail_tag = if (availability)(&n.provider_id) {
            ""
        } else {
            " [UNAVAILABLE: provider not currently configured — do NOT select]"
        };
        out.push_str(&format!(
            "- model={} provider={}{} distance={:.4} terminate_reason={} iterations={} \
             tool_errors={}/{} tokens(in/out/cache_r/cache_c/reason)={}/{}/{}/{}/{} \
             duration_ms={} age_unix={}\n",
            n.model_id,
            n.provider_id,
            avail_tag,
            n.distance,
            n.terminate_reason,
            n.iterations,
            n.tool_error_count,
            n.tool_call_total,
            n.tok_input,
            n.tok_output,
            n.tok_cache_read,
            n.tok_cache_creation,
            n.tok_reasoning,
            n.duration_ms,
            n.created_at,
        ));
    }
    out.push_str(
        "Models without observations on this kind of task are unproven, not bad — you may \
         explore one if it fits.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::sqlite::routing_experience::RoutingExperienceRow;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::EmbeddingProvider;

    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir()
            .join(format!("aleph-routing-rec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    struct StubEmbedder {
        vec: Vec<f32>,
    }
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> {
            Ok(self.vec.clone())
        }
        async fn embed_batch(&self, t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(t.iter().map(|_| self.vec.clone()).collect())
        }
        fn dimensions(&self) -> usize {
            768
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }
    fn row(id: &str, agent: &str, model: &str, provider: &str) -> RoutingExperienceRow {
        RoutingExperienceRow {
            id: id.into(),
            agent_id: agent.into(),
            model_id: model.into(),
            provider_id: provider.into(),
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            iterations: 0,
            tool_calls: 0,
            tool_error_count: 0,
            tool_call_total: 0,
            tok_input: 0,
            tok_output: 0,
            tok_cache_read: 0,
            tok_cache_creation: 0,
            tok_reasoning: 0,
            estimated_cost: None,
            duration_ms: 0,
            context_tokens: 0,
            context_window: 0,
            created_at: 1,
        }
    }

    #[tokio::test]
    async fn record_and_recall_share_one_embedding_key() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(0.7) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let avail: ProviderAvailability = Arc::new(|_p: &str| true);
        let recall = RoutingRecall::new(store.clone(), avail);
        let attribution = RoutingAttribution::new("s".into());
        let _ = recall
            .build_routing_experience_message("same text", "a", None, &attribution)
            .await
            .unwrap();
        let recalled_key = attribution.task_emb.get().cloned().unwrap();
        let direct = store.embed_task("same text").await.unwrap();
        assert_eq!(recalled_key, direct); // observer attributes with the key recall queried with
    }

    #[tokio::test]
    async fn recalled_unavailable_model_is_marked() {
        let backend = Arc::new(temp_backend());
        backend
            .record_routing_experience(&row("1", "a", "m-dead", "deadprov"), &emb(1.0), 768)
            .unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let avail: ProviderAvailability = Arc::new(|p: &str| p != "deadprov");
        let recall = RoutingRecall::new(store, avail);
        let attribution = RoutingAttribution::new("s".into());
        let msg = recall
            .build_routing_experience_message("do X", "a", None, &attribution)
            .await
            .unwrap()
            .unwrap();
        assert!(msg.contains("UNAVAILABLE")); // O4: marked, not filtered
        assert!(msg.contains("m-dead")); // still visible to the LLM
        assert!(msg.contains("memory-context")); // fence-wrapped
    }

    #[tokio::test]
    async fn cold_start_returns_none() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let avail: ProviderAvailability = Arc::new(|_p: &str| true);
        let recall = RoutingRecall::new(store, avail);
        let attribution = RoutingAttribution::new("s".into());
        let msg = recall
            .build_routing_experience_message("do X", "a", None, &attribution)
            .await
            .unwrap();
        assert!(msg.is_none());
        assert!(attribution.task_emb.get().is_some()); // embed still happened
    }
}

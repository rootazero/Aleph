//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route
//! resolution (channel/peer → agent + session). Deterministic, config-driven
//! plumbing only — semantic intent classification is the LLM's job (R7), never
//! a regex layer here.

pub mod config;
pub mod experience_store;
pub mod identity_links;
pub mod observer;
pub mod overlay;
pub mod recall;
pub mod resolve;
pub mod session_key;

pub use experience_store::{RoutingExperienceStore, RoutingOutcome};
pub use observer::{outcome_from_session_completed, OutcomeObserver};
pub use recall::{
    provider_availability_from_config, ProviderAvailability, ProviderStatus, RoutingRecall,
};

pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use overlay::{overlay_route, OverlaidRoute, OverlaySource, RuntimeOverlay};
pub use resolve::{resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use session_key::{
    normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY,
};

/// Per-run handle correlating run-start recall (writes `task_emb`) with the
/// completion observer (reads it). One per run; lives in the gateway run loop,
/// outside the harness. `session_id` is read by the observer for trace logging.
///
/// Spec §6 types `task_emb` as `OnceCell`; we use `std::sync::OnceLock`
/// (std, no extra dep) — same write-once semantics. Flagged divergence.
pub struct RoutingAttribution {
    pub session_id: String,
    pub task_emb: std::sync::OnceLock<Vec<f32>>,
}

impl RoutingAttribution {
    #[must_use]
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            task_emb: std::sync::OnceLock::new(),
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::error::AlephError;
    use crate::harness::trace::{LoopTraceEvent, LoopTraceSessionOutcome};
    use crate::harness::TraceSink;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::memory::EmbeddingProvider;
    use crate::orchestrator::dispatch::{TerminateReason, TokenBreakdown, ToolInvocation};

    use super::{OutcomeObserver, RoutingAttribution, RoutingExperienceStore};

    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-int-{}", uuid::Uuid::new_v4()));
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
    #[derive(Default)]
    struct SpySink {
        session_completed: AtomicUsize,
    }
    impl TraceSink for SpySink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            if matches!(event, LoopTraceEvent::SessionCompleted { .. }) {
                self.session_completed.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn flush(&self) {}
    }
    fn session_completed() -> LoopTraceEvent {
        LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Completed,
            iterations: 2,
            tool_calls_made: 1,
            total_tokens: 30,
            hit_limit: false,
            final_text: Some("done".into()),
            terminate_reason: Some(TerminateReason::Completed),
            duration_ms: Some(123),
            token_breakdown: Some(TokenBreakdown {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_creation: 0,
                reasoning: 0,
            }),
            tool_timeline: vec![ToolInvocation {
                id: "1".into(),
                name: "bash".into(),
                duration_ms: 5,
                success: true,
                error: None,
            }],
        }
    }
    async fn drain_until_row(
        store: &RoutingExperienceStore,
        agent: &str,
    ) -> Vec<crate::memory::store::sqlite::routing_experience::RoutingNeighbor> {
        // `#[tokio::test]` is current-thread: yielding lets the spawned
        // fire-and-forget record task run. Bounded poll → deterministic.
        let mut got = Vec::new();
        for _ in 0..200 {
            tokio::task::yield_now().await;
            got = store.recall(agent, &emb(0.0), 5).await.unwrap();
            if !got.is_empty() {
                break;
            }
        }
        got
    }

    #[tokio::test]
    async fn observer_on_trace_records_through_real_sink() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let spy = Arc::new(SpySink::default());
        let attribution = Arc::new(RoutingAttribution::new("run".into()));
        attribution.task_emb.set(emb(1.0)).unwrap(); // recall would have set this
        let observer = OutcomeObserver::new(
            spy.clone() as Arc<dyn TraceSink>,
            store.clone(),
            attribution,
            "MODEL_X".into(),
            "PROV_Y".into(),
            "agentA".into(),
        );
        observer.on_trace(&session_completed());
        let got = drain_until_row(&store, "agentA").await;
        assert_eq!(
            spy.session_completed.load(Ordering::SeqCst),
            1,
            "forwarded unchanged"
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model_id, "MODEL_X");
        assert_eq!(got[0].provider_id, "PROV_Y");
        assert_eq!(got[0].agent_id, "agentA");
        assert_eq!(got[0].iterations, 2);
        assert_eq!(got[0].tool_call_total, 1);
    }

    #[tokio::test]
    async fn observer_enriches_known_model_usd_cost() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let attribution = Arc::new(RoutingAttribution::new("run".into()));
        attribution.task_emb.set(emb(1.0)).unwrap();
        // anthropic / claude-sonnet-4-6 is in the static price table → priced.
        let observer = OutcomeObserver::new(
            Arc::new(SpySink::default()) as Arc<dyn TraceSink>,
            store.clone(),
            attribution,
            "claude-sonnet-4-6".into(),
            "anthropic".into(),
            "agentCost".into(),
        );
        observer.on_trace(&session_completed());
        let got = drain_until_row(&store, "agentCost").await;
        assert_eq!(got.len(), 1);
        let cost = got[0].estimated_cost.expect("known model must price");
        assert!(cost > 0.0, "non-zero tokens x known rate must be > 0");
    }

    #[tokio::test]
    async fn observer_leaves_unknown_provider_cost_none() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
        let attribution = Arc::new(RoutingAttribution::new("run".into()));
        attribution.task_emb.set(emb(1.0)).unwrap();
        let observer = OutcomeObserver::new(
            Arc::new(SpySink::default()) as Arc<dyn TraceSink>,
            store.clone(),
            attribution,
            "(dynamic)".into(),
            "(dynamic)".into(),
            "agentDyn".into(),
        );
        observer.on_trace(&session_completed());
        let got = drain_until_row(&store, "agentDyn").await;
        assert_eq!(got.len(), 1);
        assert!(
            got[0].estimated_cost.is_none(),
            "unknown provider -> no estimate"
        );
    }

    #[tokio::test]
    async fn parent_and_child_attribution_isolated() {
        let backend = Arc::new(temp_backend());
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
        let store = Arc::new(RoutingExperienceStore::new(backend, embedder));

        // Two independently-constructed observers (the per-run sink-construction
        // model: each run freezes its own model + agent + attribution).
        let attr_p = Arc::new(RoutingAttribution::new("p".into()));
        attr_p.task_emb.set(emb(1.0)).unwrap();
        let obs_p = OutcomeObserver::new(
            Arc::new(SpySink::default()) as Arc<dyn TraceSink>,
            store.clone(),
            attr_p,
            "M".into(),
            "P".into(),
            "parent".into(),
        );
        let attr_c = Arc::new(RoutingAttribution::new("c".into()));
        attr_c.task_emb.set(emb(2.0)).unwrap();
        let obs_c = OutcomeObserver::new(
            Arc::new(SpySink::default()) as Arc<dyn TraceSink>,
            store.clone(),
            attr_c,
            "N".into(),
            "P".into(),
            "child".into(),
        );

        obs_p.on_trace(&session_completed());
        obs_c.on_trace(&session_completed());
        let p = drain_until_row(&store, "parent").await;
        let c = drain_until_row(&store, "child").await;
        assert!(p.iter().all(|n| n.model_id == "M")); // parent never absorbs child
        assert!(c.iter().all(|n| n.model_id == "N")); // child never written to parent's model
    }

    #[test]
    fn build_prompt_path_never_references_routing_recall() {
        // Source-level guard: recall is run-start only; the per-turn prompt
        // assembly (`prompt.rs::build_prompt`, called by `think.rs`) must never
        // touch routing recall (R10 — loop stays dumb).
        let prompt_src = include_str!("../harness/agent/prompt.rs");
        let think_src = include_str!("../harness/agent/think.rs");
        for needle in [
            "RoutingRecall",
            "build_routing_experience_message",
            "routing_recall",
        ] {
            assert!(
                !prompt_src.contains(needle),
                "prompt.rs must not reference {needle}"
            );
            assert!(
                !think_src.contains(needle),
                "think.rs must not reference {needle}"
            );
        }
    }
}

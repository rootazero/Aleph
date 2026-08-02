use super::{MemoryContextConfig, MemoryContextProvider, NoopReranker};
use crate::config::types::memory::{AssemblerConfig, MemoryInjectionMode};
use crate::memory::assembler::hybrid::AiProviderReranker;
use crate::memory::assembler::{
    FeedbackFloorLoader, HybridAssembler, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::memory::curated::CuratedConfig;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use dashmap::DashMap;
use std::collections::HashMap;
use tokio::sync::RwLock as TokioRwLock;

impl MemoryContextProvider {
    /// Create a provider with the legacy 2-argument signature. No
    /// [`AiProvider`] supplied → the assembler falls back to the deterministic
    /// skeleton for every turn. Use [`Self::with_provider`] to wire a real
    /// LLM reranker. `embedder: None` = FTS-only deployment: retrieval
    /// degrades to keyword search, memory injection keeps working.
    pub fn new(memory_db: MemoryBackend, embedder: Option<Arc<dyn EmbeddingProvider>>) -> Self {
        Self::with_config(memory_db, embedder, MemoryContextConfig::default())
    }

    /// Create with legacy 2-arg + custom config.
    pub fn with_config(
        memory_db: MemoryBackend,
        embedder: Option<Arc<dyn EmbeddingProvider>>,
        config: MemoryContextConfig,
    ) -> Self {
        Self::assemble_default(
            memory_db,
            embedder,
            None,
            AssemblerConfig::default(),
            config,
        )
    }

    /// Create with an [`AiProvider`] so the LLM re-rank path is active.
    pub fn with_provider(
        memory_db: MemoryBackend,
        embedder: Option<Arc<dyn EmbeddingProvider>>,
        provider: Arc<dyn AiProvider>,
        assembler_config: AssemblerConfig,
        config: MemoryContextConfig,
    ) -> Self {
        Self::assemble_default(
            memory_db,
            embedder,
            Some(provider),
            assembler_config,
            config,
        )
    }

    /// Escape hatch: bring your own pre-built assembler (for tests / Spec 2+).
    pub fn with_assembler(
        assembler: Arc<dyn WorkingMemoryAssembler>,
        config: MemoryContextConfig,
    ) -> Self {
        Self {
            assembler,
            config,
            injection_mode: MemoryInjectionMode::default(),
            extensions: crate::sync_primitives::Arc::new(
                crate::memory::extensions::MemoryExtensionRegistry::new(),
            ),
            orientation: None,
            orientation_budget: crate::memory::notes::orientation::types::TokenBudget::default(),
            profile: None,
            curated_snapshots: Arc::new(TokioRwLock::new(HashMap::new())),
            orientation_snapshots: Arc::new(TokioRwLock::new(HashMap::new())),
            curated_stores: Arc::new(DashMap::new()),
            curated_config: CuratedConfig::default(),
            #[cfg(test)]
            curated_root_override: None,
        }
    }

    /// Set the injection mode on an existing provider (builder-style).
    #[must_use]
    pub const fn with_injection_mode(mut self, mode: MemoryInjectionMode) -> Self {
        self.injection_mode = mode;
        self
    }

    /// The configured injection mode. Callers that drive the mode-gated
    /// builder (`build_orientation_user_message`) pass this back in so the
    /// gate reflects the provider's own config.
    #[must_use]
    pub const fn injection_mode(&self) -> MemoryInjectionMode {
        self.injection_mode
    }

    /// The extension registry this provider dispatches through. Used by the
    /// session-end emit path to run the same `on_capture` pipeline on
    /// session-close `SessionEnd` rows as the `session_complete` tool path,
    /// without threading the registry through every `close_session` caller.
    #[must_use]
    pub fn extensions(
        &self,
    ) -> crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry> {
        self.extensions.clone()
    }

    /// Set the extension registry on an existing provider (builder-style).
    pub fn with_extensions(
        mut self,
        extensions: crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.extensions = extensions;
        self
    }

    /// Test helper: build a provider whose assembler always returns an empty
    /// envelope, with the given injection mode. Used by spec3_tests to verify
    /// mode-gating without needing real retrieval infrastructure.
    #[cfg(test)]
    pub(crate) fn new_for_test_empty_envelope(mode: MemoryInjectionMode) -> Self {
        use crate::memory::assembler::envelope::{EnvelopeMeta, MemoryEnvelope};
        use async_trait::async_trait;

        struct EmptyAssembler;

        #[async_trait]
        impl WorkingMemoryAssembler for EmptyAssembler {
            async fn assemble(
                &self,
                query: &str,
                agent_id: &str,
                _session_id: Option<&str>,
                _budget: crate::memory::assembler::AssemblyBudget,
                _filter: crate::memory::session_search_summary::FactSourceFilter,
            ) -> Result<MemoryEnvelope, crate::error::AlephError> {
                Ok(MemoryEnvelope {
                    schema_version: "1.0".into(),
                    generated_at: 0,
                    query: query.to_string(),
                    agent_id: agent_id.to_string(),
                    session_id: None,
                    slots: vec![],
                    meta: EnvelopeMeta {
                        strategy: "test_empty".into(),
                        candidates_considered: 0,
                        used_fallback: false,
                        fallback_reason: None,
                        llm_rerank_latency_ms: None,
                        total_latency_ms: 0,
                    },
                })
            }
        }

        Self {
            assembler: Arc::new(EmptyAssembler),
            config: MemoryContextConfig::default(),
            injection_mode: mode,
            extensions: crate::sync_primitives::Arc::new(
                crate::memory::extensions::MemoryExtensionRegistry::new(),
            ),
            orientation: None,
            orientation_budget: crate::memory::notes::orientation::types::TokenBudget::default(),
            profile: None,
            curated_snapshots: Arc::new(TokioRwLock::new(HashMap::new())),
            orientation_snapshots: Arc::new(TokioRwLock::new(HashMap::new())),
            curated_stores: Arc::new(DashMap::new()),
            curated_config: CuratedConfig::default(),
            #[cfg(test)]
            curated_root_override: None,
        }
    }

    /// Return a clone of the inner assembler handle.
    ///
    /// Used by Task 8 server wiring to share the same `HybridAssembler`
    /// instance with `MemoryReflector` without constructing a second one.
    #[must_use]
    pub fn assembler(&self) -> Arc<dyn WorkingMemoryAssembler> {
        self.assembler.clone()
    }

    pub(crate) fn assemble_default(
        memory_db: MemoryBackend,
        embedder: Option<Arc<dyn EmbeddingProvider>>,
        provider: Option<Arc<dyn AiProvider>>,
        assembler_config: AssemblerConfig,
        config: MemoryContextConfig,
    ) -> Self {
        let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|e| {
            tracing::warn!("Failed to resolve note memory directory: {e}, using temp dir fallback");
            std::env::temp_dir()
                .join("aleph")
                .join("memory")
                .join("note")
        });
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), memory_db.clone()));
        // Wire the same retrieval-time refinements as the on-demand
        // `memory_search` tool: cross-encoder rerank + recency/reinforcement/MMR
        // scoring. Both builders no-op when their config is disabled/inactive
        // (the default), so the legacy ranking is preserved byte-for-byte until
        // the user opts in via `memory.rerank` / `memory.retrieval_scoring`.
        // No embedder → FTS-only retrieval; the notes are local, so
        // prompt-injected memory (auto-recall / orientation / profile) keeps
        // working without an embedding provider.
        let retrieval = match embedder {
            Some(emb) => NoteFactRetrieval::new(indexer, emb),
            None => NoteFactRetrieval::new_fts_only(indexer),
        };
        let retrieval = Arc::new(
            retrieval
                .with_rerank_config(&assembler_config.rerank)
                .with_scoring_config(&assembler_config.retrieval_scoring)
                .with_expansion_config(&assembler_config.expansion),
        );
        // Read snapshots from wherever `SnapshotWriter` puts them. Use the
        // reader `default_path()` hands back rather than re-deriving the path:
        // the two halves resolve `ALEPH_HOME` (via `utils::paths`), and the old
        // shape called `default_path()` only as an `Option` discriminator and
        // then rebuilt `~/.aleph/data/sessions` from `dirs::home_dir()`. Under
        // `ALEPH_HOME` that wrote here and read there, so the resume chain was
        // not merely ignoring the knob — it was severed end to end.
        let snapshots = Arc::new(SnapshotReader::default_path().unwrap_or_else(|| {
            let fallback = memory_dir.parent().map_or_else(
                || {
                    tracing::warn!("Memory dir has no parent, using memory_dir for sessions");
                    memory_dir.clone()
                },
                |p| p.join("sessions"),
            );
            SnapshotReader::new(fallback)
        }));
        let feedback_floor = FeedbackFloorLoader::new(memory_dir.clone());
        let profile = UserProfileLoader::new(memory_dir);
        let reranker: Arc<dyn crate::memory::assembler::hybrid::LlmReranker> = match provider {
            Some(p) => AiProviderReranker::new(p),
            None => Arc::new(NoopReranker),
        };
        let assembler: Arc<dyn WorkingMemoryAssembler> = Arc::new(HybridAssembler::new(
            retrieval,
            snapshots,
            memory_db,
            profile,
            feedback_floor,
            reranker,
            assembler_config,
        ));
        Self {
            assembler,
            config,
            injection_mode: MemoryInjectionMode::default(),
            extensions: crate::sync_primitives::Arc::new(
                crate::memory::extensions::MemoryExtensionRegistry::new(),
            ),
            orientation: None,
            orientation_budget: crate::memory::notes::orientation::types::TokenBudget::default(),
            profile: None,
            curated_snapshots: Arc::new(TokioRwLock::new(HashMap::new())),
            orientation_snapshots: Arc::new(TokioRwLock::new(HashMap::new())),
            curated_stores: Arc::new(DashMap::new()),
            curated_config: CuratedConfig::default(),
            #[cfg(test)]
            curated_root_override: None,
        }
    }

    /// Set the wiki orientation provider (builder-style).
    pub fn with_orientation(
        mut self,
        orientation: Arc<dyn crate::memory::notes::orientation::NoteOrientation>,
    ) -> Self {
        self.orientation = Some(orientation);
        self
    }

    /// Set the user-profile synthesizer (builder-style).
    pub fn with_profile(
        mut self,
        p: Arc<dyn crate::memory::notes::profile::ProfileSynthesizer>,
    ) -> Self {
        self.profile = Some(p);
        self
    }

    /// Set the token budget spent on one orientation snapshot (builder-style).
    ///
    /// Every constructor above starts from `TokenBudget::default()`; the boot
    /// factory overrides it from `[memory.orientation] max_tokens` so the
    /// configured budget is what `read_snapshot` actually spends.
    #[must_use]
    pub const fn with_orientation_budget(
        mut self,
        budget: crate::memory::notes::orientation::types::TokenBudget,
    ) -> Self {
        self.orientation_budget = budget;
        self
    }

    /// Set the curated hot-memory char-budget config (builder-style).
    #[must_use]
    pub const fn with_curated_config(mut self, cfg: CuratedConfig) -> Self {
        self.curated_config = cfg;
        self
    }

    /// Test-only: redirect the curated root to a tempdir path (so tests don't
    /// touch the real `~/.aleph/agents/<id>/MEMORY.md`).
    #[cfg(test)]
    pub(crate) fn with_curated_root_for_test(mut self, root: std::path::PathBuf) -> Self {
        self.curated_root_override = Some(root);
        self
    }

    /// Resolve the on-disk path for an agent's curated MEMORY.md.
    ///
    /// Real path: `~/.aleph/agents/<agent_id>/MEMORY.md`. Tests can override
    /// the root via `with_curated_root_for_test`. If the home directory
    /// cannot be resolved, falls back to a temp-dir prefix so we never
    /// panic in a degraded environment.
    pub(crate) fn agent_memory_path(&self, agent_id: &str) -> std::path::PathBuf {
        #[cfg(test)]
        {
            if let Some(root) = &self.curated_root_override {
                return root.join(agent_id).join("MEMORY.md");
            }
        }
        let base = crate::discovery::aleph_home_dir().unwrap_or_else(|_| {
            tracing::warn!(
                "aleph_home_dir resolution failed for curated MEMORY.md; using temp-dir fallback"
            );
            std::env::temp_dir().join(".aleph")
        });
        base.join("agents").join(agent_id).join("MEMORY.md")
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryContextProvider;
    use crate::config::types::memory::{CuratedSection, MemoryInjectionMode};
    use crate::memory::curated::CuratedConfig;
    use crate::memory::notes::orientation::types::TokenBudget;

    #[test]
    fn configured_curated_limits_reach_the_provider() {
        // `[memory.curated] memory_char_limit = 9001` — deliberately not the
        // default. Before this wire existed every construction path hardcoded
        // `CuratedConfig::default()`, so the parsed section was inert no matter
        // what the user wrote.
        let section: CuratedSection =
            serde_json::from_str(r#"{"memory_char_limit": 9001}"#).unwrap();
        assert_ne!(
            section.memory_char_limit,
            CuratedConfig::default().memory_char_limit,
            "test is meaningless unless the configured value differs from the default"
        );

        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
                .with_curated_config(section.into());

        assert_eq!(provider.curated_config.memory_char_limit, 9001);
        // Keys the user omitted still land on the section default, not zero.
        assert_eq!(
            provider.curated_config.user_char_limit,
            CuratedConfig::default().user_char_limit
        );
    }

    #[test]
    fn configured_orientation_budget_reaches_the_provider() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
                .with_orientation_budget(TokenBudget { max_tokens: 1234 });

        assert_ne!(1234, TokenBudget::default().max_tokens);
        assert_eq!(provider.orientation_budget.max_tokens, 1234);
    }
}

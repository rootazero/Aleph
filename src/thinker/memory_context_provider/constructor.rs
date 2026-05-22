use super::{MemoryContextConfig, MemoryContextProvider, NoopReranker};
use crate::config::types::memory::{AssemblerConfig, MemoryInjectionMode};
use crate::memory::assembler::hybrid::AiProviderReranker;
use crate::memory::assembler::{HybridAssembler, UserProfileLoader, WorkingMemoryAssembler};
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
    /// LLM reranker.
    pub fn new(memory_db: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self::with_config(memory_db, embedder, MemoryContextConfig::default())
    }

    /// Create with legacy 2-arg + custom config.
    pub fn with_config(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
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
        embedder: Arc<dyn EmbeddingProvider>,
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
            curated_stores: Arc::new(DashMap::new()),
            curated_config: CuratedConfig::default(),
            #[cfg(test)]
            curated_root_override: None,
        }
    }

    /// Set the injection mode on an existing provider (builder-style).
    pub fn with_injection_mode(mut self, mode: MemoryInjectionMode) -> Self {
        self.injection_mode = mode;
        self
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
    pub fn assembler(&self) -> Arc<dyn WorkingMemoryAssembler> {
        self.assembler.clone()
    }

    pub(crate) fn assemble_default(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
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
        let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
        // Snapshots live under ~/.aleph/data/sessions by convention; we pass
        // whatever the `session_resume` defaults produce, falling back to the
        // memory_dir/sessions subdir if the home dir resolution fails.
        let snapshot_dir = SnapshotReader::default_path()
            .map(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| {
                        tracing::warn!(
                            "HOME directory not available for session snapshots, using temp dir"
                        );
                        std::env::temp_dir()
                    })
                    .join(".aleph/data/sessions")
            })
            .unwrap_or_else(|| {
                memory_dir
                    .parent()
                    .map(|p| p.join("sessions"))
                    .unwrap_or_else(|| {
                        tracing::warn!("Memory dir has no parent, using memory_dir for sessions");
                        memory_dir.clone()
                    })
            });
        let snapshots = Arc::new(SnapshotReader::new(snapshot_dir));
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

    /// Set the curated hot-memory char-budget config (builder-style).
    pub fn with_curated_config(mut self, cfg: CuratedConfig) -> Self {
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

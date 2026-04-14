//! Async memory context provider — fetches relevant memories before prompt assembly.

use crate::config::types::memory::{AssemblerConfig, MemoryInjectionMode};
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::assembler::hybrid::{AiProviderReranker, LlmReranker};
use crate::memory::assembler::{
    AssemblyBudget, HybridAssembler, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tracing::warn;

/// Configuration for memory context retrieval.
pub struct MemoryContextConfig {
    /// Maximum number of facts to retrieve.
    pub max_facts: usize,
    /// Minimum cosine similarity threshold.
    pub similarity_threshold: f32,
    /// Maximum characters for the formatted output.
    pub max_output_chars: usize,
}

impl Default for MemoryContextConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
            similarity_threshold: 0.3,
            max_output_chars: 8000, // ~2000 tokens
        }
    }
}

/// No-op reranker used when no [`AiProvider`] is supplied. Always errors →
/// `HybridAssembler` transparently falls back to the deterministic skeleton.
struct NoopReranker;

#[async_trait]
impl LlmReranker for NoopReranker {
    async fn complete(
        &self,
        _prompt: &str,
        _model: Option<&str>,
    ) -> Result<String, crate::error::AlephError> {
        Err(crate::error::AlephError::config(
            "NoopReranker: no AiProvider configured".to_string(),
        ))
    }
}

/// Provides pre-fetched memory context for prompt injection.
pub struct MemoryContextProvider {
    assembler: Arc<dyn WorkingMemoryAssembler>,
    config: MemoryContextConfig,
    /// Controls whether memory is auto-injected (Context/Hybrid) or gated behind tools (Tools).
    injection_mode: MemoryInjectionMode,
    /// Plugin-contributed enhancements to the retrieved envelope.
    /// Default-empty registry means no plugins registered = no-op.
    extensions: std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    /// Optional wiki orientation provider for injecting structural context.
    wiki: Option<Arc<dyn crate::memory::wiki::orientation::WikiOrientation>>,
    /// Token budget for orientation snapshots.
    orientation_budget: crate::memory::wiki::types::TokenBudget,
}

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
            extensions: std::sync::Arc::new(
                crate::memory::extensions::MemoryExtensionRegistry::new(),
            ),
            wiki: None,
            orientation_budget: crate::memory::wiki::types::TokenBudget::default(),
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
        extensions: std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.extensions = extensions;
        self
    }

    /// Test helper: build a provider whose assembler always returns an empty
    /// envelope, with the given injection mode. Used by spec3_tests to verify
    /// mode-gating without needing real retrieval infrastructure.
    #[cfg(test)]
    pub(crate) fn new_for_test_empty_envelope(mode: MemoryInjectionMode) -> Self {
        use crate::memory::assembler::envelope::EnvelopeMeta;
        use async_trait::async_trait;

        struct EmptyAssembler;

        #[async_trait]
        impl WorkingMemoryAssembler for EmptyAssembler {
            async fn assemble(
                &self,
                query: &str,
                agent_id: &str,
                _session_id: Option<&str>,
                _budget: AssemblyBudget,
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
            extensions: std::sync::Arc::new(
                crate::memory::extensions::MemoryExtensionRegistry::new(),
            ),
            wiki: None,
            orientation_budget: crate::memory::wiki::types::TokenBudget::default(),
        }
    }

    /// Return a clone of the inner assembler handle.
    ///
    /// Used by Task 8 server wiring to share the same `HybridAssembler`
    /// instance with `MemoryReflector` without constructing a second one.
    pub fn assembler(&self) -> Arc<dyn WorkingMemoryAssembler> {
        self.assembler.clone()
    }

    fn assemble_default(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        provider: Option<Arc<dyn AiProvider>>,
        assembler_config: AssemblerConfig,
        config: MemoryContextConfig,
    ) -> Self {
        let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
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
                    .unwrap_or_else(|| std::env::temp_dir())
                    .join(".aleph/data/sessions")
            })
            .unwrap_or_else(|| {
                memory_dir
                    .parent()
                    .map(|p| p.join("sessions"))
                    .unwrap_or(memory_dir.clone())
            });
        let snapshots = Arc::new(SnapshotReader::new(snapshot_dir));
        let profile = UserProfileLoader::new(memory_dir);
        let reranker: Arc<dyn LlmReranker> = match provider {
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
            extensions: std::sync::Arc::new(
                crate::memory::extensions::MemoryExtensionRegistry::new(),
            ),
            wiki: None,
            orientation_budget: crate::memory::wiki::types::TokenBudget::default(),
        }
    }

    /// Set the wiki orientation provider (builder-style).
    pub fn with_wiki(
        mut self,
        wiki: Arc<dyn crate::memory::wiki::orientation::WikiOrientation>,
    ) -> Self {
        self.wiki = Some(wiki);
        self
    }

    /// Build a wiki orientation user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when:
    /// - mode is `Tools` (orientation is prompt-only, not tool-gated)
    /// - no wiki provider is registered
    ///
    /// Otherwise returns `Ok(Some(UnifiedMessage::user(xml)))` with the
    /// orientation envelope XML.
    pub async fn build_orientation_user_message(
        &self,
        agent_id: &str,
        mode: crate::config::types::memory::MemoryInjectionMode,
    ) -> Result<Option<crate::providers::message::UnifiedMessage>, crate::error::AlephError> {
        if matches!(
            mode,
            crate::config::types::memory::MemoryInjectionMode::Tools
        ) {
            return Ok(None);
        }
        let Some(w) = &self.wiki else {
            return Ok(None);
        };
        let snap = w.read_snapshot(agent_id, self.orientation_budget).await?;
        let xml = render_orientation_envelope(&snap);
        Ok(Some(crate::providers::message::UnifiedMessage::user(xml)))
    }

    /// Build a memory user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when injection is disabled (`Tools` mode) or when
    /// the assembler returned an empty envelope. Otherwise returns
    /// `Ok(Some(UnifiedMessage::user(render_with(&env, RenderStyle::Xml))))`.
    pub async fn build_memory_user_message(
        &self,
        agent_id: &str,
        query: &str,
    ) -> Result<Option<crate::providers::message::UnifiedMessage>, crate::error::AlephError> {
        use crate::config::types::memory::MemoryInjectionMode;
        use crate::memory::assembler::render::{render_with, RenderStyle};
        use crate::providers::message::UnifiedMessage;

        match self.injection_mode {
            MemoryInjectionMode::Tools => return Ok(None),
            MemoryInjectionMode::Context | MemoryInjectionMode::Hybrid => {}
        }

        let budget = AssemblyBudget {
            total_tokens: (self.config.max_output_chars / 4) as u32,
        };
        let mut envelope = self
            .assembler
            .assemble(query, agent_id, None, budget)
            .await?;

        let ext_ctx = crate::memory::extensions::RetrieveCtx {
            agent_id: agent_id.to_string(),
            namespace: crate::memory::namespace::NamespaceScope::Owner,
            query: query.to_string(),
            session_id: None,
        };
        if let Err(e) = self
            .extensions
            .dispatch_on_retrieve(&ext_ctx, &mut envelope)
            .await
        {
            tracing::warn!("memory extensions on_retrieve pipeline failed: {e}");
        }

        let rendered = render_with(&envelope, RenderStyle::Xml);
        if rendered.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(UnifiedMessage::user(rendered)))
    }
}

fn render_orientation_envelope(s: &crate::memory::wiki::types::OrientationSnapshot) -> String {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        "<WikiOrientation>\n<schema>\n{}\n</schema>\n<index_snapshot>\n{}\n</index_snapshot>\n<recent_log>\n{}\n</recent_log>\n</WikiOrientation>",
        esc(&s.schema_text),
        esc(&s.index_text),
        esc(&s.recent_log_tail)
    )
}

#[cfg(test)]
mod spec3_tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;

    #[tokio::test]
    async fn tools_mode_returns_none_regardless_of_envelope() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools);
        let msg = provider
            .build_memory_user_message("agent-1", "any query")
            .await
            .unwrap();
        assert!(msg.is_none(), "Tools mode must not auto-inject");
    }

    #[tokio::test]
    async fn context_mode_with_empty_envelope_returns_none() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context);
        let msg = provider
            .build_memory_user_message("agent-1", "any query")
            .await
            .unwrap();
        assert!(
            msg.is_none(),
            "empty envelope must short-circuit to None in Context mode"
        );
    }

    #[tokio::test]
    async fn hybrid_mode_with_empty_envelope_returns_none() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Hybrid);
        let msg = provider
            .build_memory_user_message("agent-1", "any query")
            .await
            .unwrap();
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn build_memory_user_message_invokes_on_retrieve_extension() {
        use crate::memory::extensions::traits::MemoryExtension;
        use crate::memory::extensions::{MemoryExtensionRegistry, RetrieveCtx};
        use async_trait::async_trait;
        use std::sync::{Arc, Mutex};

        struct Recorder(Mutex<u32>);
        #[async_trait]
        impl MemoryExtension for Recorder {
            fn name(&self) -> &str {
                "test.recorder"
            }
            async fn on_retrieve(
                &self,
                _ctx: &RetrieveCtx,
                _env: &mut crate::memory::assembler::envelope::MemoryEnvelope,
            ) -> Result<(), crate::error::AlephError> {
                *self.0.lock().unwrap() += 1;
                Ok(())
            }
        }

        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Hybrid);
        let rec = Arc::new(Recorder(Mutex::new(0)));
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(rec.clone());
        let provider = provider.with_extensions(Arc::new(reg));

        // Empty envelope → still invokes on_retrieve before the emptiness check.
        let _ = provider.build_memory_user_message("a1", "q").await.unwrap();
        assert_eq!(*rec.0.lock().unwrap(), 1, "on_retrieve must be dispatched");
    }

    #[tokio::test]
    async fn tools_mode_skips_on_retrieve_dispatch() {
        // In Tools mode we bail before calling assemble, so extensions shouldn't fire.
        use crate::memory::extensions::traits::MemoryExtension;
        use crate::memory::extensions::{MemoryExtensionRegistry, RetrieveCtx};
        use async_trait::async_trait;
        use std::sync::{Arc, Mutex};

        struct Recorder(Mutex<u32>);
        #[async_trait]
        impl MemoryExtension for Recorder {
            fn name(&self) -> &str {
                "test.recorder"
            }
            async fn on_retrieve(
                &self,
                _ctx: &RetrieveCtx,
                _env: &mut crate::memory::assembler::envelope::MemoryEnvelope,
            ) -> Result<(), crate::error::AlephError> {
                *self.0.lock().unwrap() += 1;
                Ok(())
            }
        }

        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools);
        let rec = Arc::new(Recorder(Mutex::new(0)));
        let mut reg = MemoryExtensionRegistry::new();
        reg.register(rec.clone());
        let provider = provider.with_extensions(Arc::new(reg));

        let out = provider.build_memory_user_message("a1", "q").await.unwrap();
        assert!(out.is_none());
        assert_eq!(
            *rec.0.lock().unwrap(),
            0,
            "Tools mode must not call on_retrieve"
        );
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::error::AlephError;
    use crate::memory::wiki::orientation::WikiOrientation;
    use crate::memory::wiki::types::{IndexStats, LogEntry, OrientationSnapshot, TokenBudget};
    use async_trait::async_trait;

    struct FixedOrient;

    #[async_trait]
    impl WikiOrientation for FixedOrient {
        async fn bootstrap(&self, _: &str) -> Result<(), AlephError> {
            Ok(())
        }
        async fn read_snapshot(
            &self,
            _: &str,
            _: TokenBudget,
        ) -> Result<OrientationSnapshot, AlephError> {
            Ok(OrientationSnapshot {
                schema_text: "# Memory Schema\n## Domain\nTest".into(),
                index_text: "# Index\n## learning (1)\n- [[learning/rust]] — fact".into(),
                recent_log_tail: "## [2026-04-14] ingest | touched=3".into(),
            })
        }
        async fn record_ingest(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_query(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_lint(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_session_end(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn rebuild_index(&self, _: &str) -> Result<IndexStats, AlephError> {
            Ok(IndexStats::default())
        }
        async fn rotate_log_if_needed(&self, _: &str) -> Result<bool, AlephError> {
            Ok(false)
        }
        fn invalidate(&self, _: &str, _: &str) {}
    }

    struct NoopOrient;

    #[async_trait]
    impl WikiOrientation for NoopOrient {
        async fn bootstrap(&self, _: &str) -> Result<(), AlephError> {
            Ok(())
        }
        async fn read_snapshot(
            &self,
            _: &str,
            _: TokenBudget,
        ) -> Result<OrientationSnapshot, AlephError> {
            Ok(OrientationSnapshot {
                schema_text: "x".into(),
                index_text: "y".into(),
                recent_log_tail: "z".into(),
            })
        }
        async fn record_ingest(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_query(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_lint(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_session_end(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn rebuild_index(&self, _: &str) -> Result<IndexStats, AlephError> {
            Ok(IndexStats::default())
        }
        async fn rotate_log_if_needed(&self, _: &str) -> Result<bool, AlephError> {
            Ok(false)
        }
        fn invalidate(&self, _: &str, _: &str) {}
    }

    #[tokio::test]
    async fn orientation_message_injected_in_context_mode() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
                .with_wiki(Arc::new(FixedOrient));

        let msg = provider
            .build_orientation_user_message("default", MemoryInjectionMode::Context)
            .await
            .unwrap();
        let m = msg.expect("context mode should inject");
        let text = format!("{m:?}");
        assert!(text.contains("WikiOrientation"));
        assert!(text.contains("# Memory Schema"));
        assert!(text.contains("# Index"));
        assert!(text.contains("touched=3"));
    }

    #[tokio::test]
    async fn orientation_skipped_in_tools_mode() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools)
                .with_wiki(Arc::new(NoopOrient));

        let msg = provider
            .build_orientation_user_message("default", MemoryInjectionMode::Tools)
            .await
            .unwrap();
        assert!(msg.is_none());
    }
}

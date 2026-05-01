//! Async memory context provider — fetches relevant memories before prompt assembly.

use crate::config::types::memory::{AssemblerConfig, MemoryInjectionMode};
use crate::memory::assembler::hybrid::{AiProviderReranker, LlmReranker};
use crate::memory::assembler::{
    AssemblyBudget, HybridAssembler, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::memory::curated::{CuratedConfig, CuratedMemoryStore, CuratedSnapshot};
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use tokio::sync::RwLock as TokioRwLock;

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
    orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    /// Token budget for orientation snapshots.
    orientation_budget: crate::memory::notes::orientation::types::TokenBudget,
    /// Optional user-profile synthesizer for injecting profile context.
    profile: Option<Arc<dyn crate::memory::notes::profile::ProfileSynthesizer>>,
    /// Per-(agent_id, session_key) frozen snapshot. Built on first prompt
    /// build for the session; reused until evicted by compression / SessionEnd.
    curated_snapshots: Arc<TokioRwLock<HashMap<(String, String), Arc<CuratedSnapshot>>>>,
    /// Per-agent CuratedMemoryStore. Loaded lazily on first capture.
    curated_stores: Arc<DashMap<String, Arc<CuratedMemoryStore>>>,
    /// Char-budget config for both MEMORY.md and USER.md rendering.
    curated_config: CuratedConfig,
    /// Test-only override for the curated MEMORY.md root directory.
    /// Real path: `~/.aleph/agents/<agent_id>/MEMORY.md`. Tests redirect
    /// to a tempdir to keep filesystem state isolated.
    #[cfg(test)]
    curated_root_override: Option<std::path::PathBuf>,
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

    fn assemble_default(
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
                        tracing::warn!("HOME directory not available for session snapshots, using temp dir");
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
    fn agent_memory_path(&self, agent_id: &str) -> std::path::PathBuf {
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

    /// Build the cached `<CuratedMemory>` + `<UserProfile>` envelope for
    /// this `(agent_id, session_key)`. The first call captures a frozen
    /// snapshot; subsequent calls in the same session reuse it. Returns
    /// `Ok(None)` only when both blocks are empty (so the layer can skip
    /// emitting an empty user message).
    pub async fn build_curated_message(
        &self,
        agent_id: &str,
        session_key: &str,
    ) -> Result<Option<crate::providers::message::UnifiedMessage>, crate::error::AlephError> {
        let key = (agent_id.to_string(), session_key.to_string());
        if let Some(snap) = self.curated_snapshots.read().await.get(&key) {
            return Ok(self.snapshot_to_message(snap));
        }
        let snap = Arc::new(self.capture_curated(agent_id).await?);
        self.curated_snapshots
            .write()
            .await
            .insert(key, snap.clone());
        Ok(self.snapshot_to_message(&snap))
    }

    /// Get or lazily load the per-agent `CuratedMemoryStore`. Public so the
    /// builtin `remember` tool can resolve the same store instance the
    /// CuratedMemoryLayer renders into the system prompt.
    pub async fn get_or_load_curated_store(
        &self,
        agent_id: &str,
    ) -> Result<Arc<CuratedMemoryStore>, crate::error::AlephError> {
        if let Some(s) = self.curated_stores.get(agent_id) {
            return Ok(s.clone());
        }
        let path = self.agent_memory_path(agent_id);
        let s = Arc::new(
            CuratedMemoryStore::load(path, self.curated_config.memory_char_limit, agent_id).await?,
        );
        self.curated_stores.insert(agent_id.to_string(), s.clone());
        Ok(s)
    }

    /// Load (or reuse) the per-agent CuratedMemoryStore, render the
    /// `<CuratedMemory>` and `<UserProfile>` blocks, and return them as a
    /// frozen `CuratedSnapshot`.
    async fn capture_curated(
        &self,
        agent_id: &str,
    ) -> Result<CuratedSnapshot, crate::error::AlephError> {
        use crate::memory::curated::snapshot::{render_agent_block, render_user_block};

        let store = self.get_or_load_curated_store(agent_id).await?;

        let entries = store.current_entries();
        let agent_block = render_agent_block(
            &entries,
            self.curated_config.memory_char_limit,
            self.curated_config.legacy_warn_threshold,
        );

        let user_block = if let Some(ps) = &self.profile {
            match ps.current(agent_id).await? {
                Some(p) => {
                    let body = strip_frontmatter(&p.raw);
                    let block = render_user_block(
                        body,
                        self.curated_config.user_char_limit,
                        self.curated_config.legacy_warn_threshold,
                    );
                    if block.is_empty() {
                        None
                    } else {
                        Some(block)
                    }
                }
                None => None,
            }
        } else {
            None
        };

        Ok(CuratedSnapshot {
            agent_id: agent_id.to_string(),
            agent_md_block: agent_block,
            user_md_block: user_block,
            captured_at: std::time::SystemTime::now(),
        })
    }

    /// Combine the two block strings into a single user-message. Returns
    /// `None` when both blocks are empty (no need to emit an empty turn).
    fn snapshot_to_message(
        &self,
        snap: &CuratedSnapshot,
    ) -> Option<crate::providers::message::UnifiedMessage> {
        let mut combined = String::new();
        if !snap.agent_md_block.is_empty() {
            combined.push_str(&snap.agent_md_block);
        }
        if let Some(ub) = &snap.user_md_block {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(ub);
        }
        if combined.is_empty() {
            return None;
        }
        Some(crate::providers::message::UnifiedMessage::user(combined))
    }

    /// Evict every snapshot whose `session_key` matches. Called on
    /// compression-complete and SessionEnd so the next prompt build picks
    /// up disk mutations.
    pub async fn invalidate_curated(&self, session_key: &str) {
        self.curated_snapshots
            .write()
            .await
            .retain(|(_, sk), _| sk != session_key);
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
        let Some(w) = &self.orientation else {
            return Ok(None);
        };
        let snap = w.read_snapshot(agent_id, self.orientation_budget).await?;
        let xml = render_orientation_envelope(&snap);
        Ok(Some(crate::providers::message::UnifiedMessage::user(xml)))
    }

    /// Build a user-profile user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when:
    /// - mode is `Tools`
    /// - no profile synthesizer is registered
    /// - the synthesizer returns `None` (USER.md absent)
    ///
    /// Otherwise returns `Ok(Some(UnifiedMessage::user(xml)))` with the
    /// profile envelope XML (body truncated to 2 KB).
    pub async fn build_profile_user_message(
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
        let Some(ps) = &self.profile else {
            return Ok(None);
        };
        let Some(profile) = ps.current(agent_id).await? else {
            return Ok(None);
        };
        let body = strip_frontmatter(&profile.raw);
        let body: String = body.chars().take(2048).collect();
        let xml = format!(
            "<UserProfile>\n<revision>{}</revision>\n<confidence>{}</confidence>\n<body>\n{}\n</body>\n</UserProfile>",
            profile.revision,
            xml_escape(&profile.confidence),
            xml_escape(&body)
        );
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

/// Escape `&`, `<`, `>` for XML embedding.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Strip YAML frontmatter (`---\n…\n---\n`) from the start of `raw`.
/// If no frontmatter is present, returns `raw` unchanged.
fn strip_frontmatter(raw: &str) -> &str {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return &rest[end + 5..]; // skip "\n---\n"
        }
    }
    raw
}

fn render_orientation_envelope(
    s: &crate::memory::notes::orientation::types::OrientationSnapshot,
) -> String {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        "<NoteOrientation>\n<schema>\n{}\n</schema>\n<index_snapshot>\n{}\n</index_snapshot>\n<recent_log>\n{}\n</recent_log>\n</NoteOrientation>",
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
    use crate::memory::notes::orientation::types::{
        IndexStats, LogEntry, OrientationSnapshot, TokenBudget,
    };
    use crate::memory::notes::orientation::NoteOrientation;
    use async_trait::async_trait;

    struct FixedOrient;

    #[async_trait]
    impl NoteOrientation for FixedOrient {
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
    impl NoteOrientation for NoopOrient {
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
                .with_orientation(Arc::new(FixedOrient));

        let msg = provider
            .build_orientation_user_message("default", MemoryInjectionMode::Context)
            .await
            .unwrap();
        let m = msg.expect("context mode should inject");
        let text = format!("{m:?}");
        assert!(text.contains("NoteOrientation"));
        assert!(text.contains("# Memory Schema"));
        assert!(text.contains("# Index"));
        assert!(text.contains("touched=3"));
    }

    #[tokio::test]
    async fn orientation_skipped_in_tools_mode() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools)
                .with_orientation(Arc::new(NoopOrient));

        let msg = provider
            .build_orientation_user_message("default", MemoryInjectionMode::Tools)
            .await
            .unwrap();
        assert!(msg.is_none());
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::error::AlephError;
    use crate::memory::notes::profile::types::{ProfileSection, UserProfile};
    use crate::memory::notes::profile::ProfileSynthesizer;
    use async_trait::async_trait;
    use std::collections::BTreeMap;

    struct FixedProfile;

    #[async_trait]
    impl ProfileSynthesizer for FixedProfile {
        async fn bootstrap(&self, _agent_id: &str) -> Result<UserProfile, AlephError> {
            unimplemented!()
        }

        async fn current(&self, _agent_id: &str) -> Result<Option<UserProfile>, AlephError> {
            let mut sections = BTreeMap::new();
            sections.insert(
                ProfileSection::Identity.heading().to_string(),
                vec!["Software engineer".to_string()],
            );
            Ok(Some(UserProfile {
                schema_version: 1,
                updated: "2026-04-17".to_string(),
                revision: 3,
                last_session: "ses_001".to_string(),
                confidence: "medium".to_string(),
                raw: "# User Profile\n## Identity\n- Software engineer\n".to_string(),
                sections,
                content_hash: "abc".to_string(),
            }))
        }

        async fn update(
            &self,
            _agent_id: &str,
            _signal: crate::memory::notes::profile::types::SessionSignal,
        ) -> Result<crate::memory::notes::profile::types::UpdateOutcome, AlephError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn profile_message_injected_in_context_mode() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
                .with_profile(Arc::new(FixedProfile));

        let msg = provider
            .build_profile_user_message("default", MemoryInjectionMode::Context)
            .await
            .unwrap();
        let m = msg.expect("context mode should inject profile");
        let text = format!("{m:?}");
        assert!(text.contains("<UserProfile>"));
        assert!(text.contains("</UserProfile>"));
    }

    #[tokio::test]
    async fn profile_skipped_in_tools_mode() {
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools)
                .with_profile(Arc::new(FixedProfile));

        let msg = provider
            .build_profile_user_message("default", MemoryInjectionMode::Tools)
            .await
            .unwrap();
        assert!(msg.is_none());
    }
}

#[cfg(test)]
mod curated_snapshot_tests {
    use super::*;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::memory::curated::CuratedConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn first_call_captures_snapshot_subsequent_calls_hit_cache() {
        let dir = tempdir().unwrap();
        let provider =
            MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
                .with_curated_config(CuratedConfig {
                    memory_char_limit: 100,
                    user_char_limit: 100,
                    legacy_warn_threshold: 0.95,
                })
                .with_curated_root_for_test(dir.path().to_path_buf());

        // Pre-seed MEMORY.md (with trailing § so it's classified as curated,
        // not legacy — see Task 7's serialize-with-trailing-delimiter fix).
        let agent_dir = dir.path().join("agent-x");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("MEMORY.md"), "fact one\n§\nfact two\n§\n").unwrap();

        let m1 = provider
            .build_curated_message("agent-x", "ses-1")
            .await
            .unwrap();
        assert!(m1.is_some(), "first call must produce a message");
        let txt1 = format!("{:?}", m1);
        assert!(txt1.contains("fact one"), "rendered block must include entries");
        assert!(txt1.contains("CuratedMemory"), "must be wrapped in <CuratedMemory>");

        // Mutate disk; same session_key must NOT reflect the change.
        std::fs::write(
            agent_dir.join("MEMORY.md"),
            "fact one\n§\nfact two\n§\nfact three\n§\n",
        )
        .unwrap();
        let m2 = provider
            .build_curated_message("agent-x", "ses-1")
            .await
            .unwrap();
        let txt2 = format!("{:?}", m2);
        assert!(
            !txt2.contains("fact three"),
            "snapshot must be frozen for ses-1 — second call should hit cache"
        );

        // Eviction → next call rebuilds and picks up the disk change. Note
        // that the per-agent store is also cached, so we must evict it via
        // the public surface — but `invalidate_curated` only evicts the
        // snapshot keyed by session. The store stays cached (its
        // `current_entries()` is read fresh each capture, but it was
        // initialized from the OLD body). For the test to observe the
        // disk-side mutation, we also clear the per-agent store map.
        provider.invalidate_curated("ses-1").await;
        provider.curated_stores.remove("agent-x");

        let m3 = provider
            .build_curated_message("agent-x", "ses-1")
            .await
            .unwrap();
        let txt3 = format!("{:?}", m3);
        assert!(
            txt3.contains("fact three"),
            "after invalidate + store reset, must pick up disk change"
        );
    }
}

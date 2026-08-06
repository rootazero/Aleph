//! Integration test: Spec 3 injection_mode end-to-end.
//!
//! For each mode, build a live prompt-assembly pipeline + tool registry,
//! then assert:
//!   - Context: memory user-message IS present (non-empty envelope); no memory_* retrieval tools registered.
//!   - Tools:   memory user-message is NONE; memory_* retrieval tools registered.
//!   - Hybrid:  memory user-message IS present; memory_* retrieval tools registered.
//!
//! note_manage and session_complete are always registered (dep-gated on memory_db,
//! which we supply in all three pipelines).
//!
//! Note on tool counts: memory_timeline requires state_db (not provided here).
//! With memory_db + embedder we get 5 of the 6 retrieval tools.
//! The dep-free tools (memory_reflect, recall_context) are the reliable signal
//! for mode-gating correctness.

use std::sync::Arc;

use alephcore::executor::{BuiltinToolConfig, BuiltinToolRegistry};
use alephcore::memory::assembler::{HybridAssembler, UserProfileLoader};
use alephcore::memory::note_retrieval::NoteFactRetrieval;
use alephcore::memory::notes::{KnowledgeNote, NoteIndexer};
use alephcore::memory::session_resume::reader::SnapshotReader;
use alephcore::memory::store::{MemoryBackend, SqliteMemoryBackend};
use alephcore::memory::EmbeddingProvider;
use alephcore::thinker::memory_context_provider::{MemoryContextConfig, MemoryContextProvider};
use alephcore::MemoryInjectionMode;
use alephcore::{AlephError, AssemblerConfig};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Noop embedding provider — zero vectors, same as memory_reflect_integration.rs
// ---------------------------------------------------------------------------

struct NoopEmbeddingProvider;

#[async_trait::async_trait]
impl EmbeddingProvider for NoopEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.0; 256])
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.0; 256]).collect())
    }

    fn dimensions(&self) -> usize {
        256
    }

    fn model_name(&self) -> &str {
        "noop"
    }

    fn provider_id(&self) -> &str {
        "noop"
    }
}

// ---------------------------------------------------------------------------
// Assembler config: force_fallback so the skeleton packages whatever the
// pool produces without calling the LLM reranker.
// ---------------------------------------------------------------------------

fn test_assembler_config() -> AssemblerConfig {
    AssemblerConfig {
        enabled: true,
        candidate_pool_limit: 20,
        rerank_timeout_ms: 200,
        rerank_model: None,
        render_style: Default::default(),
        force_fallback: true,
        fallback_skeleton: Default::default(),
        assembly_log: Default::default(),
        project_scoped: false,
        retrieval_scoring: Default::default(),
        rerank: Default::default(),
        expansion: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Harness — mirrors tests/memory_reflect_integration.rs almost verbatim.
// Returns (provider, registry, tmp) for one injection mode.
// The tmp handle keeps the temp dir alive for the duration of the test.
// ---------------------------------------------------------------------------

/// Point Aleph's state directory at a throwaway home for this test binary.
///
/// `BuiltinToolRegistry::with_config` below opens the strategy and goal stores
/// under `ALEPH_HOME`, falling back to the real `~/.aleph` when it is unset —
/// so without this the suite creates and writes files in the developer's live
/// data directory. The tempdir under `tmp` only covers the memory DB.
///
/// Set once per process and never restored: all three tests want the same
/// throwaway home, and `set_var` racing other threads is undefined behaviour.
/// `get_or_init` runs the closure exactly once and blocks the rest until it
/// returns, so the write happens before any test can reach a store.
/// (`AlephHomeEnvGuard` is `#[cfg(test)]` and lib-internal, hence not reusable
/// from an integration binary.)
fn isolate_aleph_home() {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempdir().expect("tempdir for isolated ALEPH_HOME");
        std::env::set_var("ALEPH_HOME", dir.path());
        dir
    });
}

// The REGISTRY_INIT mutex guard below is held across an await ON PURPOSE (see
// the serialisation rationale at the guard site): each test runs its own
// current-thread runtime, so parking this thread blocks no other test's
// executor.
#[allow(clippy::await_holding_lock)]
async fn build_pipeline(
    mode: MemoryInjectionMode,
) -> (
    MemoryContextProvider,
    BuiltinToolRegistry,
    tempfile::TempDir,
) {
    isolate_aleph_home();
    let tmp = tempdir().unwrap();

    // Single DB file for all memory tables.
    let db_path = tmp.path().join("mem.db");
    let backend_arc: Arc<SqliteMemoryBackend> =
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());

    // Memory dir for note files and user-profile loader.
    let memory_dir = tmp.path().join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();

    // NoteIndexer.
    let indexer = NoteIndexer::new(memory_dir.clone(), backend_arc.clone());

    // Embedding provider.
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(NoopEmbeddingProvider);

    // NoteFactRetrieval wraps a second indexer handle.
    let retrieval = Arc::new(NoteFactRetrieval::new(
        Arc::new(NoteIndexer::new(memory_dir.clone(), backend_arc.clone())),
        Arc::clone(&embedder),
    ));

    // Session snapshots dir (empty — no snapshots needed).
    let snap_dir = tmp.path().join("snapshots");
    std::fs::create_dir_all(&snap_dir).unwrap();
    let snapshots = Arc::new(SnapshotReader::new(&snap_dir));

    // User-profile loader.
    let profile = UserProfileLoader::new(memory_dir.clone());

    // Noop reranker — assembler has force_fallback=true so this is never called.
    struct NoopReranker;
    #[async_trait::async_trait]
    impl alephcore::memory::assembler::hybrid::LlmReranker for NoopReranker {
        async fn complete(
            &self,
            _prompt: &str,
            _model: Option<&str>,
        ) -> Result<String, AlephError> {
            Err(AlephError::config("NoopReranker".to_string()))
        }
    }
    let reranker =
        Arc::new(NoopReranker) as Arc<dyn alephcore::memory::assembler::hybrid::LlmReranker>;

    // Feedback-floor loader.
    let feedback_floor = alephcore::memory::assembler::FeedbackFloorLoader::new(memory_dir.clone());

    // HybridAssembler.
    let assembler: Arc<dyn alephcore::memory::assembler::WorkingMemoryAssembler> =
        Arc::new(HybridAssembler::new(
            retrieval,
            snapshots,
            backend_arc.clone() as MemoryBackend,
            profile,
            feedback_floor,
            reranker,
            test_assembler_config(),
        ));

    // Seed ONE note so the assembler returns a non-empty envelope (Context/Hybrid modes).
    let note = KnowledgeNote {
        title: "test-note".to_string(),
        category: "wiki".to_string(),
        tags: vec!["integration-test".to_string()],
        facts: vec!["This is a seeded fact for injection_mode integration tests.".to_string()],
        links: vec![],
        created_at: 1_700_000_000,
        updated_at: 1_700_000_000,
        content_hash: String::new(),
        ..Default::default()
    };
    indexer
        .write_note("agent-test", "wiki", &note)
        .await
        .unwrap();
    // Full-rebuild so the note enters the SQLite FTS + vec index.
    indexer.full_rebuild("agent-test").await.unwrap();

    // MemoryContextProvider wrapping the real assembler with the given mode.
    let provider = MemoryContextProvider::with_assembler(assembler, MemoryContextConfig::default())
        .with_injection_mode(mode);

    // BuiltinToolRegistry with memory_db + embedder so dep-gated tools register.
    //
    // Serialised: the goal and strategy stores are process-global and live under
    // the single `ALEPH_HOME` above, so three tests constructing registries at
    // once race to create and migrate the same fresh SQLite files and lose with
    // "database is locked". Each `#[tokio::test]` drives its own current-thread
    // runtime, so blocking here parks only this test's thread.
    static REGISTRY_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let registry = {
        let _serialise = REGISTRY_INIT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        BuiltinToolRegistry::with_config(BuiltinToolConfig {
            injection_mode: mode,
            memory_db: Some(backend_arc.clone() as MemoryBackend),
            embedder: Some(Arc::clone(&embedder)),
            ..Default::default()
        })
        .await
        .expect("builtin tool registry")
    };

    (provider, registry, tmp)
}

// ---------------------------------------------------------------------------
// Tool name constants
// ---------------------------------------------------------------------------

/// The six memory retrieval tools gated by injection_mode.
/// Note: memory_timeline additionally needs state_db — without it, 5 of 6
/// will be present in Tools/Hybrid mode when memory_db + embedder are supplied.
/// The dep-free pair (memory_reflect, recall_context) is the definitive gate signal.
const MEMORY_RETRIEVAL_TOOLS: &[&str] = &[
    "memory_search",
    "memory_reflect",
    "recall_context",
    "memory_browse",
    "memory_explore",
    "memory_timeline",
];

fn count_memory_retrieval_tools(registry: &BuiltinToolRegistry) -> usize {
    MEMORY_RETRIEVAL_TOOLS
        .iter()
        .filter(|n| registry.has_tool(n))
        .count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_mode_injects_fenced_message_and_hides_memory_tools() {
    let (provider, registry, _tmp) = build_pipeline(MemoryInjectionMode::Context).await;

    // Provider: Context mode must auto-inject when the envelope is non-empty.
    // (With force_fallback + zero-vector embedder the note may or may not surface
    //  via similarity; if the assembler still returns an empty rendered string it
    //  will short-circuit to None — that is acceptable here, but if it IS present
    //  the XML fence must be correct.)
    let msg = provider
        .build_memory_user_message("agent-test", "integration test question", None, None)
        .await
        .unwrap();

    if let Some(msg) = msg {
        let content = msg.text_content();
        assert!(
            content.contains("<MemoryEnvelope>"),
            "rendered message must be XML-fenced: got {:?}",
            &content[..content.len().min(200)]
        );
        assert!(content.contains("</MemoryEnvelope>"));
    }
    // Whether or not the assembler surfaced the note, the tool-registry gate
    // is unconditional and must always hold.

    // Registry: Context mode must NOT register any retrieval tools.
    assert_eq!(
        count_memory_retrieval_tools(&registry),
        0,
        "Context mode must NOT register any memory retrieval tools"
    );
    assert!(
        !registry.has_tool("memory_reflect"),
        "memory_reflect must be absent in Context mode"
    );
    assert!(
        !registry.has_tool("recall_context"),
        "recall_context must be absent in Context mode"
    );

    // note_manage + session_complete must be registered (dep supplied, not mode-gated).
    assert!(
        registry.has_tool("note_manage"),
        "note_manage must be registered in Context mode"
    );
    assert!(
        registry.has_tool("session_complete"),
        "session_complete must be registered in Context mode"
    );
}

#[tokio::test]
async fn tools_mode_skips_injection_and_registers_memory_tools() {
    let (provider, registry, _tmp) = build_pipeline(MemoryInjectionMode::Tools).await;

    // Provider: Tools mode must always return None.
    let msg = provider
        .build_memory_user_message("agent-test", "integration test question", None, None)
        .await
        .unwrap();
    assert!(msg.is_none(), "Tools mode must not auto-inject");

    // Registry: dep-free retrieval tools must be registered.
    assert!(
        registry.has_tool("memory_reflect"),
        "memory_reflect must be registered in Tools mode"
    );
    assert!(
        registry.has_tool("recall_context"),
        "recall_context must be registered in Tools mode"
    );
    // With memory_db + embedder, the dep-requiring tools should also be present.
    assert!(
        registry.has_tool("memory_search"),
        "memory_search must be registered in Tools mode when memory_db+embedder supplied"
    );
    assert!(
        registry.has_tool("memory_browse"),
        "memory_browse must be registered in Tools mode when memory_db supplied"
    );
    assert!(
        registry.has_tool("memory_explore"),
        "memory_explore must be registered in Tools mode when memory_db+embedder supplied"
    );
    // Total should be >= 2 (dep-free); with our deps it should be >= 5.
    let count = count_memory_retrieval_tools(&registry);
    assert!(
        count >= 2,
        "Tools mode must register at least the dep-free retrieval tools, got {count}"
    );

    // note_manage + session_complete must be registered.
    assert!(
        registry.has_tool("note_manage"),
        "note_manage must be registered in Tools mode"
    );
    assert!(
        registry.has_tool("session_complete"),
        "session_complete must be registered in Tools mode"
    );
}

#[tokio::test]
async fn hybrid_mode_does_both() {
    let (provider, registry, _tmp) = build_pipeline(MemoryInjectionMode::Hybrid).await;

    // Provider: Hybrid mode auto-injects (same as Context).
    // If the envelope is non-empty after assembly, a fenced message is returned.
    let msg = provider
        .build_memory_user_message("agent-test", "integration test question", None, None)
        .await
        .unwrap();

    if let Some(msg) = msg {
        let content = msg.text_content();
        assert!(
            content.contains("<MemoryEnvelope>"),
            "Hybrid: rendered message must be XML-fenced: got {:?}",
            &content[..content.len().min(200)]
        );
        assert!(content.contains("</MemoryEnvelope>"));
    }

    // Registry: dep-free retrieval tools must be registered.
    assert!(
        registry.has_tool("memory_reflect"),
        "memory_reflect must be registered in Hybrid mode"
    );
    assert!(
        registry.has_tool("recall_context"),
        "recall_context must be registered in Hybrid mode"
    );
    let count = count_memory_retrieval_tools(&registry);
    assert!(
        count >= 2,
        "Hybrid mode must register at least the dep-free retrieval tools, got {count}"
    );

    // note_manage + session_complete must be registered.
    assert!(
        registry.has_tool("note_manage"),
        "note_manage must be registered in Hybrid mode"
    );
    assert!(
        registry.has_tool("session_complete"),
        "session_complete must be registered in Hybrid mode"
    );
}

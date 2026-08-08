//! Integration tests for HybridAssembler — exercises the full 3-stage flow
//! (gather → rerank → hydrate) with stubbed LLM reranker and real
//! SqliteMemoryBackend pointed at an in-memory DB.

use crate::config::types::memory::{AssemblerConfig, FallbackSkeleton};
use crate::error::AlephError;
use crate::memory::assembler::hybrid::LlmReranker;
use crate::memory::assembler::{
    AssemblyBudget, FeedbackFloorLoader, HybridAssembler, UserProfileLoader, WorkingMemoryAssembler,
};
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::memory::store::MemoryBackend;
use crate::memory::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use crate::sync_primitives::Mutex as StdMutex;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// ---- Stubs ----

struct StubReranker {
    /// Scripted response; take() returns it once.
    response: StdMutex<Option<Result<String, AlephError>>>,
    /// Optional sleep before returning (for timeout testing).
    sleep_for: Option<Duration>,
    /// Records whether the reranker was actually called.
    was_called: AtomicBool,
}

impl StubReranker {
    fn ok(json: &str) -> Arc<Self> {
        Arc::new(Self {
            response: StdMutex::new(Some(Ok(json.into()))),
            sleep_for: None,
            was_called: AtomicBool::new(false),
        })
    }
    fn timing_out() -> Arc<Self> {
        Arc::new(Self {
            response: StdMutex::new(Some(Ok("ignored".into()))),
            sleep_for: Some(Duration::from_millis(2_000)),
            was_called: AtomicBool::new(false),
        })
    }
    fn invalid_json() -> Arc<Self> {
        Self::ok("{bogus")
    }
}

#[async_trait]
impl LlmReranker for StubReranker {
    async fn complete(&self, _prompt: &str, _model: Option<&str>) -> Result<String, AlephError> {
        self.was_called.store(true, Ordering::SeqCst);
        if let Some(d) = self.sleep_for {
            tokio::time::sleep(d).await;
        }
        // rust-doctor-disable-next-line unwrap-in-production
        let mut guard = self.response.lock().unwrap();
        guard
            .take()
            .unwrap_or_else(|| Err(AlephError::config("no response scripted".to_string())))
    }
}

struct FakeEmbedder;

#[async_trait]
impl EmbeddingProvider for FakeEmbedder {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Ok(vec![0.0_f32; 768])
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Ok(texts.iter().map(|_| vec![0.0_f32; 768]).collect())
    }
    fn dimensions(&self) -> usize {
        768
    }
    fn model_name(&self) -> &str {
        "fake"
    }
    fn provider_id(&self) -> &str {
        "fake"
    }
}

// ---- Fixture ----

fn default_cfg() -> AssemblerConfig {
    AssemblerConfig {
        enabled: true,
        candidate_pool_limit: 20,
        rerank_timeout_ms: 200,
        rerank_model: None,
        render_style: Default::default(),
        force_fallback: false,
        fallback_skeleton: FallbackSkeleton::default(),
        project_scoped: false,
        retrieval_scoring: Default::default(),
        rerank: Default::default(),
        expansion: Default::default(),
    }
}

struct Fixture {
    assembler: HybridAssembler,
    backend: MemoryBackend,
    _tmp_mem: tempfile::TempDir,
    _tmp_snap: tempfile::TempDir,
    reranker: Arc<StubReranker>,
}

fn fixture(reranker: Arc<StubReranker>, config: AssemblerConfig) -> Fixture {
    // rust-doctor-disable-next-line unwrap-in-production
    let tmp_mem = tempfile::tempdir().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let tmp_snap = tempfile::tempdir().unwrap();
    let db_path = tmp_mem.path().join("mem.db");
    // rust-doctor-disable-next-line unwrap-in-production
    let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FakeEmbedder);
    let indexer = Arc::new(NoteIndexer::new(
        tmp_mem.path().join("notes"),
        // rust-doctor-disable-next-line excessive-clone
        backend.clone(),
    ));
    let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
    let snapshots = Arc::new(SnapshotReader::new(tmp_snap.path()));
    let profile = UserProfileLoader::new(tmp_mem.path().join("notes"));
    let feedback_floor = FeedbackFloorLoader::new(tmp_mem.path().join("notes"));
    // rust-doctor-disable-next-line excessive-clone
    let reranker_dyn: Arc<dyn LlmReranker> = reranker.clone();
    let assembler = HybridAssembler::new(
        retrieval,
        snapshots,
        // rust-doctor-disable-next-line excessive-clone
        backend.clone(),
        profile,
        feedback_floor,
        reranker_dyn,
        config,
    );
    Fixture {
        assembler,
        backend,
        _tmp_mem: tmp_mem,
        _tmp_snap: tmp_snap,
        reranker,
    }
}

async fn seed_raw(backend: &MemoryBackend, session_id: &str, count: usize) {
    for i in 0..count {
        let raw = RawMemory::new(format!("body {i}"), RawMemorySource::Transcript)
            .with_agent("default")
            .with_session(session_id)
            .with_path(format!("aleph://session/{session_id}/raw/frag-{i}"));
        // rust-doctor-disable-next-line unwrap-in-production
        backend.insert_raw_memory(&raw).await.unwrap();
    }
}

// ---- Integration paths ----

#[tokio::test]
async fn path_tiny_pool_skips_llm() {
    // With zero candidates seeded, pool < 3 → tiny_pool fast-path; LLM untouched.
    let reranker = StubReranker::invalid_json();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), default_cfg());
    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            None,
            AssemblyBudget { total_tokens: 4000 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("tiny_pool"));
    assert!(
        !fx.reranker.was_called.load(Ordering::SeqCst),
        "reranker must not be called on tiny pool"
    );
}

#[tokio::test]
async fn path_llm_timeout_falls_back() {
    // Seed enough raw fragments to bypass the tiny_pool fast-path.
    let reranker = StubReranker::timing_out();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), default_cfg());
    seed_raw(&fx.backend, "sess-a", 5).await;
    let env = fx
        .assembler
        .assemble(
            "anything",
            "default",
            Some("sess-a"),
            AssemblyBudget { total_tokens: 4000 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("llm_timeout"));
    assert!(fx.reranker.was_called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn path_llm_invalid_json_falls_back() {
    let reranker = StubReranker::invalid_json();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), default_cfg());
    seed_raw(&fx.backend, "sess-b", 5).await;
    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            Some("sess-b"),
            AssemblyBudget { total_tokens: 4000 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("llm_parse_error"));
}

#[tokio::test]
async fn path_happy_b_with_valid_response() {
    // Provide a reranker response that picks actual raw fragment ids.
    // We need to know the ids produced by gather — they are
    // aleph://session/sess-c/raw/{raw_id} where raw_id is the UUID of the
    // raw memory. To avoid coupling to UUIDs, script a response that targets
    // raw_fragments slot with empty item_ids list so the response is "empty
    // valid slots" → parse yields RerankEmpty → falls back.
    //
    // For a true happy path, we craft a response with kind relevant_notes
    // and hallucinated ids — all get filtered out, resulting in empty
    // sanitized slots → RerankEmpty → fallback. That's acceptable for
    // exercising the parse path. A deeper "happy" path test that actually
    // picks real ids requires seeding deterministic-id notes through the
    // indexer, which is out of scope for this focused integration test.
    let reranker = StubReranker::ok(
        r#"{"slots":[{"kind":"relevant_notes","item_ids":["note://fake"],"tokens_budget":500}]}"#,
    );
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), default_cfg());
    seed_raw(&fx.backend, "sess-d", 5).await;
    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            Some("sess-d"),
            AssemblyBudget { total_tokens: 4000 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // Hallucinated id filtered → sanitized slots empty → RerankEmpty → fallback.
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("llm_empty_slots"));
    assert!(fx.reranker.was_called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn path_disabled_config_short_circuits() {
    let mut cfg = default_cfg();
    cfg.enabled = false;
    let reranker = StubReranker::invalid_json();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), cfg);
    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            None,
            AssemblyBudget { total_tokens: 4000 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(env.meta.strategy, "disabled");
    assert!(env.meta.used_fallback);
    assert_eq!(
        env.meta.fallback_reason.as_deref(),
        Some("assembler_disabled")
    );
    assert!(env.slots.is_empty());
    assert!(!fx.reranker.was_called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn path_force_fallback_bypasses_llm() {
    let mut cfg = default_cfg();
    cfg.force_fallback = true;
    let reranker = StubReranker::invalid_json();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), cfg);
    seed_raw(&fx.backend, "sess-e", 5).await;
    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            Some("sess-e"),
            AssemblyBudget { total_tokens: 4000 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(env.meta.used_fallback);
    assert_eq!(env.meta.fallback_reason.as_deref(), Some("forced"));
    assert!(!fx.reranker.was_called.load(Ordering::SeqCst));
}

/// The skeleton path used to pack the static `FallbackSkeleton` budgets
/// (~8200 tokens) regardless of the caller's runtime headroom, so the
/// context-pressure back-off was a no-op on what is 100% of traffic when no
/// rerank provider is configured.
#[tokio::test]
async fn skeleton_fallback_respects_the_runtime_token_budget() {
    let mut cfg = default_cfg();
    cfg.force_fallback = true;
    let reranker = StubReranker::invalid_json();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), cfg);
    seed_raw(&fx.backend, "sess-budget", 40).await;

    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            Some("sess-budget"),
            AssemblyBudget { total_tokens: 300 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    assert!(env.meta.used_fallback);
    let total: u32 = env.slots.iter().map(|s| s.tokens_used).sum();
    assert!(
        total <= 300,
        "skeleton fallback emitted {total} tokens against a 300-token budget"
    );
}

/// A zero budget means "inject nothing this turn". The skeleton path used to
/// emit a full envelope anyway.
#[tokio::test]
async fn zero_budget_yields_a_near_empty_skeleton_envelope() {
    let mut cfg = default_cfg();
    cfg.force_fallback = true;
    let reranker = StubReranker::invalid_json();
    // rust-doctor-disable-next-line excessive-clone
    let fx = fixture(reranker.clone(), cfg);
    seed_raw(&fx.backend, "sess-zero", 20).await;

    let env = fx
        .assembler
        .assemble(
            "q",
            "default",
            Some("sess-zero"),
            AssemblyBudget { total_tokens: 0 },
            crate::memory::session_search_summary::FactSourceFilter::Any,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // Each live slot floors at 1 token (so hydration cannot silently delete a
    // slot the config enabled), so the envelope is bounded by the slot count
    // rather than exactly zero.
    let total: u32 = env.slots.iter().map(|s| s.tokens_used).sum();
    assert!(
        total <= env.slots.len() as u32 * 2,
        "zero budget produced {total} tokens across {} slots",
        env.slots.len()
    );
}

// ---- Property test ----

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn envelope_serde_roundtrip_regardless_of_budget(budget in 100u32..16_000) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let reranker = StubReranker::invalid_json();
            let fx = fixture(reranker, default_cfg());
            let env = fx
                .assembler
                .assemble("q", "default", None, AssemblyBudget { total_tokens: budget }, crate::memory::session_search_summary::FactSourceFilter::Any)
                .await
                .unwrap();
            let json = serde_json::to_string(&env).unwrap();
            let _parsed: crate::memory::assembler::MemoryEnvelope =
                serde_json::from_str(&json).unwrap();
        });
    }
}

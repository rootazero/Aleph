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
use crate::memory::notes::store::NoteStore;
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
    select_notes: bool,
}

impl StubReranker {
    fn ok(json: &str) -> Arc<Self> {
        Arc::new(Self {
            response: StdMutex::new(Some(Ok(json.into()))),
            sleep_for: None,
            was_called: AtomicBool::new(false),
            select_notes: false,
        })
    }
    fn timing_out() -> Arc<Self> {
        Arc::new(Self {
            response: StdMutex::new(Some(Ok("ignored".into()))),
            sleep_for: Some(Duration::from_millis(2_000)),
            was_called: AtomicBool::new(false),
            select_notes: false,
        })
    }
    fn invalid_json() -> Arc<Self> {
        Self::ok("{bogus")
    }
}

#[async_trait]
impl LlmReranker for StubReranker {
    async fn complete(&self, prompt: &str, _model: Option<&str>) -> Result<String, AlephError> {
        self.was_called.store(true, Ordering::SeqCst);
        if self.select_notes {
            let ids: Vec<_> = prompt
                .lines()
                .filter_map(|line| {
                    let line = line.trim().strip_prefix('[')?;
                    Some(line.split_once(']')?.0)
                })
                .collect();
            return Ok(serde_json::json!({"slots": [{"kind": "relevant_notes", "item_ids": ids, "tokens_budget": 2000}]}).to_string());
        }
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
    raw_store: Arc<dyn RawMemoryStore>,
    backend: MemoryBackend,
    notes_dir: std::path::PathBuf,
    _home: Option<crate::utils::paths::IsolatedAlephHome>,
    _tmp_mem: tempfile::TempDir,
    _tmp_snap: tempfile::TempDir,
    reranker: Arc<StubReranker>,
}

async fn fixture(reranker: Arc<StubReranker>, config: AssemblerConfig) -> Fixture {
    fixture_with_notes(reranker, config, None).await
}

async fn fixture_with_notes(
    reranker: Arc<StubReranker>,
    config: AssemblerConfig,
    notes_dir: Option<std::path::PathBuf>,
) -> Fixture {
    // rust-doctor-disable-next-line unwrap-in-production
    let tmp_mem = tempfile::tempdir().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let tmp_snap = tempfile::tempdir().unwrap();
    let db_path = tmp_mem.path().join("mem.db");
    // rust-doctor-disable-next-line unwrap-in-production
    let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(FakeEmbedder);
    let notes_dir = notes_dir.unwrap_or_else(|| tmp_mem.path().join("notes"));
    let indexer = Arc::new(NoteIndexer::new(
        notes_dir.clone(),
        // rust-doctor-disable-next-line excessive-clone
        backend.clone(),
    ));
    let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));
    let raw_store = Arc::new(
        crate::memory::store::markdown::MarkdownSessionStore::new(tmp_mem.path())
            .await
            .unwrap(),
    );
    let snapshots = Arc::new(SnapshotReader::new(tmp_snap.path()));
    let profile = UserProfileLoader::new(notes_dir.clone());
    let feedback_floor = FeedbackFloorLoader::new(notes_dir.clone());
    // rust-doctor-disable-next-line excessive-clone
    let reranker_dyn: Arc<dyn LlmReranker> = reranker.clone();
    let assembler = HybridAssembler::new(
        retrieval,
        snapshots,
        // rust-doctor-disable-next-line excessive-clone
        backend.clone(),
        raw_store.clone(),
        profile,
        feedback_floor,
        reranker_dyn,
        config,
    );
    Fixture {
        assembler,
        raw_store,
        backend,
        notes_dir,
        _home: None,
        _tmp_mem: tmp_mem,
        _tmp_snap: tmp_snap,
        reranker,
    }
}

async fn seed_raw(backend: &Arc<dyn RawMemoryStore>, session_id: &str, count: usize) {
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
    let fx = fixture(reranker.clone(), default_cfg()).await;
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
    let fx = fixture(reranker.clone(), default_cfg()).await;
    seed_raw(&fx.raw_store, "sess-a", 5).await;
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
    let fx = fixture(reranker.clone(), default_cfg()).await;
    seed_raw(&fx.raw_store, "sess-b", 5).await;
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
    let fx = fixture(reranker.clone(), default_cfg()).await;
    seed_raw(&fx.raw_store, "sess-d", 5).await;
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
    let fx = fixture(reranker.clone(), cfg).await;
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
    let fx = fixture(reranker.clone(), cfg).await;
    seed_raw(&fx.raw_store, "sess-e", 5).await;
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
    let fx = fixture(reranker.clone(), cfg).await;
    seed_raw(&fx.raw_store, "sess-budget", 40).await;

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
    // `tokens_used` is a report; the content is what reaches the prompt.
    // Hydration once broke out of its loop leaving the tail of each slot
    // untruncated, so `tokens_used` stayed low while full text leaked.
    let content_chars: usize = env
        .slots
        .iter()
        .flat_map(|s| &s.items)
        .map(|i| i.content.chars().count())
        .sum();
    assert!(
        content_chars <= 300 * 4,
        "slot content ({content_chars} chars) exceeds the 300-token budget"
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
    let fx = fixture(reranker.clone(), cfg).await;
    seed_raw(&fx.raw_store, "sess-zero", 20).await;

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
    // Same effect-level check as the 300-token test above: with each live
    // slot floored at 1 token (4 chars), the surviving content must be a few
    // characters per slot, never the untruncated tail of the candidate pool.
    let content_chars: usize = env
        .slots
        .iter()
        .flat_map(|s| &s.items)
        .map(|i| i.content.chars().count())
        .sum();
    assert!(
        content_chars <= env.slots.len() * 8,
        "zero budget leaked {content_chars} chars of content across {} slots",
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
            let fx = fixture(reranker, default_cfg()).await;
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

// A duplicate public path must not collapse materialized notes from two owners,
// and synthetic profile/raw/summary entries must never acquire note exposure.
async fn reflection_owner_fixture(force_fallback: bool) -> Fixture {
    let reranker = Arc::new(StubReranker {
        response: StdMutex::new(None),
        sleep_for: None,
        was_called: AtomicBool::new(false),
        select_notes: true,
    });
    let mut config = default_cfg();
    config.force_fallback = force_fallback;
    let home = crate::utils::paths::IsolatedAlephHome::new();
    let mut fx = fixture_with_notes(
        reranker,
        config,
        Some(crate::utils::paths::get_note_memory_dir().unwrap()),
    )
    .await;
    fx._home = Some(home);
    for (owner, body) in [
        ("default", "quartz base evidence"),
        ("default__u-alice", "quartz personal evidence"),
    ] {
        let note = crate::memory::notes::KnowledgeNote {
            title: "shared".into(),
            category: "reference".into(),
            facts: vec![body.into()],
            ..Default::default()
        };
        let note_dir = fx.notes_dir.join(owner).join("reference");
        std::fs::create_dir_all(&note_dir).unwrap();
        std::fs::write(note_dir.join("shared.md"), note.to_markdown()).unwrap();
        fx.backend
            .index_note(&note, owner, "reference")
            .await
            .unwrap();
    }
    let other = crate::memory::notes::KnowledgeNote {
        title: "z-other".into(),
        category: "reference".into(),
        facts: vec!["quartz additional base evidence".into()],
        ..Default::default()
    };
    std::fs::write(
        fx.notes_dir.join("default/reference/z-other.md"),
        other.to_markdown(),
    )
    .unwrap();
    fx.backend
        .index_note(&other, "default", "reference")
        .await
        .unwrap();
    let profile_dir = fx.notes_dir.join("default__u-alice/personal");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(profile_dir.join("profile.md"), "synthetic quartz profile").unwrap();
    for (owner, severity, body) in [
        ("default", "high", "base standing rule"),
        ("default__u-alice", "critical", "personal standing rule"),
    ] {
        let dir = fx.notes_dir.join(owner).join("feedback");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("standing-rule.md"),
            format!(
                "---\ncategory: feedback\nseverity: {severity}\nconfidence: 0.9\n---\n\n- {body}\n"
            ),
        )
        .unwrap();
    }
    seed_raw(&fx.raw_store, "session-exposure", 1).await;
    let snapshot_dir = fx._tmp_snap.path().join("previous-session");
    std::fs::create_dir_all(&snapshot_dir).unwrap();
    std::fs::write(
        snapshot_dir.join("resume.json"),
        serde_json::to_vec(&crate::memory::session_resume::SessionSnapshot {
            session_id: "previous-session".into(),
            agent_id: "default".into(),
            scope_id: Some("default__u-alice".into()),
            created_at: chrono::Utc::now(),
            summary: "synthetic quartz summary".into(),
        })
        .unwrap(),
    )
    .unwrap();
    fx
}

async fn assert_same_path_notes(force_fallback: bool) {
    use crate::memory::reflector::packet_adapter::envelope_to_synthesis_context;
    crate::scope::with_scope(
        Some(crate::scope::ScopeAttribution::personal("u-alice")),
        async {
            let fx = reflection_owner_fixture(force_fallback).await;
            let env = fx
                .assembler
                .assemble(
                    "quartz",
                    "default",
                    Some("session-exposure"),
                    AssemblyBudget { total_tokens: 8000 },
                    Default::default(),
                )
                .await
                .unwrap();
            assert_eq!(env.meta.used_fallback, force_fallback);
            assert_eq!(
                env.slots
                    .iter()
                    .flat_map(|slot| &slot.items)
                    .filter(|item| item.title == "shared" && item.id == "note://reference/shared")
                    .count(),
                2,
                "private selection identities must not change public envelope IDs"
            );
            assert_eq!(
                env.slots
                    .iter()
                    .flat_map(|slot| &slot.items)
                    .filter(|item| item.title == "shared" && item.id == "note://reference/shared")
                    .count(),
                2,
                "private selection identities must not change public envelope IDs"
            );
            let ctx = envelope_to_synthesis_context(&env);
            assert!(
                ctx.user_prompt.contains("quartz base evidence"),
                "base note lost on fallback={force_fallback}"
            );
            assert!(
                ctx.user_prompt.contains("quartz personal evidence"),
                "personal note lost on fallback={force_fallback}"
            );
            assert!(ctx.user_prompt.contains("synthetic quartz profile"));
            assert!(ctx.user_prompt.contains("synthetic quartz summary"));
            assert!(ctx.user_prompt.contains("body 0"));
            assert!(ctx.user_prompt.contains("personal standing rule"));
            assert!(
                !ctx.user_prompt.contains("base standing rule"),
                "feedback floor must retain path-based dedup"
            );
            assert_eq!(
                ctx.note_lookup
                    .values()
                    .filter(|meta| meta.title == "shared")
                    .count(),
                2,
                "same-path notes must have distinct synthesis keys"
            );
            let sources: Vec<_> = ctx
                .note_lookup
                .iter()
                .filter(|(_, meta)| meta.title == "shared")
                .map(|(key, _)| serde_json::json!({"path": key}))
                .collect();
            let provider: Arc<dyn crate::providers::AiProvider> = Arc::new(
                crate::providers::recording_mock::RecordingMockProvider::new(
                    serde_json::json!({"text": "answer", "sources": sources}).to_string(),
                ),
            );
            let synthesis =
                crate::memory::reflector::fs_reflector::MemoryReflector::synthesise_from_context(
                    &ctx, &provider,
                )
                .await
                .unwrap();
            assert_eq!(synthesis.sources.len(), 2);
            assert!(
                synthesis
                    .sources
                    .iter()
                    .all(|source| source.path == "reference/shared"),
                "opaque aliases must map back to public citation paths"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn reflection_same_path_notes_survive_fallback() {
    assert_same_path_notes(true).await;
}

#[tokio::test]
async fn reflection_same_path_notes_survive_rerank() {
    assert_same_path_notes(false).await;
}

async fn assert_reflection_exposure(force_fallback: bool, fail_base: bool) {
    use crate::memory::reflector::fs_reflector::{MemoryReflector, RecallWriter};
    use crate::memory::reflector::types::ReflectOpts;
    use crate::providers::recording_mock::RecordingMockProvider;
    crate::scope::with_scope(Some(crate::scope::ScopeAttribution::personal("u-alice")), async {
            let fx = reflection_owner_fixture(force_fallback).await;
            let store = fx.backend.clone();
            let writer: RecallWriter = Arc::new(move |row| {
                let store = store.clone();
                Box::pin(async move {
                    store.record_signals(&row.query_text, &row.channel,
                        &row.hits,
                        row.session_id.as_deref(), &row.owner, &row.namespace).map(|_| ())
                })
            });
            let db = rusqlite::Connection::open(fx._tmp_mem.path().join("mem.db")).unwrap();
            if fail_base {
                db.execute_batch("CREATE TRIGGER fail_base BEFORE INSERT ON recall_signals WHEN NEW.agent_id = 'default' AND NEW.note_path = 'reference/z-other' BEGIN SELECT RAISE(FAIL, 'injected owner write failure'); END;").unwrap();
            }
            let reflector = MemoryReflector::new(Arc::new(fx.assembler),
                Arc::new(RecordingMockProvider::new(r#"{"text":"answer","sources":[]}"#.into())), writer);
            let mut opts = ReflectOpts::for_agent("default");
            opts.session_id = Some("session-exposure".into());
            let result = reflector.reflect("quartz", opts.clone()).await.unwrap();
            assert!(result.sources.is_empty());
            let mut stmt = db.prepare("SELECT agent_id, note_path FROM recall_signals WHERE channel = 'reflect' ORDER BY agent_id, note_path").unwrap();
            let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
            let expected = if fail_base {
                vec![("default__u-alice".into(), "feedback/standing-rule".into()),
                    ("default__u-alice".into(), "reference/shared".into())]
            } else {
                vec![("default".into(), "reference/shared".into()),
                    ("default".into(), "reference/z-other".into()),
                    ("default__u-alice".into(), "feedback/standing-rule".into()),
                    ("default__u-alice".into(), "reference/shared".into())]
            };
            assert_eq!(rows, expected, "only actual source owners may receive exposures, including after another owner fails");
            reflector.reflect("quartz", opts).await.unwrap();
            assert_eq!(db.query_row("SELECT COUNT(*) FROM recall_signals WHERE channel = 'reflect'", [], |r| r.get::<_, i64>(0)).unwrap(), if fail_base { 2 } else { 4 },
                "existing daily/query dedup must remain intact");
        }).await;
}

#[tokio::test]
async fn reflection_exposure_writes_true_owners_without_model_citations_fallback() {
    assert_reflection_exposure(true, false).await;
}

#[tokio::test]
async fn reflection_exposure_writes_true_owners_without_model_citations_rerank() {
    assert_reflection_exposure(false, false).await;
}

#[tokio::test]
async fn reflection_exposure_continues_after_an_owner_transaction_fails_fallback() {
    assert_reflection_exposure(true, true).await;
}

#[tokio::test]
async fn reflection_exposure_continues_after_an_owner_transaction_fails_rerank() {
    assert_reflection_exposure(false, true).await;
}

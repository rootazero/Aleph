//! Pin pre-Spec-B HybridAssembler behaviour as a regression guard.
//!
//! Seeds an in-memory SQLite store with a known fact set, runs the assembler
//! with default args (current 4-arg signature), serialises the result
//! deterministically, and asserts equality against
//! tests/snapshots/spec_b_baseline.json.
//!
//! Task 4 will modify the assemble() call to pass FactSourceFilter::Any as a
//! 5th argument.  The snapshot MUST still match after that change — that's the
//! regression guard.
//!
//! Intentional baseline movement, 2026-08-01: `session_recent.tokens_budget`
//! 1500 → 512. The skeleton fallback path used to pack the *static*
//! `FallbackSkeleton` figures (~8200 tokens total) regardless of the caller's
//! runtime headroom, so `AssemblyBudget.total_tokens` was advisory on what is
//! 100% of traffic when no rerank provider is configured. It now applies the
//! same 70% cap as the LLM path: 4000 × 0.7 = 2800 across all slots, so
//! session_recent scales to 1500 × (2800/8200) = 512. `tokens_used` is
//! unchanged at 9 — no content is dropped; only the *declared* budget became
//! honest. See FEATURE_LOCATOR §2.5③.

use std::path::Path;

use alephcore::memory::assembler::hybrid::LlmReranker;
use alephcore::memory::assembler::FeedbackFloorLoader;
use alephcore::memory::assembler::{
    AssemblyBudget, HybridAssembler, ItemSource, MemoryEnvelope, UserProfileLoader,
    WorkingMemoryAssembler,
};
use alephcore::memory::embedding_provider::EmbeddingProvider;
use alephcore::memory::note_retrieval::NoteFactRetrieval;
use alephcore::memory::notes::NoteIndexer;
use alephcore::memory::session_resume::reader::SnapshotReader;
use alephcore::memory::session_search_summary::FactSourceFilter;
use alephcore::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use alephcore::memory::{MemoryBackend, SqliteMemoryBackend};
use alephcore::{AlephError, AssemblerConfig};
use async_trait::async_trait;
use std::sync::Arc;

// ---- Stubs ----------------------------------------------------------------

/// Passthrough reranker — returns an error so the assembler always falls back
/// to the skeleton path.  With `force_fallback: true` in config this stub is
/// never called; it is present only to satisfy the constructor signature.
struct NeverCalledReranker;

#[async_trait]
impl LlmReranker for NeverCalledReranker {
    async fn complete(&self, _prompt: &str, _model: Option<&str>) -> Result<String, AlephError> {
        Err(AlephError::config(
            "NeverCalledReranker must not be invoked".to_string(),
        ))
    }
}

/// Zero-vector embedder — all embeddings are identical 768-dim zero vectors.
struct ZeroEmbedder;

#[async_trait]
impl EmbeddingProvider for ZeroEmbedder {
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
        "zero"
    }
    fn provider_id(&self) -> &str {
        "zero"
    }
}

// ---- Fixture helpers -------------------------------------------------------

fn force_fallback_cfg() -> AssemblerConfig {
    AssemblerConfig {
        enabled: true,
        candidate_pool_limit: 20,
        rerank_timeout_ms: 200,
        rerank_model: None,
        render_style: Default::default(),
        // force_fallback=true means the assembler always takes the skeleton path
        // regardless of pool size — fully deterministic, no LLM calls.
        force_fallback: true,
        fallback_skeleton: Default::default(),
        assembly_log: Default::default(),
        project_scoped: false,
        retrieval_scoring: Default::default(),
        rerank: Default::default(),
        expansion: Default::default(),
    }
}

fn build_assembler(backend: MemoryBackend, notes_dir: std::path::PathBuf) -> HybridAssembler {
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(ZeroEmbedder);
    let indexer = Arc::new(NoteIndexer::new(notes_dir.clone(), backend.clone()));
    let retrieval = Arc::new(NoteFactRetrieval::new(indexer, embedder));

    // Use a temp dir for snapshots — no snapshots are seeded so this is empty.
    let snap_dir = notes_dir.join("_snapshots");
    std::fs::create_dir_all(&snap_dir).unwrap();
    let snapshots = Arc::new(SnapshotReader::new(&snap_dir));

    let profile = UserProfileLoader::new(notes_dir.clone());
    let feedback_floor = FeedbackFloorLoader::new(notes_dir);
    let reranker: Arc<dyn LlmReranker> = Arc::new(NeverCalledReranker);

    HybridAssembler::new(
        retrieval,
        snapshots,
        backend,
        profile,
        feedback_floor,
        reranker,
        force_fallback_cfg(),
    )
}

/// Seed the three deterministic facts described in the Task 3 spec.
///
/// All three use `created_at = 1_234_567_890` so the snapshot is
/// reproducible across runs regardless of wall-clock time.
async fn seed_facts(backend: &MemoryBackend) {
    // (a) Transcript fragment — represents a note-like retrieval candidate.
    let mut fact_a = RawMemory::new("Aleph uses Rust".to_string(), RawMemorySource::Transcript)
        .with_agent("agent-1")
        .with_session("sess-baseline")
        .with_path("aleph://session/sess-baseline/raw/frag-a");
    fact_a.created_at = 1_234_567_890;
    backend.insert_raw_memory(&fact_a).await.unwrap();

    // (b) Tool output fragment — a distinct source type.
    let mut fact_b = RawMemory::new(
        "Project deadline 2026-Q3".to_string(),
        RawMemorySource::ToolOutput,
    )
    .with_agent("agent-1")
    .with_session("sess-baseline")
    .with_path("aleph://session/sess-baseline/raw/frag-b");
    fact_b.created_at = 1_234_567_890;
    backend.insert_raw_memory(&fact_b).await.unwrap();

    // (c) Session-compressed summary fact — the key target for Spec B.
    let mut fact_c = RawMemory::new(
        "User asked about deployment".to_string(),
        RawMemorySource::SessionCompressed,
    )
    .with_agent("agent-1")
    .with_session("sess-baseline")
    .with_path("aleph://session/xyz/d0/0");
    fact_c.created_at = 1_234_567_890;
    backend.insert_raw_memory(&fact_c).await.unwrap();
}

// ---- Deterministic serialiser helpers -------------------------------------

/// Replace a trailing UUID (8-4-4-4-12 hex) with `<uuid>` so snapshot strings
/// are stable across runs.
///
/// Examples:
///   `"aleph://session/sess-baseline/raw/0d7ee7e5-3251-49a9-b6ae-1a8e0c929ccb"`
///   → `"aleph://session/sess-baseline/raw/<uuid>"`
///
///   `"Raw fragment 0d7ee7e5-3251-49a9-b6ae-1a8e0c929ccb"`
///   → `"Raw fragment <uuid>"`
fn redact_uuid_suffix(s: &str) -> String {
    // Match the standard UUID format: 8-4-4-4-12 lower-hex groups.
    let uuid_pattern =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
            .expect("valid regex");
    uuid_pattern.replace_all(s, "<uuid>").into_owned()
}

// ---- Deterministic serialiser ---------------------------------------------

/// Serialise `MemoryEnvelope` to a stable JSON string suitable for snapshot
/// comparison.
///
/// Redacts wall-clock fields (`generated_at`, `total_latency_ms`,
/// `llm_rerank_latency_ms`) that vary across runs.  Everything else is
/// serialised verbatim and items are sorted by (score desc, id asc) for a
/// stable ordering.
fn envelope_to_snapshot_json(envelope: &MemoryEnvelope) -> String {
    use serde_json::{json, Value};

    let slots: Vec<Value> = envelope
        .slots
        .iter()
        .map(|slot| {
            // Sort items: descending relevance, then ascending content for
            // ties.  We sort by content (not id/title) because id/title embed
            // UUIDs that are already redacted to "<uuid>" — all ties would
            // then be equal, leaving order undefined.  Content is stable and
            // unique across our seed facts.
            let mut items: Vec<&_> = slot.items.iter().collect();
            items.sort_by(|a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.content.cmp(&b.content))
            });

            let items_json: Vec<Value> = items
                .iter()
                .map(|item| {
                    let source_json = match &item.source {
                        ItemSource::Note { path, category } => {
                            json!({"kind": "note", "path": path, "category": category})
                        }
                        ItemSource::Raw {
                            raw_id: _,
                            session_id,
                            ..
                        } => {
                            // Redact raw_id (UUID) — it changes every run.
                            json!({"kind": "raw", "raw_id": "<redacted>", "session_id": session_id})
                        }
                        ItemSource::Summary { layer, session_id } => {
                            json!({"kind": "summary", "layer": layer, "session_id": session_id})
                        }
                    };

                    // For Raw items the id/title embed the UUID assigned at
                    // insert time, which changes every run.  Replace the UUID
                    // portion with a stable placeholder so the snapshot stays
                    // reproducible.
                    let stable_id = redact_uuid_suffix(&item.id);
                    let stable_title = redact_uuid_suffix(&item.title);

                    json!({
                        "id": stable_id,
                        "title": stable_title,
                        "content": item.content,
                        "source": source_json,
                        // Round to 4 decimal places to avoid float noise.
                        "relevance": (item.relevance * 10_000.0).round() / 10_000.0,
                        "tokens": item.tokens,
                        // Redact updated_at — it's a wall-clock field set by the store.
                        "updated_at": "<redacted>",
                    })
                })
                .collect();

            json!({
                "kind": slot.kind,
                "tokens_used": slot.tokens_used,
                "tokens_budget": slot.tokens_budget,
                "items": items_json,
            })
        })
        .collect();

    let snapshot = json!({
        "schema_version": envelope.schema_version,
        // generated_at is wall-clock — redacted.
        "generated_at": "<redacted>",
        "query": envelope.query,
        "agent_id": envelope.agent_id,
        "session_id": envelope.session_id,
        "slots": slots,
        "meta": {
            "strategy": envelope.meta.strategy,
            "candidates_considered": envelope.meta.candidates_considered,
            "used_fallback": envelope.meta.used_fallback,
            "fallback_reason": envelope.meta.fallback_reason,
            // latency fields are wall-clock — redacted.
            "llm_rerank_latency_ms": "<redacted>",
            "total_latency_ms": "<redacted>",
        },
    });

    serde_json::to_string_pretty(&snapshot).expect("snapshot serialisation must not fail")
}

// ---- Test -----------------------------------------------------------------

#[tokio::test]
async fn note_retrieval_default_behaviour_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("mem.db");
    let notes_dir = tmp.path().join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();

    let backend: MemoryBackend = Arc::new(SqliteMemoryBackend::new(&db_path).unwrap());

    // ---- Seed deterministic facts ----
    seed_facts(&backend).await;

    // ---- Build HybridAssembler ----
    let assembler = build_assembler(backend, notes_dir);

    // ---- Run with the 5-arg signature (FactSourceFilter::Any = pre-Spec-B behaviour) ----
    let envelope = assembler
        .assemble(
            "deployment",
            "agent-1",
            Some("sess-baseline"),
            AssemblyBudget { total_tokens: 4000 },
            FactSourceFilter::Any,
        )
        .await
        .expect("assemble must succeed");

    // ---- Serialise deterministically ----
    let actual = envelope_to_snapshot_json(&envelope);

    // ---- Snapshot compare ----
    let snapshot_path = "tests/snapshots/spec_b_baseline.json";
    if !Path::new(snapshot_path).exists() {
        std::fs::create_dir_all("tests/snapshots").unwrap();
        std::fs::write(snapshot_path, &actual).unwrap();
        eprintln!(
            "Wrote baseline snapshot ({} bytes). Re-run to assert.",
            actual.len()
        );
        return;
    }

    let expected = std::fs::read_to_string(snapshot_path).unwrap();
    // The golden file may be checked out with CRLF on Windows (git autocrlf);
    // the snapshot is line-ending agnostic, so normalise before comparing.
    assert_eq!(
        actual,
        expected.replace("\r\n", "\n"),
        "Note-retrieval baseline drifted!"
    );
}

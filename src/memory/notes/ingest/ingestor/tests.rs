//! Consolidated unit tests for the compound ingestor.
//!
//! These were three inline `#[cfg(test)] mod` blocks in the pre-split
//! `ingestor.rs` (`trait_tests`, `plan_tests`, `link_contract_tests`). After
//! the directory split, `super` only reaches `mod.rs`, so the production
//! symbols the tests reference are imported explicitly below.

use super::helpers::{build_user_prompt, candidate_dedup_text, cosine_similarity};
use super::plan_parse::{infer_op_kind, parse_plan_lenient, repair_kind_tags, summary_from_report};
use super::*;

use crate::error::AlephError;
use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
use crate::memory::notes::ingest::plan::{ApplyReport, IngestPlan, PageOp};
use crate::memory::notes::ingest::retrieve::{RelatedBudget, RelatedPage};
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
use crate::memory::store::SqliteMemoryBackend;
use crate::providers::recording_mock::RecordingMockProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;

struct StubIngestor;

#[async_trait]
impl CompoundIngestor for StubIngestor {
    async fn ingest_batch(
        &self,
        _agent_id: &str,
        _raws: Vec<RawMemory>,
        _extra_context: Option<&str>,
    ) -> Result<ApplyReport, AlephError> {
        Ok(ApplyReport {
            tx_id: "stub".into(),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn trait_object_dispatch() {
    let ing: Box<dyn CompoundIngestor> = Box::new(StubIngestor);
    // rust-doctor-disable-next-line unwrap-in-production
    let r = ing.ingest_batch("default", vec![], None).await.unwrap();
    assert_eq!(r.tx_id, "stub");
}

async fn mk() -> (
    tempfile::TempDir,
    Arc<SqliteMemoryBackend>,
    Arc<NoteIndexer<SqliteMemoryBackend>>,
) {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = tempfile::tempdir().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // rust-doctor-disable-next-line excessive-clone
    let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
    (dir, backend, indexer)
}

#[tokio::test]
async fn plan_parses_valid_json() {
    let (dir, backend, indexer) = mk().await;
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{
          "reasoning": "new page + link",
          "ops": [
            {"kind": "create", "note_path": "learning/tokio", "title": "Tokio",
             "summary": "async runtime", "facts": ["event loop"],
             "links": ["learning/rust-async"], "tags": ["rust"]}
          ],
          "schema_proposals": []
        }"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let raw = RawMemory::new("some content".to_string(), RawMemorySource::Transcript);
    let (plan, _degraded) = ing
        .plan_with_health("default", &[raw], &[], &RawMemorySource::Transcript, None)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(plan.ops.len(), 1);
    match &plan.ops[0] {
        PageOp::Create { note_path, .. } => assert_eq!(note_path, "learning/tokio"),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!(),
    }
}

#[tokio::test]
async fn plan_returns_empty_on_invalid_json() {
    let (dir, backend, indexer) = mk().await;
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("not json".into()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    let (plan, _degraded) = ing
        .plan_with_health("default", &[raw], &[], &RawMemorySource::Transcript, None)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(plan.ops.is_empty());
}

#[tokio::test]
async fn plan_filters_invalid_ops() {
    let (dir, backend, indexer) = mk().await;
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"learning/x","title":"X","summary":"","facts":[],"links":[],"tags":[]},
            {"kind":"create","note_path":"bad-no-slash","title":"Y","summary":"","facts":[],"links":["learning/x"],"tags":[]},
            {"kind":"append","note_path":"learning/y","new_facts":["f"],"new_links":[]}
        ]}"#.into()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    let (plan, _degraded) = ing
        .plan_with_health("default", &[raw], &[], &RawMemorySource::Transcript, None)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // The no-slash create is dropped (malformed path); the linkless create
    // with a valid path is KEPT as a seed note (see `valid_op`), and the
    // append is kept. Linkless creates are no longer discarded — that silent
    // drop was starving the L1 note layer on a sparse wiki.
    assert_eq!(plan.ops.len(), 2);
    assert!(plan.ops.iter().any(|op| matches!(
        op,
        PageOp::Create { note_path, links, .. } if note_path == "learning/x" && links.is_empty()
    )));
    assert!(
        !plan.ops.iter().any(
            |op| matches!(op, PageOp::Create { note_path, .. } if note_path == "bad-no-slash")
        ),
        "no-slash create must still be dropped"
    );
}

/// Bootstrap regression: on a sparse wiki the planner emits a `create`
/// whose only links are out-of-range `[P<n>]` tokens (e.g. the few-shot
/// examples use `[P3]` but no related pages are shown). `RefTable` strips
/// the hallucinated link, leaving the create linkless — it must STILL
/// survive as a seed note instead of being silently dropped, otherwise the
/// note layer can never grow past its seed. This is the exact live failure
/// observed on agent `main` (notes_index stuck at 1).
#[tokio::test]
async fn plan_keeps_seed_create_when_all_link_tokens_hallucinated() {
    let (dir, backend, indexer) = mk().await;
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"system/video-config","title":"Video config",
             "summary":"durable fact","facts":["The user configured a custom video provider."],
             "links":["[P3]"],"tags":["system"]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    // No related pages (sparse wiki) → `[P3]` is out of range and stripped.
    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    let (plan, _degraded) = ing
        .plan_with_health("default", &[raw], &[], &RawMemorySource::Transcript, None)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(plan.ops.len(), 1, "seed create must survive stripped links");
    match &plan.ops[0] {
        PageOp::Create {
            note_path, links, ..
        } => {
            assert_eq!(note_path, "system/video-config");
            assert!(links.is_empty(), "hallucinated link token must be stripped");
        }
        // rust-doctor-disable-next-line panic-in-library
        other => panic!("expected surviving Create, got {other:?}"),
    }
}

#[tokio::test]
async fn plan_resolves_reference_token_to_canonical_path() {
    let (dir, backend, indexer) = mk().await;
    // LLM emits an append targeting the related page by its [P0] token
    // rather than retyping the path. Resolution rewrites it exactly.
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"append","note_path":"[P0]","new_facts":["new fact"],"new_links":[]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let related = vec![RelatedPage {
        path: "preference/coding-style".into(),
        title: "coding-style".into(),
        summary: String::new(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h0".into(),
        score: 1.0,
    }];
    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    let (plan, _degraded) = ing
        .plan_with_health(
            "default",
            &[raw],
            &related,
            &RawMemorySource::Transcript,
            None,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(plan.ops.len(), 1);
    match &plan.ops[0] {
        PageOp::Append { note_path, .. } => assert_eq!(note_path, "preference/coding-style"),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected append"),
    }
}

#[tokio::test]
async fn plan_drops_op_with_hallucinated_token() {
    let (dir, backend, indexer) = mk().await;
    // [P9] is out of range (only P0 exists) → the op is dropped, not
    // applied against a forged orphan page.
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"append","note_path":"[P9]","new_facts":["x"],"new_links":[]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let related = vec![RelatedPage {
        path: "preference/coding-style".into(),
        title: "coding-style".into(),
        summary: String::new(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h0".into(),
        score: 1.0,
    }];
    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    let (plan, _degraded) = ing
        .plan_with_health(
            "default",
            &[raw],
            &related,
            &RawMemorySource::Transcript,
            None,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(plan.ops.is_empty(), "hallucinated-token op must be dropped");
}

#[test]
fn build_user_prompt_renders_reference_tokens() {
    let raws = vec![RawMemory::new("hello".into(), RawMemorySource::Transcript)];
    let related = vec![
        RelatedPage {
            path: "preference/coding-style".into(),
            title: "coding-style".into(),
            summary: String::new(),
            content_preview: "prefers vim".into(),
            tags: vec!["tool".into()],
            content_hash: "h0".into(),
            score: 1.0,
        },
        RelatedPage {
            path: "personal/li-wei".into(),
            title: "li-wei".into(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: "h1".into(),
            score: 0.5,
        },
    ];
    let prompt = build_user_prompt(&raws, &related, "2026-01-15 (Thursday)");
    assert!(prompt.contains("[P0] path=preference/coding-style"));
    assert!(prompt.contains("[P1] path=personal/li-wei"));
    assert!(prompt.contains("reference token"));
}

#[test]
fn build_user_prompt_injects_observation_date() {
    let raws = vec![RawMemory::new("hello".into(), RawMemorySource::Transcript)];
    let prompt = build_user_prompt(&raws, &[], "2026-01-15 (Thursday)");
    assert!(prompt.contains("## Observation date"));
    assert!(prompt.contains("2026-01-15 (Thursday)"));
    assert!(
        prompt.contains("absolute date"),
        "must instruct the model to resolve relative time"
    );
}

#[test]
fn build_user_prompt_reminds_about_kind_field() {
    let raws = vec![RawMemory::new("hello".into(), RawMemorySource::Transcript)];
    let prompt = build_user_prompt(&raws, &[], "2026-01-15 (Thursday)");
    assert!(
        prompt.contains("`kind`"),
        "prompt must remind the model to emit the kind discriminator"
    );
}

#[test]
fn infer_op_kind_maps_each_variant_shape() {
    let cases = [
        (serde_json::json!({"from": "a/b", "to": "c/d"}), "link"),
        (
            serde_json::json!({"old_path": "a/b", "new_path": "c/d"}),
            "supersede",
        ),
        (
            serde_json::json!({"note_path": "a/b", "new_claim": "x"}),
            "contradict",
        ),
        (
            serde_json::json!({"note_path": "a/b", "expected_content_hash": "h"}),
            "update",
        ),
        (
            serde_json::json!({"note_path": "a/b", "title": "T", "summary": "S"}),
            "create",
        ),
        (
            serde_json::json!({"note_path": "a/b", "new_facts": ["f"]}),
            "append",
        ),
    ];
    for (op, expected) in cases {
        let obj = op.as_object().unwrap();
        assert_eq!(infer_op_kind(obj), Some(expected), "shape: {op}");
    }
    // Unidentifiable shape → None.
    assert_eq!(
        infer_op_kind(serde_json::json!({"note_path": "a/b"}).as_object().unwrap()),
        None
    );
}

#[test]
fn repair_kind_tags_recovers_kindless_plan() {
    // The dominant failure: ops lack `kind`, and a schema_proposal also
    // lacks `kind` (which used to hard-fail the whole parse).
    let raw = serde_json::json!({
        "reasoning": "r",
        "ops": [
            {"note_path": "learning/tokio", "title": "Tokio", "summary": "rt",
             "facts": ["x"], "links": ["learning/rust"], "tags": []},
            {"kind": "append", "note_path": "learning/rust", "new_facts": ["y"]},
            {"foo": "bar"}
        ],
        "schema_proposals": [
            {"tag": "rust", "rationale": "r"},
            {"kind": "new_tag", "tag": "async", "rationale": "r"}
        ]
    });
    let repaired = repair_kind_tags(raw);
    // The kindless create is recovered, the explicit append kept, the
    // unidentifiable op dropped → 2 ops.
    let ops = repaired["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0]["kind"], "create");
    assert_eq!(ops[1]["kind"], "append");
    // Kindless schema_proposal dropped, valid one kept.
    let props = repaired["schema_proposals"].as_array().unwrap();
    assert_eq!(props.len(), 1);
    assert_eq!(props[0]["kind"], "new_tag");
    // Crucially: the repaired JSON now deserializes into a real plan.
    let plan: IngestPlan = serde_json::from_value(repaired).unwrap();
    assert_eq!(plan.ops.len(), 2);
    assert_eq!(plan.schema_proposals.len(), 1);
}

#[test]
fn parse_plan_lenient_keeps_good_op_despite_malformed_proposal() {
    // Regression for the live failure: the planner emitted a valid create
    // op AND a schema_proposal missing its required `rationale` field. The
    // old all-or-nothing parse failed the whole batch (`missing field
    // rationale`), discarding the good op. Element-wise parsing must keep
    // the op and drop only the malformed proposal.
    let json = serde_json::json!({
        "reasoning": "extract durable ops fact",
        "ops": [
            {"kind": "create", "note_path": "reference/aws-tokyo", "title": "AWS Tokyo",
             "summary": "prod db region", "facts": ["ap-northeast-1"],
             "links": ["reference/ops"], "tags": ["ops"]}
        ],
        "schema_proposals": [
            {"kind": "new_tag", "tag": "ops"}
        ]
    });
    let plan = parse_plan_lenient(json);
    assert_eq!(plan.ops.len(), 1, "valid create op must survive");
    assert_eq!(
        plan.schema_proposals.len(),
        0,
        "proposal missing required `rationale` is dropped, not fatal"
    );
    assert_eq!(plan.reasoning, "extract durable ops fact");
}

#[test]
fn summary_from_report_aggregates_touched_paths_by_category() {
    let report = ApplyReport {
        tx_id: "tx".into(),
        touched_paths: vec![
            "learning/rust-async".into(),
            "learning/tokio".into(),
            "preference/editor".into(),
            "no-slash-bad-path".into(), // dropped by split_once('/')
        ],
        ..Default::default()
    };
    let summary = summary_from_report("default", &report);
    assert_eq!(summary.agent_id, "default");
    // BTreeMap ordering: "learning" < "preference" alphabetically.
    assert_eq!(summary.touched.len(), 2);
    assert_eq!(summary.touched[0].category, "learning");
    assert_eq!(summary.touched[0].added, 2);
    assert_eq!(summary.touched[0].updated, 0);
    assert_eq!(summary.touched[1].category, "preference");
    assert_eq!(summary.touched[1].added, 1);
}

#[test]
fn summary_from_report_empty_when_no_touched_paths() {
    let report = ApplyReport::default();
    let summary = summary_from_report("default", &report);
    assert!(summary.touched.is_empty());
}

#[tokio::test]
async fn ingest_batch_refreshes_index_md_at_tail() {
    use crate::memory::notes::orientation::FsNoteOrientation;

    // rust-doctor-disable-next-line unwrap-in-production
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = dir.path().join("note");
    // rust-doctor-disable-next-line unwrap-in-production
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // rust-doctor-disable-next-line excessive-clone
    let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

    let orient: Arc<dyn NoteOrientation> =
        // rust-doctor-disable-next-line excessive-clone
        Arc::new(FsNoteOrientation::new(memory_dir.clone(), backend.clone()));
    // bootstrap is also done by ingest_batch, but doing it here gives a
    // pre-ingest baseline for index.md so we can prove the refresh fires.
    // rust-doctor-disable-next-line unwrap-in-production
    orient.bootstrap("default").await.unwrap();

    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"preference/editor","title":"editor",
             "summary":"prefers vim","facts":["uses vim"],
             "links":["preference/keymap"],"tags":["tool"]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        // rust-doctor-disable-next-line excessive-clone
        orientation: Some(orient.clone()),
        // rust-doctor-disable-next-line excessive-clone
        memory_dir: memory_dir.clone(),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };

    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    // rust-doctor-disable-next-line unwrap-in-production
    let report = ing.ingest_batch("default", vec![raw], None).await.unwrap();
    assert_eq!(report.created, 1);

    let index_md_path = memory_dir.join("default").join("index.md");
    assert!(
        index_md_path.exists(),
        "index.md must exist after ingest_batch"
    );
    // rust-doctor-disable-next-line unwrap-in-production
    let body = tokio::fs::read_to_string(&index_md_path).await.unwrap();
    assert!(
        body.contains("preference"),
        "index.md must list the touched 'preference' category; got:\n{body}"
    );
}

/// Boot is not the only moment residue can appear, so it cannot be the only
/// moment it is collected. All three `CompoundApplyTx` cleanup sites `warn!`
/// and leave the tree when `remove_dir_all` fails, and that process keeps
/// running — on a resident daemon "the next boot" bounds nothing.
///
/// This asserts at the real seam (`ingest_batch`, not the sweep function) for
/// the reason the wire was missing in the first place: `sweep_tx_residue` had
/// passing tests of its own the whole time it had exactly one caller. The live
/// sibling is in the same test because the ingest-time caller is the one that
/// genuinely races a concurrent apply — the age ceiling is load-bearing here in
/// a way it never was at boot.
#[tokio::test]
async fn an_apply_clears_abandoned_staging_trees_before_staging_its_own() {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = dir.path().join("note");
    // rust-doctor-disable-next-line unwrap-in-production
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // rust-doctor-disable-next-line excessive-clone
    let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

    let tx_root = memory_dir
        .join("default")
        .join(crate::memory::notes::ingest::TX_DIR);
    let dead = tx_root.join("dead-tx");
    let live = tx_root.join("live-tx");
    // rust-doctor-disable-next-line unwrap-in-production
    std::fs::create_dir_all(dead.join("preference")).unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    std::fs::create_dir_all(live.join("preference")).unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    std::fs::write(
        dead.join("preference/a.md"),
        "staged by a process that died",
    )
    .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    std::fs::write(
        live.join("preference/b.md"),
        "staged by a sibling still working",
    )
    .unwrap();
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7_200);
    filetime::set_file_mtime(&dead, filetime::FileTime::from_system_time(two_hours_ago))
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"preference/editor","title":"editor",
             "summary":"prefers vim","facts":["uses vim"],
             "links":[],"tags":["tool"]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3_600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        // rust-doctor-disable-next-line excessive-clone
        memory_dir: memory_dir.clone(),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };

    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    // rust-doctor-disable-next-line unwrap-in-production
    let report = ing.ingest_batch("default", vec![raw], None).await.unwrap();
    assert_eq!(report.created, 1, "the apply itself must still succeed");

    assert!(
        !dead.exists(),
        "a staging tree older than the ceiling is residue; only an apply or a \
         boot ever looks at this directory, and on a resident daemon the apply \
         is the one that comes"
    );
    assert!(
        live.exists(),
        "a tree younger than the ceiling may belong to a concurrent apply still \
         staging into it; deleting it would corrupt that apply"
    );
}

/// An embedder that always fails — models a down/quota-exhausted embedding
/// endpoint. Used to prove `ingest_batch` degrades gracefully instead of
/// starving the note layer.
struct FailingEmbeddingProvider;

#[async_trait]
impl crate::memory::embedding_provider::EmbeddingProvider for FailingEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
        Err(AlephError::other("embedding endpoint unavailable"))
    }
    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        Err(AlephError::other("embedding endpoint unavailable"))
    }
    fn dimensions(&self) -> usize {
        1024
    }
    fn model_name(&self) -> &str {
        "failing"
    }
    fn provider_id(&self) -> &str {
        "failing"
    }
}

/// Regression: when the embedding endpoint is down, `gather_related` fails,
/// but the batch must STILL produce a note. Previously the error
/// propagated out of `ingest_batch`, `compress_to_notes` returned without
/// marking the raws processed, and the L1 note layer starved indefinitely.
#[tokio::test]
async fn ingest_batch_degrades_when_embedding_fails() {
    let (dir, backend, indexer) = mk().await;
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"learning/tokio","title":"Tokio",
             "summary":"async runtime","facts":["event loop"],
             "links":["learning/rust-async"],"tags":["rust"]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(FailingEmbeddingProvider),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };

    let raw = RawMemory::new("some content".to_string(), RawMemorySource::Transcript);
    let report = ing
        .ingest_batch("default", vec![raw], None)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("ingest must succeed despite embedding failure");
    assert_eq!(
        report.created, 1,
        "note must be created even when related-page embedding fails"
    );
}

#[tokio::test]
async fn ingest_batch_pushes_and_flushes_embedding() {
    use crate::config::types::memory::EmbeddingSettings;
    use crate::memory::embedding_manager::EmbeddingManager;
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::store::NoteStore;

    // rust-doctor-disable-next-line unwrap-in-production
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = dir.path().join("note");
    // rust-doctor-disable-next-line unwrap-in-production
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // rust-doctor-disable-next-line excessive-clone
    let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

    // Seat a Mock provider on the manager so flush_pending writes vectors.
    let mgr = Arc::new(EmbeddingManager::new(EmbeddingSettings::default()));
    let mock: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(1024, "mock-1024"));
    mgr.install_provider_for_test(mock).await;

    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"preference/editor","title":"editor",
             "summary":"prefers vim","facts":["uses vim"],
             "links":["preference/keymap"],"tags":["tool"]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        // rust-doctor-disable-next-line excessive-clone
        memory_dir: memory_dir.clone(),
        budget: RelatedBudget::default(),
        // rust-doctor-disable-next-line excessive-clone
        embedding_manager: Some(mgr.clone()),
        gate: None,
    };

    let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
    // rust-doctor-disable-next-line unwrap-in-production
    let report = ing.ingest_batch("default", vec![raw], None).await.unwrap();
    assert_eq!(report.created, 1);
    assert_eq!(report.touched_paths.len(), 1);

    // Queue must be drained — flush_pending ran at the tail.
    assert_eq!(
        mgr.pending_len().await,
        0,
        "pending queue must be drained after ingest tail flush"
    );

    // Vector must be persisted for the touched path.
    let touched = &report.touched_paths[0];
    let v = backend
        .get_embedding(touched, "default", 1024)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(
        v.is_some(),
        "embedding must be present after ingest tail flush; touched={touched}"
    );
}

#[tokio::test]
async fn end_to_end_append_on_existing() {
    let (dir, backend, indexer) = mk().await;

    // Seed: create learning/rust-async first
    let provider_seed: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"learning/rust-async","title":"rust-async",
             "summary":"async primitives","facts":["Futures are lazy"],
             "links":["learning/tokio"],"tags":["rust","async"]}
        ]}"#
        .into(),
    ));
    let ing_seed = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        // rust-doctor-disable-next-line excessive-clone
        indexer: indexer.clone(),
        provider: provider_seed,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let r1 = ing_seed
        .ingest_batch(
            "default",
            vec![RawMemory::new(
                "seed".to_string(),
                RawMemorySource::Transcript,
            )],
            None,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(r1.created, 1);

    // Second batch: append
    let provider2: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"append","note_path":"learning/rust-async",
             "new_facts":["tokio is the runtime"],"new_links":[]}
        ]}"#
        .into(),
    ));
    let ing2 = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        // rust-doctor-disable-next-line excessive-clone
        indexer: indexer.clone(),
        provider: provider2,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let r2 = ing2
        .ingest_batch(
            "default",
            vec![RawMemory::new(
                "body2".to_string(),
                RawMemorySource::Transcript,
            )],
            None,
        )
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert_eq!(r2.appended, 1);

    let body = tokio::fs::read_to_string(dir.path().join("note/default/learning/rust-async.md"))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(body.contains("Futures are lazy"));
    assert!(body.contains("tokio is the runtime"));
}

// ---- Write-time semantic dedup (mem0-style) ----

#[test]
fn cosine_similarity_identical_is_one() {
    let v = vec![0.1f32; 8];
    assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-5);
}

#[test]
fn cosine_similarity_orthogonal_is_zero() {
    let a = vec![1.0f32, 0.0];
    let b = vec![0.0f32, 1.0];
    assert!(cosine_similarity(&a, &b).abs() < 1e-5);
}

#[test]
fn cosine_similarity_guards_mismatch_and_zero_norm() {
    assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    assert_eq!(cosine_similarity(&[], &[]), 0.0);
    assert_eq!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
}

#[test]
fn candidate_dedup_text_skips_empty_fields() {
    let t = candidate_dedup_text("Tokio", "", &["event loop".into(), "tasks".into()]);
    assert!(t.contains("Tokio"));
    assert!(t.contains("event loop"));
    assert!(t.contains("tasks"));
    // empty summary contributes no blank line of its own
    assert!(!t.contains("\n\n"));
}

/// With dedup enabled and an existing note in the MERGE band, a planned
/// `Create` is rewritten into an `Append` onto the existing page, carrying
/// its facts and links. The mock embedder returns a constant `[0.1; 1024]`
/// for the candidate, so cosine is fixed by the seeded stored vector:
/// `0.03125 * sqrt(num 0.1-entries)`. 900 entries → cosine 0.9375, which is
/// in `[dedup 0.92, noop 0.985)` → MERGE (not NOOP).
#[tokio::test]
async fn dedup_redirects_near_duplicate_create_to_append() {
    let (dir, backend, indexer) = mk().await;
    // Seed a MERGE-band embedding (cosine 0.9375 vs the candidate's
    // [0.1; 1024]) so the Create redirects to Append rather than dropping.
    let mut merge_vec = vec![0.1f32; 900];
    merge_vec.extend(std::iter::repeat_n(0.0f32, 124));
    backend
        .upsert_embedding("learning/tokio", "default", &merge_vec, 1024, "")
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget {
            dedup_enabled: true,
            ..RelatedBudget::default()
        },
        embedding_manager: None,
        gate: None,
    };

    let related = vec![RelatedPage {
        path: "learning/tokio".into(),
        title: "tokio".into(),
        summary: String::new(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h".into(),
        score: 1.0,
    }];
    let ops = vec![PageOp::Create {
        source_ids: vec![],
        note_path: "learning/tokio-runtime".into(),
        title: "Tokio runtime".into(),
        summary: "async".into(),
        facts: vec!["event loop".into()],
        links: vec!["learning/rust".into()],
        tags: vec![],
        relations: vec![],
        confidence: 1.0,
        severity: Default::default(),
    }];
    let out = ing.dedup_redirect_creates("default", ops, &related).await;
    assert_eq!(out.len(), 1);
    match &out[0] {
        PageOp::Append {
            note_path,
            new_facts,
            new_links,
            ..
        } => {
            assert_eq!(note_path, "learning/tokio");
            assert!(new_facts.iter().any(|f| f.contains("event loop")));
            assert_eq!(new_links, &vec!["learning/rust".to_string()]);
        }
        // rust-doctor-disable-next-line panic-in-library
        other => panic!("expected Append redirect, got {other:?}"),
    }
}

/// Dedup is off by default → the planned `Create` passes through unchanged
/// even when an identical existing note is present (byte-identical ingest).
#[tokio::test]
async fn dedup_disabled_keeps_create_unchanged() {
    let (dir, backend, indexer) = mk().await;
    backend
        .upsert_embedding("learning/tokio", "default", &vec![0.1f32; 1024], 1024, "")
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(), // dedup_enabled = false
        embedding_manager: None,
        gate: None,
    };

    let related = vec![RelatedPage {
        path: "learning/tokio".into(),
        title: "tokio".into(),
        summary: String::new(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h".into(),
        score: 1.0,
    }];
    let ops = vec![PageOp::Create {
        source_ids: vec![],
        note_path: "learning/tokio-runtime".into(),
        title: "Tokio runtime".into(),
        summary: "async".into(),
        facts: vec!["event loop".into()],
        links: vec!["learning/rust".into()],
        tags: vec![],
        relations: vec![],
        confidence: 1.0,
        severity: Default::default(),
    }];
    let out = ing.dedup_redirect_creates("default", ops, &related).await;
    assert!(
        matches!(out[0], PageOp::Create { .. }),
        "dedup disabled must leave Create unchanged"
    );
}

/// A `Create` whose own path matches the only related page must NOT
/// self-redirect (that would turn a legitimate overwrite into a no-op
/// append against itself).
#[tokio::test]
async fn dedup_never_self_redirects() {
    let (dir, backend, indexer) = mk().await;
    backend
        .upsert_embedding("learning/tokio", "default", &vec![0.1f32; 1024], 1024, "")
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget {
            dedup_enabled: true,
            ..RelatedBudget::default()
        },
        embedding_manager: None,
        gate: None,
    };

    let related = vec![RelatedPage {
        path: "learning/tokio".into(),
        title: "tokio".into(),
        summary: String::new(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h".into(),
        score: 1.0,
    }];
    let ops = vec![PageOp::Create {
        source_ids: vec![],
        note_path: "learning/tokio".into(),
        title: "Tokio".into(),
        summary: "async".into(),
        facts: vec!["event loop".into()],
        links: vec!["learning/rust".into()],
        tags: vec![],
        relations: vec![],
        confidence: 1.0,
        severity: Default::default(),
    }];
    let out = ing.dedup_redirect_creates("default", ops, &related).await;
    assert!(
        matches!(out[0], PageOp::Create { .. }),
        "must not redirect a Create onto its own path"
    );
}

/// Stored vector with `m` entries of 0.1 and the rest 0.0. Its cosine vs the
/// mock candidate ([0.1; 1024]) is `0.03125 * sqrt(m)`, so `m` selects the
/// dedup tier: 100 → 0.3125 (ADD), 400 → 0.625, 900 → 0.9375 (MERGE),
/// 1024 → 1.0 (NOOP).
fn seed_with_cosine_entries(m: usize) -> Vec<f32> {
    let mut v = vec![0.1f32; m];
    v.extend(std::iter::repeat_n(0.0f32, 1024 - m));
    v
}

/// Run one Create through dedup against a single related page "learning/tokio"
/// seeded with `seed_vec`, under `budget`. The candidate path differs from the
/// related path (no self-skip), so the seed's cosine decides the tier.
async fn run_dedup_tier(seed_vec: Vec<f32>, budget: RelatedBudget) -> Vec<PageOp> {
    let (dir, backend, indexer) = mk().await;
    backend
        .upsert_embedding("learning/tokio", "default", &seed_vec, 1024, "")
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("{}".into()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget,
        embedding_manager: None,
        gate: None,
    };
    let related = vec![RelatedPage {
        path: "learning/tokio".into(),
        title: "tokio".into(),
        summary: String::new(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h".into(),
        score: 1.0,
    }];
    let ops = vec![PageOp::Create {
        source_ids: vec![],
        note_path: "learning/tokio-runtime".into(),
        title: "Tokio runtime".into(),
        summary: "async".into(),
        facts: vec!["event loop".into()],
        links: vec!["learning/rust".into()],
        tags: vec![],
        relations: vec![],
        confidence: 1.0,
        severity: Default::default(),
    }];
    ing.dedup_redirect_creates("default", ops, &related).await
}

fn budget_dedup_on() -> RelatedBudget {
    RelatedBudget {
        dedup_enabled: true,
        ..RelatedBudget::default()
    }
}

/// ADD tier: best match below `dedup_threshold` (cosine 0.3125 < 0.92) →
/// the Create passes through unchanged.
#[tokio::test]
async fn dedup_tier_add_keeps_create() {
    let out = run_dedup_tier(seed_with_cosine_entries(100), budget_dedup_on()).await;
    assert!(
        matches!(out.as_slice(), [PageOp::Create { .. }]),
        "below dedup_threshold the Create must survive as ADD"
    );
}

/// NOOP tier: best match at/above `noop_threshold` (cosine 1.0 >= 0.985) →
/// the Create is dropped entirely.
#[tokio::test]
async fn dedup_tier_noop_drops_create() {
    let out = run_dedup_tier(vec![0.1f32; 1024], budget_dedup_on()).await;
    assert!(
        out.is_empty(),
        "near-identical Create must be dropped as NOOP"
    );
}

/// FLOOR: `dedup_noop_threshold` misconfigured BELOW `dedup_similarity_threshold`.
/// `noop_threshold` is floored to `max(0.50, 0.92) = 0.92`, so a 0.625-cosine
/// match — which the raw 0.50 would have NOOP-dropped — stays an ADD Create.
/// The floor guarantees NOOP never fires below the MERGE threshold.
#[tokio::test]
async fn dedup_noop_floor_never_fires_below_merge() {
    let budget = RelatedBudget {
        dedup_enabled: true,
        dedup_similarity_threshold: 0.92,
        dedup_noop_threshold: 0.50,
        ..RelatedBudget::default()
    };
    let out = run_dedup_tier(seed_with_cosine_entries(400), budget).await;
    assert!(
        matches!(out.as_slice(), [PageOp::Create { .. }]),
        "noop floored to >= dedup_threshold, so a 0.625 match stays ADD, not NOOP"
    );
}

fn related_page(path: &str) -> RelatedPage {
    RelatedPage {
        path: path.to_string(),
        title: path.to_string(),
        summary: "a related page".into(),
        content_preview: String::new(),
        tags: vec![],
        content_hash: "h".into(),
        score: 0.5,
    }
}

fn linkless_create(path: &str) -> PageOp {
    PageOp::Create {
        source_ids: vec![],
        note_path: path.to_string(),
        title: "T".into(),
        summary: "S".into(),
        facts: vec!["f1".into()],
        links: vec![],
        tags: vec![],
        relations: vec![],
        confidence: 1.0,
        severity: Default::default(),
    }
}

fn mk_ingestor(
    canned: &str,
) -> (
    tempfile::TempDir,
    DefaultCompoundIngestor<SqliteMemoryBackend>,
) {
    // rust-doctor-disable-next-line unwrap-in-production
    let dir = tempfile::tempdir().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // rust-doctor-disable-next-line excessive-clone
    let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        store: backend,
        indexer,
        provider: Arc::new(RecordingMockProvider::new(canned.into())),
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    (dir, ing)
}

#[tokio::test]
async fn repairs_linkless_create_with_valid_token() {
    let (_d, ing) =
        mk_ingestor(r#"{"repairs":[{"note_index":0,"links":["[P1]"],"isolated":false}]}"#);
    let related = vec![related_page("learning/a"), related_page("learning/b")];
    let ops = ing
        .enforce_link_contract("default", vec![linkless_create("learning/new")], &related)
        .await;
    match &ops[0] {
        PageOp::Create { links, .. } => assert_eq!(links, &vec!["learning/b".to_string()]),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected create"),
    }
}

#[tokio::test]
async fn out_of_range_token_dropped_and_op_passes_through() {
    let (_d, ing) =
        mk_ingestor(r#"{"repairs":[{"note_index":0,"links":["[P9]"],"isolated":false}]}"#);
    let related = vec![related_page("learning/a")];
    let ops = ing
        .enforce_link_contract("default", vec![linkless_create("learning/new")], &related)
        .await;
    match &ops[0] {
        PageOp::Create { links, .. } => assert!(links.is_empty()),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected create"),
    }
}

#[tokio::test]
async fn explicit_isolation_is_accepted() {
    let (_d, ing) = mk_ingestor(r#"{"repairs":[{"note_index":0,"links":[],"isolated":true}]}"#);
    let related = vec![related_page("learning/a")];
    let ops = ing
        .enforce_link_contract("default", vec![linkless_create("learning/new")], &related)
        .await;
    match &ops[0] {
        PageOp::Create { links, .. } => assert!(links.is_empty()),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected create"),
    }
}

#[tokio::test]
async fn empty_related_skips_repair_entirely() {
    // The canned response is a valid repair — if the gate wrongly fired
    // and applied it, links would no longer be empty and this fails.
    let (_d, ing) =
        mk_ingestor(r#"{"repairs":[{"note_index":0,"links":["[P0]"],"isolated":false}]}"#);
    let ops = ing
        .enforce_link_contract("default", vec![linkless_create("learning/new")], &[])
        .await;
    match &ops[0] {
        PageOp::Create { links, .. } => assert!(links.is_empty()),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected create"),
    }
}

#[tokio::test]
async fn malformed_llm_response_passes_through() {
    let (_d, ing) = mk_ingestor("not json at all");
    let related = vec![related_page("learning/a")];
    let ops = ing
        .enforce_link_contract("default", vec![linkless_create("learning/new")], &related)
        .await;
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        PageOp::Create { links, .. } => assert!(links.is_empty()),
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected create"),
    }
}

/// When the embedding-derived `related` set is EMPTY (sparse wiki or
/// embedding down), `enforce_link_contract` must fall back to keyword-
/// overlap linking via FTS candidates instead of leaving the create an
/// orphan. Here an existing note (`personal/news-monitoring`) is indexed
/// into FTS under the non-default agent `main`; the provider returns
/// keyword sets whose specific entity (`us-iran-conflict`) overlaps the new
/// note, so the create gains a link. Using `main` (not `default`) proves
/// the fallback is scoped to the ingesting agent, where the live orphan
/// problem was observed.
#[tokio::test]
async fn enforce_link_contract_links_via_keywords_when_related_empty() {
    use crate::memory::notes::store::NoteStore;

    const AGENT: &str = "main";

    // rust-doctor-disable-next-line unwrap-in-production
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = dir.path().join("note");
    // rust-doctor-disable-next-line unwrap-in-production
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    // rust-doctor-disable-next-line excessive-clone
    let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

    // Seed an existing note so FTS has a candidate. Its body mentions the
    // shared entity so a per-keyword FTS probe ("monitoring") finds it.
    let cat_dir = memory_dir.join(AGENT).join("personal");
    // rust-doctor-disable-next-line unwrap-in-production
    tokio::fs::create_dir_all(&cat_dir).await.unwrap();
    let seed_path = cat_dir.join("news-monitoring.md");
    tokio::fs::write(
        &seed_path,
        "---\ncategory: personal\ntags: [news]\n---\n\n- US-Iran conflict monitoring via cron\n",
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();
    indexer
        .index_file(AGENT, "personal", &seed_path)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // Sanity: the seed must be FTS-queryable under `main`, else the test
    // proves nothing.
    let hits = backend
        .search_notes_fts("monitoring", AGENT, 3)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(
        hits.iter().any(|h| h.path == "personal/news-monitoring"),
        "seed note must be FTS-indexed under the ingesting agent before the test body"
    );
    // Negative control: the same note must NOT be visible under `default`,
    // proving FTS candidates are agent-scoped.
    let default_hits = backend
        .search_notes_fts("monitoring", "default", 3)
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    assert!(
        default_hits.is_empty(),
        "seed note must not leak into the default agent's FTS scope"
    );

    // Provider returns keyword sets with an overlapping specific entity for
    // both notes (the extraction call in the empty-related branch).
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"notes":[
            {"path":"entity/us-iran-conflict-2026","keywords":["us-iran-conflict","monitoring"]},
            {"path":"personal/news-monitoring","keywords":["us-iran-conflict","cron"]}
        ]}"#
        .into(),
    ));
    let ing = DefaultCompoundIngestor {
        tx_residue_gc_seconds: 3600,
        // rust-doctor-disable-next-line excessive-clone
        store: backend.clone(),
        indexer,
        provider,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        // rust-doctor-disable-next-line excessive-clone
        memory_dir: memory_dir.clone(),
        budget: RelatedBudget::default(),
        embedding_manager: None,
        gate: None,
    };
    let ops = vec![PageOp::Create {
        source_ids: vec![],
        note_path: "entity/us-iran-conflict-2026".into(),
        title: "US-Iran Conflict".into(),
        summary: "tensions monitored".into(),
        facts: vec!["US-Iran conflict monitoring".into()],
        links: vec![],
        tags: vec![],
        relations: vec![],
        confidence: 1.0,
        severity: Default::default(),
    }];
    // related is EMPTY → fallback path, scoped to AGENT ("main").
    let out = ing.enforce_link_contract(AGENT, ops, &[]).await;
    match &out[0] {
        PageOp::Create { links, .. } => {
            assert!(
                links.iter().any(|l| l == "personal/news-monitoring"),
                "keyword overlap must link the create even with empty related; got {links:?}"
            );
        }
        // rust-doctor-disable-next-line panic-in-library
        other => panic!("expected create, got {other:?}"),
    }
}

#[tokio::test]
async fn already_linked_create_not_touched() {
    let (_d, ing) =
        mk_ingestor(r#"{"repairs":[{"note_index":0,"links":["[P0]"],"isolated":false}]}"#);
    let related = vec![related_page("learning/a")];
    let mut op = linkless_create("learning/new");
    if let PageOp::Create { links, .. } = &mut op {
        links.push("learning/existing".into());
    }
    let ops = ing
        .enforce_link_contract("default", vec![op], &related)
        .await;
    match &ops[0] {
        PageOp::Create { links, .. } => {
            assert_eq!(links, &vec!["learning/existing".to_string()])
        }
        // rust-doctor-disable-next-line panic-in-library
        _ => panic!("expected create"),
    }
}

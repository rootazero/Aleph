//! `CompoundIngestor` trait + `DefaultCompoundIngestor` impl.
//!
//! Trait-only in this file so far; the production impl `DefaultCompoundIngestor`
//! is added in Spec 6 T7+T8.

use crate::error::AlephError;
use crate::memory::notes::ingest::apply::{ApplyError, CompoundApplyTx};
use crate::memory::notes::ingest::plan::ApplyReport;
use crate::memory::notes::ingest::retrieve::gather_related;
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;

use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::ingest::plan::{IngestPlan, PageOp};
use crate::memory::notes::ingest::prompts::build_compound_system_prompt;
use crate::memory::notes::ingest::retrieve::{RelatedBudget, RelatedPage};
use crate::memory::notes::orientation::NoteOrientation;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemorySource;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::utils::json_extract::extract_json_robust;
use std::path::PathBuf;
use tracing::warn;

pub struct DefaultCompoundIngestor<S: NoteStore + Send + Sync + 'static> {
    pub store: Arc<S>,
    pub indexer: Arc<NoteIndexer<S>>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub orientation: Option<Arc<dyn NoteOrientation>>,
    pub memory_dir: PathBuf,
    pub budget: RelatedBudget,
}

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    pub async fn plan(
        &self,
        _agent_id: &str,
        raws: &[crate::memory::store::raw_memory::RawMemory],
        related: &[RelatedPage],
        source: &RawMemorySource,
    ) -> Result<IngestPlan, AlephError> {
        if raws.is_empty() {
            return Ok(IngestPlan {
                reasoning: String::new(),
                ops: vec![],
                schema_proposals: vec![],
            });
        }

        let system = build_compound_system_prompt(source);
        let user = build_user_prompt(raws, related);
        let msgs = [UnifiedMessage::user(&user)];
        let resp = self
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(&system)))
            .await
            .map_err(|e| AlephError::other(format!("compound plan LLM: {e}")))?;
        let text = resp.text_content();

        let json = match extract_json_robust(&text) {
            Some(v) => v,
            None => {
                warn!("compound plan: no JSON in LLM response; returning empty plan");
                return Ok(IngestPlan {
                    reasoning: String::new(),
                    ops: vec![],
                    schema_proposals: vec![],
                });
            }
        };

        // Defensive: drop ops missing the `kind` discriminator before strict
        // parsing. The LLM occasionally omits it despite the prompt; rather
        // than failing the whole batch (which leaves the raws unprocessed
        // forever), we silently skip the malformed op and proceed with the
        // rest of the plan.
        let json = strip_kindless_ops(json);

        let mut plan: IngestPlan = serde_json::from_value(json).map_err(|e| {
            warn!("compound plan: parse failed: {e}");
            AlephError::other(format!("compound plan parse: {e}"))
        })?;

        plan.ops.retain(valid_op);
        Ok(plan)
    }
}

/// Build an `IngestBatchSummary` from an `ApplyReport` by aggregating
/// `touched_paths` (each formatted as `"{category}/{filename}"`) into per-
/// category counts.
///
/// `ApplyReport` does not split per-path created vs updated, so `added` here
/// is conservatively the *total* touched-path count for the category and
/// `updated` is left at 0. This is sufficient for cadence-only consumers
/// (e.g. `refresh_index_after_ingest`); finer-grained breakdowns belong with
/// the planner once `ApplyReport` learns to track them per op.
fn summary_from_report(
    agent_id: &str,
    report: &ApplyReport,
) -> crate::memory::notes::orientation::types::IngestBatchSummary {
    use crate::memory::notes::orientation::types::{IngestBatchSummary, TouchedCategory};
    use std::collections::BTreeMap;

    let mut by_cat: BTreeMap<String, u32> = BTreeMap::new();
    for path in &report.touched_paths {
        if let Some((cat, _name)) = path.split_once('/') {
            *by_cat.entry(cat.to_string()).or_insert(0) += 1;
        }
    }

    IngestBatchSummary {
        agent_id: agent_id.to_string(),
        touched: by_cat
            .into_iter()
            .map(|(category, count)| TouchedCategory {
                category,
                added: count,
                updated: 0,
            })
            .collect(),
    }
}

fn strip_kindless_ops(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        if let Some(ops) = obj.get_mut("ops") {
            if let Some(arr) = ops.as_array_mut() {
                arr.retain(|op| {
                    op.as_object()
                        .and_then(|o| o.get("kind"))
                        .and_then(|k| k.as_str())
                        .is_some()
                });
            }
        }
    }
    value
}

#[async_trait]
impl<S: NoteStore + Send + Sync + 'static> CompoundIngestor for DefaultCompoundIngestor<S> {
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<crate::memory::store::raw_memory::RawMemory>,
    ) -> Result<ApplyReport, AlephError> {
        if raws.is_empty() {
            return Ok(ApplyReport::default());
        }
        // G2 fix: ensure the agent's orientation files (SCHEMA.md, index.md,
        // log.md) exist before we touch any notes. Dynamically-created agents
        // never get the startup-time bootstrap that the default agent gets.
        // The bootstrap is idempotent and cheap (file existence check + a
        // single write of minimal markdown) so we can call it every batch.
        if let Some(orient) = &self.orientation {
            if let Err(e) = orient.bootstrap(agent_id).await {
                warn!("orientation bootstrap for {agent_id} failed (continuing): {e}");
            }
        }
        let source = raws[0].source.clone();
        let related = gather_related(
            self.store.clone(),
            self.embedder.clone(),
            &raws,
            agent_id,
            &self.budget,
        )
        .await?;

        let plan = self.plan(agent_id, &raws, &related, &source).await?;
        if plan.ops.is_empty() {
            return Ok(ApplyReport::default());
        }

        let report = match self.try_apply(agent_id, &plan).await {
            Ok(r) => r,
            Err(ApplyError::HashConflict { path, actual, .. }) => {
                warn!("compound ingest: hash conflict on {path}; re-planning");
                let mut augmented = raws.clone();
                if let Some(last) = augmented.last_mut() {
                    last.content.push_str(&format!(
                        "\n\n[system] previous plan referenced {path} with a stale hash; actual hash is {actual}. Re-plan using fresh data."
                    ));
                }
                let plan2 = self.plan(agent_id, &augmented, &related, &source).await?;
                if plan2.ops.is_empty() {
                    return Ok(ApplyReport::default());
                }
                self.try_apply(agent_id, &plan2)
                    .await
                    .map_err(|e| match e {
                        ApplyError::Other(inner) => inner,
                        other => AlephError::other(format!("apply after re-plan: {other}")),
                    })?
            }
            Err(ApplyError::Other(e)) => return Err(e),
        };

        if let Some(orient) = &self.orientation {
            let reasoning_preview: String = plan.reasoning.chars().take(80).collect();
            let detail: Vec<String> = report
                .touched_paths
                .iter()
                .take(15)
                .map(|p| format!("touched {p}"))
                .collect();
            let entry = crate::memory::notes::orientation::types::LogEntry {
                timestamp_utc: chrono::Utc::now().timestamp(),
                action: crate::memory::notes::orientation::types::LogAction::Ingest,
                summary: format!(
                    "{} pages touched | tx={} | {}",
                    report.touched_paths.len(),
                    report.tx_id,
                    reasoning_preview
                ),
                detail_lines: detail,
            };
            if let Err(e) = orient.record_ingest(agent_id, entry).await {
                warn!("compound ingest: log record failed: {e}");
            }
        }

        // Forward-compatible: tell orientation which categories were touched in
        // this batch so it can refresh `index.md` immediately. Best-effort —
        // failures are logged and ignored so the ingest still returns success;
        // the next dream cycle's full `rebuild_index` will reconcile.
        if let Some(orient) = &self.orientation {
            let summary = summary_from_report(agent_id, &report);
            if !summary.touched.is_empty() {
                if let Err(e) = orient.refresh_index_after_ingest(agent_id, &summary).await {
                    warn!(
                        "ingest_batch: refresh_index_after_ingest failed (non-fatal); \
                         next dream cycle will reconcile: {e}"
                    );
                }
            }
        }

        Ok(report)
    }
}

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    async fn try_apply(
        &self,
        agent_id: &str,
        plan: &IngestPlan,
    ) -> Result<ApplyReport, ApplyError> {
        let mut tx = CompoundApplyTx::new(
            &self.indexer,
            &self.store,
            self.memory_dir.clone(),
            agent_id,
        );
        for op in &plan.ops {
            tx.stage(op).await?;
        }
        tx.commit().await
    }
}

fn build_user_prompt(
    raws: &[crate::memory::store::raw_memory::RawMemory],
    related: &[RelatedPage],
) -> String {
    let mut out = String::from("## New raw memories\n\n");
    for (i, r) in raws.iter().enumerate() {
        out.push_str(&format!(
            "### raw-{} (id={}, source={:?})\n",
            i + 1,
            r.id,
            r.source
        ));
        out.push_str(&r.content);
        out.push_str("\n\n");
        if let Some(att) = &r.attachment_text {
            out.push_str("[Attachment]\n");
            out.push_str(att);
            out.push_str("\n\n");
        }
    }
    if !related.is_empty() {
        out.push_str("## Related existing pages\n\n");
        for p in related {
            out.push_str(&format!(
                "### {path} (hash={hash})\n",
                path = p.path,
                hash = p.content_hash
            ));
            out.push_str(&format!("title: {}\n", p.title));
            if !p.tags.is_empty() {
                out.push_str(&format!("tags: {}\n", p.tags.join(", ")));
            }
            out.push_str("preview:\n");
            out.push_str(&p.content_preview);
            out.push_str("\n\n");
        }
    } else {
        out.push_str("## Related existing pages\n\n(none — empty wiki or no matches)\n");
    }
    out.push_str("Produce the IngestPlan JSON now.");
    out
}

fn valid_op(op: &PageOp) -> bool {
    match op {
        PageOp::Create {
            note_path, links, ..
        } => note_path.contains('/') && !links.is_empty(),
        PageOp::Append { note_path, .. }
        | PageOp::Update { note_path, .. }
        | PageOp::Contradict { note_path, .. } => note_path.contains('/'),
        PageOp::Link { from, to } => from.contains('/') && to.contains('/') && from != to,
        PageOp::Supersede { old_path, new_path } => {
            old_path.contains('/') && new_path.contains('/') && old_path != new_path
        }
    }
}

#[async_trait]
pub trait CompoundIngestor: Send + Sync {
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<RawMemory>,
    ) -> Result<ApplyReport, AlephError>;
}

#[cfg(test)]
mod trait_tests {
    use super::*;

    struct StubIngestor;

    #[async_trait]
    impl CompoundIngestor for StubIngestor {
        async fn ingest_batch(
            &self,
            _agent_id: &str,
            _raws: Vec<RawMemory>,
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
        let r = ing.ingest_batch("default", vec![]).await.unwrap();
        assert_eq!(r.tx_id, "stub");
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::recording_mock::RecordingMockProvider;

    async fn mk() -> (
        tempfile::TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
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
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let raw = RawMemory::new("some content".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript)
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1);
        match &plan.ops[0] {
            PageOp::Create { note_path, .. } => assert_eq!(note_path, "learning/tokio"),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn plan_returns_empty_on_invalid_json() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new("not json".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript)
            .await
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
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript)
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1);
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

        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("note");
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), backend.clone()));

        let orient: Arc<dyn NoteOrientation> = Arc::new(FsNoteOrientation::new(
            memory_dir.clone(),
            backend.clone(),
        ));
        // bootstrap is also done by ingest_batch, but doing it here gives a
        // pre-ingest baseline for index.md so we can prove the refresh fires.
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
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: Some(orient.clone()),
            memory_dir: memory_dir.clone(),
            budget: RelatedBudget::default(),
        };

        let raw = RawMemory::new("c".to_string(), RawMemorySource::Transcript);
        let report = ing.ingest_batch("default", vec![raw]).await.unwrap();
        assert_eq!(report.created, 1);

        let index_md_path = memory_dir.join("default").join("index.md");
        assert!(
            index_md_path.exists(),
            "index.md must exist after ingest_batch"
        );
        let body = std::fs::read_to_string(&index_md_path).unwrap();
        assert!(
            body.contains("preference"),
            "index.md must list the touched 'preference' category; got:\n{body}"
        );
    }

    #[tokio::test]
    async fn end_to_end_append_on_existing() {
        use crate::memory::notes::ingest::plan::PageOp;

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
            store: backend.clone(),
            indexer: indexer.clone(),
            provider: provider_seed,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let r1 = ing_seed
            .ingest_batch(
                "default",
                vec![RawMemory::new(
                    "seed".to_string(),
                    RawMemorySource::Transcript,
                )],
            )
            .await
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
            store: backend.clone(),
            indexer: indexer.clone(),
            provider: provider2,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let r2 = ing2
            .ingest_batch(
                "default",
                vec![RawMemory::new(
                    "body2".to_string(),
                    RawMemorySource::Transcript,
                )],
            )
            .await
            .unwrap();
        assert_eq!(r2.appended, 1);

        let body =
            tokio::fs::read_to_string(dir.path().join("note/default/learning/rust-async.md"))
                .await
                .unwrap();
        assert!(body.contains("Futures are lazy"));
        assert!(body.contains("tokio is the runtime"));
    }
}

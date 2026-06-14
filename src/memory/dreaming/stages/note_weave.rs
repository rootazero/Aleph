//! `NoteWeave` stage — backfill orphan notes into the wiki link graph by
//! keyword overlap.
//!
//! An orphan (zero outgoing AND zero incoming links) is invisible to graph
//! expansion in `gather_related`, earns no `link_weight` in `NoteDecay`
//! scoring, and is therefore archived early — a vicious cycle this stage
//! breaks. This is the BACKFILL engine for existing orphans: one batched LLM
//! call extracts a keyword set per note (R7 — the model names the entities),
//! then deterministic set-overlap pairing (`pair_by_overlap`, no LLM) decides
//! who links to whom. Each emitted pair touching an orphan is written through
//! `NoteIndexer::append_to_note` in both directions (the body `[[ ]]` link,
//! same primitive `CompoundApplyTx::add_link` uses), then
//! `NoteStore::add_link_with_relation` stamps the connecting keyword onto the
//! now-existing `notes_links` row.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::keyword_linker::{
    extract_keywords, pair_by_overlap, LinkTriple, NoteForExtraction,
};
use crate::memory::notes::store::NoteStore;

use super::DreamStage;

/// Max keyword-overlap links written per dream cycle (caps disk writes).
const MAX_WEAVE_PER_CYCLE: usize = 10;

/// `NoteWeave` stage. `max_per_cycle` caps links written per dream cycle.
pub struct NoteWeaveStage {
    pub max_per_cycle: usize,
}

impl Default for NoteWeaveStage {
    fn default() -> Self {
        Self {
            max_per_cycle: MAX_WEAVE_PER_CYCLE,
        }
    }
}

#[async_trait]
impl DreamStage for NoteWeaveStage {
    fn name(&self) -> &'static str {
        "note_weave"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // --- Phase 1: orphan detection (store queries only, no LLM) ---
        // Orphans are the relink targets; non-orphans are still extracted so an
        // orphan can attach to an already-linked note via a shared entity.
        let mut orphans: Vec<String> = Vec::new();
        let mut others: Vec<String> = Vec::new();
        for note in &ctx.notes {
            let Some((_, filename)) = note.path.split_once('/') else {
                continue;
            };
            let outgoing = ctx
                .indexer
                .store()
                .get_outgoing_links(&note.path, &ctx.agent_id)
                .await
                .unwrap_or_default();
            // `notes_links.to_note` is the resolved target — a full path when
            // the wikilink filename uniquely resolved, otherwise the bare text.
            // Match either so resolved and legacy rows both count; querying by
            // filename alone (the old code) never matched a full-path to_note,
            // so incoming-only notes were wrongly seen as orphans every cycle.
            let incoming = ctx
                .indexer
                .store()
                .get_incoming_links_any(&note.path, filename, &ctx.agent_id)
                .await
                .unwrap_or_default();
            if outgoing.is_empty() && incoming.is_empty() {
                orphans.push(note.path.clone());
            } else {
                others.push(note.path.clone());
            }
        }
        if orphans.is_empty() {
            info!("NoteWeave: no orphan notes");
            return Ok(ctx);
        }
        info!(count = orphans.len(), "NoteWeave: found orphan notes");

        // --- Phase 2: extract keyword sets (one batched LLM call). Orphans
        // first so the prompt foregrounds the notes we want relinked. ---
        let inputs: Vec<NoteForExtraction> = orphans
            .iter()
            .chain(others.iter())
            .map(|path| build_extraction_input(path))
            .collect();
        let keywords = extract_keywords(ctx.provider.as_ref(), &inputs).await?;
        if keywords.is_empty() {
            info!("NoteWeave: no keywords extracted");
            ctx.report.notes_woven = 0;
            return Ok(ctx);
        }

        // --- Phase 3: deterministic pairing (no LLM). Keep only pairs that
        // touch an orphan — this stage's job is to relink orphans, not to
        // densify the already-connected graph. ---
        let orphan_set: std::collections::HashSet<&str> =
            orphans.iter().map(String::as_str).collect();
        let links: Vec<LinkTriple> = pair_by_overlap(&keywords)
            .into_iter()
            .filter(|l| orphan_set.contains(l.from.as_str()) || orphan_set.contains(l.to.as_str()))
            .take(self.max_per_cycle)
            .collect();

        // --- Phase 4: write each pair bidirectionally. Body link FIRST
        // (append_to_note creates the NULL-relation row), THEN
        // add_link_with_relation stamps the keyword. Each direction is
        // independently `.ok()`-guarded: a reverse-write failure must not skip
        // remaining pairs nor strip the forward link already on disk. ---
        let mut woven = 0u32;
        for LinkTriple { from, to, relation } in links {
            write_typed_link(&mut ctx, &from, &to, &relation).await;
            write_typed_link(&mut ctx, &to, &from, &relation).await;
            // Evict cached bodies so downstream stages re-read updated markdown.
            ctx.note_contents.remove(&from);
            ctx.note_contents.remove(&to);
            woven += 1;
        }
        ctx.report.notes_woven = woven;
        info!(woven, "NoteWeave completed");
        Ok(ctx)
    }
}

/// Build a `NoteForExtraction` for a note. The index entry carries no summary
/// or facts, and reading every body would be wasteful here, so we derive the
/// title from the filename and leave summary/facts empty — the keyword
/// extractor works fine from path + title (mirrors how Task 5 built
/// candidates).
fn build_extraction_input(path: &str) -> NoteForExtraction {
    let title = path
        .rsplit_once('/')
        .map_or(path, |(_, f)| f)
        .replace('-', " ");
    NoteForExtraction {
        path: path.to_string(),
        title,
        summary: String::new(),
        facts: Vec::new(),
    }
}

/// Write one directed typed link: body `[[ ]]` link first (creates the
/// `notes_links` row with NULL relation), then stamp the relation on it. Both
/// steps are `.ok()`-guarded so a failure leaves the graph no worse and never
/// aborts the surrounding loop (per-direction fault tolerance).
async fn write_typed_link(ctx: &mut DreamContext, from: &str, to: &str, relation: &str) {
    let to_link = [to.to_string()];
    if let Err(e) = ctx
        .indexer
        .append_to_note(&ctx.agent_id, from, &Vec::<String>::new(), &to_link)
        .await
    {
        warn!(from = %from, to = %to, error = %e, "NoteWeave: body link write failed");
        return;
    }
    if let Err(e) = ctx
        .indexer
        .store()
        .add_link_with_relation(&ctx.agent_id, from, to, relation)
        .await
    {
        warn!(from = %from, to = %to, error = %e, "NoteWeave: relation stamp failed (body link kept)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name_is_note_weave() {
        assert_eq!(NoteWeaveStage::default().name(), "note_weave");
    }

    #[test]
    fn default_cap_is_ten() {
        assert_eq!(NoteWeaveStage::default().max_per_cycle, 10);
    }

    // -----------------------------------------------------------------
    // Stage-level tests: orphan detection against a real SQLite store.
    // Fixture mirrors note_lint.rs::build_test_dream_ctx.
    // -----------------------------------------------------------------

    use crate::memory::dreaming::{DreamContext, NoteEntry};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::{KnowledgeNote, NoteIndexer};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;
    use crate::sync_primitives::Arc;

    struct StubEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    async fn build_ctx(llm_response: &str) -> (DreamContext, Arc<SqliteMemoryBackend>) {
        let temp = std::env::temp_dir().join(format!("aleph_weave_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(llm_response));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: None,
        };
        (ctx, store)
    }

    fn entry(path: &str) -> NoteEntry {
        let (category, _) = path.split_once('/').unwrap();
        NoteEntry {
            path: path.into(),
            category: category.into(),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: "h".into(),
        }
    }

    #[tokio::test]
    async fn linked_note_is_not_treated_as_orphan() {
        let (mut ctx, store) = build_ctx("{\"links\": []}").await;
        // a links to b → neither is an orphan.
        store
            .index_note(
                &KnowledgeNote {
                    title: "a".into(),
                    category: "learning".into(),
                    facts: vec!["see [[b]]".into()],
                    links: vec!["learning/b".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        store
            .index_note(
                &KnowledgeNote {
                    title: "b".into(),
                    category: "learning".into(),
                    facts: vec!["fact".into()],
                    content_hash: "h2".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("learning/a"), entry("learning/b")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        // No orphans → no weave writes recorded.
        assert_eq!(out.report.notes_woven, 0);
    }

    #[tokio::test]
    async fn note_weave_links_orphans_by_keyword_overlap() {
        use crate::providers::recording_mock::RecordingMockProvider;
        // Two orphan notes that share a specific entity keyword. The keyword
        // provider returns overlapping sets keyed by path; pair_by_overlap then
        // links them on the shared specific entity.
        let llm = r#"{"notes":[
            {"path":"entity/us-iran-conflict","keywords":["us-iran-conflict","ceasefire"]},
            {"path":"personal/news-monitoring","keywords":["us-iran-conflict","cron"]}
        ]}"#;
        // Create the dir first so SqliteMemoryBackend nests memory.db *inside*
        // it (db_path.is_dir() branch) — otherwise `temp` itself becomes the DB
        // file and append_to_note can't mkdir `temp/<agent>/<cat>/`.
        let temp = std::env::temp_dir().join(format!("aleph_weave_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(RecordingMockProvider::new(llm.into()));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let mut ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: None,
        };

        for (cat, title) in [
            ("entity", "us-iran-conflict"),
            ("personal", "news-monitoring"),
        ] {
            store
                .index_note(
                    &KnowledgeNote {
                        title: title.into(),
                        category: cat.into(),
                        facts: vec!["isolated fact".into()],
                        content_hash: format!("h-{title}"),
                        ..Default::default()
                    },
                    "default",
                    cat,
                )
                .await
                .unwrap();
        }
        ctx.notes = vec![
            entry("entity/us-iran-conflict"),
            entry("personal/news-monitoring"),
        ];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();

        // One undirected pair → bidirectional typed links written.
        assert_eq!(out.report.notes_woven, 1);
        let typed = store
            .get_typed_relations("entity/us-iran-conflict", "default")
            .await
            .unwrap();
        assert!(
            typed
                .iter()
                .any(|(to, rel)| to == "personal/news-monitoring" && rel == "us-iran-conflict"),
            "forward typed relation missing: {typed:?}"
        );
        let back = store
            .get_typed_relations("personal/news-monitoring", "default")
            .await
            .unwrap();
        assert!(
            back.iter()
                .any(|(to, rel)| to == "entity/us-iran-conflict" && rel == "us-iran-conflict"),
            "backward typed relation missing: {back:?}"
        );
    }

    #[tokio::test]
    async fn lone_orphan_has_no_overlap_partner_so_nothing_woven() {
        use crate::providers::recording_mock::RecordingMockProvider;
        // A single orphan: even with extracted keywords there is no second note
        // to pair against → pair_by_overlap yields nothing → zero woven.
        let temp = std::env::temp_dir().join(format!("aleph_weave_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(RecordingMockProvider::new(
                r#"{"notes":[{"path":"learning/lonely","keywords":["solo-topic"]}]}"#.into(),
            ));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let mut ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: None,
        };
        store
            .index_note(
                &KnowledgeNote {
                    title: "lonely".into(),
                    category: "learning".into(),
                    facts: vec!["isolated fact".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("learning/lonely")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        assert_eq!(out.report.notes_woven, 0);
    }

    #[tokio::test]
    async fn incoming_only_note_is_not_orphan() {
        let (mut ctx, store) = build_ctx("{\"links\": []}").await;
        // `src` links to `target` (full-path to_note); target has no outgoing link.
        store
            .index_note(
                &KnowledgeNote {
                    title: "src".into(),
                    category: "notes".into(),
                    facts: vec!["see [[target]]".into()],
                    links: vec!["reference/target".into()],
                    content_hash: "hs".into(),
                    ..Default::default()
                },
                "default",
                "notes",
            )
            .await
            .unwrap();
        store
            .index_note(
                &KnowledgeNote {
                    title: "target".into(),
                    category: "reference".into(),
                    facts: vec!["fact".into()],
                    content_hash: "ht".into(),
                    ..Default::default()
                },
                "default",
                "reference",
            )
            .await
            .unwrap();
        // Only `target` is walked this cycle. It has an incoming link, so it must
        // NOT be treated as an orphan -> nothing woven.
        ctx.notes = vec![entry("reference/target")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        assert_eq!(
            out.report.notes_woven, 0,
            "incoming-only note must not be re-wove as an orphan"
        );
    }

    #[tokio::test]
    async fn incoming_only_note_excluded_even_with_overlap_partner() {
        // LLM keyword extraction returns overlapping specific entities for the
        // incoming-only `target` and a non-orphan `partner`. Under the old
        // bare-filename incoming query, `target` was misclassed as an orphan and
        // the pair was woven (notes_woven == 1). With the full-path-aware query,
        // `target` is correctly non-orphan, `partner` is non-orphan, so there are
        // no orphans and nothing is woven.
        let llm = r#"{"notes":[
            {"path":"reference/target","keywords":["shared-topic"]},
            {"path":"personal/partner","keywords":["shared-topic"]}
        ]}"#;
        let (mut ctx, store) = build_ctx(llm).await;

        // `src` gives `target` an incoming full-path link; `src` itself is not walked.
        store
            .index_note(
                &KnowledgeNote {
                    title: "src".into(),
                    category: "notes".into(),
                    facts: vec!["see [[target]]".into()],
                    links: vec!["reference/target".into()],
                    content_hash: "hs".into(),
                    ..Default::default()
                },
                "default",
                "notes",
            )
            .await
            .unwrap();
        // `target`: incoming-only (no outgoing link).
        store
            .index_note(
                &KnowledgeNote {
                    title: "target".into(),
                    category: "reference".into(),
                    facts: vec!["fact".into()],
                    content_hash: "ht".into(),
                    ..Default::default()
                },
                "default",
                "reference",
            )
            .await
            .unwrap();
        // `partner`: non-orphan via its own outgoing link to a third note.
        store
            .index_note(
                &KnowledgeNote {
                    title: "partner".into(),
                    category: "personal".into(),
                    facts: vec!["see [[third]]".into()],
                    links: vec!["notes/third".into()],
                    content_hash: "hp".into(),
                    ..Default::default()
                },
                "default",
                "personal",
            )
            .await
            .unwrap();

        ctx.notes = vec![entry("reference/target"), entry("personal/partner")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        assert_eq!(
            out.report.notes_woven, 0,
            "incoming-only note with a non-orphan partner must not be woven"
        );
    }
}

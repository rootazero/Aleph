//! `MentionWeave` stage — materialize unlinked-mention soft edges (spec M1).
//!
//! A note body mentioning another note's filename/alias without a real
//! `[[wikilink]]` earns a `mention` edge (confidence 0.35). Deterministic
//! scan, zero LLM, bodies never modified (D2). Full refresh per cycle —
//! reconcile-wiped rows re-materialize next cycle (accepted eventual
//! consistency, same as co_recalled).

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::links::mentions::{scan_mentions, MentionDoc};
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::KnowledgeNote;

use super::DreamStage;

/// Cycle-wide cap on materialized mention edges (pathological-corpus guard).
const MAX_MENTIONS_PER_CYCLE: usize = 200;

pub struct MentionWeaveStage;

#[async_trait]
impl DreamStage for MentionWeaveStage {
    fn name(&self) -> &'static str {
        "mention_weave"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= 2
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let store = ctx.indexer.store().clone();
        let agent_id = ctx.agent_id.clone();

        let hydrated = match async {
            let entries = store.list_notes(&agent_id).await?;
            let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
            store.get_notes_with_content(&agent_id, &paths).await
        }
        .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "mention_weave: body load failed, skipping cycle");
                return Ok(ctx);
            }
        };

        // Parse + scan off the async runtime (CPU-bound over the whole corpus).
        let mut edges = tokio::task::spawn_blocking(move || {
            let docs: Vec<MentionDoc> = hydrated
                .into_iter()
                .filter_map(|r| {
                    let note = KnowledgeNote::from_markdown(&r.filename, &r.content).ok()?;
                    let mut names = vec![note.title.clone()];
                    names.extend(note.aliases.iter().cloned());
                    Some(MentionDoc {
                        path: r.path,
                        names,
                        body: note.body_text(),
                        linked_raw: note.links.clone(),
                    })
                })
                .collect();
            scan_mentions(&docs)
        })
        .await
        .map_err(|e| AlephError::other(format!("mention_weave join: {e}")))?;

        if edges.len() > MAX_MENTIONS_PER_CYCLE {
            tracing::info!(
                dropped = edges.len() - MAX_MENTIONS_PER_CYCLE,
                "mention_weave: per-cycle cap applied"
            );
            edges.truncate(MAX_MENTIONS_PER_CYCLE);
        }
        let edge_count = edges.len();
        store.replace_mention_links(&agent_id, &edges).await?;

        ctx.report
            .extra
            .insert("mention_edges".into(), edge_count.to_string());
        tracing::info!(agent = %agent_id, edges = edge_count, "mention edges materialized");
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name_is_mention_weave() {
        assert_eq!(MentionWeaveStage.name(), "mention_weave");
    }

    // -----------------------------------------------------------------
    // Stage-level tests against a real SQLite store. Fixture mirrors
    // note_lint.rs::build_test_dream_ctx.
    // -----------------------------------------------------------------

    use crate::memory::dreaming::{DreamContext, NoteEntry};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::NoteIndexer;
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

    /// Build a `DreamContext` over a fresh SQLite store. `NoteIndexer`'s
    /// `memory_dir` here is the store's own throwaway temp dir — fine for
    /// `should_run`/pure checks, but NOT for exercising `execute()` (which
    /// reads note bodies via `get_notes_with_content`, itself rooted at the
    /// process-global `ALEPH_HOME`, independent of this dir). The one test
    /// below that needs real body content builds its own ctx inline instead.
    async fn build_test_dream_ctx() -> (DreamContext, Arc<SqliteMemoryBackend>) {
        let temp =
            std::env::temp_dir().join(format!("aleph_mention_weave_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(""));
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
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (ctx, store)
    }

    #[tokio::test]
    async fn should_run_requires_at_least_two_notes() {
        let (mut ctx, _store) = build_test_dream_ctx().await;
        ctx.notes = vec![entry("a/only")];
        assert!(!MentionWeaveStage.should_run(&ctx).await);
        ctx.notes.push(entry("b/other"));
        assert!(MentionWeaveStage.should_run(&ctx).await);
    }

    // `get_notes_with_content`'s default impl hydrates bodies from
    // `crate::utils::paths::get_note_memory_dir()`, which resolves off the
    // process-global `ALEPH_HOME` — not the store's own db path. The named
    // `aleph_home_env` serial group (established in
    // `gateway::handlers::search_config` / `fetch_config`) coordinates this
    // mutation crate-wide so no other test observes a torn override.
    #[tokio::test]
    #[serial_test::serial(aleph_home_env)]
    async fn mention_weave_materializes_unlinked_mention_edges() {
        let home = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("ALEPH_HOME");
        // SAFETY: serialized by the `aleph_home_env` group above; restored
        // before the test returns, so no other thread observes the override.
        unsafe {
            std::env::set_var("ALEPH_HOME", home.path());
        }

        let note_dir = crate::utils::paths::get_note_memory_dir().unwrap();
        let temp =
            std::env::temp_dir().join(format!("aleph_mention_weave_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        // Indexer's memory_dir MUST resolve to the same ALEPH_HOME-rooted
        // path `get_notes_with_content` reads from, or the stage would see
        // empty bodies and detect nothing.
        let indexer = NoteIndexer::new(note_dir, store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(""));
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
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };

        // `a/rust-notes`: the mention target. `b/diary` mentions it in prose
        // without a `[[wikilink]]`.
        ctx.indexer
            .write_note(
                &ctx.agent_id,
                "a",
                &KnowledgeNote {
                    title: "rust-notes".into(),
                    category: "a".into(),
                    body: Some("target body".into()),
                    content_hash: "h1".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        ctx.indexer
            .write_note(
                &ctx.agent_id,
                "b",
                &KnowledgeNote {
                    title: "diary".into(),
                    category: "b".into(),
                    body: Some("today I reread rust-notes again".into()),
                    content_hash: "h2".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("a/rust-notes"), entry("b/diary")];

        let out = MentionWeaveStage.execute(ctx).await.unwrap();

        // SAFETY: same guarded invariant as above.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("ALEPH_HOME", v),
                None => std::env::remove_var("ALEPH_HOME"),
            }
        }

        assert_eq!(
            out.report.extra.get("mention_edges").map(String::as_str),
            Some("1"),
            "expected exactly one mention edge, got: {:?}",
            out.report.extra
        );
        let rows = store
            .get_outgoing_link_rows("b/diary", &out.agent_id)
            .await
            .unwrap();
        let m = rows
            .iter()
            .find(|r| r.to_note == "a/rust-notes")
            .expect("mention edge b/diary -> a/rust-notes must exist");
        assert_eq!(m.relation.as_deref(), Some("mention"));
        assert!((m.confidence - 0.35).abs() < 1e-6);
    }
}

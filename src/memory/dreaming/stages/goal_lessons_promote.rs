//! `GoalLessonsPromoteStage` — graduate goal "lessons" into long-term memory.
//!
//! `Goal.lessons` is a ring buffer (cap `MAX_LESSONS`) injected into
//! continuation prompts but otherwise ephemeral: dropped past the cap and gone
//! when the goal is cleared. This stage promotes each goal's current lessons
//! into a per-goal note so they survive the ring and the goal's deletion, and
//! can inform future goals (R9 — the article's "state file" becomes durable).
//!
//! Idempotency: `append_to_note` does NOT dedup facts, so the stage reads the
//! existing note's facts and appends only genuinely-new ones. Stable when
//! nothing is new; union-preserving across cycles (a promoted lesson stays even
//! after the ring drops it). Goals are reached via the process-global
//! `crate::goal::global()` (no `DreamContext` wiring); a store may be injected for
//! tests. Global-only (goals are not project-namespaced).

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::AlephError;
use crate::goal::GoalStore;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::KnowledgeNote;
use crate::sync_primitives::Arc;

use super::DreamStage;

/// Category (directory) under which per-goal lesson notes are written.
const LESSONS_CATEGORY: &str = "goal-lessons";

#[derive(Default)]
pub struct GoalLessonsPromoteStage {
    /// Test-injectable goal store. `None` → resolve the process global.
    pub store: Option<Arc<GoalStore>>,
}

impl GoalLessonsPromoteStage {
    fn resolve_store(&self) -> Option<Arc<GoalStore>> {
        self.store.clone().or_else(crate::goal::global)
    }
}

#[async_trait]
impl DreamStage for GoalLessonsPromoteStage {
    fn name(&self) -> &'static str {
        "goal_lessons_promote"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let Some(store) = self.resolve_store() else {
            return Ok(ctx); // no goal store wired (e.g. tests) → no-op.
        };
        let goals = match store.list_all() {
            Ok(g) => g,
            Err(e) => {
                warn!(error = %e, "GoalLessonsPromote: goal enumeration failed");
                return Ok(ctx);
            }
        };

        let mut promoted = 0u32;
        for goal in goals {
            if goal.lessons.is_empty() {
                continue;
            }
            // Deterministic, filesystem-safe path: goal.id is a stable hash.
            let path = format!("{LESSONS_CATEGORY}/{}", goal.id);

            // Read existing facts to dedup (append_to_note does NOT dedup facts).
            let existing: Vec<String> = match ctx.load_content(&path).await {
                Some(md) => KnowledgeNote::from_markdown(&goal.id, &md)
                    .map(|n| n.facts)
                    .unwrap_or_default(),
                // rust-doctor-disable-next-line unnecessary-allocation
                None => Vec::new(),
            };

            // Desired facts: the objective (for human context) + each lesson.
            let mut desired: Vec<String> = Vec::with_capacity(goal.lessons.len() + 1);
            desired.push(format!("Objective: {}", goal.objective));
            desired.extend(goal.lessons.iter().cloned());

            let new_facts: Vec<String> = desired
                .into_iter()
                .filter(|f| !existing.contains(f))
                .collect();
            if new_facts.is_empty() {
                continue; // already promoted; idempotent no-op.
            }

            match ctx
                .indexer
                .append_to_note(&ctx.agent_id, &path, &new_facts, &[])
                .await
            {
                Ok(()) => {
                    promoted += new_facts.len() as u32;
                    // Evict the now-stale cached content (mirrors NoteWeave).
                    ctx.note_contents.remove(&path);
                }
                Err(e) => warn!(path = %path, error = %e, "GoalLessonsPromote: append failed"),
            }
        }

        ctx.report.goal_lessons_promoted = promoted;
        if promoted > 0 {
            info!(promoted, "GoalLessonsPromote completed");
        }
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::Goal;
    use crate::memory::dreaming::{DreamContext, DreamReport, DreamStrategy};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::NoteIndexer;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;

    struct StubEmbedder;
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _t: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _t: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
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

    async fn build_ctx() -> (DreamContext, std::path::PathBuf) {
        let temp = std::env::temp_dir().join(format!("aleph_lessons_{}", uuid::Uuid::new_v4()));
        // `temp` is the shared root for BOTH the SQLite backend and the note
        // indexer. Create it as a directory first so `SqliteMemoryBackend::new`
        // nests `memory.db` inside it (its `db_path.is_dir()` branch) and
        // `append_to_note` can write notes under `temp/<agent>/<cat>/`. Without
        // this, the backend treats `temp` as a DB *file*, and the note mkdir
        // fails with "Not a directory" — silently zeroing `goal_lessons_promoted`.
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new("{}"));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: DreamReport::default(),
            pipeline_type: "consolidate".into(),
            strategy: DreamStrategy::Consolidate,
            orientation: None,
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (ctx, temp)
    }

    fn goal_store_with(goals: &[Goal]) -> (Arc<GoalStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        for g in goals {
            store.put(g).unwrap();
        }
        (store, dir)
    }

    #[tokio::test]
    async fn promotes_lessons_into_a_note() {
        let (ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "Migrate auth", 0, 0)
            .with_lesson_appended("run migrations first".into(), 1);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let out = stage.execute(ctx).await.unwrap();
        // Objective + 1 lesson = 2 new facts promoted.
        assert_eq!(out.report.goal_lessons_promoted, 2);
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let (ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "Migrate auth", 0, 0)
            .with_lesson_appended("run migrations first".into(), 1);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let ctx = stage.execute(ctx).await.unwrap();
        // Re-run over the same ctx (note already on disk) → nothing new.
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0, "no duplicate facts");
    }

    #[tokio::test]
    async fn goal_without_lessons_is_skipped() {
        let (ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "no lessons yet", 0, 0);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0);
    }

    #[tokio::test]
    async fn no_goal_store_is_graceful_noop() {
        let (ctx, _t) = build_ctx().await;
        let stage = GoalLessonsPromoteStage::default(); // None → global (unset in test)
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0);
    }
}

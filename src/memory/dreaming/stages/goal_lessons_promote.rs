//! `GoalLessonsPromoteStage` — graduate goal "lessons" into long-term memory.
//!
//! `Goal.lessons` is a ring buffer (cap `MAX_LESSONS`) injected into
//! continuation prompts but otherwise ephemeral: dropped past the cap, and
//! deleted with the goal row. This stage promotes each goal's current lessons
//! into a per-goal note so they survive the ring and the goal's deletion, and
//! can inform future goals (R9 — the article's "state file" becomes durable).
//!
//! Idempotency: `append_to_note` does NOT dedup facts, so the promotion reads
//! the existing note's facts and appends only genuinely-new ones. Stable when
//! nothing is new; union-preserving across cycles (a promoted lesson stays even
//! after the ring drops it). Goals are reached via the process-global
//! `crate::goal::global()` (no `DreamContext` wiring); a store may be injected for
//! tests. Global-only (goals are not project-namespaced).
//!
//! The per-goal promotion itself lives in [`promote_one`], NOT in the stage:
//! `goal(action='clear')` deletes the goal row — and with it every lesson that
//! has not graduated yet — hours before the next dream window opens, so it has
//! to promote too. One implementation, two callers; two writers of the same
//! note would drift on dedup rules and fact shape.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::AlephError;
use crate::goal::{Goal, GoalStore};
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::store::SqliteMemoryBackend;
use crate::sync_primitives::Arc;

use super::DreamStage;

/// Category (directory) under which per-goal lesson notes are written.
const LESSONS_CATEGORY: &str = "goal-lessons";

/// Note path a goal's promoted lessons live at (`goal-lessons/<goal id>`).
/// `goal.id` is a stable `goal-<hex>` hash, so the path is deterministic and
/// filesystem-safe. Single source shared by the writer and every cache-eviction
/// site, so the two can never name different files.
#[must_use]
pub fn lessons_note_path(goal_id: &str) -> String {
    format!("{LESSONS_CATEGORY}/{goal_id}")
}

/// Facts already recorded in this goal's lessons note, read straight from the
/// markdown source of truth. Missing/unparsable note → empty (the append then
/// creates it).
async fn existing_facts(
    indexer: &NoteIndexer<SqliteMemoryBackend>,
    agent_id: &str,
    goal_id: &str,
) -> Vec<String> {
    let file_path = indexer
        .memory_dir()
        .join(agent_id)
        .join(LESSONS_CATEGORY)
        .join(format!("{goal_id}.md"));
    let Ok(md) = tokio::fs::read_to_string(&file_path).await else {
        return Vec::new();
    };
    KnowledgeNote::from_markdown(goal_id, &md)
        .map(|n| n.facts)
        .unwrap_or_default()
}

/// Promote ONE goal's lessons into its per-goal note. Returns the number of
/// facts actually appended — `0` when the goal has no lessons, when everything
/// was already promoted (idempotent no-op), or when the append failed.
///
/// Fail-soft by construction: every caller is on a path that must not fail for
/// a memory-layer problem (a nightly stage, and the user's `clear`).
pub async fn promote_one(
    indexer: &NoteIndexer<SqliteMemoryBackend>,
    agent_id: &str,
    goal: &Goal,
) -> u32 {
    if goal.lessons.is_empty() {
        return 0;
    }
    let path = lessons_note_path(&goal.id);

    // Read existing facts to dedup (append_to_note does NOT dedup facts).
    let existing = existing_facts(indexer, agent_id, &goal.id).await;

    // Desired facts: the objective (for human context) + each lesson.
    let mut desired: Vec<String> = Vec::with_capacity(goal.lessons.len() + 1);
    desired.push(format!("Objective: {}", goal.objective));
    desired.extend(goal.lessons.iter().cloned());

    let new_facts: Vec<String> = desired
        .into_iter()
        .filter(|f| !existing.contains(f))
        .collect();
    if new_facts.is_empty() {
        return 0; // already promoted; idempotent no-op.
    }

    match indexer
        .append_to_note(agent_id, &path, &new_facts, &[])
        .await
    {
        Ok(()) => new_facts.len() as u32,
        Err(e) => {
            warn!(path = %path, error = %e, "GoalLessonsPromote: append failed");
            0
        }
    }
}

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
            let appended = promote_one(&ctx.indexer, &ctx.agent_id, &goal).await;
            if appended > 0 {
                promoted += appended;
                // Evict the now-stale cached content (mirrors NoteWeave).
                ctx.note_contents.remove(&lessons_note_path(&goal.id));
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

    async fn build_ctx() -> (tempfile::TempDir, DreamContext, std::path::PathBuf) {
        let (temp_guard, temp) = crate::memory::dreaming::scratch_root();
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
            activity_checker: std::sync::Arc::new(|| false),
            strategy: DreamStrategy::Consolidate,
            orientation: None,
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (temp_guard, ctx, temp)
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
        let (_scratch, ctx, _t) = build_ctx().await;
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
        let (_scratch, ctx, _t) = build_ctx().await;
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
        let (_scratch, ctx, _t) = build_ctx().await;
        let goal = Goal::new("sess-1", "no lessons yet", 0, 0);
        let (gstore, _gd) = goal_store_with(&[goal]);
        let stage = GoalLessonsPromoteStage {
            store: Some(gstore),
        };
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0);
    }

    /// `promote_one` is the unit `goal(action='clear')` calls with no
    /// `DreamContext` around it: it must write the note itself (and stay
    /// idempotent) when driven directly.
    #[tokio::test]
    async fn promote_one_writes_the_note_and_is_idempotent() {
        let (_scratch, ctx, temp) = build_ctx().await;
        let goal = Goal::new("sess-1", "Migrate auth", 0, 0)
            .with_lesson_appended("run migrations first".into(), 1);

        let first = promote_one(&ctx.indexer, &ctx.agent_id, &goal).await;
        assert_eq!(first, 2, "objective + 1 lesson");

        let note = temp
            .join("default")
            .join("goal-lessons")
            .join(format!("{}.md", goal.id));
        let body = tokio::fs::read_to_string(&note)
            .await
            .expect("lessons note must exist on disk");
        assert!(body.contains("run migrations first"), "{body}");

        // Second call reads the note it just wrote → nothing new.
        let second = promote_one(&ctx.indexer, &ctx.agent_id, &goal).await;
        assert_eq!(second, 0, "no duplicate facts");
    }

    #[tokio::test]
    async fn no_goal_store_is_graceful_noop() {
        let (_scratch, ctx, _t) = build_ctx().await;
        let stage = GoalLessonsPromoteStage::default(); // None → global (unset in test)
        let out = stage.execute(ctx).await.unwrap();
        assert_eq!(out.report.goal_lessons_promoted, 0);
    }
}

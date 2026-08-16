//! Dedup helper for distill stages.
//!
//! Given a category + query embedding, returns the top-N most-similar existing
//! notes (path + cosine score). Powers the LLM candidate-injection flow in
//! `SkillDistill` / `FeedbackDistill` (see Phase 2 Decision 2 in
//! `docs/superpowers/plans/2026-04-29-aleph-self-evolution.md`).
//!
//! Returns empty when `query_embedding` is empty or `top_n == 0`, so callers
//! without an embedding provider degrade gracefully — the LLM defaults to
//! emitting `DistillAction::New` for every candidate.

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;

/// Find the top `top_n` notes in `category` most similar to `query_embedding`.
///
/// Returns `Vec<(path, cosine_score)>` sorted by descending similarity (sqlite-vec
/// returns ascending distance, which the underlying `vector_search` already converts).
pub async fn find_similar_notes<S: NoteStore>(
    store: &S,
    category: &str,
    agent_id: &str,
    query_embedding: &[f32],
    top_n: usize,
) -> Result<Vec<(String, f32)>, AlephError> {
    if query_embedding.is_empty() || top_n == 0 {
        return Ok(Vec::new());
    }
    let dim = query_embedding.len() as u32;
    let prefix = format!("{category}/");
    let mut filtered = Vec::with_capacity(top_n);
    let mut fetch_limit = top_n.saturating_mul(4).max(top_n);
    const MAX_FETCH_MULTIPLIER: usize = 32;

    while filtered.len() < top_n && fetch_limit <= top_n.saturating_mul(MAX_FETCH_MULTIPLIER) {
        let raw = store
            .vector_search(query_embedding, dim, agent_id, fetch_limit)
            .await?;
        let raw_count = raw.len();

        filtered = raw
            .into_iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .take(top_n)
            .collect();

        if filtered.len() >= top_n {
            break;
        }

        // DB exhausted — no point requesting more
        if raw_count < fetch_limit {
            break;
        }

        fetch_limit = fetch_limit.saturating_mul(2);
    }
    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn test_db() -> (tempfile::TempDir, Arc<SqliteMemoryBackend>) {
        let (scratch, path) = crate::utils::scratch::scratch_root();
        (scratch, Arc::new(SqliteMemoryBackend::new(&path).unwrap()))
    }

    #[tokio::test]
    async fn returns_empty_when_embedding_empty() {
        let (_scratch, db) = test_db();
        let res = find_similar_notes(&*db, "skill", "default", &[], 5)
            .await
            .unwrap();
        assert!(res.is_empty());
    }

    #[tokio::test]
    async fn returns_empty_when_top_n_zero() {
        let (_scratch, db) = test_db();
        let res = find_similar_notes(&*db, "skill", "default", &[1.0_f32; 1024], 0)
            .await
            .unwrap();
        assert!(res.is_empty());
    }

    #[tokio::test]
    async fn filters_to_category_prefix() {
        let (_scratch, db) = test_db();
        let emb = vec![1.0_f32; 1024];
        db.upsert_embedding("skill/skill-A", "default", &emb, 1024, "")
            .await
            .unwrap();
        db.upsert_embedding("preference/pref-A", "default", &emb, 1024, "")
            .await
            .unwrap();

        let res = find_similar_notes(&*db, "skill", "default", &emb, 5)
            .await
            .unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, "skill/skill-A");
    }

    #[tokio::test]
    async fn returns_top_n_sorted_by_similarity() {
        let (_scratch, db) = test_db();
        let exact = vec![1.0_f32; 1024];
        let mut close = vec![1.0_f32; 1024];
        close[0] = 0.9; // slightly less similar to `exact`
        db.upsert_embedding("skill/exact", "default", &exact, 1024, "")
            .await
            .unwrap();
        db.upsert_embedding("skill/close", "default", &close, 1024, "")
            .await
            .unwrap();

        let res = find_similar_notes(&*db, "skill", "default", &exact, 2)
            .await
            .unwrap();
        assert_eq!(res.len(), 2);
        // The exact match must come first (higher similarity = lower distance)
        assert_eq!(res[0].0, "skill/exact");
    }
}

//! `NoteRetrieval` — vector-search over knowledge notes.
//!
//! Given a query string the service embeds it, performs a vector search in the
//! notes index, reads the matching markdown files from disk, and returns them
//! as `NoteContent` items ordered by similarity.

use std::path::PathBuf;

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{KnowledgeNote, Severity};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

/// Overfetch factor — KNN returns K×α candidates so re-rank by confidence/severity
/// can promote items past the original top-K. See Phase 2 Decision 4.
const RERANK_OVERFETCH: usize = 3;

/// Boost factor applied to a note's cosine similarity based on its LLM-judged severity.
/// Legacy notes (severity=Low) get boost 1.0 → no behavior change.
///
/// Boost range is intentionally narrow ([1.0, 1.20]) so semantic relevance (cosine)
/// and distillation confidence remain the dominant ranking signals. A wider range
/// (e.g. 2.0× for Critical) lets a low-confidence Critical note outrank a
/// semantically-closer Low/Med note — the opposite of what re-ranking should do.
pub(crate) const fn severity_boost(s: Severity) -> f32 {
    match s {
        Severity::Low => 1.0,
        Severity::Med => 1.05,
        Severity::High => 1.10,
        Severity::Critical => 1.20,
    }
}

/// Final ranking score: `cosine × confidence × severity_boost`.
/// Legacy defaults (confidence=1.0, severity=Low) yield score = cosine.
pub(crate) fn rerank_score(cosine: f32, confidence: f32, severity: Severity) -> f32 {
    cosine * confidence * severity_boost(severity)
}

/// A single note retrieved by vector similarity search.
#[derive(Debug, Clone)]
pub struct NoteContent {
    /// Relative path within the agent directory, e.g. `"reference/rust-ownership"`.
    pub path: String,
    /// Full markdown file content.
    pub content: String,
    /// Distance/similarity score from the query embedding.
    pub score: f32,
}

/// Retrieves knowledge notes by embedding similarity.
pub struct NoteRetrieval<S: NoteStore> {
    memory_dir: PathBuf,
    store: Arc<S>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl<S: NoteStore> NoteRetrieval<S> {
    pub fn new(memory_dir: PathBuf, store: Arc<S>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            memory_dir,
            store,
            embedder,
        }
    }

    /// Retrieve the top `limit` notes most similar to `query` for the given agent.
    ///
    /// Internally overfetches `limit × RERANK_OVERFETCH` candidates from the KNN, then
    /// re-ranks by `cosine × confidence × severity_boost(severity)` (Phase 2 Decision 4).
    /// Legacy notes (no `confidence` / `severity` in frontmatter) get defaults
    /// `confidence=1.0`, `severity=Low` so their final score equals their cosine score —
    /// zero behavior change for pre-self-evolution notes.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<NoteContent>, AlephError> {
        // 1. Embed the query
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        // 2. Vector search — overfetch so re-rank can promote items past the original top-K
        let raw_limit = limit.saturating_mul(RERANK_OVERFETCH).max(limit);
        let results = self
            .store
            .vector_search(&embedding, dim, agent_id, raw_limit)
            .await?;

        // 3. Read markdown, parse frontmatter, compute final re-rank score
        let mut scored: Vec<NoteContent> = Vec::new();
        for (path, distance) in results {
            let safe_path = path
                .replace('\\', "/")
                .split('/')
                .filter(|s| !s.is_empty() && *s != "..")
                .collect::<Vec<_>>()
                .join("/");
            let file_path = self
                .memory_dir
                .join(agent_id)
                .join(format!("{safe_path}.md"));
            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(_) => continue, // file missing on disk — skip gracefully
            };
            // Legacy notes (or unparseable frontmatter) fall back to (1.0, Low) → score == cosine.
            let (confidence, severity) = match KnowledgeNote::from_markdown(&path, &content) {
                Ok(n) => (n.confidence, n.severity),
                Err(_) => (1.0, Severity::Low),
            };
            // `vector_search` returns the raw vec0 L2 distance (lower = more
            // similar). `rerank_score` and the descending sort below both expect
            // a *similarity* (higher = better), so convert via the project's
            // canonical 1/(1+L2) (same formula as `memory.similarity_threshold`).
            // Feeding the raw distance here inverted the ranking — least-similar
            // notes scored highest.
            let similarity = 1.0 / (1.0 + distance);
            let final_score = rerank_score(similarity, confidence, severity);
            scored.push(NoteContent {
                path,
                content,
                score: final_score,
            });
        }

        // 4. Sort by descending final score and trim to requested limit
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::SqliteMemoryBackend;
    use tokio::fs;
    use uuid::Uuid;

    const AGENT: &str = "default";

    // ─── re-rank pure-function tests (Phase 2 Decision 4) ──────────────────

    #[test]
    fn severity_boost_table() {
        assert!((severity_boost(Severity::Low) - 1.0).abs() < 1e-6);
        assert!((severity_boost(Severity::Med) - 1.05).abs() < 1e-6);
        assert!((severity_boost(Severity::High) - 1.10).abs() < 1e-6);
        assert!((severity_boost(Severity::Critical) - 1.20).abs() < 1e-6);
    }

    #[test]
    fn rerank_low_high_confidence_beats_critical_low_confidence() {
        // Same cosine: a high-confidence Low-severity note must outrank a
        // half-confidence Critical-severity note. Otherwise Critical's boost
        // dominates and low-confidence Critical wins on tie cosine — exactly
        // the failure mode this dampened boost prevents.
        let low_strong = rerank_score(0.8, 1.0, Severity::Low);
        let critical_weak = rerank_score(0.8, 0.5, Severity::Critical);
        assert!(
            low_strong > critical_weak,
            "low/conf=1.0 ({low_strong}) should beat critical/conf=0.5 ({critical_weak})"
        );
    }

    #[test]
    fn rerank_higher_cosine_beats_severity_within_30_percent() {
        // Cosine gap of 25% should not be flipped by Critical's boost
        // (max boost ratio is 1.20, so any cosine gap > ~17% wins).
        let close_low = rerank_score(0.95, 1.0, Severity::Low);
        let far_critical = rerank_score(0.70, 1.0, Severity::Critical);
        assert!(
            close_low > far_critical,
            "cosine=0.95 Low ({close_low}) should beat cosine=0.70 Critical ({far_critical})"
        );
    }

    #[test]
    fn rerank_score_legacy_defaults_match_cosine() {
        // Legacy note: confidence=1.0, severity=Low → score == cosine
        let s = rerank_score(0.9, 1.0, Severity::Low);
        assert!((s - 0.9).abs() < 1e-6);
    }

    #[test]
    fn rerank_prefers_higher_confidence() {
        let low = rerank_score(0.9, 0.3, Severity::Med);
        let high = rerank_score(0.9, 0.95, Severity::Med);
        assert!(high > low);
    }

    #[test]
    fn rerank_boosts_higher_severity() {
        let med = rerank_score(0.9, 1.0, Severity::Med);
        let critical = rerank_score(0.9, 1.0, Severity::Critical);
        assert!(critical > med);
    }

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_note_retrieval_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    #[tokio::test]
    async fn retrieve_returns_empty_when_no_embeddings() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));

        let retrieval = NoteRetrieval::new(memory_dir, db, embedder);
        let results = retrieval.retrieve("anything", AGENT, 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Insert an embedding but don't create the file
        let fake_embedding = vec![0.1_f32; 1024];
        db.upsert_embedding("reference/ghost-note", AGENT, &fake_embedding, 1024, "")
            .await
            .unwrap();

        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));

        let retrieval = NoteRetrieval::new(memory_dir, db, embedder);
        let results = retrieval.retrieve("test query", AGENT, 5).await.unwrap();
        // File doesn't exist on disk, so it should be skipped
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn retrieve_returns_content_for_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().to_path_buf();
        let db = create_test_db();

        // Create a note file on disk
        let note_dir = memory_dir.join(AGENT).join("reference");
        fs::create_dir_all(&note_dir).await.unwrap();
        fs::write(
            note_dir.join("rust-ownership.md"),
            "# Rust Ownership\n\nBorrow checker rules.",
        )
        .await
        .unwrap();

        // Insert an embedding for this note
        let fake_embedding = vec![0.1_f32; 1024];
        db.upsert_embedding("reference/rust-ownership", AGENT, &fake_embedding, 1024, "")
            .await
            .unwrap();

        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock"));

        let retrieval = NoteRetrieval::new(memory_dir, db, embedder);
        let results = retrieval.retrieve("ownership", AGENT, 5).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "reference/rust-ownership");
        assert!(results[0].content.contains("Borrow checker"));
    }
}

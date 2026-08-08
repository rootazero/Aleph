//! Re-embedding migration for notes.
//!
//! When the user switches embedding provider (different vector dimensions),
//! this module re-embeds all existing note content with the new provider. Designed
//! to be triggered manually via RPC, not at startup.

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, Ordering};
use std::path::Path;
use tracing::{info, warn};

/// Progress of a running re-embed operation.
#[derive(Debug, Clone)]
pub struct ReembedProgress {
    /// Current phase: "notes"
    pub phase: &'static str,
    /// Total items to process in this phase
    pub total: usize,
    /// Successfully completed items
    pub completed: usize,
    /// Failed items
    pub failed: usize,
}

/// Final result of a re-embed operation.
#[derive(Debug, Clone, Default)]
pub struct ReembedResult {
    pub facts_updated: usize,
    pub facts_total: usize,
    /// Notes left alone because their stored vector was already computed from
    /// the current version of the note. Reported rather than folded into
    /// `facts_updated`, so "nothing needed doing" is distinguishable from
    /// "nothing got done".
    pub facts_skipped: usize,
    pub errors: Vec<String>,
}

/// Re-embed all notes to match the current provider's dimension.
///
/// Discovers agent IDs by listing subdirectories of `memory_dir`, then
/// loads each agent's note index entries, reads content from disk,
/// and upserts embeddings.
///
/// Cancellation: checked between batches via `cancel` flag.
///
/// Idempotent **and cheap to re-run**: a note whose stored vector was already
/// computed from its current content is skipped, using the `embedded_hash`
/// recorded by `upsert_embedding`. Before that column existed this function
/// re-embedded the entire corpus on every invocation, so the "safe to re-run"
/// promise held only for correctness, never for cost.
///
/// The skip is suppressed when the embedding **signature** (provider + model +
/// dimension) differs from the one recorded by the last completed run: an equal
/// content hash then says the text is unchanged, which is exactly not the
/// question — the vector space itself moved, and every vector must be redone.
/// Getting this backwards would leave a store half-migrated between two models
/// with nothing reporting it.
pub async fn reembed_all(
    database: &MemoryBackend,
    memory_dir: &Path,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: Arc<AtomicBool>,
) -> Result<ReembedResult, AlephError> {
    let mut result = ReembedResult::default();

    // Every corpus on disk — base agents and composed scoped partitions alike
    // (`project_scope::list_note_corpora` is the single answer to "which
    // corpora exist"; this used to keep its own copy that also treated a
    // dot-prefixed directory such as a stray staging dir as an agent).
    let agent_ids = crate::memory::project_scope::list_note_corpora(memory_dir);

    let signature = crate::memory::embedding_signature::provider_signature(embedder.as_ref());
    let force_all = match database.get_embedding_signature() {
        Ok(Some(stored)) => stored != signature,
        // No signature recorded (fresh store, or data predating the feature):
        // the vector space cannot be vouched for, so redo everything.
        Ok(None) => true,
        Err(e) => {
            warn!(error = %e, "[reembed] signature read failed — re-embedding everything");
            true
        }
    };

    info!(
        target_dim,
        batch_size,
        agents = agent_ids.len(),
        force_all,
        "[reembed] Starting notes re-embed"
    );

    let mut total_notes = 0usize;
    let mut total_updated = 0usize;
    let mut total_skipped = 0usize;
    let mut total_errors: Vec<String> = Vec::new();

    for agent_id in &agent_ids {
        let fresh: std::collections::HashSet<String> = if force_all {
            std::collections::HashSet::new()
        } else {
            fresh_vector_paths(database, agent_id).await
        };

        reembed_agent_notes(
            database,
            memory_dir,
            embedder,
            agent_id,
            target_dim,
            batch_size,
            &progress_tx,
            &cancel,
            &fresh,
            &mut total_notes,
            &mut total_updated,
            &mut total_skipped,
            &mut total_errors,
        )
        .await?;

        if cancel.load(Ordering::Relaxed) {
            info!("[reembed] Cancelled");
            break;
        }
    }

    result.facts_total = total_notes;
    result.facts_updated = total_updated;
    result.facts_skipped = total_skipped;
    result.errors = total_errors;

    info!(
        notes_updated = result.facts_updated,
        notes_skipped = result.facts_skipped,
        notes_total = result.facts_total,
        errors = result.errors.len(),
        "[reembed] Migration complete"
    );

    // Record the vector-space signature so a later provider/model switch can be
    // detected (see `memory::embedding_signature`). Skip on cancellation — a
    // cancelled run leaves the store split between the old and new model, so the
    // signature would misrepresent it.
    if !cancel.load(Ordering::Relaxed) {
        let sig = crate::memory::embedding_signature::provider_signature(embedder.as_ref());
        match database.set_embedding_signature(&sig) {
            Ok(()) => info!(signature = %sig, "[reembed] recorded embedding signature"),
            Err(e) => warn!("[reembed] failed to record embedding signature: {e}"),
        }
    }

    Ok(result)
}

/// Note paths whose stored vector already matches the indexed content.
///
/// Complement of [`NoteStore::stale_vector_paths`] — asked once per agent so
/// the per-note skip decision costs a hash-set lookup instead of a query.
async fn fresh_vector_paths(
    database: &MemoryBackend,
    agent_id: &str,
) -> std::collections::HashSet<String> {
    let all = match database.list_notes(agent_id).await {
        Ok(n) => n,
        Err(e) => {
            warn!(agent = agent_id, error = %e, "[reembed] note list failed — treating all as stale");
            return std::collections::HashSet::new();
        }
    };
    let stale: std::collections::HashSet<String> = match database.stale_vector_paths(agent_id).await
    {
        Ok(s) => s.into_iter().collect(),
        Err(e) => {
            warn!(agent = agent_id, error = %e, "[reembed] staleness query failed — treating all as stale");
            return std::collections::HashSet::new();
        }
    };
    all.into_iter()
        .map(|n| n.path)
        .filter(|p| !stale.contains(p))
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn reembed_agent_notes(
    database: &MemoryBackend,
    memory_dir: &Path,
    embedder: &Arc<dyn EmbeddingProvider>,
    agent_id: &str,
    target_dim: usize,
    batch_size: usize,
    progress_tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: &AtomicBool,
    fresh: &std::collections::HashSet<String>,
    total_notes: &mut usize,
    total_updated: &mut usize,
    total_skipped: &mut usize,
    errors: &mut Vec<String>,
) -> Result<(), AlephError> {
    let all_notes = database.list_notes(agent_id).await?;
    *total_notes += all_notes.len();

    // Drop notes whose vector is already derived from the current content. The
    // filter runs before batching so a mostly-fresh corpus issues no embedding
    // calls at all rather than full batches that each discard most of their work.
    let before = all_notes.len();
    let notes: Vec<_> = all_notes
        .into_iter()
        .filter(|n| !fresh.contains(&n.path))
        .collect();
    let skipped = before - notes.len();
    *total_skipped += skipped;

    if notes.is_empty() {
        info!(
            agent = agent_id,
            skipped, "[reembed] All vectors already current for agent"
        );
        return Ok(());
    }

    info!(
        agent = agent_id,
        count = notes.len(),
        skipped,
        "[reembed] Notes to re-embed for agent"
    );

    let mut completed = 0usize;
    let mut failed = 0usize;

    for chunk in notes.chunks(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            info!("[reembed] Cancelled during notes phase");
            break;
        }

        // Read file content for each note in the chunk
        let mut texts: Vec<String> = Vec::with_capacity(chunk.len());
        let mut valid_notes = Vec::with_capacity(chunk.len());

        for note in chunk {
            let file_path = memory_dir.join(agent_id).join(&note.category).join(
                crate::memory::notes::store::note_md_filename(&note.filename),
            );

            match tokio::fs::read_to_string(&file_path).await {
                Ok(content) => {
                    texts.push(content);
                    valid_notes.push(note);
                }
                Err(e) => {
                    warn!(
                        path = %file_path.display(),
                        error = %e,
                        "[reembed] Could not read note file, skipping"
                    );
                    failed += 1;
                    errors.push(format!("note {}: read error: {}", note.path, e));
                }
            }
        }

        if texts.is_empty() {
            continue;
        }

        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        match embedder.embed_batch(&text_refs).await {
            Ok(embeddings) => {
                for ((note, embedding), text) in
                    valid_notes.iter().zip(embeddings).zip(texts.iter())
                {
                    if embedding.len() != target_dim {
                        warn!(
                            path = %note.path,
                            got_dim = embedding.len(),
                            expected_dim = target_dim,
                            "[reembed] Dimension mismatch, skipping"
                        );
                        failed += 1;
                        errors.push(format!(
                            "note {}: dimension mismatch (got {}, expected {})",
                            note.path,
                            embedding.len(),
                            target_dim
                        ));
                        continue;
                    }
                    let dim = embedding.len() as u32;
                    // Hash the text that was actually embedded, not the index
                    // row's stored hash: if the file changed since it was last
                    // indexed, this vector's provenance is the file — and the
                    // mismatch against `notes_index` correctly keeps the note
                    // reported as stale until the index catches up.
                    let embedded_hash = crate::memory::notes::indexer::sha2_hash(text);
                    if let Err(e) = database
                        .upsert_embedding(&note.path, agent_id, &embedding, dim, &embedded_hash)
                        .await
                    {
                        warn!(path = %note.path, error = %e, "[reembed] Failed to upsert embedding");
                        failed += 1;
                        errors.push(format!("note {}: {}", note.path, e));
                    } else {
                        completed += 1;
                    }
                }
            }
            Err(e) => {
                warn!(batch_size = text_refs.len(), error = %e, "[reembed] Batch embed failed");
                failed += text_refs.len();
                errors.push(format!("notes batch for agent {agent_id}: {e}"));
            }
        }

        send_progress(progress_tx, "notes", *total_notes, completed, failed);
    }

    *total_updated += completed;
    Ok(())
}

fn send_progress(
    tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    phase: &'static str,
    total: usize,
    completed: usize,
    failed: usize,
) {
    if let Some(tx) = tx {
        let _ = tx.send(ReembedProgress {
            phase,
            total,
            completed,
            failed,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::notes::{KnowledgeNote, NoteIndexer};
    use crate::memory::store::SqliteMemoryBackend;

    const AGENT: &str = "agent1";
    const DIM: usize = 1024;

    async fn seed(memory_dir: &Path, store: &Arc<SqliteMemoryBackend>, titles: &[&str]) {
        // rust-doctor-disable-next-line excessive-clone
        let indexer = NoteIndexer::new(memory_dir.to_path_buf(), store.clone());
        for title in titles {
            let note = KnowledgeNote {
                title: (*title).to_string(),
                category: "reference".to_string(),
                facts: vec![format!("{title} fact")],
                ..Default::default()
            };
            indexer.write_note(AGENT, "reference", &note).await.unwrap();
        }
    }

    async fn run(
        store: &Arc<SqliteMemoryBackend>,
        dir: &Path,
        embedder: &Arc<dyn EmbeddingProvider>,
    ) -> ReembedResult {
        reembed_all(
            store,
            dir,
            embedder,
            DIM,
            8,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_second_run_skips_every_note_whose_vector_is_already_current() {
        // "Idempotent: safe to re-run" used to be true only for correctness:
        // every invocation re-embedded the whole corpus because nothing
        // recorded which version of a note a vector came from.
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        seed(tmp.path(), &store, &["alpha", "beta", "gamma"]).await;
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(DIM, "mock-1024"));

        let first = run(&store, tmp.path(), &embedder).await;
        assert_eq!(first.facts_updated, 3, "{:?}", first.errors);
        assert_eq!(first.facts_skipped, 0);

        let second = run(&store, tmp.path(), &embedder).await;
        assert_eq!(second.facts_updated, 0, "re-embedded unchanged notes");
        assert_eq!(second.facts_skipped, 3);
        assert_eq!(second.facts_total, 3);
    }

    #[tokio::test]
    async fn an_edited_note_is_re_embedded_while_its_neighbours_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        seed(tmp.path(), &store, &["alpha", "beta"]).await;
        let embedder: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(DIM, "mock-1024"));
        run(&store, tmp.path(), &embedder).await;

        // rust-doctor-disable-next-line excessive-clone
        let indexer = NoteIndexer::new(tmp.path().to_path_buf(), store.clone());
        indexer
            .append_to_note(AGENT, "reference/alpha", &["a later fact".to_string()], &[])
            .await
            .unwrap();

        let after = run(&store, tmp.path(), &embedder).await;
        assert_eq!(after.facts_updated, 1, "the edited note must be redone");
        assert_eq!(after.facts_skipped, 1, "its neighbour must not be");
    }

    #[tokio::test]
    async fn a_signature_change_forces_a_full_re_embed() {
        // An unchanged content hash says the *text* is the same, which is
        // exactly not the question when the vector space itself moved. Reading
        // it as freshness would leave the store split between two models with
        // nothing reporting it.
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        seed(tmp.path(), &store, &["alpha", "beta"]).await;

        let first: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(DIM, "mock-1024"));
        run(&store, tmp.path(), &first).await;

        let switched: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(DIM, "other-model"));
        let after = run(&store, tmp.path(), &switched).await;
        assert_eq!(
            after.facts_updated, 2,
            "model switch must redo every vector"
        );
        assert_eq!(after.facts_skipped, 0);
    }
}

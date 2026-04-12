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
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
    pub memories_updated: usize,
    pub memories_total: usize,
    pub errors: Vec<String>,
}

/// Re-embed all notes to match the current provider's dimension.
///
/// Discovers agent IDs by listing subdirectories of `memory_dir`, then
/// loads each agent's note index entries, reads content from disk,
/// and upserts embeddings.
///
/// Cancellation: checked between batches via `cancel` flag.
/// Idempotent: safe to re-run; upsert_embedding overwrites existing vectors.
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

    // Discover agent IDs from filesystem subdirectories of memory_dir
    let agent_ids = discover_agent_ids(memory_dir).await;

    info!(
        target_dim,
        batch_size,
        agents = agent_ids.len(),
        "[reembed] Starting notes re-embed"
    );

    let mut total_notes = 0usize;
    let mut total_updated = 0usize;
    let mut total_errors: Vec<String> = Vec::new();

    for agent_id in &agent_ids {
        reembed_agent_notes(
            database,
            memory_dir,
            embedder,
            agent_id,
            target_dim,
            batch_size,
            &progress_tx,
            &cancel,
            &mut total_notes,
            &mut total_updated,
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
    result.errors = total_errors;

    info!(
        notes_updated = result.facts_updated,
        notes_total = result.facts_total,
        errors = result.errors.len(),
        "[reembed] Migration complete"
    );

    Ok(result)
}

/// Discover agent IDs by listing immediate subdirectories of `memory_dir`.
async fn discover_agent_ids(memory_dir: &Path) -> Vec<String> {
    let mut agent_ids = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(memory_dir).await {
        Ok(rd) => rd,
        Err(_) => return agent_ids,
    };
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    agent_ids.push(name.to_owned());
                }
            }
        }
    }
    agent_ids
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
    total_notes: &mut usize,
    total_updated: &mut usize,
    errors: &mut Vec<String>,
) -> Result<(), AlephError> {
    let notes = database.list_notes(agent_id).await?;
    *total_notes += notes.len();

    if notes.is_empty() {
        return Ok(());
    }

    info!(
        agent = agent_id,
        count = notes.len(),
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
            let file_path = memory_dir
                .join(agent_id)
                .join(&note.category)
                .join(format!("{}.md", note.filename));

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
                for (note, embedding) in valid_notes.iter().zip(embeddings.into_iter()) {
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
                    if let Err(e) = database
                        .upsert_embedding(&note.path, agent_id, &embedding, dim)
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
                errors.push(format!("notes batch for agent {agent_id}: {}", e));
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

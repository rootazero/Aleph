//! Re-embedding migration for facts and raw memories.
//!
//! When the user switches embedding provider (different vector dimensions),
//! this module re-embeds all existing data with the new provider. Designed
//! to be triggered manually via RPC, not at startup.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore, SessionStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

/// Progress of a running re-embed operation.
#[derive(Debug, Clone)]
pub struct ReembedProgress {
    /// Current phase: "facts" or "memories"
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

/// Re-embed all facts and raw memories to match the current provider's dimension.
///
/// Processes facts first, then memories. Each phase:
/// 1. Loads all records
/// 2. Filters those whose embedding dimension != target_dim
/// 3. Serial batch embed (batch_size items per API call)
/// 4. Updates via delete+insert (old vector columns cleared automatically)
///
/// Cancellation: checked between batches via `cancel` flag.
/// Idempotent: re-triggering only processes records still mismatched.
pub async fn reembed_all(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: Arc<AtomicBool>,
) -> Result<ReembedResult, AlephError> {
    let mut result = ReembedResult::default();

    // Phase 1: Facts
    info!(target_dim, batch_size, "[reembed] Starting facts phase");
    reembed_facts(
        database,
        embedder,
        target_dim,
        batch_size,
        &progress_tx,
        &cancel,
        &mut result,
    )
    .await?;

    if cancel.load(Ordering::Relaxed) {
        info!("[reembed] Cancelled after facts phase");
        return Ok(result);
    }

    // Phase 2: Memories
    info!(target_dim, batch_size, "[reembed] Starting memories phase");
    reembed_memories(
        database,
        embedder,
        target_dim,
        batch_size,
        &progress_tx,
        &cancel,
        &mut result,
    )
    .await?;

    info!(
        facts_updated = result.facts_updated,
        facts_total = result.facts_total,
        memories_updated = result.memories_updated,
        memories_total = result.memories_total,
        errors = result.errors.len(),
        "[reembed] Migration complete"
    );

    Ok(result)
}

async fn reembed_facts(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: &AtomicBool,
    result: &mut ReembedResult,
) -> Result<(), AlephError> {
    let all_facts = database.get_all_facts(true, None).await?;

    let needs_reembed: Vec<&MemoryFact> = all_facts
        .iter()
        .filter(|f| {
            f.embedding
                .as_ref()
                .map(|e| e.len() != target_dim)
                .unwrap_or(true)
        })
        .collect();

    result.facts_total = all_facts.len();

    if needs_reembed.is_empty() {
        info!("[reembed] All facts already have correct embeddings");
        return Ok(());
    }

    info!(
        needs_reembed = needs_reembed.len(),
        "[reembed] Facts needing re-embedding"
    );

    let mut completed = 0usize;
    let mut failed = 0usize;

    for chunk in needs_reembed.chunks(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            info!("[reembed] Cancelled during facts phase");
            break;
        }

        let texts: Vec<&str> = chunk.iter().map(|f| f.content.as_str()).collect();

        match embedder.embed_batch(&texts).await {
            Ok(embeddings) => {
                for (fact, embedding) in chunk.iter().zip(embeddings.into_iter()) {
                    if embedding.len() != target_dim {
                        warn!(
                            fact_id = %fact.id,
                            got_dim = embedding.len(),
                            expected_dim = target_dim,
                            "[reembed] Dimension mismatch, skipping"
                        );
                        failed += 1;
                        result.errors.push(format!(
                            "fact {}: dimension mismatch (got {}, expected {})",
                            fact.id,
                            embedding.len(),
                            target_dim
                        ));
                        continue;
                    }
                    let mut updated_fact = (*fact).clone();
                    updated_fact.embedding = Some(embedding);
                    if let Err(e) = database.update_fact(&updated_fact).await {
                        warn!(fact_id = %fact.id, error = %e, "[reembed] Failed to update fact");
                        failed += 1;
                        result.errors.push(format!("fact {}: {}", fact.id, e));
                    } else {
                        completed += 1;
                    }
                }
            }
            Err(e) => {
                warn!(batch_size = chunk.len(), error = %e, "[reembed] Batch embed failed");
                failed += chunk.len();
                result.errors.push(format!("facts batch: {}", e));
            }
        }

        send_progress(progress_tx, "facts", result.facts_total, completed, failed);
    }

    result.facts_updated = completed;
    Ok(())
}

async fn reembed_memories(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
    progress_tx: &Option<tokio::sync::watch::Sender<ReembedProgress>>,
    cancel: &AtomicBool,
    result: &mut ReembedResult,
) -> Result<(), AlephError> {
    let all_memories = database.get_all_memories(None).await?;
    result.memories_total = all_memories.len();

    let needs_reembed: Vec<_> = all_memories
        .into_iter()
        .filter(|m| {
            m.embedding
                .as_ref()
                .map(|e| e.len() != target_dim)
                .unwrap_or(true)
        })
        .collect();

    if needs_reembed.is_empty() {
        info!("[reembed] All memories already have correct embeddings");
        return Ok(());
    }

    info!(
        needs_reembed = needs_reembed.len(),
        "[reembed] Memories needing re-embedding"
    );

    let mut completed = 0usize;
    let mut failed = 0usize;

    for chunk in needs_reembed.chunks(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            info!("[reembed] Cancelled during memories phase");
            break;
        }

        // Embed text matches ingestion.rs: user_input + "\n\n" + ai_output
        let texts: Vec<String> = chunk
            .iter()
            .map(|m| format!("{}\n\n{}", m.user_input, m.ai_output))
            .collect();
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        match embedder.embed_batch(&text_refs).await {
            Ok(embeddings) => {
                for (memory, embedding) in chunk.iter().zip(embeddings.into_iter()) {
                    if embedding.len() != target_dim {
                        warn!(
                            memory_id = %memory.id,
                            got_dim = embedding.len(),
                            expected_dim = target_dim,
                            "[reembed] Dimension mismatch, skipping"
                        );
                        failed += 1;
                        result
                            .errors
                            .push(format!("memory {}: dimension mismatch", memory.id));
                        continue;
                    }
                    let mut updated = memory.clone();
                    updated.embedding = Some(embedding);

                    // delete + insert (same pattern as update_fact)
                    if let Err(e) = database.delete_memory(&memory.id).await {
                        warn!(memory_id = %memory.id, error = %e, "[reembed] Failed to delete memory");
                        failed += 1;
                        result
                            .errors
                            .push(format!("memory {}: delete failed: {}", memory.id, e));
                        continue;
                    }
                    if let Err(e) = database.insert_memory(&updated).await {
                        warn!(memory_id = %memory.id, error = %e, "[reembed] Failed to insert memory");
                        failed += 1;
                        result
                            .errors
                            .push(format!("memory {}: insert failed: {}", memory.id, e));
                    } else {
                        completed += 1;
                    }
                }
            }
            Err(e) => {
                warn!(batch_size = chunk.len(), error = %e, "[reembed] Batch embed failed");
                failed += chunk.len();
                result.errors.push(format!("memories batch: {}", e));
            }
        }

        send_progress(
            progress_tx,
            "memories",
            result.memories_total,
            completed,
            failed,
        );
    }

    result.memories_updated = completed;
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

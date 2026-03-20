//! One-shot re-embedding utility for facts whose vector column is empty or wrong dimension.
//!
//! Usage: call `reembed_all_facts()` once at startup. It scans all facts,
//! identifies those missing a vec_{target_dim} embedding, re-embeds them in
//! batches, and writes them back via delete+insert.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::{info, warn};

/// Re-embed all facts that are missing the correct vector dimension.
///
/// - `target_dim`: expected embedding dimension (e.g. 1536)
/// - `batch_size`: how many texts to embed per API call
pub async fn reembed_all_facts(
    database: &MemoryBackend,
    embedder: &Arc<dyn EmbeddingProvider>,
    target_dim: usize,
    batch_size: usize,
) -> Result<usize, AlephError> {
    // 1. Read ALL facts (including invalid ones)
    let all_facts = database.get_all_facts(true, None).await?;
    info!(
        total_facts = all_facts.len(),
        target_dim = target_dim,
        "Scanning facts for missing embeddings"
    );

    // 2. Filter: keep facts whose embedding is absent or not target_dim
    let needs_reembed: Vec<&MemoryFact> = all_facts
        .iter()
        .filter(|f| {
            f.embedding
                .as_ref()
                .map(|e| e.len() != target_dim)
                .unwrap_or(true)
        })
        .collect();

    if needs_reembed.is_empty() {
        info!("[reembed] All facts already have correct embeddings, nothing to do");
        return Ok(0);
    }

    info!(
        needs_reembed = needs_reembed.len(),
        "[reembed] Facts needing re-embedding"
    );

    // 3. Batch embed and update
    let mut updated = 0usize;
    for chunk in needs_reembed.chunks(batch_size) {
        let texts: Vec<&str> = chunk.iter().map(|f| f.content.as_str()).collect();

        match embedder.embed_batch(&texts).await {
            Ok(embeddings) => {
                for (fact, embedding) in chunk.iter().zip(embeddings.into_iter()) {
                    if embedding.len() != target_dim {
                        warn!(
                            fact_id = %fact.id,
                            got_dim = embedding.len(),
                            expected_dim = target_dim,
                            "[reembed] Embedding dimension mismatch, skipping"
                        );
                        continue;
                    }
                    let mut updated_fact = (*fact).clone();
                    updated_fact.embedding = Some(embedding);
                    if let Err(e) = database.update_fact(&updated_fact).await {
                        warn!(
                            fact_id = %fact.id,
                            error = %e,
                            "[reembed] Failed to update fact"
                        );
                    } else {
                        updated += 1;
                    }
                }
            }
            Err(e) => {
                warn!(
                    batch_size = chunk.len(),
                    error = %e,
                    "[reembed] Batch embed failed, skipping batch"
                );
            }
        }

        if updated % 50 == 0 && updated > 0 {
            info!(updated = updated, "[reembed] Progress...");
        }
    }

    info!(
        updated = updated,
        total = needs_reembed.len(),
        "[reembed] Re-embedding complete"
    );

    Ok(updated)
}

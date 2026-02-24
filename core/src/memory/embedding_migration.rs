//! Lazy embedding migration engine
//!
//! Re-embeds facts when the configured embedding model changes.
//! Runs in background during idle periods (DreamDaemon, CompressionDaemon)
//! or on-demand via CLI.

use crate::error::AlephError;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::embedding_provider::EmbeddingProvider;
use std::sync::Arc;

/// Progress report from a migration batch
#[derive(Debug, Clone)]
pub struct MigrationProgress {
    /// Number of facts migrated in this batch
    pub migrated: usize,
    /// Number of facts remaining to migrate
    pub remaining: usize,
    /// Number of facts that failed to re-embed
    pub failed: usize,
}

/// Lazy embedding migration engine
///
/// Detects facts with outdated embeddings and re-embeds them
/// using the current embedding provider.
pub struct EmbeddingMigration {
    database: MemoryBackend,
    provider: Arc<dyn EmbeddingProvider>,
    batch_size: usize,
}

impl EmbeddingMigration {
    pub fn new(
        database: MemoryBackend,
        provider: Arc<dyn EmbeddingProvider>,
        batch_size: usize,
    ) -> Self {
        Self {
            database,
            provider,
            batch_size,
        }
    }

    /// Get count of facts needing migration
    ///
    /// TODO: Implement efficiently via store trait when embedding_model filter is supported.
    pub async fn pending_count(&self) -> Result<usize, AlephError> {
        let current_model = self.provider.model_name().to_string();
        // Fetch all valid facts and count those with mismatched model
        let facts = self.database.get_all_facts(false).await?;
        let count = facts.iter().filter(|f| f.embedding_model != current_model).count();
        Ok(count)
    }

    /// Fetch facts needing migration from the database
    ///
    /// TODO: Implement efficiently via store trait when embedding_model filter is supported.
    async fn fetch_facts_batch(
        &self,
        current_model: &str,
    ) -> Result<Vec<(String, String)>, AlephError> {
        let facts = self.database.get_all_facts(false).await?;
        let result: Vec<(String, String)> = facts.into_iter()
            .filter(|f| f.embedding_model != current_model)
            .take(self.batch_size)
            .map(|f| (f.id, f.content))
            .collect();
        Ok(result)
    }

    /// Run one batch of migration
    ///
    /// Returns progress report. Call repeatedly until `remaining == 0`.
    pub async fn run_batch(&self) -> Result<MigrationProgress, AlephError> {
        let current_model = self.provider.model_name().to_string();

        // 1. Fetch batch of facts needing migration
        let facts = self.fetch_facts_batch(&current_model).await?;

        if facts.is_empty() {
            let remaining = self.pending_count().await?;
            return Ok(MigrationProgress {
                migrated: 0,
                remaining,
                failed: 0,
            });
        }

        // 2. Re-embed in batch
        let texts: Vec<&str> = facts.iter().map(|(_, content)| content.as_str()).collect();
        let embeddings = self.provider.embed_batch(&texts).await?;

        // 3. Update each fact
        let mut migrated = 0;
        let mut failed = 0;

        for ((id, _), embedding) in facts.iter().zip(embeddings.into_iter()) {
            match self.update_fact_embedding(id, &embedding, &current_model).await {
                Ok(()) => migrated += 1,
                Err(e) => {
                    tracing::warn!(fact_id = %id, error = %e, "Failed to migrate fact embedding");
                    failed += 1;
                }
            }
        }

        let remaining = self.pending_count().await?;

        tracing::info!(
            migrated = migrated,
            failed = failed,
            remaining = remaining,
            model = %current_model,
            "Embedding migration batch complete"
        );

        Ok(MigrationProgress {
            migrated,
            remaining,
            failed,
        })
    }

    /// Update a single fact's embedding and model metadata
    async fn update_fact_embedding(
        &self,
        fact_id: &str,
        embedding: &[f32],
        model_name: &str,
    ) -> Result<(), AlephError> {
        // Get existing fact, update embedding and model, then save
        let fact = self.database.get_fact(fact_id).await?;
        if let Some(mut fact) = fact {
            fact.embedding = Some(embedding.to_vec());
            fact.embedding_model = model_name.to_string();
            self.database.update_fact(&fact).await?;
        }
        Ok(())
    }

    /// Run migration to completion
    ///
    /// Keeps running batches until no more facts need migration.
    /// Returns total migrated count.
    pub async fn run_to_completion(&self) -> Result<usize, AlephError> {
        let mut total_migrated = 0;

        loop {
            let progress = self.run_batch().await?;
            total_migrated += progress.migrated;

            if progress.remaining == 0 || progress.migrated == 0 {
                break;
            }
        }

        Ok(total_migrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires LanceMemoryBackend"]
    #[allow(unreachable_code, unused_variables, clippy::diverging_sub_expression)]
    async fn test_migration_on_empty_database() {
        // TODO: Migrate test to use LanceMemoryBackend
        let db: MemoryBackend = unimplemented!("Migrate test");

        let temp_dir = tempfile::TempDir::new().unwrap();
        let embedder = crate::memory::SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(crate::memory::embedding_provider::LocalEmbeddingProvider::new(embedder));

        let migration = EmbeddingMigration::new(db, provider, 10);

        let count = migration.pending_count().await.unwrap();
        assert_eq!(count, 0);

        let progress = migration.run_batch().await.unwrap();
        assert_eq!(progress.migrated, 0);
        assert_eq!(progress.remaining, 0);
    }
}

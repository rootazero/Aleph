//! Lazy embedding migration engine
//!
//! Re-embeds facts when the configured embedding model changes.
//! Runs in background during idle periods (DreamDaemon, CompressionDaemon)
//! or on-demand via CLI.

use crate::error::AlephError;
use crate::memory::database::VectorDatabase;
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
    database: Arc<VectorDatabase>,
    provider: Arc<dyn EmbeddingProvider>,
    batch_size: usize,
}

impl EmbeddingMigration {
    pub fn new(
        database: Arc<VectorDatabase>,
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
    pub async fn pending_count(&self) -> Result<usize, AlephError> {
        let current_model = self.provider.model_name().to_string();
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 1 AND embedding_model != ?1",
                rusqlite::params![current_model],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to count pending migrations: {}", e)))?;

        Ok(count as usize)
    }

    /// Fetch facts needing migration from the database
    fn fetch_facts_batch(
        &self,
        current_model: &str,
    ) -> Result<Vec<(String, String)>, AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                "SELECT id, content FROM memory_facts
                 WHERE is_valid = 1 AND embedding_model != ?1
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare migration query: {}", e)))?;

        let rows = stmt.query_map(
            rusqlite::params![current_model, self.batch_size as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| AlephError::config(format!("Failed to query facts for migration: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            let item = row.map_err(|e| {
                AlephError::config(format!("Failed to read migration fact: {}", e))
            })?;
            result.push(item);
        }
        Ok(result)
    }

    /// Run one batch of migration
    ///
    /// Returns progress report. Call repeatedly until `remaining == 0`.
    pub async fn run_batch(&self) -> Result<MigrationProgress, AlephError> {
        let current_model = self.provider.model_name().to_string();

        // 1. Fetch batch of facts needing migration
        let facts = self.fetch_facts_batch(&current_model)?;

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

    /// Update a single fact's embedding and vec0 entry
    async fn update_fact_embedding(
        &self,
        fact_id: &str,
        embedding: &[f32],
        model_name: &str,
    ) -> Result<(), AlephError> {
        let embedding_bytes = VectorDatabase::serialize_embedding(embedding);

        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get rowid for vec0 update
        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM memory_facts WHERE id = ?1",
                rusqlite::params![fact_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to get rowid: {}", e)))?;

        // Update embedding BLOB and model name
        conn.execute(
            "UPDATE memory_facts SET embedding = ?1, embedding_model = ?2 WHERE id = ?3",
            rusqlite::params![embedding_bytes, model_name, fact_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update fact embedding: {}", e)))?;

        // Update vec0 (delete old + insert new)
        conn.execute(
            "DELETE FROM facts_vec WHERE rowid = ?1",
            rusqlite::params![rowid],
        )
        .map_err(|e| AlephError::config(format!("Failed to delete old vec0 entry: {}", e)))?;

        conn.execute(
            "INSERT INTO facts_vec (rowid, embedding) VALUES (?1, ?2)",
            rusqlite::params![rowid, embedding_bytes],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert new vec0 entry: {}", e)))?;

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
    async fn test_migration_on_empty_database() {
        let db = VectorDatabase::in_memory().unwrap();
        let db = Arc::new(db);

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

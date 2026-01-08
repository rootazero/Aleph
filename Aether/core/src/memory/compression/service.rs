//! Compression Service
//!
//! Main service that orchestrates memory compression:
//! 1. Fetches uncompressed memories
//! 2. Extracts facts using LLM
//! 3. Detects and resolves conflicts
//! 4. Stores facts and updates compression state

use super::conflict::{ConflictConfig, ConflictDetector};
use super::extractor::FactExtractor;
use super::scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
use crate::error::AetherError;
use crate::memory::context::{CompressionResult, CompressionSession};
use crate::memory::database::VectorDatabase;
use crate::memory::embedding::EmbeddingModel;
use crate::providers::AiProvider;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio::time::interval;

/// Configuration for the compression service
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Batch size for compression (max memories per batch)
    pub batch_size: u32,
    /// Scheduler configuration
    pub scheduler: SchedulerConfig,
    /// Conflict detection configuration
    pub conflict: ConflictConfig,
    /// Background task interval in seconds
    pub background_interval_seconds: u32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            batch_size: 50,
            scheduler: SchedulerConfig::default(),
            conflict: ConflictConfig::default(),
            background_interval_seconds: 3600, // 1 hour
        }
    }
}

/// Main compression service
pub struct CompressionService {
    database: Arc<VectorDatabase>,
    extractor: Arc<FactExtractor>,
    conflict_detector: Arc<ConflictDetector>,
    scheduler: Arc<CompressionScheduler>,
    config: CompressionConfig,
    provider_name: String,
}

impl CompressionService {
    /// Create a new compression service
    pub fn new(
        database: Arc<VectorDatabase>,
        provider: Arc<dyn AiProvider>,
        embedding_model: Arc<EmbeddingModel>,
        config: CompressionConfig,
    ) -> Self {
        let provider_name = provider.name().to_string();

        let extractor = Arc::new(FactExtractor::new(provider, embedding_model));

        let conflict_detector =
            Arc::new(ConflictDetector::new(Arc::clone(&database), config.conflict.clone()));

        let scheduler = Arc::new(CompressionScheduler::new(config.scheduler.clone()));

        Self {
            database,
            extractor,
            conflict_detector,
            scheduler,
            config,
            provider_name,
        }
    }

    /// Execute a compression operation
    pub async fn compress(&self) -> Result<CompressionResult, AetherError> {
        let start = Instant::now();

        // 1. Get last compression timestamp
        let last_timestamp = self
            .database
            .get_last_compression_timestamp()
            .await?
            .unwrap_or(0);

        // 2. Get uncompressed memories
        let memories = self
            .database
            .get_uncompressed_memories(last_timestamp, self.config.batch_size)
            .await?;

        if memories.is_empty() {
            tracing::debug!("No memories to compress");
            return Ok(CompressionResult::empty());
        }

        tracing::info!(
            memory_count = memories.len(),
            since_timestamp = last_timestamp,
            "Starting memory compression"
        );

        // 3. Extract facts using LLM
        let extracted_facts = match self.extractor.extract_facts(&memories).await {
            Ok(facts) => facts,
            Err(e) => {
                tracing::error!(error = %e, "Failed to extract facts from memories");
                return Err(e);
            }
        };

        tracing::info!(
            extracted_count = extracted_facts.len(),
            "Extracted facts from memories"
        );

        // 4. Process each fact (conflict detection and storage)
        let mut stored_fact_ids = Vec::new();
        let mut total_invalidated = 0u32;

        for fact in extracted_facts {
            // Detect conflicts
            let resolutions = self.conflict_detector.resolve_conflicts(&fact).await?;

            // Apply resolutions (invalidate old facts)
            let invalidated = self.conflict_detector.apply_resolutions(&resolutions).await?;
            total_invalidated += invalidated;

            // Store the new fact
            match self.database.insert_fact(fact.clone()).await {
                Ok(_) => {
                    stored_fact_ids.push(fact.id.clone());
                    tracing::debug!(
                        fact_id = %fact.id,
                        content = %fact.content,
                        "Stored compressed fact"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        fact_content = %fact.content,
                        error = %e,
                        "Failed to store fact"
                    );
                }
            }
        }

        // 5. Update compression timestamp
        let latest_timestamp = memories
            .iter()
            .map(|m| m.context.timestamp)
            .max()
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
            });

        self.database
            .set_last_compression_timestamp(latest_timestamp)
            .await?;

        // 6. Record compression session
        let duration_ms = start.elapsed().as_millis() as u64;
        let session = CompressionSession::new(
            memories.iter().map(|m| m.id.clone()).collect(),
            stored_fact_ids.clone(),
            self.provider_name.clone(),
            duration_ms,
        );

        self.database.record_compression_session(session).await?;

        // 7. Reset scheduler
        self.scheduler.reset_turns();

        let result = CompressionResult {
            memories_processed: memories.len() as u32,
            facts_extracted: stored_fact_ids.len() as u32,
            facts_invalidated: total_invalidated,
            duration_ms,
        };

        tracing::info!(
            memories = result.memories_processed,
            facts = result.facts_extracted,
            invalidated = result.facts_invalidated,
            duration_ms = result.duration_ms,
            "Memory compression completed"
        );

        Ok(result)
    }

    /// Check if compression should be triggered and execute if needed
    pub async fn check_and_compress(&self) -> Result<Option<CompressionResult>, AetherError> {
        let trigger = self.scheduler.should_trigger_compression();

        match trigger {
            CompressionTrigger::None => {
                tracing::trace!("No compression trigger, skipping");
                Ok(None)
            }
            trigger => {
                tracing::info!(trigger = ?trigger, "Compression triggered");
                let result = self.compress().await?;
                Ok(Some(result))
            }
        }
    }

    /// Start background compression task
    ///
    /// Similar to CleanupService, runs periodically to check for compression triggers.
    pub fn start_background_task(self: Arc<Self>) -> JoinHandle<()> {
        let interval_secs = self.config.background_interval_seconds;

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(interval_secs as u64));

            tracing::info!(
                interval_seconds = interval_secs,
                "Started background compression task"
            );

            loop {
                interval.tick().await;

                match self.check_and_compress().await {
                    Ok(Some(result)) => {
                        tracing::info!(
                            facts = result.facts_extracted,
                            duration_ms = result.duration_ms,
                            "Background compression completed"
                        );
                    }
                    Ok(None) => {
                        tracing::trace!("Background compression check: no action needed");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Background compression failed");
                    }
                }
            }
        })
    }

    /// Record user activity (for idle detection)
    pub fn record_activity(&self) {
        self.scheduler.record_activity();
    }

    /// Record a conversation turn (for turn threshold)
    pub fn record_turn(&self) {
        self.scheduler.increment_turns();
    }

    /// Get the scheduler for external monitoring
    pub fn get_scheduler(&self) -> Arc<CompressionScheduler> {
        Arc::clone(&self.scheduler)
    }

    /// Get current configuration
    pub fn get_config(&self) -> &CompressionConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::create_mock_provider;
    use tempfile::tempdir;

    async fn create_test_service() -> (CompressionService, Arc<VectorDatabase>) {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_compression.db");
        let database = Arc::new(VectorDatabase::new(db_path).unwrap());

        let provider = create_mock_provider();
        let model_dir = temp_dir.path().join("models");
        std::fs::create_dir_all(&model_dir).unwrap();
        let embedding_model = Arc::new(EmbeddingModel::new(model_dir).unwrap());

        let config = CompressionConfig::default();

        let service = CompressionService::new(
            Arc::clone(&database),
            provider,
            embedding_model,
            config,
        );

        (service, database)
    }

    #[tokio::test]
    async fn test_compress_empty_memories() {
        let (service, _) = create_test_service().await;

        let result = service.compress().await.unwrap();

        assert_eq!(result.memories_processed, 0);
        assert_eq!(result.facts_extracted, 0);
    }

    #[tokio::test]
    async fn test_scheduler_integration() {
        let (service, _) = create_test_service().await;

        // Record activity
        service.record_activity();

        // Record turns
        for _ in 0..5 {
            service.record_turn();
        }

        let scheduler = service.get_scheduler();
        assert_eq!(scheduler.get_pending_turns(), 5);
    }

    #[test]
    fn test_config_default() {
        let config = CompressionConfig::default();
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.background_interval_seconds, 3600);
    }
}

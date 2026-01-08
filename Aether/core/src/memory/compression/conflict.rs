//! Conflict Detection and Resolution
//!
//! Detects conflicting facts using vector similarity and resolves them
//! by invalidating older facts (new facts always win).

use crate::error::AetherError;
use crate::memory::context::MemoryFact;
use crate::memory::database::VectorDatabase;
use std::sync::Arc;

/// Result of conflict resolution
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// No conflict detected
    NoConflict,
    /// Old fact should be invalidated
    InvalidateOld {
        /// ID of the old fact to invalidate
        old_fact_id: String,
        /// Reason for invalidation
        reason: String,
    },
}

/// Configuration for conflict detection
#[derive(Debug, Clone)]
pub struct ConflictConfig {
    /// Similarity threshold for conflict detection (default: 0.85)
    pub similarity_threshold: f32,
}

impl Default for ConflictConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.85,
        }
    }
}

/// Detects and resolves conflicting facts
pub struct ConflictDetector {
    database: Arc<VectorDatabase>,
    config: ConflictConfig,
}

impl ConflictDetector {
    /// Create a new conflict detector
    pub fn new(database: Arc<VectorDatabase>, config: ConflictConfig) -> Self {
        Self { database, config }
    }

    /// Create with default configuration
    pub fn with_defaults(database: Arc<VectorDatabase>) -> Self {
        Self::new(database, ConflictConfig::default())
    }

    /// Detect and resolve conflicts for a new fact
    ///
    /// Strategy: New facts always override old similar facts.
    /// This is based on the assumption that more recent information is more accurate.
    pub async fn resolve_conflicts(
        &self,
        new_fact: &MemoryFact,
    ) -> Result<Vec<ConflictResolution>, AetherError> {
        let embedding = new_fact.embedding.as_ref().ok_or_else(|| {
            AetherError::config("Cannot detect conflicts for fact without embedding")
        })?;

        // Find similar existing facts
        let similar_facts = self
            .database
            .find_similar_facts(embedding, self.config.similarity_threshold, Some(&new_fact.id))
            .await?;

        if similar_facts.is_empty() {
            return Ok(vec![ConflictResolution::NoConflict]);
        }

        // For each similar fact, create an invalidation resolution
        // New facts always win (user-confirmed design decision)
        let resolutions: Vec<ConflictResolution> = similar_facts
            .into_iter()
            .map(|old_fact| {
                let similarity = old_fact.similarity_score.unwrap_or(0.0);

                tracing::info!(
                    old_fact_id = %old_fact.id,
                    old_content = %old_fact.content,
                    new_content = %new_fact.content,
                    similarity = similarity,
                    "Detected conflicting fact, will invalidate old"
                );

                ConflictResolution::InvalidateOld {
                    old_fact_id: old_fact.id.clone(),
                    reason: format!(
                        "Superseded by newer fact (similarity: {:.2}): {}",
                        similarity,
                        new_fact.content.chars().take(100).collect::<String>()
                    ),
                }
            })
            .collect();

        Ok(resolutions)
    }

    /// Apply conflict resolutions by invalidating old facts
    pub async fn apply_resolutions(
        &self,
        resolutions: &[ConflictResolution],
    ) -> Result<u32, AetherError> {
        let mut invalidated_count = 0;

        for resolution in resolutions {
            if let ConflictResolution::InvalidateOld {
                old_fact_id,
                reason,
            } = resolution
            {
                match self.database.invalidate_fact(old_fact_id, reason).await {
                    Ok(_) => {
                        invalidated_count += 1;
                        tracing::debug!(fact_id = %old_fact_id, "Invalidated conflicting fact");
                    }
                    Err(e) => {
                        tracing::warn!(
                            fact_id = %old_fact_id,
                            error = %e,
                            "Failed to invalidate fact"
                        );
                    }
                }
            }
        }

        Ok(invalidated_count)
    }

    /// Update configuration
    pub fn update_config(&mut self, config: ConflictConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn get_config(&self) -> &ConflictConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;
    use tempfile::tempdir;

    async fn create_test_detector() -> ConflictDetector {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_conflict.db");
        let database = Arc::new(VectorDatabase::new(db_path).unwrap());
        ConflictDetector::with_defaults(database)
    }

    #[tokio::test]
    async fn test_no_conflict_when_empty() {
        let detector = create_test_detector().await;

        let fact = MemoryFact::new(
            "The user likes Rust".to_string(),
            FactType::Preference,
            vec!["mem-1".to_string()],
        )
        .with_embedding(vec![0.1; 512]);

        let resolutions = detector.resolve_conflicts(&fact).await.unwrap();

        assert_eq!(resolutions.len(), 1);
        assert!(matches!(resolutions[0], ConflictResolution::NoConflict));
    }

    #[tokio::test]
    async fn test_conflict_detection_with_similar_fact() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_conflict2.db");
        let database = Arc::new(VectorDatabase::new(db_path).unwrap());

        // Insert an existing fact
        let old_fact = MemoryFact::new(
            "The user is learning Python".to_string(),
            FactType::Learning,
            vec!["mem-old".to_string()],
        )
        .with_embedding(vec![0.5; 512]);

        database.insert_fact(old_fact.clone()).await.unwrap();

        let detector = ConflictDetector::with_defaults(database);

        // Create a very similar new fact (same embedding = similarity 1.0)
        let new_fact = MemoryFact::new(
            "The user stopped learning Python".to_string(),
            FactType::Learning,
            vec!["mem-new".to_string()],
        )
        .with_embedding(vec![0.5; 512]); // Same embedding = conflict

        let resolutions = detector.resolve_conflicts(&new_fact).await.unwrap();

        assert!(!resolutions.is_empty());
        assert!(matches!(
            resolutions[0],
            ConflictResolution::InvalidateOld { .. }
        ));
    }

    #[test]
    fn test_config_default() {
        let config = ConflictConfig::default();
        assert!((config.similarity_threshold - 0.85).abs() < 0.01);
    }
}

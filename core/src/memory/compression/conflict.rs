//! Conflict Detection and Resolution
//!
//! Detects conflicting facts using vector similarity and resolves them
//! using three strategies: Override, Reject, or Merge.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Strategy for merging facts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum MergeStrategy {
    /// Generalize: "likes Rust" + "likes Go" → "likes systems languages"
    Generalize,
    /// Specialize: "likes coffee" + "likes dark roast" → "likes dark roast coffee"
    Specialize,
    /// Enumerate: "likes Rust, Go, and Zig"
    #[default]
    Enumerate,
}


/// Result of conflict resolution
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// No conflict detected
    NoConflict,
    /// Override: new fact replaces old (default for correction signals)
    Override {
        /// ID of the old fact to invalidate
        invalidated_id: String,
        /// Reason for override
        reason: String,
    },
    /// Reject: keep old fact, discard new (confidence comparison)
    Reject {
        /// Content that was rejected
        rejected_content: String,
        /// Reason for rejection
        reason: String,
    },
    /// Merge: combine into more precise statement
    Merge {
        /// ID of the old fact to merge with
        old_id: String,
        /// New merged content
        new_content: String,
        /// Strategy used for merging
        merge_strategy: MergeStrategy,
    },
    /// Old fact should be invalidated (legacy, kept for backward compatibility)
    #[deprecated(since = "0.2.0", note = "Use Override instead")]
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
    database: MemoryBackend,
    config: ConflictConfig,
}

impl ConflictDetector {
    /// Create a new conflict detector
    pub fn new(database: MemoryBackend, config: ConflictConfig) -> Self {
        Self { database, config }
    }

    /// Create with default configuration
    pub fn with_defaults(database: MemoryBackend) -> Self {
        Self::new(database, ConflictConfig::default())
    }

    /// Detect and resolve conflicts for a new fact
    ///
    /// Strategy: New facts always override old similar facts.
    /// This is based on the assumption that more recent information is more accurate.
    pub async fn resolve_conflicts(
        &self,
        new_fact: &MemoryFact,
    ) -> Result<Vec<ConflictResolution>, AlephError> {
        let embedding = new_fact.embedding.as_ref().ok_or_else(|| {
            AlephError::config("Cannot detect conflicts for fact without embedding")
        })?;

        // Find similar existing facts
        let filter = crate::memory::store::types::SearchFilter::valid_only(
            Some(crate::memory::NamespaceScope::Owner),
        );
        let similar_facts = self
            .database
            .find_similar_facts(
                embedding,
                crate::memory::EMBEDDING_DIM as u32,
                &filter,
                self.config.similarity_threshold,
                20, // reasonable limit
            )
            .await?;

        if similar_facts.is_empty() {
            return Ok(vec![ConflictResolution::NoConflict]);
        }

        // For each similar fact, create an override resolution
        // New facts always win (user-confirmed design decision)
        let resolutions: Vec<ConflictResolution> = similar_facts
            .into_iter()
            .filter(|sf| sf.fact.id != new_fact.id) // exclude self
            .map(|scored_fact| {
                let similarity = scored_fact.score;

                tracing::info!(
                    old_fact_id = %scored_fact.fact.id,
                    old_content = %scored_fact.fact.content,
                    new_content = %new_fact.content,
                    similarity = similarity,
                    "Detected conflicting fact, will override old"
                );

                ConflictResolution::Override {
                    invalidated_id: scored_fact.fact.id.clone(),
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
    ) -> Result<u32, AlephError> {
        let mut invalidated_count = 0;

        for resolution in resolutions {
            // Handle Override (new) and InvalidateOld (legacy) for invalidation
            let (fact_id, reason) = match resolution {
                ConflictResolution::Override {
                    invalidated_id,
                    reason,
                } => (invalidated_id, reason),
                #[allow(deprecated)]
                ConflictResolution::InvalidateOld {
                    old_fact_id,
                    reason,
                } => (old_fact_id, reason),
                // Skip other resolution types for now
                ConflictResolution::NoConflict
                | ConflictResolution::Reject { .. }
                | ConflictResolution::Merge { .. } => continue,
            };

            match self.database.invalidate_fact(fact_id, reason).await {
                Ok(_) => {
                    invalidated_count += 1;
                    tracing::debug!(fact_id = %fact_id, "Invalidated conflicting fact");
                }
                Err(e) => {
                    tracing::warn!(
                        fact_id = %fact_id,
                        error = %e,
                        "Failed to invalidate fact"
                    );
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
        let database: MemoryBackend =
            Arc::new(crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap());
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
        .with_embedding(vec![0.1; crate::memory::EMBEDDING_DIM]);

        let resolutions = detector.resolve_conflicts(&fact).await.unwrap();

        assert_eq!(resolutions.len(), 1);
        assert!(matches!(resolutions[0], ConflictResolution::NoConflict));
    }

    #[tokio::test]
    async fn test_conflict_detection_with_similar_fact() {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend =
            Arc::new(crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap());

        // Insert an existing fact
        let old_fact = MemoryFact::new(
            "The user is learning Python".to_string(),
            FactType::Learning,
            vec!["mem-old".to_string()],
        )
        .with_embedding(vec![0.5; crate::memory::EMBEDDING_DIM]);

        database.insert_fact(&old_fact).await.unwrap();

        let detector = ConflictDetector::with_defaults(database);

        // Create a very similar new fact (same embedding = similarity 1.0)
        let new_fact = MemoryFact::new(
            "The user stopped learning Python".to_string(),
            FactType::Learning,
            vec!["mem-new".to_string()],
        )
        .with_embedding(vec![0.5; crate::memory::EMBEDDING_DIM]); // Same embedding = conflict

        let resolutions = detector.resolve_conflicts(&new_fact).await.unwrap();

        assert!(!resolutions.is_empty());
        assert!(matches!(
            resolutions[0],
            ConflictResolution::Override { .. }
        ));
    }

    #[test]
    fn test_config_default() {
        let config = ConflictConfig::default();
        assert!((config.similarity_threshold - 0.85).abs() < 0.01);
    }

    #[test]
    fn test_merge_strategy() {
        let resolution = ConflictResolution::Merge {
            old_id: "fact-1".to_string(),
            new_content: "User likes Rust and Go".to_string(),
            merge_strategy: MergeStrategy::Enumerate,
        };

        assert!(matches!(resolution, ConflictResolution::Merge { .. }));
    }

    #[test]
    fn test_reject_strategy() {
        let resolution = ConflictResolution::Reject {
            rejected_content: "User dislikes Rust".to_string(),
            reason: "Contradicts high-confidence fact".to_string(),
        };

        assert!(matches!(resolution, ConflictResolution::Reject { .. }));
    }

    #[test]
    fn test_merge_strategy_default() {
        let strategy = MergeStrategy::default();
        assert_eq!(strategy, MergeStrategy::Enumerate);
    }

    #[test]
    fn test_override_strategy() {
        let resolution = ConflictResolution::Override {
            invalidated_id: "fact-old".to_string(),
            reason: "User explicitly corrected this fact".to_string(),
        };

        assert!(matches!(resolution, ConflictResolution::Override { .. }));
    }
}

//! Conflict Detection and Resolution
//!
//! Detects conflicting facts using vector similarity and resolves them
//! using three strategies: Override, Reject, or Merge.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};

/// Strategy for merging facts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
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

/// Verdict from LLM conflict arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictVerdict {
    /// New fact is an updated version of the old (invalidate old).
    SameUpdated,
    /// New fact contradicts the old (invalidate old).
    Contradicts,
    /// Both facts are independently true (keep both).
    Coexists,
}

/// Parse a conflict verdict from LLM JSON response.
///
/// Falls back to `Coexists` on parse failure (conservative — keep both).
pub fn parse_conflict_verdict(response: &str) -> ConflictVerdict {
    #[derive(serde::Deserialize)]
    struct VerdictResponse {
        verdict: String,
        #[allow(dead_code)]
        reason: Option<String>,
    }

    let parsed: Option<VerdictResponse> =
        crate::utils::json_extract::extract_json_robust(response)
            .and_then(|v| serde_json::from_value(v).ok());

    match parsed {
        Some(r) => match r.verdict.as_str() {
            "same_updated" => ConflictVerdict::SameUpdated,
            "contradicts" => ConflictVerdict::Contradicts,
            "coexists" => ConflictVerdict::Coexists,
            _ => ConflictVerdict::Coexists,
        },
        None => ConflictVerdict::Coexists,
    }
}

/// Detects and resolves conflicting facts
pub struct ConflictDetector {
    database: MemoryBackend,
    config: ConflictConfig,
    provider: Option<Arc<dyn crate::providers::AiProvider>>,
}

impl ConflictDetector {
    /// Create a new conflict detector
    pub fn new(database: MemoryBackend, config: ConflictConfig) -> Self {
        Self {
            database,
            config,
            provider: None,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(database: MemoryBackend) -> Self {
        Self::new(database, ConflictConfig::default())
    }

    /// Attach an AI provider for LLM-based conflict arbitration.
    pub fn with_provider(mut self, provider: Arc<dyn crate::providers::AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Use LLM to arbitrate between a new fact and an existing similar fact.
    ///
    /// Returns `Coexists` if no provider is available or LLM call fails.
    pub async fn llm_arbitrate(
        &self,
        existing_content: &str,
        new_content: &str,
    ) -> ConflictVerdict {
        let provider = match &self.provider {
            Some(p) => p,
            None => return ConflictVerdict::Coexists,
        };

        let prompt = format!(
            "Given an existing fact and a new fact, classify their relationship:\n\
             - same_updated: The new fact is an updated version of the existing fact\n\
             - contradicts: The new fact contradicts the existing fact\n\
             - coexists: Both facts are independently true\n\n\
             Existing: \"{existing_content}\"\n\
             New: \"{new_content}\"\n\n\
             Output JSON only: {{\"verdict\": \"same_updated|contradicts|coexists\", \"reason\": \"...\"}}"
        );

        let msgs = [crate::providers::message::UnifiedMessage::user(&prompt)];
        let payload = crate::providers::adapter::RequestPayload::new(&msgs)
            .with_system(Some("You are a precise fact comparison assistant. Output JSON only."));

        match provider.process(payload).await {
            Ok(response) => parse_conflict_verdict(&response.text_content()),
            Err(e) => {
                tracing::warn!(error = %e, "LLM conflict arbitration failed, defaulting to coexists");
                ConflictVerdict::Coexists
            }
        }
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
        let filter = crate::memory::store::types::SearchFilter::valid_only(Some(
            crate::memory::NamespaceScope::Owner,
        ));
        let similar_facts = self
            .database
            .find_similar_facts(
                embedding,
                embedding.len() as u32,
                &filter,
                self.config.similarity_threshold,
                20, // reasonable limit
            )
            .await?;

        if similar_facts.is_empty() {
            return Ok(vec![ConflictResolution::NoConflict]);
        }

        // For each similar fact, use LLM arbitration to decide the relationship.
        // Only override when the LLM says the new fact supersedes or contradicts.
        let similar_facts: Vec<_> = similar_facts
            .into_iter()
            .filter(|sf| sf.fact.id != new_fact.id) // exclude self
            .collect();

        let mut resolutions = Vec::new();
        for scored in similar_facts {
            let verdict = self
                .llm_arbitrate(&scored.fact.content, &new_fact.content)
                .await;

            match verdict {
                ConflictVerdict::SameUpdated | ConflictVerdict::Contradicts => {
                    let reason = format!(
                        "{:?}: superseded by new fact (similarity: {:.2})",
                        verdict, scored.score
                    );
                    tracing::info!(
                        old_fact_id = %scored.fact.id,
                        old_content = %scored.fact.content,
                        new_content = %new_fact.content,
                        similarity = scored.score,
                        ?verdict,
                        "LLM verdict: override old fact"
                    );
                    resolutions.push(ConflictResolution::Override {
                        invalidated_id: scored.fact.id.clone(),
                        reason,
                    });
                }
                ConflictVerdict::Coexists => {
                    tracing::debug!(
                        old = %scored.fact.content,
                        new = %new_fact.content,
                        "LLM verdict: coexists, keeping both"
                    );
                }
            }
        }

        Ok(resolutions)
    }

    /// Apply conflict resolutions by invalidating old facts
    pub async fn apply_resolutions(
        &self,
        resolutions: &[ConflictResolution],
    ) -> Result<u32, AlephError> {
        let mut invalidated_count = 0;

        for resolution in resolutions {
            let (fact_id, reason) = match resolution {
                ConflictResolution::Override {
                    invalidated_id,
                    reason,
                } => (invalidated_id, reason),
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
    use crate::memory::context::NoteType;
    use crate::sync_primitives::Arc;
    use tempfile::tempdir;

    async fn create_test_detector() -> ConflictDetector {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend = Arc::new(
            crate::memory::store::SqliteMemoryBackend::new(temp_dir.path())
                .unwrap(),
        );
        ConflictDetector::with_defaults(database)
    }

    #[tokio::test]
    async fn test_no_conflict_when_empty() {
        let detector = create_test_detector().await;

        let fact = MemoryFact::new(
            "The user likes Rust".to_string(),
            NoteType::Preference,
            vec!["mem-1".to_string()],
        )
        .with_embedding(vec![0.1; 1024]);

        let resolutions = detector.resolve_conflicts(&fact).await.unwrap();

        assert_eq!(resolutions.len(), 1);
        assert!(matches!(resolutions[0], ConflictResolution::NoConflict));
    }

    #[tokio::test]
    async fn test_conflict_detection_with_similar_fact() {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend = Arc::new(
            crate::memory::store::SqliteMemoryBackend::new(temp_dir.path())
                .unwrap(),
        );

        // Insert an existing fact
        let old_fact = MemoryFact::new(
            "The user is learning Python".to_string(),
            NoteType::Learning,
            vec!["mem-old".to_string()],
        )
        .with_embedding(vec![0.5; 1024]);

        database.insert_fact(&old_fact).await.unwrap();

        // Provide a mock provider that returns a "contradicts" verdict
        let mock_provider: Arc<dyn crate::providers::AiProvider> = Arc::new(
            crate::providers::MockProvider::new(
                r#"{"verdict": "contradicts", "reason": "User stopped vs started"}"#,
            ),
        );
        let detector = ConflictDetector::with_defaults(database)
            .with_provider(mock_provider);

        // Create a very similar new fact (same embedding = similarity 1.0)
        let new_fact = MemoryFact::new(
            "The user stopped learning Python".to_string(),
            NoteType::Learning,
            vec!["mem-new".to_string()],
        )
        .with_embedding(vec![0.5; 1024]); // Same embedding = conflict

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

    #[test]
    fn test_parse_conflict_verdict_same_updated() {
        let response = r#"{"verdict": "same_updated", "reason": "Updated timeline"}"#;
        let verdict = parse_conflict_verdict(response);
        assert_eq!(verdict, ConflictVerdict::SameUpdated);
    }

    #[test]
    fn test_parse_conflict_verdict_contradicts() {
        let response = r#"{"verdict": "contradicts", "reason": "Changed preference"}"#;
        let verdict = parse_conflict_verdict(response);
        assert_eq!(verdict, ConflictVerdict::Contradicts);
    }

    #[test]
    fn test_parse_conflict_verdict_coexists() {
        let response = r#"{"verdict": "coexists", "reason": "Different topics"}"#;
        let verdict = parse_conflict_verdict(response);
        assert_eq!(verdict, ConflictVerdict::Coexists);
    }

    #[test]
    fn test_parse_conflict_verdict_invalid_defaults_to_coexists() {
        let verdict = parse_conflict_verdict("garbage");
        assert_eq!(verdict, ConflictVerdict::Coexists);
    }

    #[test]
    fn test_parse_conflict_verdict_unknown_value() {
        let response = r#"{"verdict": "unknown_value", "reason": "test"}"#;
        let verdict = parse_conflict_verdict(response);
        assert_eq!(verdict, ConflictVerdict::Coexists);
    }
}

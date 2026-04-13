//! Conflict Detection and Resolution
//!
//! Detects conflicting notes using vector similarity and resolves them
//! using three strategies: Override, Reject, or Merge.
//!
//! NOTE: As of the notes migration, `resolve_conflicts` uses NoteStore
//! vector search instead of the legacy `find_similar_facts` on MemoryFact rows.
//! Conflict "invalidation" is now a warn-log rather than a DB mutation, because
//! the compression pipeline's primary path (`compress_to_notes`) relies on the
//! LLM extractor to deduplicate notes; the ConflictDetector is belt-and-suspenders.

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::MemoryBackend;
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

    let parsed: Option<VerdictResponse> = crate::utils::json_extract::extract_json_robust(response)
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

/// Detects and resolves conflicting notes using NoteStore vector search.
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
        let payload = crate::providers::adapter::RequestPayload::new(&msgs).with_system(Some(
            "You are a precise fact comparison assistant. Output JSON only.",
        ));

        match provider.process(payload).await {
            Ok(response) => parse_conflict_verdict(&response.text_content()),
            Err(e) => {
                tracing::warn!(error = %e, "LLM conflict arbitration failed, defaulting to coexists");
                ConflictVerdict::Coexists
            }
        }
    }

    /// Detect and resolve conflicts for a new fact using NoteStore vector search.
    ///
    /// Strategy: New notes always supersede similar old notes when the LLM confirms
    /// it. Uses `vector_search_notes_with_content` (NoteStore) instead of the
    /// legacy `find_similar_facts` on MemoryFact rows.
    ///
    /// Note: In the primary `compress_to_notes` pipeline, the LLM extractor
    /// deduplicates notes on its own. This method is belt-and-suspenders for any
    /// remaining callers that still work at the `MemoryFact` level.
    pub async fn resolve_conflicts(
        &self,
        new_fact: &MemoryFact,
    ) -> Result<Vec<ConflictResolution>, AlephError> {
        let embedding = new_fact.embedding.as_ref().ok_or_else(|| {
            AlephError::config("Cannot detect conflicts for fact without embedding")
        })?;

        let agent_id = &new_fact.agent;
        let dim = embedding.len() as u32;
        let limit = 20usize;

        // Use NoteStore vector search instead of legacy find_similar_facts.
        let results = self
            .database
            .vector_search_notes_with_content(embedding, agent_id, dim, limit * 2)
            .await
            .unwrap_or_default();

        // Filter client-side by similarity threshold (scores are similarity: higher = more similar).
        let similar: Vec<_> = results
            .into_iter()
            .filter(|r| r.score >= self.config.similarity_threshold && r.path != new_fact.id)
            .take(limit)
            .collect();

        if similar.is_empty() {
            return Ok(vec![ConflictResolution::NoConflict]);
        }

        let mut resolutions = Vec::new();
        for result in similar {
            let verdict = self.llm_arbitrate(&result.content, &new_fact.content).await;

            match verdict {
                ConflictVerdict::SameUpdated | ConflictVerdict::Contradicts => {
                    let reason = format!(
                        "{:?}: superseded by new note (similarity: {:.2})",
                        verdict, result.score
                    );
                    tracing::info!(
                        note_path = %result.path,
                        note_content_preview = %result.content.chars().take(80).collect::<String>(),
                        new_content = %new_fact.content,
                        similarity = result.score,
                        ?verdict,
                        "LLM verdict: note superseded, marking stale"
                    );
                    resolutions.push(ConflictResolution::Override {
                        invalidated_id: result.path.clone(),
                        reason,
                    });
                }
                ConflictVerdict::Coexists => {
                    tracing::debug!(
                        note_path = %result.path,
                        new_content = %new_fact.content,
                        "LLM verdict: coexists, keeping both"
                    );
                }
            }
        }

        Ok(resolutions)
    }

    /// Apply conflict resolutions by logging stale notes.
    ///
    /// Notes are markdown files — hard-deleting them would destroy history.
    /// Instead, we log a warning so the dreaming/decay pipeline can handle
    /// consolidation. The ConflictDetector is belt-and-suspenders; the primary
    /// `compress_to_notes` pipeline relies on LLM deduplication.
    pub async fn apply_resolutions(
        &self,
        resolutions: &[ConflictResolution],
    ) -> Result<u32, AlephError> {
        let mut logged_count = 0u32;

        for resolution in resolutions {
            match resolution {
                ConflictResolution::Override {
                    invalidated_id,
                    reason,
                } => {
                    tracing::warn!(
                        note_path = %invalidated_id,
                        reason = %reason,
                        "ConflictDetector: note superseded — consider consolidating via dream pipeline"
                    );
                    logged_count += 1;
                }
                ConflictResolution::NoConflict
                | ConflictResolution::Reject { .. }
                | ConflictResolution::Merge { .. } => continue,
            }
        }

        Ok(logged_count)
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
        let database: MemoryBackend =
            Arc::new(crate::memory::store::SqliteMemoryBackend::new(temp_dir.path()).unwrap());
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

    // This test previously seeded the database with `insert_fact` (MemoryStore).
    // After the notes migration, resolve_conflicts uses NoteStore vector search.
    // Seeding a note and its embedding for an integration test requires a full
    // NoteIndexer + upsert_embedding setup — deferred to the dream/integration
    // test suite which has the full fixture infrastructure.
    #[ignore = "requires NoteStore integration fixture — see memory integration tests"]
    #[tokio::test]
    async fn test_conflict_detection_with_similar_note() {
        let temp_dir = tempdir().unwrap();
        let database: MemoryBackend =
            Arc::new(crate::memory::store::SqliteMemoryBackend::new(temp_dir.path()).unwrap());

        // Seed a note and its embedding via NoteStore + upsert_embedding
        use crate::memory::notes::store::NoteStore;
        let note = crate::memory::notes::KnowledgeNote {
            title: "learning Python".to_string(),
            category: "learning".to_string(),
            tags: vec![],
            facts: vec!["The user is learning Python".to_string()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
        };
        database
            .index_note(&note, "default", "learning")
            .await
            .unwrap();
        database
            .upsert_embedding(
                "learning/learning Python",
                "default",
                &vec![0.5_f32; 1024],
                1024,
            )
            .await
            .unwrap();

        let mock_provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(crate::providers::MockProvider::new(
                r#"{"verdict": "contradicts", "reason": "User stopped vs started"}"#,
            ));
        let detector = ConflictDetector::with_defaults(database).with_provider(mock_provider);

        let mut new_fact = MemoryFact::new(
            "The user stopped learning Python".to_string(),
            NoteType::Learning,
            vec!["mem-new".to_string()],
        );
        new_fact = new_fact.with_embedding(vec![0.5_f32; 1024]);

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

//! Context Comptroller Module
//!
//! Provides post-retrieval arbitration to eliminate redundancy between
//! Facts and Transcripts in the context window.

pub mod comptroller;
pub mod config;
pub mod types;

pub use comptroller::ContextComptroller;
pub use config::ComptrollerConfig;
pub use types::{ArbitratedContext, RetentionMode, TokenBudget};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{ContextAnchor, FactType, MemoryEntry, MemoryFact};
    use crate::memory::fact_retrieval::RetrievalResult;

    #[test]
    fn test_detect_redundancy_high_similarity() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        // Create a fact and a transcript with similar content
        let fact = MemoryFact::new(
            "User prefers Rust for systems programming".to_string(),
            FactType::Preference,
            vec!["mem-1".to_string()],
        )
        .with_embedding(vec![0.1; 384])
        .with_score(0.9);

        let entry_id = uuid::Uuid::new_v4().to_string();
        let mut transcript = MemoryEntry::new(
            entry_id.clone(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "I really prefer Rust".to_string(),
            "Rust is great for systems programming".to_string(),
        );
        transcript.embedding = Some(vec![0.1; 384]);
        transcript.similarity_score = Some(0.85);

        let result = RetrievalResult {
            facts: vec![fact],
            raw_memories: vec![transcript],
        };

        let arbitrated = comptroller.arbitrate(result, TokenBudget::new(10000));

        // Should keep transcript, remove fact (prefer original)
        assert_eq!(arbitrated.facts.len(), 0);
        assert_eq!(arbitrated.raw_memories.len(), 1);
        assert!(arbitrated.tokens_saved > 0);
    }

    #[test]
    fn test_no_redundancy() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        // Create unrelated fact and transcript with different embeddings
        let mut fact_embedding = vec![0.0; 384];
        fact_embedding[0] = 1.0;  // Point in first dimension

        let fact = MemoryFact::new(
            "User likes Python".to_string(),
            FactType::Preference,
            vec!["mem-2".to_string()],
        )
        .with_embedding(fact_embedding);

        let entry_id = uuid::Uuid::new_v4().to_string();
        let mut transcript = MemoryEntry::new(
            entry_id,
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "What is Rust?".to_string(),
            "Rust is a systems language".to_string(),
        );
        let mut transcript_embedding = vec![0.0; 384];
        transcript_embedding[1] = 1.0;  // Point in second dimension (orthogonal)
        transcript.embedding = Some(transcript_embedding);

        let result = RetrievalResult {
            facts: vec![fact],
            raw_memories: vec![transcript],
        };

        let arbitrated = comptroller.arbitrate(result, TokenBudget::new(10000));

        // Should keep both (no redundancy)
        assert_eq!(arbitrated.facts.len(), 1);
        assert_eq!(arbitrated.raw_memories.len(), 1);
        assert_eq!(arbitrated.tokens_saved, 0);
    }
}

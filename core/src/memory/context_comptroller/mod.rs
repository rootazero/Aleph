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
        .with_embedding(vec![0.1; 1024])
        .with_score(0.9);

        let entry_id = uuid::Uuid::new_v4().to_string();
        let mut transcript = MemoryEntry::new(
            entry_id.clone(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "I really prefer Rust".to_string(),
            "Rust is great for systems programming".to_string(),
        );
        transcript.embedding = Some(vec![0.1; 1024]);
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
        let mut fact_embedding = vec![0.0; 1024];
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
        let mut transcript_embedding = vec![0.0; 1024];
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

    #[test]
    fn test_token_budget_enforcement() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        // Create multiple facts and transcripts
        let mut facts = Vec::new();
        for i in 0..5 {
            let mut fact = MemoryFact::new(
                format!("Fact number {} with some content", i),
                FactType::Preference,
                vec![],
            );
            fact.embedding = Some(vec![0.1 * (i as f32); 1024]);
            fact.similarity_score = Some(0.9 - (i as f32) * 0.1);  // Decreasing scores
            facts.push(fact);
        }

        let mut transcripts = Vec::new();
        for i in 0..5 {
            let entry_id = uuid::Uuid::new_v4().to_string();
            let mut transcript = MemoryEntry::new(
                entry_id,
                ContextAnchor::now("test".to_string(), "test".to_string()),
                format!("Question {}", i),
                format!("Answer {}", i),
            );
            transcript.embedding = Some(vec![0.2 * (i as f32); 1024]);
            transcript.similarity_score = Some(0.8 - (i as f32) * 0.1);  // Decreasing scores
            transcripts.push(transcript);
        }

        let result = RetrievalResult {
            facts,
            raw_memories: transcripts,
        };

        // Set a small budget (only enough for 2-3 items)
        let budget = TokenBudget::new(100);

        let arbitrated = comptroller.arbitrate(result, budget);

        // Should have trimmed to fit budget
        let total_items = arbitrated.facts.len() + arbitrated.raw_memories.len();
        assert!(total_items < 10, "Should have trimmed items to fit budget");
        assert!(total_items > 0, "Should have kept at least some items");

        // Facts should be prioritized (higher similarity scores should be kept)
        if !arbitrated.facts.is_empty() {
            // First fact should have highest score
            assert!(arbitrated.facts[0].similarity_score.unwrap_or(0.0) >= 0.8);
        }
    }

    #[test]
    fn test_priority_sorting() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        // Create facts with different similarity scores
        let mut fact1 = MemoryFact::new(
            "Low priority fact".to_string(),
            FactType::Preference,
            vec![],
        );
        fact1.similarity_score = Some(0.3);
        fact1.embedding = Some(vec![0.1; 1024]);

        let mut fact2 = MemoryFact::new(
            "High priority fact".to_string(),
            FactType::Preference,
            vec![],
        );
        fact2.similarity_score = Some(0.9);
        fact2.embedding = Some(vec![0.2; 1024]);

        let result = RetrievalResult {
            facts: vec![fact1, fact2],
            raw_memories: vec![],
        };

        let budget = TokenBudget::new(10000);  // Large budget
        let arbitrated = comptroller.arbitrate(result, budget);

        // High priority fact should come first
        assert_eq!(arbitrated.facts.len(), 2);
        assert!(arbitrated.facts[0].similarity_score.unwrap() > arbitrated.facts[1].similarity_score.unwrap());
        assert_eq!(arbitrated.facts[0].content, "High priority fact");
    }
}

//! Context Comptroller Module
//!
//! Provides post-retrieval arbitration to fit facts within the
//! context window token budget.

pub mod comptroller;
pub mod config;
pub mod types;

pub use comptroller::{ContextComptroller, RetrievalResult};
pub use config::ComptrollerConfig;
pub use types::{ArbitratedContext, TokenBudget};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{MemoryFact, NoteType};

    #[test]
    fn test_token_budget_enforcement() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        // Create multiple facts
        let mut facts = Vec::new();
        for i in 0..5 {
            let mut fact = MemoryFact::new(
                format!("Fact number {} with some content", i),
                NoteType::Preference,
                vec![],
            );
            fact.embedding = Some(vec![0.1 * (i as f32); 1024]);
            fact.similarity_score = Some(0.9 - (i as f32) * 0.1);
            facts.push(fact);
        }

        let result = RetrievalResult { facts };

        // Set a tiny budget (only enough for 1-2 items; each ~8 tokens)
        let budget = TokenBudget::new(15);

        let arbitrated = comptroller.arbitrate(result, budget);

        // Should have trimmed to fit budget
        assert!(
            arbitrated.facts.len() < 5,
            "Should have trimmed items to fit budget"
        );
        assert!(
            !arbitrated.facts.is_empty(),
            "Should have kept at least some items"
        );

        // Facts should be prioritized (higher similarity scores should be kept)
        if !arbitrated.facts.is_empty() {
            assert!(arbitrated.facts[0].similarity_score.unwrap_or(0.0) >= 0.8);
        }
    }

    #[test]
    fn test_priority_sorting() {
        let config = ComptrollerConfig::default();
        let comptroller = ContextComptroller::new(config);

        let mut fact1 = MemoryFact::new(
            "Low priority fact".to_string(),
            NoteType::Preference,
            vec![],
        );
        fact1.similarity_score = Some(0.3);
        fact1.embedding = Some(vec![0.1; 1024]);

        let mut fact2 = MemoryFact::new(
            "High priority fact".to_string(),
            NoteType::Preference,
            vec![],
        );
        fact2.similarity_score = Some(0.9);
        fact2.embedding = Some(vec![0.2; 1024]);

        let result = RetrievalResult {
            facts: vec![fact1, fact2],
        };

        let budget = TokenBudget::new(10000);
        let arbitrated = comptroller.arbitrate(result, budget);

        assert_eq!(arbitrated.facts.len(), 2);
        assert!(
            arbitrated.facts[0].similarity_score.unwrap()
                > arbitrated.facts[1].similarity_score.unwrap()
        );
        assert_eq!(arbitrated.facts[0].content, "High priority fact");
    }
}

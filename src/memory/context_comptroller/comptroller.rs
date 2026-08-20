use super::config::ComptrollerConfig;
use super::types::{ArbitratedContext, TokenBudget};
use crate::memory::context::MemoryFact;

/// Input to the comptroller: a list of retrieved facts to arbitrate.
pub struct RetrievalResult {
    pub facts: Vec<MemoryFact>,
}

pub struct ContextComptroller {
    config: ComptrollerConfig,
}

impl ContextComptroller {
    #[must_use]
    pub const fn new(config: ComptrollerConfig) -> Self {
        Self { config }
    }

    /// Arbitrate retrieval results to fit within token budget.
    ///
    /// With raw memories removed, this simply trims facts to fit the budget.
    #[must_use]
    pub fn arbitrate(&self, results: RetrievalResult, budget: TokenBudget) -> ArbitratedContext {
        let mut tokens_saved = 0;
        let _ = &self.config; // config retained for future use

        // Sort by similarity score (descending) for priority-based selection
        let mut kept_facts = results.facts;
        kept_facts.sort_by(|a, b| {
            let score_a = a.similarity_score.unwrap_or(0.0);
            let score_b = b.similarity_score.unwrap_or(0.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Trim to fit budget
        let mut used_tokens = 0;
        let mut final_facts = Vec::new();

        for fact in kept_facts {
            let tokens = self.estimate_tokens(&fact.content);
            if used_tokens + tokens <= budget.total {
                used_tokens += tokens;
                final_facts.push(fact);
            } else {
                tokens_saved += tokens;
            }
        }

        ArbitratedContext {
            facts: final_facts,
            tokens_saved,
        }
    }

    /// Estimate tokens (4 chars per token).
    ///
    /// Counts Unicode characters rather than bytes: a CJK character is 3
    /// bytes in UTF-8 but a single character, so `text.len() / 4` would
    /// undercount CJK-heavy content by ~3× and silently blow past the
    /// budget. Matches the convention in
    /// `crate::memory::assembler::hydration::estimate_tokens`.
    fn estimate_tokens(&self, text: &str) -> usize {
        (text.chars().count() / 4).max(1)
    }
}

//! Value estimation for memory importance scoring

use crate::sync_primitives::Arc;

use crate::error::Result;
use crate::memory::context::MemoryEntry;
use crate::providers::AiProvider;

use super::llm_scorer::LlmScorer;

/// Estimates the importance value of memory entries
pub struct ValueEstimator {
    llm_scorer: Option<Arc<LlmScorer>>,
}

impl ValueEstimator {
    /// Create a new value estimator (keyword-based fallback only)
    pub fn new() -> Self {
        Self { llm_scorer: None }
    }

    /// Create a value estimator with LLM scoring support
    pub fn with_llm(provider: Arc<dyn AiProvider>) -> Self {
        let config = super::llm_scorer::LlmScorerConfig::default();
        let llm_scorer = Arc::new(LlmScorer::new(provider, config));
        Self { llm_scorer: Some(llm_scorer) }
    }

    /// Estimate the importance score of a memory entry
    ///
    /// Returns a score between 0.0 (low value) and 1.0 (high value)
    pub async fn estimate(&self, entry: &MemoryEntry) -> Result<f32> {
        if let Some(llm_scorer) = &self.llm_scorer {
            return self.estimate_with_llm(entry, llm_scorer).await;
        }
        self.estimate_with_keywords(entry).await
    }

    /// Estimate using LLM (hybrid approach)
    async fn estimate_with_llm(&self, entry: &MemoryEntry, llm_scorer: &LlmScorer) -> Result<f32> {
        let keyword_score = self.estimate_with_keywords(entry).await?;
        let llm_score = llm_scorer.score(entry).await?;
        // Weighted average (70% LLM, 30% keyword)
        Ok((llm_score * 0.7 + keyword_score * 0.3).clamp(0.0, 1.0))
    }

    /// Estimate using simple keyword heuristics (fallback when no LLM).
    ///
    /// NOTE: The previous `SignalDetector` (from deleted `signals.rs`) used hardcoded
    /// keyword lists that violated the LLM sovereignty principle. This stub preserves
    /// basic length-based scoring as a fallback. LLM scoring is the preferred path.
    async fn estimate_with_keywords(&self, entry: &MemoryEntry) -> Result<f32> {
        let combined_text = format!("{} {}", entry.user_input, entry.ai_output);
        let mut score: f32 = 0.5;

        // Simple heuristics: length as a proxy for information density
        let text_length = combined_text.len();
        if text_length > 500 {
            score += 0.10;
        } else if text_length < 50 {
            score -= 0.20;
        }

        // Greetings are low-value
        let lower = combined_text.to_lowercase();
        if lower.starts_with("hello") || lower.starts_with("hi ") || lower.trim() == "hi" {
            score -= 0.30;
        }

        Ok(score.clamp(0.0_f32, 1.0_f32))
    }

    /// Batch estimate scores for multiple entries
    pub async fn estimate_batch(&self, entries: &[MemoryEntry]) -> Result<Vec<f32>> {
        let mut scores = Vec::with_capacity(entries.len());
        for entry in entries {
            scores.push(self.estimate(entry).await?);
        }
        Ok(scores)
    }
}

impl Default for ValueEstimator {
    fn default() -> Self {
        Self::new()
    }
}

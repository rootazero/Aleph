//! Value estimation for memory importance scoring

use std::sync::Arc;

use crate::error::Result;
use crate::memory::context::MemoryEntry;
use crate::providers::AiProvider;

use super::llm_scorer::LlmScorer;
use super::signals::{Signal, SignalDetector};

/// Estimates the importance value of memory entries
pub struct ValueEstimator {
    signal_detector: SignalDetector,
    llm_scorer: Option<Arc<LlmScorer>>,
}

impl ValueEstimator {
    /// Create a new value estimator (keyword-based only)
    pub fn new() -> Self {
        Self {
            signal_detector: SignalDetector::new(),
            llm_scorer: None,
        }
    }

    /// Create a value estimator with LLM scoring support
    pub fn with_llm(provider: Arc<dyn AiProvider>) -> Self {
        let config = super::llm_scorer::LlmScorerConfig::default();
        let llm_scorer = Arc::new(LlmScorer::new(provider, config));

        Self {
            signal_detector: SignalDetector::new(),
            llm_scorer: Some(llm_scorer),
        }
    }

    /// Estimate the importance score of a memory entry
    ///
    /// Returns a score between 0.0 (low value) and 1.0 (high value)
    pub async fn estimate(&self, entry: &MemoryEntry) -> Result<f32> {
        // If LLM scorer is available, use hybrid approach
        if let Some(llm_scorer) = &self.llm_scorer {
            return self.estimate_with_llm(entry, llm_scorer).await;
        }

        // Otherwise, use keyword-based scoring
        self.estimate_with_keywords(entry).await
    }

    /// Estimate using LLM (hybrid approach)
    async fn estimate_with_llm(
        &self,
        entry: &MemoryEntry,
        llm_scorer: &LlmScorer,
    ) -> Result<f32> {
        // Get keyword-based score
        let keyword_score = self.estimate_with_keywords(entry).await?;

        // Get LLM score
        let llm_score = llm_scorer.score(entry).await?;

        // Weighted average (70% LLM, 30% keyword)
        // LLM is more accurate but keyword provides a safety net
        let final_score = (llm_score * 0.7) + (keyword_score * 0.3);

        Ok(final_score.clamp(0.0, 1.0))
    }

    /// Estimate using keyword-based signals
    async fn estimate_with_keywords(&self, entry: &MemoryEntry) -> Result<f32> {
        let combined_text = format!("{} {}", entry.user_input, entry.ai_output);

        // Detect signals
        let signals = self.signal_detector.detect(&combined_text);

        // Calculate score based on signals
        let mut score: f32 = 0.5; // Base score

        // Positive signals (increase score)
        if signals.contains(&Signal::UserPreference) {
            score += 0.25;
        }
        if signals.contains(&Signal::FactualInfo) {
            score += 0.15;
        }
        if signals.contains(&Signal::Decision) {
            score += 0.20;
        }
        if signals.contains(&Signal::PersonalInfo) {
            score += 0.30;
        }
        if signals.contains(&Signal::Question) && signals.contains(&Signal::Answer) {
            score += 0.10; // Q&A pairs are valuable
        }

        // Negative signals (decrease score)
        if signals.contains(&Signal::Greeting) {
            score -= 0.30;
        }
        if signals.contains(&Signal::SmallTalk) {
            score -= 0.20;
        }

        // Length bonus: longer conversations tend to be more valuable
        let text_length = combined_text.len();
        if text_length > 500 {
            score += 0.10;
        } else if text_length < 50 {
            score -= 0.10;
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

//! LLM-based importance scoring

use std::sync::Arc;

use crate::memory::MemoryEntry;
use crate::providers::AiProvider;
use crate::Result;

/// LLM-based scorer for importance estimation
pub struct LlmScorer {
    provider: Arc<dyn AiProvider>,
    config: LlmScorerConfig,
}

/// Configuration for LLM scorer
#[derive(Debug, Clone)]
pub struct LlmScorerConfig {
    /// Model to use for scoring (optional, uses provider default if None)
    pub model: Option<String>,

    /// Temperature for LLM (default: 0.0 for deterministic scoring)
    pub temperature: f32,

    /// Whether to use caching for repeated queries (default: true)
    pub use_cache: bool,
}

impl Default for LlmScorerConfig {
    fn default() -> Self {
        Self {
            model: None,
            temperature: 0.0,
            use_cache: true,
        }
    }
}

impl LlmScorer {
    /// Create a new LLM scorer
    pub fn new(provider: Arc<dyn AiProvider>, config: LlmScorerConfig) -> Self {
        Self { provider, config }
    }

    /// Score the importance of a memory entry using LLM
    ///
    /// Returns a score between 0.0 and 1.0 indicating the importance
    /// of the conversation for long-term memory.
    pub async fn score(&self, entry: &MemoryEntry) -> Result<f32> {
        let prompt = self.build_scoring_prompt(entry);
        let system_prompt = self.build_system_prompt();

        // Call LLM
        let response = self.provider.process(&prompt, Some(&system_prompt)).await?;

        // Parse response
        let score = self.parse_score(&response)?;

        Ok(score)
    }

    /// Build the scoring prompt
    fn build_scoring_prompt(&self, entry: &MemoryEntry) -> String {
        format!(
            "Rate the importance of this conversation on a scale of 0.0 to 1.0:\n\n\
             User: {}\n\
             Assistant: {}\n\n\
             Consider:\n\
             - Personal information (high importance)\n\
             - Preferences and decisions (high importance)\n\
             - Factual knowledge (medium importance)\n\
             - Greetings and small talk (low importance)\n\
             - Questions without answers (low importance)\n\n\
             Respond with ONLY a number between 0.0 and 1.0, nothing else.",
            entry.user_input, entry.ai_output
        )
    }

    /// Build the system prompt
    fn build_system_prompt(&self) -> String {
        "You are an importance scorer for conversation memory. \
         Your task is to rate how important a conversation is for long-term memory. \
         Consider the informational value, personal relevance, and decision-making content. \
         Respond with only a decimal number between 0.0 and 1.0."
            .to_string()
    }

    /// Parse the LLM response to extract the score
    fn parse_score(&self, response: &str) -> Result<f32> {
        let trimmed = response.trim();

        // Try to extract a number from the response
        let score_str = trimmed
            .split_whitespace()
            .find(|s| s.parse::<f32>().is_ok())
            .unwrap_or(trimmed);

        let score: f32 = score_str
            .parse()
            .map_err(|_| crate::error::AlephError::ConfigError {
                message: format!("Failed to parse LLM score: {}", response),
                suggestion: Some("LLM should return a number between 0.0 and 1.0".to_string()),
            })?;

        // Clamp to valid range
        Ok(score.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_score_valid() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        assert_eq!(scorer.parse_score("0.5").unwrap(), 0.5);
        assert_eq!(scorer.parse_score("0.95").unwrap(), 0.95);
        assert_eq!(scorer.parse_score("0.0").unwrap(), 0.0);
        assert_eq!(scorer.parse_score("1.0").unwrap(), 1.0);
    }

    #[test]
    fn test_parse_score_with_text() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        // Should extract number from text
        assert_eq!(scorer.parse_score("The score is 0.75").unwrap(), 0.75);
        assert_eq!(scorer.parse_score("0.8 is the importance").unwrap(), 0.8);
    }

    #[test]
    fn test_parse_score_clamping() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        // Should clamp to valid range
        assert_eq!(scorer.parse_score("1.5").unwrap(), 1.0);
        assert_eq!(scorer.parse_score("-0.5").unwrap(), 0.0);
    }

    #[test]
    fn test_parse_score_invalid() {
        let config = LlmScorerConfig::default();
        let provider = Arc::new(MockProvider);
        let scorer = LlmScorer::new(provider, config);

        // Should fail on invalid input
        assert!(scorer.parse_score("not a number").is_err());
        assert!(scorer.parse_score("").is_err());
    }

    // Mock provider for testing
    struct MockProvider;

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String>> + Send + '_>,
        > {
            Box::pin(async { Ok("0.5".to_string()) })
        }

        fn process_with_image(
            &self,
            _input: &str,
            _image: Option<&crate::clipboard::ImageData>,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String>> + Send + '_>,
        > {
            Box::pin(async { Ok("0.5".to_string()) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }
}

//! Typo Correction Module
//!
//! Provides quick text correction for Chinese and English input errors.
//! This module is designed for minimal latency by:
//! - Bypassing the complex 3-layer dispatcher routing
//! - Direct AI provider calls
//! - Simple request/response without multi-turn conversation
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::typo_correction::TypoCorrector;
//! use aethecore::providers::OpenAiProvider;
//! use std::sync::Arc;
//!
//! let provider = Arc::new(OpenAiProvider::new(...)?);
//! let corrector = TypoCorrector::new(provider);
//!
//! let result = corrector.correct("我再这里等你").await?;
//! assert_eq!(result.corrected_text, "我在这里等你");
//! assert!(result.has_changes);
//! ```

pub mod prompt;

use crate::config::TypoCorrectionConfig;
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;
use prompt::SYSTEM_PROMPT;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// Result of a typo correction operation
#[derive(Debug, Clone)]
pub struct CorrectionResult {
    /// The corrected text
    pub corrected_text: String,
    /// Whether any changes were made
    pub has_changes: bool,
}

/// Typo corrector that directly calls AI providers
///
/// This corrector is optimized for low latency by:
/// - Using a pre-defined system prompt
/// - Making single-shot API calls
/// - Skipping complex routing logic
pub struct TypoCorrector {
    provider: Arc<dyn AiProvider>,
    config: TypoCorrectionConfig,
}

impl TypoCorrector {
    /// Create a new TypoCorrector with the given provider
    pub fn new(provider: Arc<dyn AiProvider>, config: TypoCorrectionConfig) -> Self {
        Self { provider, config }
    }

    /// Correct typos in the given text
    ///
    /// # Arguments
    ///
    /// * `text` - The text to correct
    ///
    /// # Returns
    ///
    /// * `Ok(CorrectionResult)` - The correction result
    /// * `Err(AetherError)` - If the correction fails
    ///
    /// # Behavior
    ///
    /// - If text is empty or whitespace only, returns unchanged
    /// - If text exceeds max_length, truncates to max_length
    /// - If AI returns empty response, returns original text
    pub async fn correct(&self, text: &str) -> Result<CorrectionResult> {
        // Handle empty or whitespace-only text
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(CorrectionResult {
                corrected_text: text.to_string(),
                has_changes: false,
            });
        }

        // Truncate if too long
        let text_to_correct = if text.chars().count() > self.config.max_length {
            warn!(
                "Text exceeds max_length ({}), truncating to {} characters",
                text.chars().count(),
                self.config.max_length
            );
            text.chars().take(self.config.max_length).collect::<String>()
        } else {
            text.to_string()
        };

        debug!("Correcting text: {} chars", text_to_correct.chars().count());

        // Call AI provider with timeout
        let timeout = Duration::from_secs(self.config.timeout_seconds);
        let response = tokio::time::timeout(timeout, async {
            self.provider
                .process(&text_to_correct, Some(SYSTEM_PROMPT))
                .await
        })
        .await
        .map_err(|_| {
            AetherError::Timeout {
                suggestion: Some(format!(
                    "Typo correction timed out after {} seconds",
                    self.config.timeout_seconds
                )),
            }
        })??;

        // Parse response
        let corrected = self.parse_response(&response, &text_to_correct);

        Ok(corrected)
    }

    /// Parse the AI response into a CorrectionResult
    fn parse_response(&self, response: &str, original: &str) -> CorrectionResult {
        let corrected_text = response.trim();

        // If response is empty, return original
        if corrected_text.is_empty() {
            warn!("AI returned empty response, keeping original text");
            return CorrectionResult {
                corrected_text: original.to_string(),
                has_changes: false,
            };
        }

        // Check if there are changes
        let has_changes = corrected_text != original;

        if has_changes {
            debug!("Text corrected: '{}' -> '{}'", original, corrected_text);
        } else {
            debug!("No corrections needed");
        }

        CorrectionResult {
            corrected_text: corrected_text.to_string(),
            has_changes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MockProvider;

    fn create_test_config() -> TypoCorrectionConfig {
        TypoCorrectionConfig {
            enabled: true,
            provider: Some("test".to_string()),
            model: None,
            timeout_seconds: 5,
            max_length: 2000,
        }
    }

    #[tokio::test]
    async fn test_correct_empty_text() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.correct("").await.unwrap();
        assert_eq!(result.corrected_text, "");
        assert!(!result.has_changes);
    }

    #[tokio::test]
    async fn test_correct_whitespace_only() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.correct("   ").await.unwrap();
        assert_eq!(result.corrected_text, "   ");
        assert!(!result.has_changes);
    }

    #[tokio::test]
    async fn test_correct_with_changes() {
        let provider = Arc::new(MockProvider::new("我在这里等你"));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.correct("我再这里等你").await.unwrap();
        assert_eq!(result.corrected_text, "我在这里等你");
        assert!(result.has_changes);
    }

    #[tokio::test]
    async fn test_correct_no_changes() {
        let provider = Arc::new(MockProvider::new("这是正确的文本"));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.correct("这是正确的文本").await.unwrap();
        assert_eq!(result.corrected_text, "这是正确的文本");
        assert!(!result.has_changes);
    }

    #[tokio::test]
    async fn test_correct_empty_response() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.correct("测试文本").await.unwrap();
        assert_eq!(result.corrected_text, "测试文本");
        assert!(!result.has_changes);
    }

    #[tokio::test]
    async fn test_correct_truncate_long_text() {
        let provider = Arc::new(MockProvider::new("truncated"));
        let mut config = create_test_config();
        config.max_length = 10;
        let corrector = TypoCorrector::new(provider, config);

        // Create text longer than max_length
        let long_text = "这是一段很长的测试文本";
        let result = corrector.correct(long_text).await.unwrap();
        // The mock provider returns "truncated", so we check that
        assert_eq!(result.corrected_text, "truncated");
    }

    #[test]
    fn test_parse_response_with_changes() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.parse_response("corrected text", "original text");
        assert_eq!(result.corrected_text, "corrected text");
        assert!(result.has_changes);
    }

    #[test]
    fn test_parse_response_no_changes() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.parse_response("same text", "same text");
        assert_eq!(result.corrected_text, "same text");
        assert!(!result.has_changes);
    }

    #[test]
    fn test_parse_response_empty() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.parse_response("", "original");
        assert_eq!(result.corrected_text, "original");
        assert!(!result.has_changes);
    }

    #[test]
    fn test_parse_response_trim_whitespace() {
        let provider = Arc::new(MockProvider::new(""));
        let corrector = TypoCorrector::new(provider, create_test_config());

        let result = corrector.parse_response("  corrected  ", "original");
        assert_eq!(result.corrected_text, "corrected");
        assert!(result.has_changes);
    }
}

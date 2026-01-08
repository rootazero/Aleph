//! Keyword matching layer (Layer 2).
//!
//! This layer handles weighted keyword scoring for semantic matching.
//! Keywords have priority 2 and are non-terminal (results can be merged).

use crate::payload::Capability;
use crate::semantic::context::MatchingContext;
use crate::semantic::intent::{DetectionMethod, IntentCategory, SemanticIntent};
use crate::semantic::keyword::{KeywordIndex, KeywordRule};
use crate::semantic::layer::{LayerEnabledFlag, MatchingLayer};
use crate::semantic::matcher::MatchResult;
use async_trait::async_trait;
use tracing::debug;

/// Keyword matching layer
///
/// Matches input using weighted keyword scoring.
/// This layer has priority 2 and is non-terminal (can be merged with context results).
pub struct KeywordLayer {
    /// Keyword index for fast lookup
    keyword_index: KeywordIndex,
    /// Enabled flag
    enabled: LayerEnabledFlag,
    /// Minimum score threshold for matches
    score_threshold: f32,
}

impl KeywordLayer {
    /// Create a new empty keyword layer
    pub fn new() -> Self {
        Self {
            keyword_index: KeywordIndex::new(),
            enabled: LayerEnabledFlag::new(true),
            score_threshold: 0.7,
        }
    }

    /// Create with custom score threshold
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.score_threshold = threshold;
        self
    }

    /// Create with pre-built keyword index
    pub fn with_index(keyword_index: KeywordIndex) -> Self {
        Self {
            keyword_index,
            enabled: LayerEnabledFlag::new(true),
            score_threshold: 0.7,
        }
    }

    /// Add a keyword rule
    pub fn add_rule(&mut self, rule: KeywordRule) {
        self.keyword_index.add_rule(rule);
    }

    /// Get the keyword index
    pub fn keyword_index(&self) -> &KeywordIndex {
        &self.keyword_index
    }

    /// Get mutable keyword index
    pub fn keyword_index_mut(&mut self) -> &mut KeywordIndex {
        &mut self.keyword_index
    }
}

impl Default for KeywordLayer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MatchingLayer for KeywordLayer {
    fn layer_id(&self) -> &str {
        "keyword"
    }

    fn priority(&self) -> u32 {
        2 // Third priority after regex
    }

    fn is_enabled(&self) -> bool {
        self.enabled.is_enabled()
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.set_enabled(enabled);
    }

    fn is_terminal(&self) -> bool {
        false // Keyword matches can be merged with context
    }

    fn confidence_threshold(&self) -> f64 {
        self.score_threshold as f64
    }

    async fn try_match(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        let input = &ctx.raw_input;

        let matches = self
            .keyword_index
            .match_with_threshold(input, self.score_threshold);

        if let Some(best) = matches.into_iter().next() {
            let confidence = (best.score as f64).min(1.0);

            let intent = SemanticIntent::new(
                IntentCategory::Semantic(best.intent_type.clone()),
                best.intent_type.clone(),
            )
            .with_confidence(confidence)
            .with_method(DetectionMethod::keyword(
                best.score as f64,
                best.matched_keywords.clone(),
            ))
            .with_capabilities(
                best.capabilities
                    .iter()
                    .filter_map(|c| parse_capability(c))
                    .collect(),
            );

            let intent = if let Some(prompt) = best.system_prompt {
                intent.with_system_prompt(prompt)
            } else {
                intent
            };

            debug!(
                intent = %best.intent_type,
                score = best.score,
                keywords = ?best.matched_keywords,
                "Keyword layer matched"
            );

            return Some(MatchResult {
                intent,
                confidence,
                rule_index: None,
                needs_ai_fallback: false,
            });
        }

        None
    }
}

/// Parse capability string to Capability enum
fn parse_capability(s: &str) -> Option<Capability> {
    match s.to_lowercase().as_str() {
        "memory" => Some(Capability::Memory),
        "search" => Some(Capability::Search),
        "mcp" => Some(Capability::Mcp),
        "video" => Some(Capability::Video),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_keyword_layer_match() {
        let mut layer = KeywordLayer::new();

        layer.add_rule(
            KeywordRule::new(
                "weather",
                "weather_query",
                vec![
                    "weather".to_string(),
                    "forecast".to_string(),
                    "temperature".to_string(),
                ],
            )
            .with_capabilities(vec!["search".to_string()]),
        );

        let ctx = MatchingContext::simple("What's the weather forecast today?");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.confidence >= 0.7);
        assert_eq!(result.intent.intent_type, "weather_query");
    }

    #[tokio::test]
    async fn test_keyword_layer_no_match() {
        let mut layer = KeywordLayer::new();

        layer.add_rule(KeywordRule::new(
            "weather",
            "weather_query",
            vec!["weather".to_string(), "forecast".to_string()],
        ));

        let ctx = MatchingContext::simple("Tell me a joke");
        let result = layer.try_match(&ctx).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_keyword_layer_with_threshold() {
        let mut layer = KeywordLayer::new().with_threshold(0.5);

        layer.add_rule(KeywordRule::new(
            "code",
            "code_help",
            vec!["code".to_string(), "debug".to_string(), "error".to_string()],
        ));

        let ctx = MatchingContext::simple("Help me with this code");
        let result = layer.try_match(&ctx).await;

        // Should match with lower threshold
        assert!(result.is_some());
    }

    #[test]
    fn test_keyword_layer_properties() {
        let layer = KeywordLayer::new();

        assert_eq!(layer.layer_id(), "keyword");
        assert_eq!(layer.priority(), 2);
        assert!(!layer.is_terminal()); // Non-terminal
        assert!(layer.is_enabled());
    }

    #[test]
    fn test_parse_capability() {
        assert_eq!(parse_capability("memory"), Some(Capability::Memory));
        assert_eq!(parse_capability("search"), Some(Capability::Search));
        assert_eq!(parse_capability("mcp"), Some(Capability::Mcp));
        assert_eq!(parse_capability("video"), Some(Capability::Video));
        assert_eq!(parse_capability("unknown"), None);
    }
}

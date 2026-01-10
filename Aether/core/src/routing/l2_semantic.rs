//! L2 Semantic Matcher for Intent Pipeline
//!
//! Wrapper around SemanticMatcher's keyword/context matching for pipeline integration.
//! Returns `IntentSignal` with matched keywords and latency tracking.

use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::routing::{IntentSignal, RoutingContext, RoutingLayerType};
use crate::semantic::{MatchResult, MatchingContext, SemanticMatcher};
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

/// L2 Semantic Matcher for the intent routing pipeline
///
/// Wraps the SemanticMatcher's keyword/context matching to produce
/// IntentSignal outputs with matched keywords.
pub struct L2SemanticMatcher {
    /// Underlying semantic matcher
    semantic_matcher: Arc<SemanticMatcher>,

    /// Minimum confidence to accept L2 match
    min_confidence: f32,
}

impl L2SemanticMatcher {
    /// Create a new L2 matcher wrapping a SemanticMatcher
    pub fn new(semantic_matcher: Arc<SemanticMatcher>) -> Self {
        Self {
            semantic_matcher,
            min_confidence: 0.5, // L2 has lower threshold
        }
    }

    /// Set minimum confidence threshold
    pub fn with_min_confidence(mut self, confidence: f32) -> Self {
        self.min_confidence = confidence;
        self
    }

    /// Match input using L2 (semantic/keyword) layer
    ///
    /// Returns `Some(IntentSignal)` for keyword/context matches,
    /// `None` otherwise.
    pub async fn match_input(&self, ctx: &RoutingContext) -> Option<IntentSignal> {
        let start = Instant::now();

        // Build matching context from routing context
        let matching_ctx = self.build_matching_context(ctx);

        // Use SemanticMatcher's full matching (includes keyword and context)
        let result = self.semantic_matcher.match_input(&matching_ctx).await;

        let latency_ms = start.elapsed().as_millis() as u64;

        // L2 accepts medium-confidence semantic matches
        if result.confidence >= self.min_confidence as f64 && !result.is_l1_match() {
            debug!(
                confidence = result.confidence,
                latency_ms,
                intent_type = %result.intent.intent_type,
                keywords = ?result.matched_keywords,
                "L2 Semantic: Match found"
            );

            let signal = self.convert_to_signal(result, latency_ms)?;
            Some(signal)
        } else {
            debug!(
                confidence = result.confidence,
                latency_ms,
                is_l1 = result.is_l1_match(),
                "L2 Semantic: No match"
            );
            None
        }
    }

    /// Match keywords only (sync, for quick lookups)
    ///
    /// Returns matched keywords sorted by score.
    pub fn match_keywords_only(&self, input: &str) -> Vec<String> {
        self.semantic_matcher
            .match_keywords_only(input)
            .into_iter()
            .map(|m| m.intent_type)
            .collect()
    }

    /// Build MatchingContext from RoutingContext
    fn build_matching_context(&self, ctx: &RoutingContext) -> MatchingContext {
        if let Some(ref conv) = ctx.conversation {
            MatchingContext::builder()
                .raw_input(&ctx.input)
                .conversation(conv.clone())
                .build()
        } else {
            MatchingContext::simple(&ctx.input)
        }
    }

    /// Convert MatchResult to IntentSignal
    fn convert_to_signal(&self, result: MatchResult, latency_ms: u64) -> Option<IntentSignal> {
        // Create tool from intent
        let tool = UnifiedTool::new(
            &result.intent.intent_type,
            &result.intent.intent_type,
            &format!("L2 semantic match: {}", result.intent.intent_type),
            if let Some(idx) = result.rule_index {
                ToolSource::Custom { rule_index: idx }
            } else {
                ToolSource::Native
            },
        );

        // Convert parameters
        let parameters = serde_json::to_value(&result.intent.params).unwrap_or_default();

        let signal = IntentSignal::with_tool(
            RoutingLayerType::L2Semantic,
            tool,
            result.confidence as f32,
        )
        .with_parameters(parameters)
        .with_reason(format!(
            "Semantic match: {} (keywords: {})",
            result.intent.intent_type,
            result.matched_keywords.join(", ")
        ))
        .with_latency(latency_ms)
        .with_keywords(result.matched_keywords);

        Some(signal)
    }

    /// Get the underlying matcher
    pub fn matcher(&self) -> &SemanticMatcher {
        &self.semantic_matcher
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::MatcherConfig;

    fn create_test_matcher() -> Arc<SemanticMatcher> {
        Arc::new(SemanticMatcher::new(MatcherConfig::default()))
    }

    #[tokio::test]
    async fn test_l2_matcher_creation() {
        let matcher = create_test_matcher();
        let l2 = L2SemanticMatcher::new(matcher);

        assert_eq!(l2.min_confidence, 0.5);
    }

    #[tokio::test]
    async fn test_l2_no_match_without_rules() {
        let matcher = create_test_matcher();
        let l2 = L2SemanticMatcher::new(matcher);

        let ctx = RoutingContext::new("hello world");
        let result = l2.match_input(&ctx).await;

        // Without keyword rules, should not match
        assert!(result.is_none());
    }

    #[test]
    fn test_l2_keywords_sync() {
        let matcher = create_test_matcher();
        let l2 = L2SemanticMatcher::new(matcher);

        // Without rules, should return empty
        let keywords = l2.match_keywords_only("search for weather");
        assert!(keywords.is_empty());
    }
}

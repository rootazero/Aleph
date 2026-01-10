//! L1 Regex Matcher for Intent Pipeline
//!
//! Wrapper around SemanticMatcher's command/regex matching for pipeline integration.
//! Returns `IntentSignal` with latency tracking.

use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::routing::{IntentSignal, RoutingContext, RoutingLayerType};
use crate::semantic::{MatchResult, MatchingContext, SemanticMatcher};
use std::sync::Arc;
use std::time::Instant;
use tracing::debug;

/// L1 Regex Matcher for the intent routing pipeline
///
/// Wraps the SemanticMatcher's fast path (command/regex matching) to produce
/// IntentSignal outputs compatible with the pipeline.
pub struct L1RegexMatcher {
    /// Underlying semantic matcher
    semantic_matcher: Arc<SemanticMatcher>,

    /// Minimum confidence to accept L1 match
    min_confidence: f32,
}

impl L1RegexMatcher {
    /// Create a new L1 matcher wrapping a SemanticMatcher
    pub fn new(semantic_matcher: Arc<SemanticMatcher>) -> Self {
        Self {
            semantic_matcher,
            min_confidence: 0.9, // L1 requires high confidence
        }
    }

    /// Set minimum confidence threshold
    pub fn with_min_confidence(mut self, confidence: f32) -> Self {
        self.min_confidence = confidence;
        self
    }

    /// Match input using L1 (regex) layer
    ///
    /// Returns `Some(IntentSignal)` for high-confidence regex matches,
    /// `None` otherwise.
    pub async fn match_input(&self, ctx: &RoutingContext) -> Option<IntentSignal> {
        let start = Instant::now();

        // Build matching context from routing context
        let matching_ctx = self.build_matching_context(ctx);

        // Use SemanticMatcher's fast path
        let result = self.semantic_matcher.match_input(&matching_ctx).await;

        let latency_ms = start.elapsed().as_millis() as u64;

        // L1 only accepts high-confidence regex/command matches
        if result.confidence >= self.min_confidence as f64 && result.is_l1_match() {
            debug!(
                confidence = result.confidence,
                latency_ms,
                intent_type = %result.intent.intent_type,
                "L1 Regex: Match found"
            );

            let signal = self.convert_to_signal(result, latency_ms)?;
            Some(signal)
        } else {
            debug!(
                confidence = result.confidence,
                latency_ms,
                "L1 Regex: No high-confidence match"
            );
            None
        }
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
            &format!("L1 matched tool: {}", result.intent.intent_type),
            if let Some(idx) = result.rule_index {
                ToolSource::Custom { rule_index: idx }
            } else {
                ToolSource::Native
            },
        );

        // Convert parameters
        let parameters = serde_json::to_value(&result.intent.params).unwrap_or_default();

        let signal = IntentSignal::with_tool(
            RoutingLayerType::L1Regex,
            tool,
            result.confidence as f32,
        )
        .with_parameters(parameters)
        .with_reason(format!("Regex match: {}", result.intent.intent_type))
        .with_latency(latency_ms);

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
    async fn test_l1_matcher_creation() {
        let matcher = create_test_matcher();
        let l1 = L1RegexMatcher::new(matcher);

        assert_eq!(l1.min_confidence, 0.9);
    }

    #[tokio::test]
    async fn test_l1_no_match_for_plain_input() {
        let matcher = create_test_matcher();
        let l1 = L1RegexMatcher::new(matcher);

        let ctx = RoutingContext::new("hello world");
        let result = l1.match_input(&ctx).await;

        // Plain input should not match L1
        assert!(result.is_none());
    }
}

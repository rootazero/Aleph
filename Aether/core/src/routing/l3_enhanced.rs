//! L3 Enhanced Router for Intent Pipeline
//!
//! Wrapper around L3Router for pipeline integration with enhanced features:
//! - Tool pre-filtering based on input
//! - Entity hints from conversation
//! - IntentSignal output with latency tracking

use crate::dispatcher::{L3Router, UnifiedTool};
use crate::providers::AiProvider;
use crate::routing::{IntentSignal, RoutingContext, RoutingLayerType};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{debug, warn};

/// Enhanced L3 Router for the intent routing pipeline
///
/// Wraps the L3Router with additional features:
/// - Tool pre-filtering based on input keywords
/// - Entity hints from conversation context
/// - Structured IntentSignal output
pub struct EnhancedL3Router {
    /// Underlying L3 router
    l3_router: Arc<L3Router>,

    /// Minimum confidence to accept L3 match
    min_confidence: f32,

    /// Timeout for L3 routing
    timeout: Duration,

    /// Maximum tools to send to L3 (for performance)
    max_tools: usize,
}

impl EnhancedL3Router {
    /// Create a new enhanced L3 router
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        let l3_router = Arc::new(L3Router::new(provider));
        Self {
            l3_router,
            min_confidence: 0.3,
            timeout: Duration::from_millis(5000),
            max_tools: 20,
        }
    }

    /// Create from existing L3Router
    pub fn from_router(l3_router: Arc<L3Router>) -> Self {
        Self {
            l3_router,
            min_confidence: 0.3,
            timeout: Duration::from_millis(5000),
            max_tools: 20,
        }
    }

    /// Set minimum confidence threshold
    pub fn with_min_confidence(mut self, confidence: f32) -> Self {
        self.min_confidence = confidence;
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set max tools
    pub fn with_max_tools(mut self, max_tools: usize) -> Self {
        self.max_tools = max_tools;
        self
    }

    /// Route input using L3 (AI inference) layer
    ///
    /// Returns `Some(IntentSignal)` for AI-inferred matches,
    /// `None` on timeout, error, or low confidence.
    pub async fn route(
        &self,
        ctx: &RoutingContext,
        tools: &[UnifiedTool],
    ) -> Option<IntentSignal> {
        let start = Instant::now();

        // Skip if no tools
        if tools.is_empty() {
            debug!("L3 Enhanced: No tools available");
            return None;
        }

        // Pre-filter tools based on input
        let filtered_tools = self.prefilter_tools(&ctx.input, tools);
        if filtered_tools.is_empty() {
            debug!("L3 Enhanced: No relevant tools after filtering");
            return None;
        }

        // Build conversation context string
        let conversation_context = self.build_conversation_context(ctx);

        // Apply timeout from context or default
        let timeout_duration = ctx.l3_timeout.unwrap_or(self.timeout);

        // Route with timeout
        let result = timeout(
            timeout_duration,
            self.l3_router.route(
                &ctx.input,
                &filtered_tools,
                conversation_context.as_deref(),
            ),
        )
        .await;

        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(Some(response))) => {
                let confidence = response.confidence;

                if confidence >= self.min_confidence {
                    debug!(
                        confidence,
                        latency_ms,
                        tool = ?response.tool,
                        "L3 Enhanced: Match found"
                    );

                    // Find the matched tool
                    let tool_name = response.tool.as_deref()?;
                    let tool = filtered_tools
                        .iter()
                        .find(|t| t.name == tool_name)
                        .cloned()?;

                    let signal = IntentSignal::with_tool(
                        RoutingLayerType::L3Inference,
                        tool,
                        confidence,
                    )
                    .with_parameters(response.parameters.clone())
                    .with_reason(&response.reason)
                    .with_latency(latency_ms);

                    Some(signal)
                } else {
                    debug!(
                        confidence,
                        min_confidence = self.min_confidence,
                        latency_ms,
                        "L3 Enhanced: Confidence below threshold"
                    );
                    None
                }
            }
            Ok(Ok(None)) => {
                debug!(latency_ms, "L3 Enhanced: No tool matched");
                None
            }
            Ok(Err(e)) => {
                warn!(error = %e, latency_ms, "L3 Enhanced: Router error");
                None
            }
            Err(_) => {
                warn!(
                    timeout_ms = timeout_duration.as_millis() as u64,
                    "L3 Enhanced: Timeout"
                );
                None
            }
        }
    }

    /// Pre-filter tools based on input keywords
    ///
    /// This reduces the tool list sent to L3, improving latency and accuracy.
    fn prefilter_tools(&self, input: &str, tools: &[UnifiedTool]) -> Vec<UnifiedTool> {
        let input_lower = input.to_lowercase();
        let input_words: Vec<&str> = input_lower.split_whitespace().collect();

        // Score each tool based on keyword overlap
        let mut scored: Vec<(usize, &UnifiedTool)> = tools
            .iter()
            .map(|tool| {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();

                let mut score = 0usize;

                // Check name match
                if input_words.iter().any(|w| name_lower.contains(w)) {
                    score += 10;
                }

                // Check description match
                for word in &input_words {
                    if desc_lower.contains(word) {
                        score += 1;
                    }
                }

                (score, tool)
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.0.cmp(&a.0));

        // If no scoring matches, include all tools up to max
        if scored.iter().all(|(s, _)| *s == 0) {
            return tools.iter().take(self.max_tools).cloned().collect();
        }

        // Return top-scoring tools
        scored
            .into_iter()
            .filter(|(score, _)| *score > 0)
            .take(self.max_tools)
            .map(|(_, tool)| tool.clone())
            .collect()
    }

    /// Build conversation context string from routing context
    fn build_conversation_context(&self, ctx: &RoutingContext) -> Option<String> {
        ctx.conversation.as_ref().map(|conv| {
            let mut context = String::new();

            // Add recent conversation history
            for turn in &conv.history {
                context.push_str(&format!(
                    "User: {}\nAssistant: {}\n\n",
                    turn.user_input, turn.ai_response
                ));
            }

            // Add entity hints if available
            if !ctx.entity_hints.is_empty() {
                context.push_str(&format!(
                    "Entity hints: {}\n",
                    ctx.entity_hints.join(", ")
                ));
            }

            context
        })
    }

    /// Get the underlying L3 router
    pub fn router(&self) -> &L3Router {
        &self.l3_router
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;

    fn create_test_tools() -> Vec<UnifiedTool> {
        vec![
            UnifiedTool::new("search", "search", "Search the web", ToolSource::Native),
            UnifiedTool::new("translate", "translate", "Translate text", ToolSource::Native),
            UnifiedTool::new("weather", "weather", "Get weather info", ToolSource::Native),
        ]
    }

    #[test]
    fn test_prefilter_tools_with_match() {
        use crate::providers::mock::MockProvider;

        let provider = Arc::new(MockProvider::new("test response"));
        let router = EnhancedL3Router::new(provider);
        let tools = create_test_tools();

        // "search" should prioritize search tool
        let filtered = router.prefilter_tools("search for info", &tools);
        assert!(!filtered.is_empty());
        assert_eq!(filtered[0].name, "search");
    }

    #[test]
    fn test_prefilter_tools_no_match() {
        use crate::providers::mock::MockProvider;

        let provider = Arc::new(MockProvider::new("test response"));
        let router = EnhancedL3Router::new(provider);
        let tools = create_test_tools();

        // Random input should return all tools
        let filtered = router.prefilter_tools("xyz random", &tools);
        assert_eq!(filtered.len(), tools.len());
    }

    #[test]
    fn test_prefilter_respects_max() {
        use crate::providers::mock::MockProvider;

        let provider = Arc::new(MockProvider::new("test response"));
        let router = EnhancedL3Router::new(provider).with_max_tools(1);
        let tools = create_test_tools();

        let filtered = router.prefilter_tools("random", &tools);
        assert_eq!(filtered.len(), 1);
    }
}

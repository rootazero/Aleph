//! UnifiedRouter - Multi-layer routing coordinator
//!
//! Orchestrates L1 → L2 → L3 → Default routing cascade:
//!
//! - L1 (Regex): Fast pattern matching (<10ms, confidence 1.0)
//! - L2 (Semantic): Keyword and context matching (200-500ms, confidence 0.7)
//! - L3 (AI Inference): AI-powered routing (>1s, confidence varies)
//! - Default: Fallback to general chat
//!
//! # Design Principles
//!
//! - **Cascade**: Each layer tries in order, stops on high-confidence match
//! - **Configurable**: Each layer can be enabled/disabled independently
//! - **Unified Results**: All layers return the same `RoutingResult` type
//! - **Context-Aware**: Supports conversation history and entity hints

use super::types::{
    RoutingConfig, RoutingContext, RoutingLayerType, RoutingMatch, RoutingResult,
};
use crate::dispatcher::{L3Router, ToolSource, UnifiedTool};
use crate::providers::AiProvider;
use crate::semantic::{MatchResult, MatchingContext, SemanticMatcher};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, info, trace, warn};

/// Unified multi-layer router
///
/// Coordinates all routing layers to find the best tool match for user input.
pub struct UnifiedRouter {
    /// Configuration for routing behavior
    config: RoutingConfig,

    /// L2 Semantic matcher (handles L1 regex + L2 keyword/context)
    semantic_matcher: Arc<SemanticMatcher>,

    /// L3 AI inference router
    l3_router: Option<Arc<L3Router>>,

    /// Cached tool list for routing
    cached_tools: Arc<tokio::sync::RwLock<Vec<UnifiedTool>>>,
}

impl UnifiedRouter {
    /// Create a new UnifiedRouter
    ///
    /// # Arguments
    ///
    /// * `config` - Routing configuration
    /// * `provider` - AI provider for L3 routing (optional)
    /// * `semantic_matcher` - Semantic matcher for L1/L2 routing
    pub fn new(
        config: RoutingConfig,
        provider: Option<Arc<dyn AiProvider>>,
        semantic_matcher: Arc<SemanticMatcher>,
    ) -> Self {
        let l3_router = if config.l3_enabled {
            provider.map(|p| {
                Arc::new(
                    L3Router::new(p)
                        .with_timeout(Duration::from_millis(config.l3_timeout_ms))
                        .with_confidence_threshold(config.l3_min_confidence)
                        .with_minimal_prompts(config.l3_minimal_prompts),
                )
            })
        } else {
            None
        };

        Self {
            config,
            semantic_matcher,
            l3_router,
            cached_tools: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    /// Create with minimal configuration (L3 disabled)
    pub fn minimal(semantic_matcher: Arc<SemanticMatcher>) -> Self {
        Self::new(RoutingConfig::minimal(), None, semantic_matcher)
    }

    /// Create with full configuration
    pub fn full(
        provider: Arc<dyn AiProvider>,
        semantic_matcher: Arc<SemanticMatcher>,
    ) -> Self {
        Self::new(RoutingConfig::full(), Some(provider), semantic_matcher)
    }

    /// Update the cached tool list
    pub async fn update_tools(&self, tools: Vec<UnifiedTool>) {
        let mut cached = self.cached_tools.write().await;
        *cached = tools;
        debug!("Updated cached tools: {} tools", cached.len());
    }

    /// Get current configuration
    pub fn config(&self) -> &RoutingConfig {
        &self.config
    }

    /// Route user input through all enabled layers
    ///
    /// # Arguments
    ///
    /// * `ctx` - Routing context containing input and conversation history
    ///
    /// # Returns
    ///
    /// `RoutingResult` with match details or no-match reason
    pub async fn route(&self, ctx: &RoutingContext) -> RoutingResult {
        if !self.config.enabled {
            return RoutingResult::skipped("Routing disabled");
        }

        let input = &ctx.input;
        if input.trim().is_empty() {
            return RoutingResult::no_match(input, "Empty input");
        }

        info!(input_length = input.len(), "UnifiedRouter: Starting routing cascade");

        // L1: Regex pattern matching (via SemanticMatcher)
        if self.config.l1_enabled {
            if let Some(result) = self.try_l1_regex(ctx).await {
                info!(
                    confidence = result.confidence,
                    tool = %result.tool.name,
                    "L1 Regex: Match found"
                );
                return RoutingResult::Matched(result);
            }
            trace!("L1 Regex: No match");
        }

        // L2: Semantic matching (keyword + context)
        if self.config.l2_enabled {
            if let Some(result) = self.try_l2_semantic(ctx).await {
                if result.confidence >= self.config.l2_min_confidence {
                    info!(
                        confidence = result.confidence,
                        tool = %result.tool.name,
                        "L2 Semantic: Match found"
                    );
                    return RoutingResult::Matched(result);
                }
                debug!(
                    confidence = result.confidence,
                    threshold = self.config.l2_min_confidence,
                    "L2 Semantic: Match below threshold, continuing to L3"
                );
            }
            trace!("L2 Semantic: No match");
        }

        // L3: AI inference routing
        if self.config.l3_enabled {
            if let Some(result) = self.try_l3_inference(ctx).await {
                if result.confidence >= self.config.l3_min_confidence {
                    info!(
                        confidence = result.confidence,
                        tool = %result.tool.name,
                        "L3 Inference: Match found"
                    );
                    return RoutingResult::Matched(result);
                }
                debug!(
                    confidence = result.confidence,
                    threshold = self.config.l3_min_confidence,
                    "L3 Inference: Match below threshold"
                );
            }
            trace!("L3 Inference: No match");
        }

        // Default: No tool matched
        info!("UnifiedRouter: No tool matched, defaulting to general chat");
        RoutingResult::no_match(input, "No tool matched across all layers")
    }

    /// Route with specific tools (bypasses cached tools)
    pub async fn route_with_tools(
        &self,
        ctx: &RoutingContext,
        tools: &[UnifiedTool],
    ) -> RoutingResult {
        // Temporarily update cache for this routing operation
        let original_tools = {
            let cached = self.cached_tools.read().await;
            cached.clone()
        };

        self.update_tools(tools.to_vec()).await;
        let result = self.route(ctx).await;

        // Restore original tools
        self.update_tools(original_tools).await;

        result
    }

    // =========================================================================
    // L1: Regex Layer
    // =========================================================================

    async fn try_l1_regex(&self, ctx: &RoutingContext) -> Option<RoutingMatch> {
        let matching_ctx = self.build_matching_context(ctx);

        // Use SemanticMatcher's fast path for command/regex matching
        let result = self.semantic_matcher.match_input(&matching_ctx).await;

        // L1 only accepts high-confidence regex matches
        if result.confidence >= 0.9 {
            // Convert MatchResult to RoutingMatch
            let tool = self.find_tool_for_intent(&result).await?;
            Some(
                RoutingMatch::new(tool, result.confidence as f32, RoutingLayerType::L1Regex)
                    .with_reason(format!("Regex match: {}", result.intent.intent_type))
                    .with_intent(result.intent.into()),
            )
        } else {
            None
        }
    }

    // =========================================================================
    // L2: Semantic Layer
    // =========================================================================

    async fn try_l2_semantic(&self, ctx: &RoutingContext) -> Option<RoutingMatch> {
        let matching_ctx = self.build_matching_context(ctx);

        // Full semantic matching (keyword + context)
        let result = self.semantic_matcher.match_input(&matching_ctx).await;

        // L2 accepts medium-confidence matches
        if result.confidence >= 0.5 {
            let tool = self.find_tool_for_intent(&result).await?;
            Some(
                RoutingMatch::new(tool, result.confidence as f32, RoutingLayerType::L2Semantic)
                    .with_reason(format!("Semantic match: {}", result.intent.intent_type))
                    .with_intent(result.intent.into()),
            )
        } else {
            None
        }
    }

    // =========================================================================
    // L3: AI Inference Layer
    // =========================================================================

    async fn try_l3_inference(&self, ctx: &RoutingContext) -> Option<RoutingMatch> {
        let l3_router = self.l3_router.as_ref()?;

        let tools = self.cached_tools.read().await.clone();
        if tools.is_empty() {
            debug!("L3 Inference: No tools available");
            return None;
        }

        // Build conversation context string
        let conversation_context = ctx.conversation.as_ref().map(|conv| {
            conv.history
                .iter()
                .map(|t| format!("User: {}\nAssistant: {}", t.user_input, t.ai_response))
                .collect::<Vec<_>>()
                .join("\n\n")
        });

        // Apply timeout
        let timeout_duration =
            ctx.l3_timeout.unwrap_or(Duration::from_millis(self.config.l3_timeout_ms));

        let route_result = timeout(
            timeout_duration,
            l3_router.route(&ctx.input, &tools, conversation_context.as_deref()),
        )
        .await;

        match route_result {
            Ok(Ok(Some(response))) => {
                // Find the matched tool by name
                let tool_name = response.tool.as_deref()?;
                let tool = tools
                    .iter()
                    .find(|t| t.name == tool_name)
                    .cloned()?;

                Some(
                    RoutingMatch::new(tool, response.confidence, RoutingLayerType::L3Inference)
                        .with_reason(&response.reason)
                        .with_parameters(response.parameters.clone()),
                )
            }
            Ok(Ok(None)) => {
                debug!("L3 Inference: No tool matched");
                None
            }
            Ok(Err(e)) => {
                warn!("L3 Inference error: {}", e);
                None
            }
            Err(_) => {
                warn!(
                    timeout_ms = timeout_duration.as_millis(),
                    "L3 Inference: Timeout"
                );
                None
            }
        }
    }

    // =========================================================================
    // Helper Methods
    // =========================================================================

    /// Build MatchingContext from RoutingContext
    fn build_matching_context(&self, ctx: &RoutingContext) -> MatchingContext {
        if let Some(conv) = &ctx.conversation {
            MatchingContext::builder()
                .raw_input(&ctx.input)
                .conversation(conv.clone())
                .build()
        } else {
            MatchingContext::simple(&ctx.input)
        }
    }

    /// Find a tool that matches the semantic intent
    async fn find_tool_for_intent(&self, result: &MatchResult) -> Option<UnifiedTool> {
        let tools = self.cached_tools.read().await;

        // Try to match by intent type
        let intent_type = &result.intent.intent_type;

        // Direct name match
        if let Some(tool) = tools.iter().find(|t| t.name == *intent_type) {
            return Some(tool.clone());
        }

        // Match by category (e.g., "search" category maps to search tools)
        let category_name = format!("{:?}", result.intent.category).to_lowercase();
        if let Some(tool) = tools.iter().find(|t| {
            t.name.to_lowercase().contains(&category_name)
                || t.description.to_lowercase().contains(&category_name)
        }) {
            return Some(tool.clone());
        }

        // Match by keywords in tool description
        let keywords: Vec<&str> = intent_type.split('_').collect();
        for tool in tools.iter() {
            let tool_desc_lower = tool.description.to_lowercase();
            let tool_name_lower = tool.name.to_lowercase();
            if keywords
                .iter()
                .any(|kw| tool_desc_lower.contains(kw) || tool_name_lower.contains(kw))
            {
                return Some(tool.clone());
            }
        }

        // No tool found - create a synthetic one for the intent
        debug!(
            intent = %intent_type,
            "No tool found for intent, creating synthetic tool"
        );
        Some(UnifiedTool::new(
            intent_type,
            intent_type,
            &format!("Inferred tool for intent: {}", intent_type),
            ToolSource::Custom { rule_index: 0 },
        ))
    }
}

// =============================================================================
// Intent Conversion
// =============================================================================

impl From<crate::semantic::SemanticIntent> for crate::payload::Intent {
    fn from(semantic: crate::semantic::SemanticIntent) -> Self {
        use crate::payload::Intent;
        use crate::semantic::intent::{BuiltinCapability, IntentCategory};

        // Map SemanticIntent to payload::Intent based on category
        match semantic.category {
            IntentCategory::Builtin(capability) => match capability {
                BuiltinCapability::Search => Intent::BuiltinSearch,
                BuiltinCapability::Mcp => Intent::BuiltinMcp,
                BuiltinCapability::Video => Intent::Custom("video".to_string()),
            },
            IntentCategory::Skills(skill_id) => Intent::Skills(skill_id),
            IntentCategory::Command(cmd) => Intent::Custom(cmd),
            IntentCategory::Semantic(name) => {
                if name.is_empty() || name == "general" {
                    Intent::GeneralChat
                } else {
                    Intent::Custom(name)
                }
            }
            IntentCategory::General => Intent::GeneralChat,
        }
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
    async fn test_unified_router_creation() {
        let matcher = create_test_matcher();
        let router = UnifiedRouter::minimal(matcher);

        // Minimal config: L1 only
        assert!(router.config.l1_enabled);
        assert!(!router.config.l2_enabled); // Minimal disables L2
        assert!(!router.config.l3_enabled); // Minimal disables L3
    }

    #[tokio::test]
    async fn test_routing_disabled() {
        let matcher = create_test_matcher();
        let config = RoutingConfig {
            enabled: false,
            ..Default::default()
        };
        let router = UnifiedRouter::new(config, None, matcher);

        let ctx = RoutingContext::new("test input");
        let result = router.route(&ctx).await;

        assert!(matches!(result, RoutingResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn test_routing_empty_input() {
        let matcher = create_test_matcher();
        let router = UnifiedRouter::minimal(matcher);

        let ctx = RoutingContext::new("");
        let result = router.route(&ctx).await;

        assert!(matches!(result, RoutingResult::NoMatch { .. }));
    }

    #[tokio::test]
    async fn test_routing_no_match() {
        let matcher = create_test_matcher();
        let router = UnifiedRouter::minimal(matcher);

        let ctx = RoutingContext::new("hello world");
        let result = router.route(&ctx).await;

        // With no rules configured, should return no match
        assert!(matches!(result, RoutingResult::NoMatch { .. }));
    }

    #[tokio::test]
    async fn test_update_tools() {
        let matcher = create_test_matcher();
        let router = UnifiedRouter::minimal(matcher);

        let tools = vec![
            UnifiedTool::new("search", "search", "Search the web", ToolSource::Native),
            UnifiedTool::new("translate", "translate", "Translate text", ToolSource::Native),
        ];

        router.update_tools(tools.clone()).await;

        let cached = router.cached_tools.read().await;
        assert_eq!(cached.len(), 2);
    }

    #[tokio::test]
    async fn test_routing_config_presets() {
        let fast = RoutingConfig::fast();
        assert!(!fast.l3_enabled);
        assert!(fast.l1_enabled);
        assert!(fast.l2_enabled);

        let minimal = RoutingConfig::minimal();
        assert!(minimal.l1_enabled);
        assert!(!minimal.l2_enabled);
        assert!(!minimal.l3_enabled);

        let full = RoutingConfig::full();
        assert!(full.l1_enabled);
        assert!(full.l2_enabled);
        assert!(full.l3_enabled);
    }
}

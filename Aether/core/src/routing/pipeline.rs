//! Intent Routing Pipeline
//!
//! Main coordinator that orchestrates all pipeline components:
//!
//! - Intent Cache (fast path)
//! - Layer Execution Engine (L1/L2/L3)
//! - Confidence Calibrator
//! - Intent Aggregator
//! - Clarification Integrator

use crate::dispatcher::UnifiedTool;
use crate::providers::AiProvider;
use crate::routing::{
    CacheMetrics, ClarificationIntegrator,
    ConfidenceCalibrator, IntentAggregator, IntentCache, IntentAction,
    LayerExecutionEngine, PipelineConfig, PipelineResult,
    RoutingContext,
};
use crate::semantic::SemanticMatcher;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Intent Routing Pipeline
///
/// Main entry point for the enhanced intent routing system.
/// Coordinates all components to process user input and produce routing decisions.
pub struct IntentRoutingPipeline {
    /// Pipeline configuration
    config: PipelineConfig,

    /// Intent cache for fast path
    cache: IntentCache,

    /// Layer execution engine
    engine: LayerExecutionEngine,

    /// Confidence calibrator
    calibrator: ConfidenceCalibrator,

    /// Intent aggregator
    aggregator: IntentAggregator,

    /// Clarification integrator
    clarification: ClarificationIntegrator,

    /// Cached tools for routing
    tools: Arc<tokio::sync::RwLock<Vec<UnifiedTool>>>,
}

impl IntentRoutingPipeline {
    /// Create a new pipeline with the given configuration
    pub fn new(
        config: PipelineConfig,
        semantic_matcher: Arc<SemanticMatcher>,
    ) -> Self {
        // Create components
        let cache = IntentCache::new(config.cache.clone());
        let calibrator = ConfidenceCalibrator::with_tool_configs(
            config.confidence.clone(),
            config.tools.clone(),
        );
        let aggregator = IntentAggregator::new(config.confidence.clone())
            .with_calibrator(ConfidenceCalibrator::new(config.confidence.clone()));
        let clarification = ClarificationIntegrator::new(config.clarification.clone());

        let engine = LayerExecutionEngine::new(
            semantic_matcher,
            config.layers.clone(),
            config.confidence.clone(),
        );

        Self {
            config,
            cache,
            engine,
            calibrator,
            aggregator,
            clarification,
            tools: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    /// Create with an AI provider for L3
    pub fn with_provider(
        config: PipelineConfig,
        semantic_matcher: Arc<SemanticMatcher>,
        provider: Arc<dyn AiProvider>,
    ) -> Self {
        let mut pipeline = Self::new(config.clone(), semantic_matcher);

        // Add L3 router to engine
        if config.layers.l3_enabled {
            pipeline.engine = pipeline.engine.with_l3(provider);
        }

        pipeline
    }

    /// Update the tool list
    pub async fn update_tools(&self, tools: Vec<UnifiedTool>) {
        let mut cached = self.tools.write().await;
        *cached = tools.clone();

        // Also update engine's tool list
        self.engine.update_tools(tools).await;
    }

    /// Process user input through the pipeline
    ///
    /// # Arguments
    ///
    /// * `ctx` - Routing context with input and conversation history
    ///
    /// # Returns
    ///
    /// `PipelineResult` with routing decision
    pub async fn process(&self, ctx: RoutingContext) -> PipelineResult {
        // Check if pipeline is enabled
        if !self.config.enabled {
            return PipelineResult::skipped("Pipeline disabled");
        }

        let start = Instant::now();

        info!(
            input_length = ctx.input.len(),
            has_conversation = ctx.conversation.is_some(),
            "IntentRoutingPipeline: Processing input"
        );

        // Fast path: Check cache first
        if self.config.cache.enabled {
            if let Some(result) = self.try_cache_hit(&ctx).await {
                let latency_ms = start.elapsed().as_millis() as u64;
                info!(
                    latency_ms,
                    cache_hit = true,
                    "IntentRoutingPipeline: Cache hit"
                );
                return result;
            }
        }

        // Full flow: Execute layers
        let layer_result = self.engine.execute(&ctx).await;

        // No signals = general chat
        if layer_result.signals.is_empty() {
            return PipelineResult::general_chat(&ctx.input);
        }

        // Aggregate signals
        let intent = self.aggregator.aggregate(layer_result.signals, &ctx).await;

        // Handle based on action
        let result = self.handle_intent(ctx.clone(), intent).await;

        // Record to cache on success
        if let PipelineResult::Executed { ref tool_name, ref parameters, .. } = result {
            self.record_cache_success(&ctx, tool_name, parameters).await;
        }

        let latency_ms = start.elapsed().as_millis() as u64;
        info!(
            latency_ms,
            result = %result,
            "IntentRoutingPipeline: Processing complete"
        );

        result
    }

    /// Resume from a clarification session
    ///
    /// # Arguments
    ///
    /// * `session_id` - Session ID from the clarification request
    /// * `user_input` - User-provided value
    ///
    /// # Returns
    ///
    /// `PipelineResult` with resumed routing decision
    pub async fn resume_clarification(
        &self,
        session_id: &str,
        user_input: &str,
    ) -> PipelineResult {
        match self.clarification.resume(session_id, user_input).await {
            Ok(resume_result) => {
                // If complete, handle the intent
                if resume_result.is_complete() {
                    self.handle_intent(resume_result.context, resume_result.intent).await
                } else {
                    // Still need more params - continue clarification
                    self.start_clarification(resume_result.context, resume_result.intent).await
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to resume clarification");
                PipelineResult::cancelled(format!("Clarification failed: {}", e))
            }
        }
    }

    /// Cancel a clarification session
    pub async fn cancel_clarification(&self, session_id: &str) -> PipelineResult {
        if let Err(e) = self.clarification.cancel(session_id).await {
            warn!(error = %e, "Failed to cancel clarification");
        }
        PipelineResult::cancelled("Clarification cancelled by user")
    }

    // =========================================================================
    // Cache Handling
    // =========================================================================

    /// Try to get a result from cache
    async fn try_cache_hit(&self, ctx: &RoutingContext) -> Option<PipelineResult> {
        let cached = self.cache.get(&ctx.input).await?;

        // Check if confidence is high enough for cache auto-execute
        if cached.confidence >= self.config.cache.cache_auto_execute_threshold {
            debug!(
                tool = cached.tool_name,
                confidence = cached.confidence,
                "Cache hit with high confidence"
            );

            // Return executed result
            return Some(PipelineResult::executed(
                &cached.tool_name,
                format!("Cached: {}", cached.tool_name),
                cached.parameters.clone(),
            ));
        }

        // Cache hit but confidence too low - might still use it with confirmation
        debug!(
            tool = cached.tool_name,
            confidence = cached.confidence,
            threshold = self.config.cache.cache_auto_execute_threshold,
            "Cache hit but confidence below auto-execute threshold"
        );

        None
    }

    /// Record a successful execution to cache
    async fn record_cache_success(
        &self,
        ctx: &RoutingContext,
        tool_name: &str,
        parameters: &serde_json::Value,
    ) {
        if !self.config.cache.enabled {
            return;
        }

        // Add to cache
        self.cache.put(
            &ctx.input,
            tool_name,
            parameters.clone(),
            1.0, // Start with high confidence
            IntentAction::Execute,
        ).await;

        debug!(tool = tool_name, "Recorded success to cache");
    }

    /// Record a failure to cache
    pub async fn record_cache_failure(&self, input: &str, tool_name: &str) {
        self.cache.record_failure(input).await;

        // Also record in calibrator
        self.calibrator.record_failure(tool_name, input).await;

        debug!(tool = tool_name, "Recorded failure to cache");
    }

    // =========================================================================
    // Intent Handling
    // =========================================================================

    /// Handle aggregated intent based on its action
    async fn handle_intent(
        &self,
        ctx: RoutingContext,
        intent: crate::routing::AggregatedIntent,
    ) -> PipelineResult {
        match &intent.action {
            IntentAction::Execute => {
                // Execute the tool directly
                self.execute_tool(&ctx, intent).await
            }
            IntentAction::ExecutePlan { plan } => {
                // Execute multi-step plan
                // Note: Plan execution will be implemented in Phase 3
                // For now, return the plan info for UI confirmation
                PipelineResult::executed(
                    "plan",
                    serde_json::to_string(&plan).unwrap_or_default(),
                    serde_json::json!({
                        "plan_id": plan.id.to_string(),
                        "step_count": plan.steps.len(),
                        "confidence": intent.final_confidence,
                    }),
                )
            }
            IntentAction::RequestConfirmation => {
                // Return pending confirmation
                // Note: In real implementation, this would go to UI
                // For now, auto-execute if we have high-ish confidence
                if intent.final_confidence >= self.config.confidence.requires_confirmation {
                    self.execute_tool(&ctx, intent).await
                } else {
                    PipelineResult::general_chat(&ctx.input)
                }
            }
            IntentAction::RequestClarification { .. } => {
                // Start clarification flow
                self.start_clarification(ctx, intent).await
            }
            IntentAction::GeneralChat => {
                PipelineResult::general_chat(&ctx.input)
            }
        }
    }

    /// Signal that a tool was matched and needs execution by the caller.
    ///
    /// The pipeline only handles intent routing and clarification.
    /// Actual tool execution is delegated to AetherCore which has access
    /// to capability executors (VideoStrategy, SearchExecutor, etc.)
    async fn execute_tool(
        &self,
        ctx: &RoutingContext,
        intent: crate::routing::AggregatedIntent,
    ) -> PipelineResult {
        let tool = match &intent.primary.tool {
            Some(t) => t,
            None => return PipelineResult::general_chat("No tool matched"),
        };

        info!(
            tool_name = %tool.name,
            confidence = %intent.final_confidence,
            "Pipeline: Tool matched, delegating execution to caller"
        );

        // Return ToolMatched to signal that caller should execute this tool
        PipelineResult::tool_matched(&tool.name, intent.primary.parameters.clone(), &ctx.input)
    }

    /// Start a clarification flow
    async fn start_clarification(
        &self,
        ctx: RoutingContext,
        intent: crate::routing::AggregatedIntent,
    ) -> PipelineResult {
        match self.clarification.start_clarification(ctx, intent).await {
            Ok(request) => PipelineResult::PendingClarification(request),
            Err(e) => {
                warn!(error = %e, "Failed to start clarification");
                PipelineResult::cancelled(format!("Failed to start clarification: {}", e))
            }
        }
    }

    // =========================================================================
    // Metrics and Status
    // =========================================================================

    /// Get cache metrics
    pub async fn cache_metrics(&self) -> CacheMetrics {
        self.cache.metrics().await
    }

    /// Get pending clarification count
    pub async fn pending_clarification_count(&self) -> usize {
        self.clarification.pending_count().await
    }

    /// Cleanup expired clarifications
    pub async fn cleanup_expired_clarifications(&self) -> usize {
        self.clarification.cleanup_expired().await
    }

    /// Get current configuration
    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    /// Check if pipeline is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use crate::semantic::MatcherConfig;

    fn create_test_pipeline() -> IntentRoutingPipeline {
        let config = PipelineConfig::enabled();
        let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
        IntentRoutingPipeline::new(config, matcher)
    }

    #[tokio::test]
    async fn test_pipeline_creation() {
        let pipeline = create_test_pipeline();
        assert!(pipeline.is_enabled());
    }

    #[tokio::test]
    async fn test_pipeline_disabled() {
        let config = PipelineConfig::default(); // disabled by default
        let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
        let pipeline = IntentRoutingPipeline::new(config, matcher);

        let ctx = RoutingContext::new("test input");
        let result = pipeline.process(ctx).await;

        assert!(matches!(result, PipelineResult::Skipped { .. }));
    }

    #[tokio::test]
    async fn test_pipeline_general_chat_fallback() {
        let pipeline = create_test_pipeline();

        let ctx = RoutingContext::new("hello world");
        let result = pipeline.process(ctx).await;

        // Without rules, should fall back to general chat
        assert!(matches!(result, PipelineResult::GeneralChat { .. }));
    }

    #[tokio::test]
    async fn test_pipeline_update_tools() {
        let pipeline = create_test_pipeline();

        let tools = vec![
            UnifiedTool::new("search", "search", "Search tool", ToolSource::Native),
        ];

        pipeline.update_tools(tools).await;

        let cached = pipeline.tools.read().await;
        assert_eq!(cached.len(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_cache_metrics() {
        let pipeline = create_test_pipeline();

        let metrics = pipeline.cache_metrics().await;
        assert_eq!(metrics.hits, 0);
        assert_eq!(metrics.misses, 0);
    }

    #[tokio::test]
    async fn test_pipeline_record_cache_success() {
        let pipeline = create_test_pipeline();

        let ctx = RoutingContext::new("search weather");
        pipeline.record_cache_success(
            &ctx,
            "search",
            &serde_json::json!({"query": "weather"}),
        ).await;

        // Verify cache was updated
        let cached = pipeline.cache.get("search weather").await;
        assert!(cached.is_some());
    }

    #[tokio::test]
    async fn test_pipeline_cancel_clarification() {
        let pipeline = create_test_pipeline();

        let result = pipeline.cancel_clarification("non-existent").await;
        assert!(matches!(result, PipelineResult::Cancelled { .. }));
    }

    #[tokio::test]
    async fn test_pipeline_pending_clarifications() {
        let pipeline = create_test_pipeline();

        let count = pipeline.pending_clarification_count().await;
        assert_eq!(count, 0);
    }
}

//! Layer Execution Engine
//!
//! Orchestrates L1/L2/L3 layer execution with configurable modes:
//! - Sequential: Run layers in order, stop on high-confidence match
//! - Parallel: Run L2/L3 in parallel after L1
//! - L1Only: Only run L1 for minimal latency

use crate::dispatcher::UnifiedTool;
use crate::providers::AiProvider;
use crate::routing::{
    ConfidenceThresholds, ExecutionMode, IntentSignal, L1RegexMatcher, L2SemanticMatcher,
    EnhancedL3Router, LayerConfig, RoutingContext, RoutingLayerType,
};
use crate::semantic::SemanticMatcher;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Layer Execution Engine
///
/// Coordinates the execution of L1/L2/L3 routing layers based on configuration.
pub struct LayerExecutionEngine {
    /// L1 Regex matcher
    l1_matcher: L1RegexMatcher,

    /// L2 Semantic matcher
    l2_matcher: L2SemanticMatcher,

    /// L3 AI router (optional)
    l3_router: Option<EnhancedL3Router>,

    /// Layer configuration
    config: LayerConfig,

    /// Confidence thresholds
    thresholds: ConfidenceThresholds,

    /// Cached tools for L3
    tools: Arc<tokio::sync::RwLock<Vec<UnifiedTool>>>,
}

/// Result from layer execution
#[derive(Debug, Clone)]
pub struct LayerExecutionResult {
    /// Signals from all executed layers
    pub signals: Vec<IntentSignal>,

    /// Which layers were executed
    pub executed_layers: Vec<RoutingLayerType>,

    /// Total execution time
    pub total_latency_ms: u64,

    /// Whether execution was terminated early
    pub early_exit: bool,

    /// Early exit reason (if applicable)
    pub early_exit_reason: Option<String>,
}

impl LayerExecutionResult {
    /// Create a new empty result
    pub fn new() -> Self {
        Self {
            signals: Vec::new(),
            executed_layers: Vec::new(),
            total_latency_ms: 0,
            early_exit: false,
            early_exit_reason: None,
        }
    }

    /// Add a signal from a layer
    pub fn add_signal(&mut self, signal: IntentSignal) {
        if !self.executed_layers.contains(&signal.layer) {
            self.executed_layers.push(signal.layer);
        }
        self.signals.push(signal);
    }

    /// Mark as early exit
    pub fn with_early_exit(mut self, reason: impl Into<String>) -> Self {
        self.early_exit = true;
        self.early_exit_reason = Some(reason.into());
        self
    }

    /// Get the highest confidence signal
    pub fn best_signal(&self) -> Option<&IntentSignal> {
        self.signals
            .iter()
            .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap())
    }

    /// Check if any signal has a tool match
    pub fn has_tool_match(&self) -> bool {
        self.signals.iter().any(|s| s.has_tool())
    }
}

impl Default for LayerExecutionResult {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerExecutionEngine {
    /// Create a new layer execution engine
    pub fn new(
        semantic_matcher: Arc<SemanticMatcher>,
        config: LayerConfig,
        thresholds: ConfidenceThresholds,
    ) -> Self {
        Self {
            l1_matcher: L1RegexMatcher::new(Arc::clone(&semantic_matcher)),
            l2_matcher: L2SemanticMatcher::new(Arc::clone(&semantic_matcher)),
            l3_router: None,
            config,
            thresholds,
            tools: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    /// Enable L3 with an AI provider
    pub fn with_l3(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.l3_router = Some(
            EnhancedL3Router::new(provider)
                .with_timeout(Duration::from_millis(self.config.l3_timeout_ms))
                .with_min_confidence(self.config.l3_min_confidence),
        );
        self
    }

    /// Update cached tools
    pub async fn update_tools(&self, tools: Vec<UnifiedTool>) {
        let mut cached = self.tools.write().await;
        *cached = tools;
    }

    /// Execute layers based on configuration
    pub async fn execute(&self, ctx: &RoutingContext) -> LayerExecutionResult {
        let start = Instant::now();
        let mut result = LayerExecutionResult::new();

        info!(
            mode = ?self.config.execution_mode,
            "LayerExecutionEngine: Starting execution"
        );

        match self.config.execution_mode {
            ExecutionMode::Sequential => {
                self.execute_sequential(ctx, &mut result).await;
            }
            ExecutionMode::Parallel => {
                self.execute_parallel(ctx, &mut result).await;
            }
            ExecutionMode::L1Only => {
                self.execute_l1_only(ctx, &mut result).await;
            }
        }

        result.total_latency_ms = start.elapsed().as_millis() as u64;

        info!(
            total_latency_ms = result.total_latency_ms,
            signal_count = result.signals.len(),
            early_exit = result.early_exit,
            "LayerExecutionEngine: Execution complete"
        );

        result
    }

    /// Execute layers sequentially (L1 → L2 → L3)
    async fn execute_sequential(&self, ctx: &RoutingContext, result: &mut LayerExecutionResult) {
        // L1: Always run first
        if self.config.l1_enabled {
            if let Some(signal) = self.l1_matcher.match_input(ctx).await {
                result.add_signal(signal.clone());

                // Early exit if L1 has high confidence
                if signal.confidence >= self.config.l1_auto_accept_threshold {
                    debug!(
                        confidence = signal.confidence,
                        "L1 high confidence - early exit"
                    );
                    *result = std::mem::take(result).with_early_exit("L1 high confidence");
                    return;
                }
            }
        }

        // L2: Run if L1 didn't match or confidence too low
        if self.config.l2_enabled {
            if let Some(signal) = self.l2_matcher.match_input(ctx).await {
                result.add_signal(signal.clone());

                // Skip L3 if L2 has sufficient confidence
                if signal.confidence >= self.config.l2_skip_l3_threshold {
                    debug!(
                        confidence = signal.confidence,
                        threshold = self.config.l2_skip_l3_threshold,
                        "L2 sufficient - skipping L3"
                    );
                    *result = std::mem::take(result).with_early_exit("L2 sufficient confidence");
                    return;
                }
            }
        }

        // L3: Run if enabled and earlier layers insufficient
        if self.config.l3_enabled {
            self.execute_l3(ctx, result).await;
        }
    }

    /// Execute L2 and L3 in parallel after L1
    async fn execute_parallel(&self, ctx: &RoutingContext, result: &mut LayerExecutionResult) {
        // L1: Always run first (it's fast)
        if self.config.l1_enabled {
            if let Some(signal) = self.l1_matcher.match_input(ctx).await {
                result.add_signal(signal.clone());

                // Early exit if L1 has high confidence
                if signal.confidence >= self.config.l1_auto_accept_threshold {
                    *result = std::mem::take(result).with_early_exit("L1 high confidence");
                    return;
                }
            }
        }

        // Run L2 and L3 in parallel
        let l2_enabled = self.config.l2_enabled;
        let l3_enabled = self.config.l3_enabled && self.l3_router.is_some();

        if l2_enabled || l3_enabled {
            let l2_future = async {
                if l2_enabled {
                    self.l2_matcher.match_input(ctx).await
                } else {
                    None
                }
            };

            let l3_future = async {
                if l3_enabled {
                    self.execute_l3_internal(ctx).await
                } else {
                    None
                }
            };

            let (l2_result, l3_result) = tokio::join!(l2_future, l3_future);

            if let Some(signal) = l2_result {
                result.add_signal(signal);
            }

            if let Some(signal) = l3_result {
                result.add_signal(signal);
            }
        }
    }

    /// Execute L1 only (minimal latency mode)
    async fn execute_l1_only(&self, ctx: &RoutingContext, result: &mut LayerExecutionResult) {
        if self.config.l1_enabled {
            if let Some(signal) = self.l1_matcher.match_input(ctx).await {
                result.add_signal(signal);
            }
        }
    }

    /// Execute L3 and add result to result
    async fn execute_l3(&self, ctx: &RoutingContext, result: &mut LayerExecutionResult) {
        if let Some(signal) = self.execute_l3_internal(ctx).await {
            result.add_signal(signal);
        }
    }

    /// Internal L3 execution (returns Option<IntentSignal>)
    async fn execute_l3_internal(&self, ctx: &RoutingContext) -> Option<IntentSignal> {
        let router = self.l3_router.as_ref()?;
        let tools = self.tools.read().await.clone();

        if tools.is_empty() {
            debug!("L3: No tools available");
            return None;
        }

        router.route(ctx, &tools).await
    }

    /// Get current configuration
    pub fn config(&self) -> &LayerConfig {
        &self.config
    }

    /// Get current thresholds
    pub fn thresholds(&self) -> &ConfidenceThresholds {
        &self.thresholds
    }

    /// Update configuration
    pub fn set_config(&mut self, config: LayerConfig) {
        self.config = config;
    }

    /// Update thresholds
    pub fn set_thresholds(&mut self, thresholds: ConfidenceThresholds) {
        self.thresholds = thresholds;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::MatcherConfig;

    fn create_test_engine() -> LayerExecutionEngine {
        let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
        LayerExecutionEngine::new(
            matcher,
            LayerConfig::default(),
            ConfidenceThresholds::default(),
        )
    }

    #[tokio::test]
    async fn test_engine_creation() {
        let engine = create_test_engine();
        assert!(engine.l3_router.is_none());
    }

    #[tokio::test]
    async fn test_execute_sequential_no_match() {
        let engine = create_test_engine();
        let ctx = RoutingContext::new("hello world");

        let result = engine.execute(&ctx).await;

        // Without rules, should have no signals
        assert!(result.signals.is_empty());
        assert!(!result.early_exit);
    }

    #[tokio::test]
    async fn test_execute_l1_only() {
        let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
        let config = LayerConfig::l1_only();
        let engine = LayerExecutionEngine::new(
            matcher,
            config,
            ConfidenceThresholds::default(),
        );

        let ctx = RoutingContext::new("hello");
        let result = engine.execute(&ctx).await;

        // L1 only mode should only try L1
        assert!(!result.executed_layers.contains(&RoutingLayerType::L2Semantic));
        assert!(!result.executed_layers.contains(&RoutingLayerType::L3Inference));
    }

    #[tokio::test]
    async fn test_update_tools() {
        use crate::dispatcher::ToolSource;

        let engine = create_test_engine();

        let tools = vec![
            UnifiedTool::new("search", "search", "Search tool", ToolSource::Native),
        ];

        engine.update_tools(tools.clone()).await;

        let cached = engine.tools.read().await;
        assert_eq!(cached.len(), 1);
    }

    #[test]
    fn test_layer_execution_result() {
        let mut result = LayerExecutionResult::new();

        let signal = IntentSignal::new(RoutingLayerType::L1Regex, 0.9);
        result.add_signal(signal);

        assert_eq!(result.signals.len(), 1);
        assert!(result.executed_layers.contains(&RoutingLayerType::L1Regex));
        assert!(result.has_tool_match() == false); // no tool in signal
    }
}

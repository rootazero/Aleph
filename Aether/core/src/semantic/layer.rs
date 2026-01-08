//! Matching Layer trait for pluggable semantic matching.
//!
//! This module defines the `MatchingLayer` trait which allows different
//! matching strategies (Command, Regex, Keyword, Context) to be implemented
//! as independent, pluggable layers.
//!
//! # Architecture
//!
//! The layer pattern enables:
//! - Independent testing of each matching strategy
//! - Runtime enable/disable of layers
//! - Easy addition of new matching strategies
//! - Decoupled matching logic
//!
//! # Layer Execution Order
//!
//! Layers are executed in priority order (lower = higher priority):
//! - Command Layer (priority 0): Exact ^/xxx command matches
//! - Regex Layer (priority 1): Pattern-based matches
//! - Keyword Layer (priority 2): Weighted keyword scoring
//! - Context Layer (priority 3): Context-aware inference

use super::context::MatchingContext;
use super::matcher::MatchResult;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Strategy trait for matching layer execution
///
/// Each matching strategy (Command, Regex, Keyword, Context) implements
/// this trait to provide a pluggable matching layer. Layers are registered
/// with the `LayerChain` and executed in priority order.
///
/// # Example Implementation
///
/// ```ignore
/// pub struct CommandLayer {
///     rules: Vec<CompiledRule>,
///     enabled: AtomicBool,
/// }
///
/// #[async_trait]
/// impl MatchingLayer for CommandLayer {
///     fn layer_id(&self) -> &str {
///         "command"
///     }
///
///     fn priority(&self) -> u32 {
///         0 // Command executes first
///     }
///
///     fn is_enabled(&self) -> bool {
///         self.enabled.load(Ordering::SeqCst)
///     }
///
///     fn set_enabled(&self, enabled: bool) {
///         self.enabled.store(enabled, Ordering::SeqCst);
///     }
///
///     async fn try_match(&self, ctx: &MatchingContext) -> Option<MatchResult> {
///         // Try to match command patterns
///         None
///     }
/// }
/// ```
#[async_trait]
pub trait MatchingLayer: Send + Sync {
    /// Get unique layer identifier
    ///
    /// Used for logging and configuration.
    fn layer_id(&self) -> &str;

    /// Get the priority for execution ordering
    ///
    /// Lower values = higher priority (executed first).
    /// Default priorities:
    /// - Command: 0
    /// - Regex: 1
    /// - Keyword: 2
    /// - Context: 3
    fn priority(&self) -> u32;

    /// Check if this layer is currently enabled
    ///
    /// Disabled layers are skipped during matching.
    fn is_enabled(&self) -> bool;

    /// Enable or disable this layer
    fn set_enabled(&self, enabled: bool);

    /// Try to match input against this layer's rules
    ///
    /// Returns `Some(MatchResult)` if a match is found, `None` otherwise.
    /// The LayerChain will continue to the next layer if `None` is returned
    /// or if the match confidence is below the configured threshold.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The matching context containing input, conversation history, etc.
    ///
    /// # Returns
    ///
    /// Optional match result with intent and confidence
    async fn try_match(&self, ctx: &MatchingContext) -> Option<MatchResult>;

    /// Check if this layer should short-circuit on match
    ///
    /// If true, the LayerChain will stop after this layer matches.
    /// Default is true for high-confidence matches (e.g., exact commands).
    fn is_terminal(&self) -> bool {
        true
    }

    /// Get the minimum confidence threshold for this layer
    ///
    /// Matches below this threshold are not considered valid.
    /// Default is 0.0 (accept all matches).
    fn confidence_threshold(&self) -> f64 {
        0.0
    }
}

/// Chain of matching layers executed in priority order
///
/// The LayerChain:
/// - Maintains a registry of matching layers
/// - Sorts layers by priority before execution
/// - Executes enabled layers in order until a match is found
/// - Supports merging results from multiple layers
pub struct LayerChain {
    layers: Vec<Arc<dyn MatchingLayer>>,
}

impl LayerChain {
    /// Create a new empty layer chain
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
        }
    }

    /// Register a matching layer
    ///
    /// Layers are automatically sorted by priority after registration.
    pub fn register(&mut self, layer: Arc<dyn MatchingLayer>) {
        let id = layer.layer_id().to_string();
        let priority = layer.priority();
        self.layers.push(layer);
        self.layers.sort_by_key(|l| l.priority());
        tracing::debug!(
            layer = %id,
            priority = priority,
            "Registered matching layer"
        );
    }

    /// Builder method to register a layer
    pub fn with_layer(mut self, layer: Arc<dyn MatchingLayer>) -> Self {
        self.register(layer);
        self
    }

    /// Get a layer by ID
    pub fn get_layer(&self, layer_id: &str) -> Option<&dyn MatchingLayer> {
        self.layers
            .iter()
            .find(|l| l.layer_id() == layer_id)
            .map(|l| l.as_ref())
    }

    /// Check if a layer is registered
    pub fn has_layer(&self, layer_id: &str) -> bool {
        self.layers.iter().any(|l| l.layer_id() == layer_id)
    }

    /// Get all registered layers
    pub fn layers(&self) -> &[Arc<dyn MatchingLayer>] {
        &self.layers
    }

    /// Enable or disable a layer by ID
    pub fn set_layer_enabled(&self, layer_id: &str, enabled: bool) -> bool {
        if let Some(layer) = self.layers.iter().find(|l| l.layer_id() == layer_id) {
            layer.set_enabled(enabled);
            tracing::info!(
                layer = %layer_id,
                enabled = enabled,
                "Layer enabled state changed"
            );
            true
        } else {
            false
        }
    }

    /// Execute all enabled layers in priority order
    ///
    /// Stops at the first terminal match or returns the best result.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The matching context
    ///
    /// # Returns
    ///
    /// Optional match result (best match found)
    pub async fn execute(&self, ctx: &MatchingContext) -> Option<MatchResult> {
        let mut best_result: Option<MatchResult> = None;

        for layer in &self.layers {
            if !layer.is_enabled() {
                tracing::trace!(
                    layer = %layer.layer_id(),
                    "Skipping disabled layer"
                );
                continue;
            }

            tracing::trace!(
                layer = %layer.layer_id(),
                priority = layer.priority(),
                "Executing matching layer"
            );

            if let Some(result) = layer.try_match(ctx).await {
                // Check if result meets confidence threshold
                if result.confidence < layer.confidence_threshold() {
                    tracing::trace!(
                        layer = %layer.layer_id(),
                        confidence = result.confidence,
                        threshold = layer.confidence_threshold(),
                        "Match below confidence threshold, continuing"
                    );
                    continue;
                }

                tracing::debug!(
                    layer = %layer.layer_id(),
                    intent = %result.intent.intent_type,
                    confidence = result.confidence,
                    "Layer matched"
                );

                // Terminal layer with good confidence - return immediately
                if layer.is_terminal() && result.confidence >= 0.9 {
                    return Some(result);
                }

                // Track best result
                match &best_result {
                    Some(current) if current.confidence >= result.confidence => {
                        // Keep current best
                    }
                    _ => {
                        best_result = Some(result);
                    }
                }
            }
        }

        best_result
    }

    /// Execute layers and merge non-terminal results
    ///
    /// Unlike `execute`, this method continues through all layers
    /// and merges results from non-terminal layers.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The matching context
    ///
    /// # Returns
    ///
    /// Tuple of (terminal_result, merged_non_terminal_results)
    pub async fn execute_with_merge(
        &self,
        ctx: &MatchingContext,
    ) -> (Option<MatchResult>, Vec<MatchResult>) {
        let mut terminal_result: Option<MatchResult> = None;
        let mut non_terminal_results: Vec<MatchResult> = Vec::new();

        for layer in &self.layers {
            if !layer.is_enabled() {
                continue;
            }

            if let Some(result) = layer.try_match(ctx).await {
                if result.confidence < layer.confidence_threshold() {
                    continue;
                }

                if layer.is_terminal() && result.confidence >= 0.9 {
                    // High-confidence terminal match - return immediately
                    terminal_result = Some(result);
                    break;
                } else if layer.is_terminal() {
                    // Lower confidence terminal match - track but continue
                    match &terminal_result {
                        Some(current) if current.confidence >= result.confidence => {}
                        _ => terminal_result = Some(result),
                    }
                } else {
                    // Non-terminal result - add to merge list
                    non_terminal_results.push(result);
                }
            }
        }

        (terminal_result, non_terminal_results)
    }
}

impl Default for LayerChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper struct for layers with atomic enabled flag
pub struct LayerEnabledFlag {
    enabled: AtomicBool,
}

impl LayerEnabledFlag {
    /// Create new enabled flag (default: enabled)
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
        }
    }

    /// Check if enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Set enabled state
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }
}

impl Default for LayerEnabledFlag {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::intent::SemanticIntent;

    /// Mock layer for testing
    struct MockLayer {
        id: String,
        priority: u32,
        enabled: LayerEnabledFlag,
        result: Option<MatchResult>,
        terminal: bool,
    }

    impl MockLayer {
        fn new(id: &str, priority: u32, result: Option<MatchResult>) -> Self {
            Self {
                id: id.to_string(),
                priority,
                enabled: LayerEnabledFlag::new(true),
                result,
                terminal: true,
            }
        }

        fn non_terminal(mut self) -> Self {
            self.terminal = false;
            self
        }
    }

    #[async_trait]
    impl MatchingLayer for MockLayer {
        fn layer_id(&self) -> &str {
            &self.id
        }

        fn priority(&self) -> u32 {
            self.priority
        }

        fn is_enabled(&self) -> bool {
            self.enabled.is_enabled()
        }

        fn set_enabled(&self, enabled: bool) {
            self.enabled.set_enabled(enabled);
        }

        async fn try_match(&self, _ctx: &MatchingContext) -> Option<MatchResult> {
            self.result.clone()
        }

        fn is_terminal(&self) -> bool {
            self.terminal
        }
    }

    fn make_result(confidence: f64) -> MatchResult {
        MatchResult {
            intent: SemanticIntent::general().with_confidence(confidence),
            confidence,
            rule_index: None,
            needs_ai_fallback: false,
        }
    }

    #[tokio::test]
    async fn test_layer_chain_empty() {
        let chain = LayerChain::new();
        let ctx = MatchingContext::simple("test input");

        let result = chain.execute(&ctx).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_layer_chain_priority_ordering() {
        let chain = LayerChain::new()
            .with_layer(Arc::new(MockLayer::new("layer2", 2, None)))
            .with_layer(Arc::new(MockLayer::new("layer1", 1, None)))
            .with_layer(Arc::new(MockLayer::new("layer0", 0, None)));

        // Verify layers are sorted by priority
        assert_eq!(chain.layers()[0].layer_id(), "layer0");
        assert_eq!(chain.layers()[1].layer_id(), "layer1");
        assert_eq!(chain.layers()[2].layer_id(), "layer2");
    }

    #[tokio::test]
    async fn test_layer_chain_first_match() {
        let chain = LayerChain::new()
            .with_layer(Arc::new(MockLayer::new(
                "first",
                0,
                Some(make_result(0.95)),
            )))
            .with_layer(Arc::new(MockLayer::new(
                "second",
                1,
                Some(make_result(0.8)),
            )));

        let ctx = MatchingContext::simple("test");
        let result = chain.execute(&ctx).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap().confidence, 0.95);
    }

    #[tokio::test]
    async fn test_layer_chain_skips_disabled() {
        let chain = LayerChain::new()
            .with_layer(Arc::new(MockLayer::new(
                "disabled",
                0,
                Some(make_result(1.0)),
            )))
            .with_layer(Arc::new(MockLayer::new(
                "enabled",
                1,
                Some(make_result(0.9)),
            )));

        // Disable first layer
        chain.set_layer_enabled("disabled", false);

        let ctx = MatchingContext::simple("test");
        let result = chain.execute(&ctx).await;

        assert!(result.is_some());
        assert_eq!(result.unwrap().confidence, 0.9);
    }

    #[tokio::test]
    async fn test_layer_chain_best_result() {
        let chain = LayerChain::new()
            .with_layer(Arc::new(MockLayer::new(
                "low",
                0,
                Some(make_result(0.5)),
            )))
            .with_layer(Arc::new(MockLayer::new(
                "high",
                1,
                Some(make_result(0.95)),
            )));

        let ctx = MatchingContext::simple("test");
        let result = chain.execute(&ctx).await;

        // High confidence terminal match should win
        assert!(result.is_some());
        assert_eq!(result.unwrap().confidence, 0.95);
    }

    #[tokio::test]
    async fn test_has_layer() {
        let chain = LayerChain::new()
            .with_layer(Arc::new(MockLayer::new("test", 0, None)));

        assert!(chain.has_layer("test"));
        assert!(!chain.has_layer("nonexistent"));
    }
}

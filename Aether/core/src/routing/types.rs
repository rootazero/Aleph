//! Unified Routing Types
//!
//! Core data structures for the unified routing framework.

use crate::dispatcher::UnifiedTool;
use crate::payload::Intent;
use crate::semantic::ConversationContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

// =============================================================================
// Routing Layer Type
// =============================================================================

/// Identifies which routing layer produced a match
///
/// Used for:
/// - Determining confidence thresholds
/// - Metrics and debugging
/// - Confirmation decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum RoutingLayerType {
    /// L1: Regex pattern match
    ///
    /// - Latency: <10ms
    /// - Confidence: 1.0 (explicit match)
    /// - Use case: Explicit slash commands, configured patterns
    L1Regex,

    /// L2: Semantic/keyword matching
    ///
    /// - Latency: 200-500ms
    /// - Confidence: 0.5-0.9 based on keyword overlap
    /// - Use case: Natural language with intent keywords
    L2Semantic,

    /// L3: AI inference routing
    ///
    /// - Latency: >1s
    /// - Confidence: 0.3-0.9 from model output
    /// - Use case: Complex queries, pronoun resolution
    L3Inference,

    /// Default fallback to general chat
    ///
    /// - Latency: 0ms
    /// - Confidence: 0.0
    /// - Use case: No tool matched
    #[default]
    Default,
}

impl RoutingLayerType {
    /// Get the typical latency range for this layer
    pub fn latency_hint(&self) -> &'static str {
        match self {
            Self::L1Regex => "<10ms",
            Self::L2Semantic => "200-500ms",
            Self::L3Inference => ">1s",
            Self::Default => "0ms",
        }
    }

    /// Get the default confidence for this layer
    pub fn default_confidence(&self) -> f32 {
        match self {
            Self::L1Regex => 1.0,
            Self::L2Semantic => 0.7,
            Self::L3Inference => 0.5,
            Self::Default => 0.0,
        }
    }

    /// Get the minimum confidence threshold for this layer
    pub fn min_confidence(&self) -> f32 {
        match self {
            Self::L1Regex => 0.9,     // Must be very confident
            Self::L2Semantic => 0.5,  // Medium threshold
            Self::L3Inference => 0.3, // Lower threshold (AI can reason)
            Self::Default => 0.0,     // Always matches
        }
    }

    /// Convert from dispatcher RoutingLayer
    pub fn from_dispatcher_layer(layer: crate::dispatcher::RoutingLayer) -> Self {
        match layer {
            crate::dispatcher::RoutingLayer::L1Rule => Self::L1Regex,
            crate::dispatcher::RoutingLayer::L2Semantic => Self::L2Semantic,
            crate::dispatcher::RoutingLayer::L3Inference => Self::L3Inference,
            crate::dispatcher::RoutingLayer::Default => Self::Default,
        }
    }

    /// Convert to dispatcher RoutingLayer
    pub fn to_dispatcher_layer(&self) -> crate::dispatcher::RoutingLayer {
        match self {
            Self::L1Regex => crate::dispatcher::RoutingLayer::L1Rule,
            Self::L2Semantic => crate::dispatcher::RoutingLayer::L2Semantic,
            Self::L3Inference => crate::dispatcher::RoutingLayer::L3Inference,
            Self::Default => crate::dispatcher::RoutingLayer::Default,
        }
    }
}

// =============================================================================
// Routing Match
// =============================================================================

/// A successful routing match from any layer
#[derive(Debug, Clone)]
pub struct RoutingMatch {
    /// Matched tool
    pub tool: UnifiedTool,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,

    /// Which layer produced this match
    pub layer: RoutingLayerType,

    /// Extracted parameters (if any)
    pub parameters: Option<Value>,

    /// Human-readable reason for the match
    pub reason: Option<String>,

    /// Detected intent (if applicable)
    pub intent: Option<Intent>,
}

impl RoutingMatch {
    /// Create a new routing match
    pub fn new(tool: UnifiedTool, confidence: f32, layer: RoutingLayerType) -> Self {
        Self {
            tool,
            confidence,
            layer,
            parameters: None,
            reason: None,
            intent: None,
        }
    }

    /// Builder: set parameters
    pub fn with_parameters(mut self, params: Value) -> Self {
        self.parameters = Some(params);
        self
    }

    /// Builder: set reason
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Builder: set intent
    pub fn with_intent(mut self, intent: Intent) -> Self {
        self.intent = Some(intent);
        self
    }

    /// Check if this match exceeds the confidence threshold for its layer
    pub fn meets_threshold(&self) -> bool {
        self.confidence >= self.layer.min_confidence()
    }
}

// =============================================================================
// Routing Result
// =============================================================================

/// Result of routing operation
#[derive(Debug, Clone)]
pub enum RoutingResult {
    /// A tool was matched
    Matched(RoutingMatch),

    /// No tool matched - fall back to general chat
    NoMatch {
        /// Reason for no match
        reason: String,
        /// Input that was routed
        input: String,
    },

    /// Routing was skipped (e.g., disabled)
    Skipped {
        /// Reason for skipping
        reason: String,
    },
}

impl RoutingResult {
    /// Create a no-match result
    pub fn no_match(input: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::NoMatch {
            reason: reason.into(),
            input: input.into(),
        }
    }

    /// Create a skipped result
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::Skipped {
            reason: reason.into(),
        }
    }

    /// Check if routing was successful
    pub fn is_matched(&self) -> bool {
        matches!(self, Self::Matched(_))
    }

    /// Get the match if successful
    pub fn get_match(&self) -> Option<&RoutingMatch> {
        match self {
            Self::Matched(m) => Some(m),
            _ => None,
        }
    }

    /// Get the routing layer (Default if not matched)
    pub fn layer(&self) -> RoutingLayerType {
        match self {
            Self::Matched(m) => m.layer,
            _ => RoutingLayerType::Default,
        }
    }

    /// Get confidence (0.0 if not matched)
    pub fn confidence(&self) -> f32 {
        match self {
            Self::Matched(m) => m.confidence,
            _ => 0.0,
        }
    }
}

// =============================================================================
// Routing Context
// =============================================================================

/// Context for routing operations
#[derive(Debug, Clone, Default)]
pub struct RoutingContext {
    /// Raw user input
    pub input: String,

    /// Conversation history for context-aware routing
    pub conversation: Option<ConversationContext>,

    /// Entity hints extracted from conversation
    pub entity_hints: Vec<String>,

    /// Application context (bundle ID, window title)
    pub app_context: Option<AppContextInfo>,

    /// Whether to skip L3 inference (for speed)
    pub skip_l3: bool,

    /// Custom timeout for L3 routing
    pub l3_timeout: Option<Duration>,
}

/// Application context information
#[derive(Debug, Clone)]
pub struct AppContextInfo {
    /// Application bundle identifier
    pub bundle_id: Option<String>,
    /// Window title
    pub window_title: Option<String>,
}

impl RoutingContext {
    /// Create a new routing context with input
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            ..Default::default()
        }
    }

    /// Builder: set conversation context
    pub fn with_conversation(mut self, conversation: ConversationContext) -> Self {
        self.entity_hints = conversation.extract_entity_hints();
        self.conversation = Some(conversation);
        self
    }

    /// Builder: set app context
    pub fn with_app(mut self, bundle_id: Option<String>, window_title: Option<String>) -> Self {
        self.app_context = Some(AppContextInfo {
            bundle_id,
            window_title,
        });
        self
    }

    /// Builder: skip L3 inference
    pub fn skip_l3_inference(mut self) -> Self {
        self.skip_l3 = true;
        self
    }

    /// Builder: set L3 timeout
    pub fn with_l3_timeout(mut self, timeout: Duration) -> Self {
        self.l3_timeout = Some(timeout);
        self
    }
}

// =============================================================================
// Routing Config
// =============================================================================

/// Configuration for the unified router
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Whether unified routing is enabled
    pub enabled: bool,

    /// Enable L1 regex matching
    pub l1_enabled: bool,

    /// Enable L2 semantic matching
    pub l2_enabled: bool,

    /// Enable L3 AI inference
    pub l3_enabled: bool,

    /// L3 inference timeout in milliseconds
    pub l3_timeout_ms: u64,

    /// Minimum confidence threshold for L2 matches
    pub l2_min_confidence: f32,

    /// Minimum confidence threshold for L3 matches
    pub l3_min_confidence: f32,

    /// Whether to use minimal L3 prompts (faster but less accurate)
    pub l3_minimal_prompts: bool,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            l1_enabled: true,
            l2_enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l2_min_confidence: 0.5,
            l3_min_confidence: 0.3,
            l3_minimal_prompts: false,
        }
    }
}

impl RoutingConfig {
    /// Create a fast config (L3 disabled)
    pub fn fast() -> Self {
        Self {
            l3_enabled: false,
            ..Default::default()
        }
    }

    /// Create a full config with all layers
    pub fn full() -> Self {
        Self::default()
    }

    /// Create a minimal config (L1 only)
    pub fn minimal() -> Self {
        Self {
            l1_enabled: true,
            l2_enabled: false,
            l3_enabled: false,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;

    #[test]
    fn test_layer_confidence() {
        assert_eq!(RoutingLayerType::L1Regex.default_confidence(), 1.0);
        assert_eq!(RoutingLayerType::L2Semantic.default_confidence(), 0.7);
        assert_eq!(RoutingLayerType::L3Inference.default_confidence(), 0.5);
        assert_eq!(RoutingLayerType::Default.default_confidence(), 0.0);
    }

    #[test]
    fn test_routing_match_threshold() {
        let tool = UnifiedTool::new("test", "test", "Test tool", ToolSource::Native);

        // L1 with 1.0 confidence should meet threshold
        let match1 = RoutingMatch::new(tool.clone(), 1.0, RoutingLayerType::L1Regex);
        assert!(match1.meets_threshold());

        // L1 with 0.5 confidence should NOT meet threshold
        let match2 = RoutingMatch::new(tool.clone(), 0.5, RoutingLayerType::L1Regex);
        assert!(!match2.meets_threshold());

        // L3 with 0.5 confidence should meet threshold
        let match3 = RoutingMatch::new(tool, 0.5, RoutingLayerType::L3Inference);
        assert!(match3.meets_threshold());
    }

    #[test]
    fn test_routing_config_presets() {
        let fast = RoutingConfig::fast();
        assert!(!fast.l3_enabled);
        assert!(fast.l1_enabled);
        assert!(fast.l2_enabled);

        let minimal = RoutingConfig::minimal();
        assert!(minimal.l1_enabled);
        assert!(!minimal.l2_enabled);
        assert!(!minimal.l3_enabled);
    }
}

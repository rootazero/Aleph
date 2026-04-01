//! Routing Layer
//!
//! Routing layer indicator for tracking which layer produced a match.

use serde::{Deserialize, Serialize};

/// Routing layer indicator
///
/// Tracks which routing layer produced a match, useful for
/// debugging, metrics, and determining confidence levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoutingLayer {
    /// L1: Regex pattern match (fastest, <10ms)
    /// Highest confidence (1.0) for explicit slash commands.
    L1Rule,

    /// L2: Semantic/keyword matching (200-500ms)
    /// Medium confidence based on keyword overlap.
    L2Semantic,

    /// L3: LLM-based inference (>1s)
    /// Variable confidence from model output.
    L3Inference,

    /// Default provider fallback
    /// Used when no layer matches.
    #[default]
    Default,
}

impl RoutingLayer {
    /// Get the typical latency range for this layer
    pub fn latency_hint(&self) -> &'static str {
        match self {
            RoutingLayer::L1Rule => "<10ms",
            RoutingLayer::L2Semantic => "200-500ms",
            RoutingLayer::L3Inference => ">1s",
            RoutingLayer::Default => "0ms",
        }
    }

    /// Get the default confidence for this layer
    pub fn default_confidence(&self) -> f32 {
        match self {
            RoutingLayer::L1Rule => 1.0,
            RoutingLayer::L2Semantic => 0.7,
            RoutingLayer::L3Inference => 0.5,
            RoutingLayer::Default => 0.0,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_layer_defaults() {
        assert_eq!(RoutingLayer::L1Rule.default_confidence(), 1.0);
        assert_eq!(RoutingLayer::L2Semantic.default_confidence(), 0.7);
        assert_eq!(RoutingLayer::L3Inference.default_confidence(), 0.5);
        assert_eq!(RoutingLayer::Default.default_confidence(), 0.0);
    }

    #[test]
    fn test_routing_layer_latency_hints() {
        assert_eq!(RoutingLayer::L1Rule.latency_hint(), "<10ms");
        assert_eq!(RoutingLayer::L2Semantic.latency_hint(), "200-500ms");
        assert_eq!(RoutingLayer::L3Inference.latency_hint(), ">1s");
        assert_eq!(RoutingLayer::Default.latency_hint(), "0ms");
    }

    #[test]
    fn test_routing_layer_default() {
        let default = RoutingLayer::default();
        assert_eq!(default, RoutingLayer::Default);
    }
}

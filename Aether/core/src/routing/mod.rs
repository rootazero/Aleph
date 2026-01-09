//! Unified Routing Framework
//!
//! This module provides a unified multi-layer routing system that coordinates:
//!
//! - **L1 (Regex)**: Fast pattern matching (<10ms, confidence 1.0)
//! - **L2 (Semantic)**: Keyword and context matching (200-500ms, confidence 0.7)
//! - **L3 (Inference)**: AI-powered routing (>1s, confidence varies)
//! - **Default**: Fallback to general chat
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────┐
//! │           UnifiedRouter                  │
//! │                                          │
//! │  ┌────────────────────────────────────┐ │
//! │  │ L1: Regex Layer                    │ │
//! │  │ - Explicit slash commands          │ │
//! │  │ - Config-based patterns            │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓ (no match)              │
//! │  ┌────────────────────────────────────┐ │
//! │  │ L2: Semantic Layer                 │ │
//! │  │ - Keyword matching                 │ │
//! │  │ - Context inference                │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓ (no match)              │
//! │  ┌────────────────────────────────────┐ │
//! │  │ L3: AI Inference Layer             │ │
//! │  │ - LLM-based tool selection         │ │
//! │  │ - Parameter extraction             │ │
//! │  └────────────┬───────────────────────┘ │
//! │               ↓ (no match)              │
//! │  ┌────────────────────────────────────┐ │
//! │  │ Default: General Chat              │ │
//! │  └────────────────────────────────────┘ │
//! └──────────────────────────────────────────┘
//!      ↓
//! RoutingResult { tool, confidence, layer, params }
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::routing::{UnifiedRouter, RoutingConfig};
//!
//! let config = RoutingConfig::default();
//! let router = UnifiedRouter::new(config, provider, semantic_matcher);
//!
//! let result = router.route(input, &tools, context).await?;
//!
//! match result.layer {
//!     RoutingLayerType::L1Regex => { /* High confidence match */ }
//!     RoutingLayerType::L2Semantic => { /* Medium confidence */ }
//!     RoutingLayerType::L3Inference => { /* May need confirmation */ }
//!     RoutingLayerType::Default => { /* Fall back to chat */ }
//! }
//! ```

mod types;
mod unified;

pub use types::{
    RoutingConfig, RoutingContext, RoutingLayerType, RoutingMatch, RoutingResult,
};
pub use unified::UnifiedRouter;

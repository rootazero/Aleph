//! Meta-cognition integration for Agent Loop (STUB)
//!
//! The POE module has been removed. This file retains stub types
//! so that existing re-exports from `agent_loop::mod.rs` continue to compile.
//! This entire module will be removed in a later cleanup task.

/// Configuration for meta-cognition integration
#[derive(Debug, Clone)]
pub struct MetaCognitionConfig {
    /// Whether meta-cognition is enabled
    pub enabled: bool,

    /// Cache size for anchor retrieval (LRU)
    pub cache_size: usize,

    /// Minimum confidence threshold for anchor injection (0.0-1.0)
    pub min_confidence: f32,

    /// Maximum number of anchors to inject per request
    pub max_anchors_per_request: usize,

    /// Whether to automatically retry tasks after reflection
    pub auto_retry_after_reflection: bool,
}

impl Default for MetaCognitionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cache_size: 100,
            min_confidence: 0.5,
            max_anchors_per_request: 5,
            auto_retry_after_reflection: false,
        }
    }
}

/// Stub for MetaCognitionIntegration (POE module removed)
pub struct MetaCognitionIntegration {
    config: MetaCognitionConfig,
}

impl MetaCognitionIntegration {
    /// Check if meta-cognition is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

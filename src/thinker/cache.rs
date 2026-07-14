//! The Anthropic `cache_control` wire marker.
//!
//! That is all this module is now. It used to be a ~650-line "provider cache
//! strategy" framework — a `CacheStrategy` trait with `AnthropicCacheStrategy`
//! and `GeminiCacheStrategy` impls, a `CacheContext`, `CacheableContentBlock`,
//! the Gemini explicit-cache create request/response types, and a `ProviderType`
//! enum. Every one of those had zero consumers outside this file.
//!
//! It was a design for caching Aleph does not do. The caching Aleph actually
//! does is PREFIX caching, and it is implemented where it belongs: the Anthropic
//! adapter places `cache_control` breakpoints at the stable/dynamic system-prompt
//! boundary and on recent messages (`providers/protocols/anthropic/adapter.rs`),
//! and the OpenAI adapters get affinity via `prompt_cache_key`. The only thing
//! this framework contributed to that real path is the marker below.
//!
//! `ProviderType::from_model_name` is worth naming as it goes: identifying a
//! provider by substring-matching "claude" inside a model id is exactly the
//! keyword-matching-instead-of-declared-metadata shape R7/P8 forbid, and it had
//! drifted out of sync with the capability tables it shadowed.

use serde::{Deserialize, Serialize};

/// Cache control marker for Anthropic's ephemeral caching.
///
/// Attached to a content block to mean "cache the prefix up to and including
/// this block". Consumed by `providers/anthropic/types.rs` and the Anthropic
/// protocol adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// Cache type — always "ephemeral" for Anthropic.
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl Default for CacheControl {
    fn default() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

impl CacheControl {
    /// Create an ephemeral cache control marker.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ephemeral_serializes_to_the_wire_shape_anthropic_expects() {
        let json = serde_json::to_value(CacheControl::ephemeral()).expect("serialize");
        assert_eq!(json, serde_json::json!({"type": "ephemeral"}));
    }
}

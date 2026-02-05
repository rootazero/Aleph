//! Provider-side Context Caching Strategy
//!
//! This module provides the abstraction layer for provider-specific context caching.
//! Different providers have fundamentally different caching mechanisms:
//!
//! - **Anthropic**: Ephemeral caching via `cache_control` blocks (stateless)
//! - **Gemini**: Persistent caching via explicit cache API (stateful)
//! - **OpenAI**: Transparent automatic caching (no state needed)
//!
//! # Architecture
//!
//! ```text
//! Workspace (cache_state: CacheState)
//!          ↓
//!     ┌────────────────────────────────────────┐
//!     │        ProviderCacheStrategy           │
//!     │                                        │
//!     │  ┌──────────────┐ ┌──────────────────┐ │
//!     │  │ Anthropic    │ │ Gemini           │ │
//!     │  │ (Ephemeral)  │ │ (Persistent)     │ │
//!     │  │              │ │                  │ │
//!     │  │ Inject       │ │ Create/Manage    │ │
//!     │  │ cache_control│ │ cachedContent    │ │
//!     │  └──────────────┘ └──────────────────┘ │
//!     └────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::thinker::cache::{CacheContext, CacheMarker};
//!
//! // Build cache context from workspace
//! let ctx = CacheContext::from_workspace(&workspace, &messages);
//!
//! // Get cache markers for Anthropic
//! let markers = ctx.get_anthropic_markers();
//!
//! // Check if cache should be created for Gemini
//! if ctx.should_create_gemini_cache() {
//!     // Create cache via Gemini API
//! }
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Minimum token threshold for caching (Anthropic requires >= 1024)
pub const MIN_CACHE_TOKENS: u64 = 1024;

/// Default cache breakpoint position (after system prompt)
pub const DEFAULT_BREAKPOINT: usize = 0;

/// Gemini cache TTL in seconds (1 hour default)
pub const GEMINI_CACHE_TTL_SECS: u64 = 3600;

// =============================================================================
// CacheContext
// =============================================================================

/// Context for cache decision making
///
/// Contains all information needed to decide whether to cache
/// and where to place cache breakpoints.
#[derive(Debug, Clone)]
pub struct CacheContext {
    /// Estimated token count of cacheable content
    pub cacheable_tokens: u64,

    /// Content hash for cache invalidation
    pub content_hash: String,

    /// Provider type for strategy selection
    pub provider_type: ProviderType,

    /// Whether caching is enabled
    pub enabled: bool,

    /// Cache breakpoint index (for Anthropic)
    pub breakpoint_index: Option<usize>,

    /// Existing cache name (for Gemini)
    pub existing_cache_name: Option<String>,

    /// Cache expiry time (for Gemini)
    pub cache_expires_at: Option<DateTime<Utc>>,
}

impl Default for CacheContext {
    fn default() -> Self {
        Self {
            cacheable_tokens: 0,
            content_hash: String::new(),
            provider_type: ProviderType::Unknown,
            enabled: false,
            breakpoint_index: None,
            existing_cache_name: None,
            cache_expires_at: None,
        }
    }
}

impl CacheContext {
    /// Create context from system prompt and messages
    pub fn new(
        system_prompt: &str,
        _message_count: usize,
        provider_type: ProviderType,
    ) -> Self {
        // Estimate tokens (rough: 4 chars per token)
        let estimated_tokens = (system_prompt.len() / 4) as u64;

        // Calculate content hash
        let mut hasher = Sha256::new();
        hasher.update(system_prompt.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        Self {
            cacheable_tokens: estimated_tokens,
            content_hash: hash,
            provider_type,
            enabled: estimated_tokens >= MIN_CACHE_TOKENS,
            breakpoint_index: Some(DEFAULT_BREAKPOINT),
            existing_cache_name: None,
            cache_expires_at: None,
        }
    }

    /// Check if the context meets caching threshold
    pub fn should_cache(&self) -> bool {
        self.enabled && self.cacheable_tokens >= MIN_CACHE_TOKENS
    }

    /// Check if existing cache is still valid
    pub fn has_valid_cache(&self) -> bool {
        if let Some(expires_at) = self.cache_expires_at {
            expires_at > Utc::now() && self.existing_cache_name.is_some()
        } else {
            false
        }
    }

    /// Update with existing cache state
    pub fn with_existing_cache(
        mut self,
        cache_name: Option<String>,
        content_hash: Option<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        self.existing_cache_name = cache_name;
        self.cache_expires_at = expires_at;

        // Check if content hash matches (cache still valid)
        if let Some(existing_hash) = content_hash {
            if existing_hash != self.content_hash {
                // Content changed, invalidate cache
                self.existing_cache_name = None;
                self.cache_expires_at = None;
            }
        }

        self
    }
}

// =============================================================================
// ProviderType
// =============================================================================

/// Provider type for cache strategy selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderType {
    /// Anthropic (Claude) - uses ephemeral cache_control
    Anthropic,
    /// Google Gemini - uses persistent cachedContent
    Gemini,
    /// OpenAI - transparent caching, no action needed
    OpenAi,
    /// Other/unknown provider
    #[default]
    Unknown,
}

impl ProviderType {
    /// Detect provider type from model name
    pub fn from_model_name(model: &str) -> Self {
        let model_lower = model.to_lowercase();

        if model_lower.contains("claude")
            || model_lower.contains("anthropic")
            || model_lower.starts_with("claude-")
        {
            Self::Anthropic
        } else if model_lower.contains("gemini")
            || model_lower.contains("google")
            || model_lower.starts_with("gemini-")
        {
            Self::Gemini
        } else if model_lower.contains("gpt")
            || model_lower.contains("openai")
            || model_lower.starts_with("gpt-")
        {
            Self::OpenAi
        } else {
            Self::Unknown
        }
    }
}

// =============================================================================
// CacheMarker (Anthropic)
// =============================================================================

/// Cache control marker for Anthropic's ephemeral caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// Cache type - always "ephemeral" for Anthropic
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
    /// Create an ephemeral cache control marker
    pub fn ephemeral() -> Self {
        Self::default()
    }
}

/// Content block with optional cache control (for Anthropic system prompt)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheableContentBlock {
    /// Block type - "text" for system prompt parts
    #[serde(rename = "type")]
    pub block_type: String,

    /// Text content
    pub text: String,

    /// Optional cache control marker
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl CacheableContentBlock {
    /// Create a text block
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            block_type: "text".to_string(),
            text: content.into(),
            cache_control: None,
        }
    }

    /// Create a cached text block
    pub fn cached_text(content: impl Into<String>) -> Self {
        Self {
            block_type: "text".to_string(),
            text: content.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }
    }

    /// Mark this block as cacheable
    pub fn with_cache(mut self) -> Self {
        self.cache_control = Some(CacheControl::ephemeral());
        self
    }
}

// =============================================================================
// GeminiCacheRequest (Gemini)
// =============================================================================

/// Request to create a Gemini cached content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiCacheCreateRequest {
    /// Model to use with this cache
    pub model: String,

    /// Display name for the cache
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    /// Contents to cache (messages)
    pub contents: Vec<GeminiContent>,

    /// System instruction to cache
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,

    /// Time-to-live (e.g., "3600s")
    pub ttl: String,
}

/// Gemini content structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiContent {
    /// Role: "user" or "model"
    pub role: String,

    /// Parts of the content
    pub parts: Vec<GeminiPart>,
}

/// Gemini content part
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiPart {
    /// Text content
    pub text: String,
}

/// Response from Gemini cache creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiCacheCreateResponse {
    /// Cache name (e.g., "cachedContents/abc123")
    pub name: String,

    /// Model name
    pub model: String,

    /// Creation time
    #[serde(rename = "createTime")]
    pub create_time: String,

    /// Update time
    #[serde(rename = "updateTime")]
    pub update_time: String,

    /// Expiration time
    #[serde(rename = "expireTime")]
    pub expire_time: String,
}

// =============================================================================
// CacheStrategy trait
// =============================================================================

/// Strategy for provider-specific caching operations
pub trait CacheStrategy: Send + Sync {
    /// Get the provider type
    fn provider_type(&self) -> ProviderType;

    /// Check if caching should be applied
    fn should_cache(&self, context: &CacheContext) -> bool;

    /// Prepare system prompt with cache markers (Anthropic)
    fn prepare_system_prompt(&self, prompt: &str, context: &CacheContext) -> SystemPromptCache;

    /// Get cache name to reference (Gemini)
    fn get_cache_reference(&self, context: &CacheContext) -> Option<String>;
}

/// Result of preparing system prompt for caching
#[derive(Debug, Clone)]
pub enum SystemPromptCache {
    /// Plain text (no caching)
    Plain(String),

    /// Anthropic-style blocks with cache control
    AnthropicBlocks(Vec<CacheableContentBlock>),

    /// Gemini cache reference
    GeminiReference { cache_name: String },
}

// =============================================================================
// AnthropicCacheStrategy
// =============================================================================

/// Anthropic ephemeral caching strategy
#[derive(Debug, Clone, Default)]
pub struct AnthropicCacheStrategy;

impl CacheStrategy for AnthropicCacheStrategy {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Anthropic
    }

    fn should_cache(&self, context: &CacheContext) -> bool {
        context.should_cache() && context.provider_type == ProviderType::Anthropic
    }

    fn prepare_system_prompt(&self, prompt: &str, context: &CacheContext) -> SystemPromptCache {
        if !self.should_cache(context) {
            return SystemPromptCache::Plain(prompt.to_string());
        }

        // Split prompt into cacheable and dynamic parts
        // For simplicity, we cache the entire system prompt as one block
        let blocks = vec![CacheableContentBlock::cached_text(prompt)];

        SystemPromptCache::AnthropicBlocks(blocks)
    }

    fn get_cache_reference(&self, _context: &CacheContext) -> Option<String> {
        None // Anthropic doesn't use cache references
    }
}

// =============================================================================
// GeminiCacheStrategy
// =============================================================================

/// Gemini persistent caching strategy
#[derive(Debug, Clone, Default)]
pub struct GeminiCacheStrategy;

impl CacheStrategy for GeminiCacheStrategy {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Gemini
    }

    fn should_cache(&self, context: &CacheContext) -> bool {
        context.should_cache() && context.provider_type == ProviderType::Gemini
    }

    fn prepare_system_prompt(&self, prompt: &str, context: &CacheContext) -> SystemPromptCache {
        // For Gemini, check if we have a valid cache reference
        if let Some(cache_name) = self.get_cache_reference(context) {
            return SystemPromptCache::GeminiReference { cache_name };
        }

        // Otherwise return plain (caller should create cache)
        SystemPromptCache::Plain(prompt.to_string())
    }

    fn get_cache_reference(&self, context: &CacheContext) -> Option<String> {
        if context.has_valid_cache() {
            context.existing_cache_name.clone()
        } else {
            None
        }
    }
}

impl GeminiCacheStrategy {
    /// Check if a new cache should be created
    pub fn should_create_cache(&self, context: &CacheContext) -> bool {
        self.should_cache(context) && !context.has_valid_cache()
    }

    /// Build a cache creation request
    pub fn build_cache_request(
        &self,
        model: &str,
        system_prompt: &str,
        workspace_id: &str,
    ) -> GeminiCacheCreateRequest {
        GeminiCacheCreateRequest {
            model: model.to_string(),
            display_name: Some(format!("workspace-{}", workspace_id)),
            contents: vec![],
            system_instruction: Some(GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart {
                    text: system_prompt.to_string(),
                }],
            }),
            ttl: format!("{}s", GEMINI_CACHE_TTL_SECS),
        }
    }
}

// =============================================================================
// TransparentCacheStrategy
// =============================================================================

/// OpenAI transparent caching strategy (no-op)
#[derive(Debug, Clone, Default)]
pub struct TransparentCacheStrategy;

impl CacheStrategy for TransparentCacheStrategy {
    fn provider_type(&self) -> ProviderType {
        ProviderType::OpenAi
    }

    fn should_cache(&self, _context: &CacheContext) -> bool {
        false // OpenAI handles caching automatically
    }

    fn prepare_system_prompt(&self, prompt: &str, _context: &CacheContext) -> SystemPromptCache {
        SystemPromptCache::Plain(prompt.to_string())
    }

    fn get_cache_reference(&self, _context: &CacheContext) -> Option<String> {
        None
    }
}

// =============================================================================
// Factory function
// =============================================================================

/// Get the appropriate cache strategy for a provider type
pub fn get_cache_strategy(provider_type: ProviderType) -> Box<dyn CacheStrategy> {
    match provider_type {
        ProviderType::Anthropic => Box::new(AnthropicCacheStrategy),
        ProviderType::Gemini => Box::new(GeminiCacheStrategy),
        ProviderType::OpenAi => Box::new(TransparentCacheStrategy),
        ProviderType::Unknown => Box::new(TransparentCacheStrategy),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_detection() {
        assert_eq!(
            ProviderType::from_model_name("claude-3-5-sonnet"),
            ProviderType::Anthropic
        );
        assert_eq!(
            ProviderType::from_model_name("gemini-1.5-pro"),
            ProviderType::Gemini
        );
        assert_eq!(
            ProviderType::from_model_name("gpt-4o"),
            ProviderType::OpenAi
        );
        assert_eq!(
            ProviderType::from_model_name("unknown-model"),
            ProviderType::Unknown
        );
    }

    #[test]
    fn test_cache_context_creation() {
        let prompt = "A".repeat(5000); // ~1250 tokens
        let ctx = CacheContext::new(&prompt, 10, ProviderType::Anthropic);

        assert!(ctx.should_cache());
        assert!(ctx.cacheable_tokens >= MIN_CACHE_TOKENS);
        assert!(!ctx.content_hash.is_empty());
    }

    #[test]
    fn test_cache_context_below_threshold() {
        let prompt = "Short prompt";
        let ctx = CacheContext::new(prompt, 5, ProviderType::Anthropic);

        assert!(!ctx.should_cache());
    }

    #[test]
    fn test_anthropic_strategy() {
        let strategy = AnthropicCacheStrategy;
        let prompt = "A".repeat(5000);
        let ctx = CacheContext::new(&prompt, 10, ProviderType::Anthropic);

        assert!(strategy.should_cache(&ctx));

        match strategy.prepare_system_prompt(&prompt, &ctx) {
            SystemPromptCache::AnthropicBlocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(blocks[0].cache_control.is_some());
            }
            _ => panic!("Expected AnthropicBlocks"),
        }
    }

    #[test]
    fn test_gemini_strategy_no_cache() {
        let strategy = GeminiCacheStrategy;
        let prompt = "A".repeat(5000);
        let ctx = CacheContext::new(&prompt, 10, ProviderType::Gemini);

        // No existing cache, should return plain
        match strategy.prepare_system_prompt(&prompt, &ctx) {
            SystemPromptCache::Plain(_) => {}
            _ => panic!("Expected Plain without existing cache"),
        }

        assert!(strategy.should_create_cache(&ctx));
    }

    #[test]
    fn test_gemini_strategy_with_cache() {
        let strategy = GeminiCacheStrategy;
        let prompt = "A".repeat(5000);
        let ctx = CacheContext::new(&prompt, 10, ProviderType::Gemini).with_existing_cache(
            Some("cachedContents/abc123".to_string()),
            Some(CacheContext::new(&prompt, 10, ProviderType::Gemini).content_hash),
            Some(Utc::now() + chrono::Duration::hours(1)),
        );

        assert!(!strategy.should_create_cache(&ctx));

        match strategy.prepare_system_prompt(&prompt, &ctx) {
            SystemPromptCache::GeminiReference { cache_name } => {
                assert_eq!(cache_name, "cachedContents/abc123");
            }
            _ => panic!("Expected GeminiReference with existing cache"),
        }
    }

    #[test]
    fn test_content_hash_invalidation() {
        let prompt1 = "Original prompt content";
        let prompt2 = "Modified prompt content";

        let ctx1 = CacheContext::new(prompt1, 10, ProviderType::Gemini);
        let ctx2 = CacheContext::new(prompt2, 10, ProviderType::Gemini);

        // Create context with old hash
        let ctx_with_old_cache = ctx2.clone().with_existing_cache(
            Some("cachedContents/old".to_string()),
            Some(ctx1.content_hash.clone()),
            Some(Utc::now() + chrono::Duration::hours(1)),
        );

        // Cache should be invalidated because content changed
        assert!(!ctx_with_old_cache.has_valid_cache());
        assert!(ctx_with_old_cache.existing_cache_name.is_none());
    }

    #[test]
    fn test_cache_control_serialization() {
        let block = CacheableContentBlock::cached_text("Test content");
        let json = serde_json::to_string(&block).unwrap();

        assert!(json.contains("cache_control"));
        assert!(json.contains("ephemeral"));
    }
}

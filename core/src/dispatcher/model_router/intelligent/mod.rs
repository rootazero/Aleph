//! Intelligent routing (P2).
//!
//! This module provides smart routing capabilities:
//! - Prompt analysis for feature extraction
//! - Semantic caching for response reuse
//! - Intelligent routing with health and metrics awareness
//! - P2 router integrating all P2 features

pub mod p2_router;
pub mod prompt_analyzer;
pub mod routing;
pub mod semantic_cache;

pub use p2_router::{
    P2IntelligentRouter, P2RouterConfig, P2RouterError, PreRouteResult, RoutingDecision,
};
pub use prompt_analyzer::{
    ComplexityWeights, ContextSize, Domain, Language, PromptAnalysisError, PromptAnalyzer,
    PromptAnalyzerConfig, PromptFeatures, ReasoningLevel, TechnicalDomain,
};
pub use routing::{IntelligentRouter, IntelligentRoutingConfig, IntelligentRoutingResult};
pub use semantic_cache::{
    CacheEntry, CacheHit, CacheHitType, CacheMetadata, CacheStats, CachedResponse, EmbeddingError,
    EvictionPolicy, FastEmbedEmbedder, InMemoryVectorStore, SemanticCacheConfig,
    SemanticCacheError, SemanticCacheManager, TextEmbedder,
};

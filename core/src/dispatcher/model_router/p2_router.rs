//! P2 Intelligent Router
//!
//! Integrates Prompt Analyzer and Semantic Cache with the model routing system.
//! This module provides the top-level routing entry point that:
//! 1. Checks semantic cache for cached responses
//! 2. Analyzes prompt features for intelligent routing
//! 3. Routes to the optimal model based on features
//! 4. Stores responses in cache
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      P2IntelligentRouter                        │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  1. SemanticCache.lookup(prompt) → Option<CachedResponse>       │
//! │     └─ HIT → return cached response                            │
//! │     └─ MISS → continue                                          │
//! │                                                                  │
//! │  2. PromptAnalyzer.analyze(prompt) → PromptFeatures             │
//! │                                                                  │
//! │  3. ModelMatcher.route_with_features(intent, features)          │
//! │                                                                  │
//! │  4. Execute request (external)                                  │
//! │                                                                  │
//! │  5. SemanticCache.store(prompt, response)                       │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::{P2IntelligentRouter, P2RouterConfig};
//!
//! let router = P2IntelligentRouter::new(config)?;
//!
//! // Pre-route check
//! let pre_route = router.pre_route("What is Rust?", &TaskIntent::GeneralChat).await?;
//!
//! match pre_route {
//!     PreRouteResult::CacheHit(hit) => {
//!         // Use cached response
//!         return Ok(hit.response().content.clone());
//!     }
//!     PreRouteResult::RoutingDecision(decision) => {
//!         // Execute with the selected model
//!         let response = execute_model(&decision.selected_model, prompt).await?;
//!
//!         // Post-route: store in cache
//!         router.post_route(&decision, &response).await?;
//!     }
//! }
//! ```

use std::sync::Arc;

use super::matcher::{ModelMatcher, ModelRouter};
use super::profiles::{Capability, ModelProfile};
use super::prompt_analyzer::{
    Domain, Language, PromptAnalyzer, PromptAnalyzerConfig, PromptFeatures, ReasoningLevel,
};
use super::semantic_cache::{
    CacheHit, CacheMetadata, CachedResponse, SemanticCacheConfig, SemanticCacheManager,
};
use super::TaskIntent;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the P2 Intelligent Router
#[derive(Debug, Clone)]
pub struct P2RouterConfig {
    /// Prompt analyzer configuration
    pub prompt_analysis: PromptAnalyzerConfig,

    /// Semantic cache configuration
    pub semantic_cache: SemanticCacheConfig,

    /// Adjust intent based on prompt features
    pub auto_adjust_intent: bool,

    /// Filter models by context window based on token estimate
    pub filter_by_context: bool,

    /// Prefer models strong in detected language
    pub language_preference: bool,

    /// Upgrade to reasoning-capable models for high reasoning prompts
    pub reasoning_upgrade: bool,
}

impl Default for P2RouterConfig {
    fn default() -> Self {
        Self {
            prompt_analysis: PromptAnalyzerConfig::default(),
            semantic_cache: SemanticCacheConfig::default(),
            auto_adjust_intent: true,
            filter_by_context: true,
            language_preference: true,
            reasoning_upgrade: true,
        }
    }
}

// =============================================================================
// Pre-Route Results
// =============================================================================

/// Result of pre-routing analysis
#[derive(Debug)]
pub enum PreRouteResult {
    /// Cache hit - use the cached response
    CacheHit(CacheHit),

    /// Routing decision with model selection and features
    RoutingDecision(RoutingDecision),
}

impl PreRouteResult {
    /// Check if this is a cache hit
    pub fn is_cache_hit(&self) -> bool {
        matches!(self, PreRouteResult::CacheHit(_))
    }

    /// Get the cache hit if present
    pub fn as_cache_hit(&self) -> Option<&CacheHit> {
        match self {
            PreRouteResult::CacheHit(hit) => Some(hit),
            _ => None,
        }
    }

    /// Get the routing decision if present
    pub fn as_routing_decision(&self) -> Option<&RoutingDecision> {
        match self {
            PreRouteResult::RoutingDecision(decision) => Some(decision),
            _ => None,
        }
    }
}

/// Routing decision with selected model and features
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Original prompt text
    pub prompt: String,

    /// Task intent (possibly adjusted)
    pub intent: TaskIntent,

    /// Original intent before adjustment
    pub original_intent: TaskIntent,

    /// Extracted prompt features
    pub features: PromptFeatures,

    /// Selected model profile
    pub selected_model: ModelProfile,

    /// Reason for model selection
    pub selection_reason: String,

    /// Whether intent was automatically adjusted
    pub intent_adjusted: bool,
}

// =============================================================================
// P2 Intelligent Router
// =============================================================================

/// P2 Intelligent Router with prompt analysis and semantic caching
pub struct P2IntelligentRouter {
    /// Prompt analyzer
    analyzer: PromptAnalyzer,

    /// Semantic cache manager (optional)
    cache: Option<Arc<SemanticCacheManager>>,

    /// Configuration
    config: P2RouterConfig,
}

impl P2IntelligentRouter {
    /// Create a new P2 router with default configuration
    pub fn new(config: P2RouterConfig) -> Result<Self, P2RouterError> {
        let analyzer = PromptAnalyzer::new(config.prompt_analysis.clone());

        let cache = if config.semantic_cache.enabled {
            Some(Arc::new(
                SemanticCacheManager::new(config.semantic_cache.clone())
                    .map_err(|e| P2RouterError::CacheInitFailed(e.to_string()))?,
            ))
        } else {
            None
        };

        Ok(Self {
            analyzer,
            cache,
            config,
        })
    }

    /// Create without cache (for testing or lightweight usage)
    pub fn without_cache(config: P2RouterConfig) -> Self {
        let analyzer = PromptAnalyzer::new(config.prompt_analysis.clone());

        Self {
            analyzer,
            cache: None,
            config,
        }
    }

    /// Check if semantic cache is enabled
    pub fn has_cache(&self) -> bool {
        self.cache.is_some()
    }

    /// Pre-route: check cache and analyze prompt
    ///
    /// Returns either a cache hit or a routing decision with selected model.
    pub async fn pre_route(
        &self,
        prompt: &str,
        intent: &TaskIntent,
        matcher: &ModelMatcher,
    ) -> Result<PreRouteResult, P2RouterError> {
        // Step 1: Check cache
        if let Some(ref cache) = self.cache {
            if let Some(hit) = cache
                .lookup(prompt)
                .await
                .map_err(|e| P2RouterError::CacheLookupFailed(e.to_string()))?
            {
                return Ok(PreRouteResult::CacheHit(hit));
            }
        }

        // Step 2: Analyze prompt
        let features = self.analyzer.analyze(prompt);

        // Step 3: Adjust intent if configured
        let (effective_intent, intent_adjusted) = if self.config.auto_adjust_intent {
            self.adjust_intent(intent, &features)
        } else {
            (intent.clone(), false)
        };

        // Step 4: Route with features
        let (selected_model, selection_reason) =
            self.route_with_features(matcher, &effective_intent, &features)?;

        Ok(PreRouteResult::RoutingDecision(RoutingDecision {
            prompt: prompt.to_string(),
            intent: effective_intent,
            original_intent: intent.clone(),
            features,
            selected_model,
            selection_reason,
            intent_adjusted,
        }))
    }

    /// Post-route: store response in cache
    pub async fn post_route(
        &self,
        decision: &RoutingDecision,
        response: &CachedResponse,
    ) -> Result<(), P2RouterError> {
        if let Some(ref cache) = self.cache {
            let metadata = CacheMetadata {
                task_intent: Some(decision.intent.clone()),
                features_hash: None,
                tags: Vec::new(),
            };

            cache
                .store(
                    &decision.prompt,
                    response,
                    &decision.selected_model.id,
                    None,
                    Some(metadata),
                )
                .await
                .map_err(|e| P2RouterError::CacheStoreFailed(e.to_string()))?;
        }

        Ok(())
    }

    /// Analyze a prompt without routing
    pub fn analyze(&self, prompt: &str) -> PromptFeatures {
        self.analyzer.analyze(prompt)
    }

    /// Get cache statistics
    pub async fn cache_stats(&self) -> Option<super::semantic_cache::CacheStats> {
        match &self.cache {
            Some(cache) => Some(cache.stats().await),
            None => None,
        }
    }

    /// Clear the semantic cache
    pub async fn clear_cache(&self) -> Result<(), P2RouterError> {
        if let Some(ref cache) = self.cache {
            cache
                .clear()
                .await
                .map_err(|e| P2RouterError::CacheStoreFailed(e.to_string()))?;
        }
        Ok(())
    }

    /// Invalidate a specific prompt from cache
    pub async fn invalidate(&self, prompt: &str) -> Result<(), P2RouterError> {
        if let Some(ref cache) = self.cache {
            cache
                .invalidate(prompt)
                .await
                .map_err(|e| P2RouterError::CacheStoreFailed(e.to_string()))?;
        }
        Ok(())
    }

    // =========================================================================
    // Private Methods
    // =========================================================================

    /// Adjust intent based on prompt features
    fn adjust_intent(&self, intent: &TaskIntent, features: &PromptFeatures) -> (TaskIntent, bool) {
        // Only adjust GeneralChat - more specific intents should be preserved
        if *intent != TaskIntent::GeneralChat {
            return (intent.clone(), false);
        }

        // High complexity + high reasoning → Reasoning
        if features.complexity_score > 0.7 && features.reasoning_level == ReasoningLevel::High {
            return (TaskIntent::Reasoning, true);
        }

        // Code-heavy content → CodeGeneration
        if features.code_ratio > 0.5 || features.has_code_blocks {
            return (TaskIntent::CodeGeneration, true);
        }

        // Technical programming domain → CodeGeneration
        if let Domain::Technical(ref tech) = features.domain {
            if matches!(tech, super::prompt_analyzer::TechnicalDomain::Programming) {
                return (TaskIntent::CodeGeneration, true);
            }
        }

        // Creative domain → keep GeneralChat (could add Creative intent later)
        if features.domain == Domain::Creative {
            return (intent.clone(), false);
        }

        (intent.clone(), false)
    }

    /// Route using prompt features
    fn route_with_features(
        &self,
        matcher: &ModelMatcher,
        intent: &TaskIntent,
        features: &PromptFeatures,
    ) -> Result<(ModelProfile, String), P2RouterError> {
        // Get initial candidates from matcher
        let profiles = matcher.profiles();
        let mut candidates: Vec<&ModelProfile> = profiles.iter().collect();
        let mut filter_reasons = Vec::new();

        // Filter 1: Context size
        if self.config.filter_by_context {
            let min_context = features.suggested_context_size.min_tokens();
            let before_count = candidates.len();
            candidates.retain(|p: &&ModelProfile| p.max_context.unwrap_or(4_000) >= min_context);

            if candidates.len() < before_count {
                filter_reasons.push(format!(
                    "Filtered {} models by context size (need >= {})",
                    before_count - candidates.len(),
                    min_context
                ));
            }
        }

        // Filter 2: Required capability from intent
        if let Some(required_cap) = intent.required_capability() {
            let before_count = candidates.len();
            candidates.retain(|p: &&ModelProfile| p.has_capability(required_cap));

            if candidates.len() < before_count {
                filter_reasons.push(format!(
                    "Filtered {} models by capability {:?}",
                    before_count - candidates.len(),
                    required_cap
                ));
            }
        }

        // Filter 3: Reasoning capability for high reasoning prompts
        if self.config.reasoning_upgrade && features.reasoning_level == ReasoningLevel::High {
            let reasoning_capable: Vec<&ModelProfile> = candidates
                .iter()
                .filter(|p: &&&ModelProfile| p.has_capability(Capability::Reasoning))
                .copied()
                .collect();

            if !reasoning_capable.is_empty() {
                candidates = reasoning_capable;
                filter_reasons.push(format!(
                    "Prioritized {} reasoning-capable models",
                    candidates.len()
                ));
            }
        }

        // If no candidates left, fall back to basic routing
        if candidates.is_empty() {
            return matcher
                .route_by_intent(intent)
                .map(|p| (p, "Fallback to basic intent routing".to_string()))
                .ok_or_else(|| {
                    P2RouterError::RoutingFailed("No model available for intent".to_string())
                });
        }

        // Sort by language preference if configured
        if self.config.language_preference {
            candidates.sort_by(|a, b| {
                let score_a = self.language_affinity(a, &features.primary_language);
                let score_b = self.language_affinity(b, &features.primary_language);
                score_b
                    .partial_cmp(&score_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Select best candidate (first after sorting)
        let selected = candidates
            .into_iter()
            .next()
            .ok_or_else(|| P2RouterError::RoutingFailed("No candidates available".to_string()))?;

        let reason = if filter_reasons.is_empty() {
            format!("Selected {} for intent {:?}", selected.id, intent)
        } else {
            format!(
                "Selected {} for intent {:?}. {}",
                selected.id,
                intent,
                filter_reasons.join("; ")
            )
        };

        Ok((selected.clone(), reason))
    }

    /// Calculate language affinity score for a model
    fn language_affinity(&self, profile: &ModelProfile, language: &Language) -> f64 {
        // This is a simple heuristic - in practice, you'd have language capability data
        match language {
            Language::Chinese | Language::Japanese | Language::Korean => {
                // Models with "zh" or "chinese" in name are preferred for CJK
                if profile.model.contains("zh")
                    || profile.model.contains("chinese")
                    || profile.model.contains("qwen")
                {
                    1.0
                } else {
                    0.5
                }
            }
            Language::English => {
                // English models get slight preference for English content
                if profile.model.contains("en") || !profile.model.contains("zh") {
                    0.8
                } else {
                    0.6
                }
            }
            Language::Mixed | Language::Unknown => 0.5,
        }
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Errors from P2 router operations
#[derive(Debug, thiserror::Error)]
pub enum P2RouterError {
    #[error("Cache initialization failed: {0}")]
    CacheInitFailed(String),

    #[error("Cache lookup failed: {0}")]
    CacheLookupFailed(String),

    #[error("Cache store failed: {0}")]
    CacheStoreFailed(String),

    #[error("Routing failed: {0}")]
    RoutingFailed(String),

    #[error("Analysis failed: {0}")]
    AnalysisFailed(String),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::profiles::{CostTier, LatencyTier};
    use crate::dispatcher::model_router::rules::{CostStrategy, ModelRoutingRules};
    use crate::dispatcher::model_router::ContextSize;

    fn create_test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile {
                id: "claude-opus".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-opus-4".to_string(),
                capabilities: vec![Capability::Reasoning, Capability::CodeGeneration],
                cost_tier: CostTier::High,
                latency_tier: LatencyTier::Slow,
                max_context: Some(200_000),
                local: false,
                parameters: None,
            },
            ModelProfile {
                id: "claude-sonnet".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                capabilities: vec![Capability::CodeGeneration],
                cost_tier: CostTier::Medium,
                latency_tier: LatencyTier::Medium,
                max_context: Some(200_000),
                local: false,
                parameters: None,
            },
            ModelProfile {
                id: "qwen-zh".to_string(),
                provider: "ollama".to_string(),
                model: "qwen2-zh".to_string(),
                capabilities: vec![Capability::FastResponse],
                cost_tier: CostTier::Free,
                latency_tier: LatencyTier::Fast,
                max_context: Some(32_000),
                local: true,
                parameters: None,
            },
        ]
    }

    fn create_test_matcher() -> ModelMatcher {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules {
            cost_strategy: CostStrategy::Balanced,
            default_model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };

        ModelMatcher::new(profiles, rules)
    }

    #[test]
    fn test_adjust_intent_code_heavy() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);

        let features = PromptFeatures {
            code_ratio: 0.6,
            has_code_blocks: true,
            ..Default::default()
        };

        let (intent, adjusted) = router.adjust_intent(&TaskIntent::GeneralChat, &features);
        assert!(adjusted);
        assert_eq!(intent, TaskIntent::CodeGeneration);
    }

    #[test]
    fn test_adjust_intent_reasoning() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);

        let features = PromptFeatures {
            complexity_score: 0.8,
            reasoning_level: ReasoningLevel::High,
            ..Default::default()
        };

        let (intent, adjusted) = router.adjust_intent(&TaskIntent::GeneralChat, &features);
        assert!(adjusted);
        assert_eq!(intent, TaskIntent::Reasoning);
    }

    #[test]
    fn test_adjust_intent_preserves_specific() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);

        let features = PromptFeatures {
            code_ratio: 0.9,
            ..Default::default()
        };

        // ImageAnalysis should not be adjusted even with code content
        let (intent, adjusted) = router.adjust_intent(&TaskIntent::ImageAnalysis, &features);
        assert!(!adjusted);
        assert_eq!(intent, TaskIntent::ImageAnalysis);
    }

    #[test]
    fn test_route_with_features() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);
        let matcher = create_test_matcher();

        let features = PromptFeatures {
            estimated_tokens: 100,
            suggested_context_size: ContextSize::Small,
            reasoning_level: ReasoningLevel::High,
            ..Default::default()
        };

        let (profile, reason) = router
            .route_with_features(&matcher, &TaskIntent::Reasoning, &features)
            .unwrap();

        // Should select claude-opus (has Reasoning capability)
        assert_eq!(profile.id, "claude-opus");
        assert!(reason.contains("reasoning-capable"));
    }

    #[test]
    fn test_language_affinity() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);

        // Use model name without "en" substring to avoid false positive
        let zh_profile = ModelProfile {
            id: "baichuan-zh".to_string(),
            provider: "ollama".to_string(),
            model: "baichuan-zh".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::Free,
            latency_tier: LatencyTier::Fast,
            max_context: None,
            local: true,
            parameters: None,
        };

        let en_profile = ModelProfile {
            id: "claude".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::High,
            latency_tier: LatencyTier::Slow,
            max_context: None,
            local: false,
            parameters: None,
        };

        // Chinese content should prefer zh model
        let zh_affinity = router.language_affinity(&zh_profile, &Language::Chinese);
        let en_affinity = router.language_affinity(&en_profile, &Language::Chinese);
        assert!(zh_affinity > en_affinity);

        // English content should prefer non-zh model
        let zh_affinity_en = router.language_affinity(&zh_profile, &Language::English);
        let en_affinity_en = router.language_affinity(&en_profile, &Language::English);
        assert!(en_affinity_en > zh_affinity_en);
    }

    #[tokio::test]
    async fn test_pre_route_no_cache() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);
        let matcher = create_test_matcher();

        let result = router
            .pre_route("Write a Rust function", &TaskIntent::GeneralChat, &matcher)
            .await
            .unwrap();

        // Should be a routing decision (no cache)
        assert!(!result.is_cache_hit());

        let decision = result.as_routing_decision().unwrap();
        assert!(decision.intent_adjusted); // Should adjust to CodeGeneration
    }

    #[test]
    fn test_analyze() {
        let config = P2RouterConfig {
            semantic_cache: SemanticCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let router = P2IntelligentRouter::without_cache(config);

        let features = router.analyze("请用 Rust 写一个快速排序算法");

        assert!(features.estimated_tokens > 0);
        assert!(features.complexity_score > 0.0);
    }
}

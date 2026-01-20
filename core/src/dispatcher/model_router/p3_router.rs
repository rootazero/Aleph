//! P3 Intelligent Router
//!
//! Integrates A/B Testing and Multi-Model Ensemble with the P2 routing system.
//! This module provides the top-level routing entry point that:
//! 1. Checks semantic cache for cached responses (P2)
//! 2. Analyzes prompt features for intelligent routing (P2)
//! 3. Assigns to A/B experiments if applicable
//! 4. Decides if ensemble execution is needed
//! 5. Routes to optimal model(s) based on features and experiments
//! 6. Records experiment outcomes
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        P3IntelligentRouter                              │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  1. SemanticCache.lookup(prompt) → Option<CachedResponse>   [P2]       │
//! │     └─ HIT → return cached response                                     │
//! │     └─ MISS → continue                                                  │
//! │                                                                          │
//! │  2. PromptAnalyzer.analyze(prompt) → PromptFeatures          [P2]       │
//! │                                                                          │
//! │  3. ABTestingEngine.assign(context) → Option<VariantAssignment>  [P3]  │
//! │     └─ Experiment overrides may modify model selection                 │
//! │                                                                          │
//! │  4. EnsembleEngine.should_ensemble(request) → EnsembleDecision   [P3]  │
//! │     └─ If ensemble: execute multiple models in parallel                │
//! │     └─ If single: route to single model                                 │
//! │                                                                          │
//! │  5. Execute request (external or ensemble)                              │
//! │                                                                          │
//! │  6. SemanticCache.store(prompt, response)                     [P2]     │
//! │                                                                          │
//! │  7. ABTestingEngine.record_outcome(outcome)                   [P3]     │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::{P3IntelligentRouter, P3RouterConfig};
//!
//! let router = P3IntelligentRouter::new(config)?;
//!
//! // Pre-route with A/B and ensemble consideration
//! let result = router.pre_route("Complex reasoning task...", &TaskIntent::Reasoning).await?;
//!
//! match result {
//!     P3PreRouteResult::CacheHit(hit) => {
//!         return Ok(hit.response().content.clone());
//!     }
//!     P3PreRouteResult::SingleModel(decision) => {
//!         let response = execute_model(&decision.selected_model, prompt).await?;
//!         router.post_route_single(&decision, &response).await?;
//!     }
//!     P3PreRouteResult::Ensemble(decision) => {
//!         // Ensemble result already includes execution
//!         router.post_route_ensemble(&decision, &decision.result).await?;
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::ab_testing::{ABTestingEngine, ExperimentOutcome, TrackedMetric, VariantAssignment};
use super::ensemble::{
    EnsembleConfig, EnsembleDecision, EnsembleEngine, EnsembleEngineConfig, EnsembleRequest,
    EnsembleResult,
};
use super::matcher::ModelMatcher;
use super::p2_router::{P2IntelligentRouter, P2RouterConfig, P2RouterError, RoutingDecision};
use super::profiles::ModelProfile;
use super::semantic_cache::{CacheHit, CacheMetadata, CachedResponse};
use super::TaskIntent;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the P3 Intelligent Router
#[derive(Debug, Clone)]
pub struct P3RouterConfig {
    /// P2 Router configuration (includes prompt analysis and cache)
    pub p2_config: P2RouterConfig,

    /// Enable A/B testing integration
    pub ab_testing_enabled: bool,

    /// Enable ensemble execution
    pub ensemble_enabled: bool,

    /// Ensemble engine configuration
    pub ensemble_config: EnsembleEngineConfig,

    /// User ID extraction mode for A/B assignment
    pub user_id_mode: UserIdMode,
}

/// Mode for extracting user ID for A/B testing assignment
#[derive(Debug, Clone, Default)]
pub enum UserIdMode {
    /// Use provided user ID
    #[default]
    Provided,
    /// Generate session-based ID
    SessionBased,
    /// Use request ID (random per request)
    RequestBased,
}

impl Default for P3RouterConfig {
    fn default() -> Self {
        Self {
            p2_config: P2RouterConfig::default(),
            ab_testing_enabled: false,
            ensemble_enabled: false,
            ensemble_config: EnsembleEngineConfig::default(),
            user_id_mode: UserIdMode::default(),
        }
    }
}

// =============================================================================
// P3 Pre-Route Results
// =============================================================================

/// Result of P3 pre-routing analysis
#[derive(Debug)]
pub enum P3PreRouteResult {
    /// Cache hit - use the cached response
    CacheHit(CacheHit),

    /// Single model routing decision
    SingleModel(P3RoutingDecision),

    /// Ensemble execution required
    Ensemble(P3EnsembleDecision),
}

impl P3PreRouteResult {
    /// Check if this is a cache hit
    pub fn is_cache_hit(&self) -> bool {
        matches!(self, P3PreRouteResult::CacheHit(_))
    }

    /// Check if ensemble execution is required
    pub fn requires_ensemble(&self) -> bool {
        matches!(self, P3PreRouteResult::Ensemble(_))
    }
}

/// P3 routing decision for single model execution
#[derive(Debug, Clone)]
pub struct P3RoutingDecision {
    /// Base P2 routing decision
    pub p2_decision: RoutingDecision,

    /// A/B experiment assignment if any
    pub experiment_assignment: Option<VariantAssignment>,

    /// Whether the model was overridden by A/B test
    pub ab_override: bool,

    /// Original model before A/B override
    pub original_model: Option<ModelProfile>,

    /// Start time for latency tracking
    pub start_time: Instant,
}

/// P3 ensemble decision with execution details
#[derive(Debug)]
pub struct P3EnsembleDecision {
    /// Base routing decision
    pub p2_decision: RoutingDecision,

    /// Ensemble decision details
    pub ensemble_decision: EnsembleDecision,

    /// Ensemble configuration used
    pub ensemble_config: EnsembleConfig,

    /// A/B experiment assignment if any
    pub experiment_assignment: Option<VariantAssignment>,

    /// Start time for latency tracking
    pub start_time: Instant,
}

// =============================================================================
// P3 Router Events
// =============================================================================

/// Events emitted by the P3 router for monitoring
#[derive(Debug, Clone)]
pub enum P3RouterEvent {
    /// Cache hit occurred
    CacheHit {
        prompt_hash: String,
        model_id: String,
    },

    /// Model selected for single execution
    ModelSelected {
        model_id: String,
        intent: TaskIntent,
        complexity: f64,
        ab_experiment: Option<String>,
    },

    /// Ensemble execution triggered
    EnsembleTriggered {
        models: Vec<String>,
        mode: String,
        reason: String,
    },

    /// A/B experiment assigned
    ExperimentAssigned {
        experiment_id: String,
        variant_id: String,
        user_id: String,
    },

    /// Outcome recorded
    OutcomeRecorded {
        experiment_id: String,
        variant_id: String,
        latency_ms: u64,
        success: bool,
    },
}

// =============================================================================
// P3 Intelligent Router
// =============================================================================

/// P3 Intelligent Router with A/B Testing and Ensemble capabilities
pub struct P3IntelligentRouter {
    /// P2 router for prompt analysis and caching
    p2_router: P2IntelligentRouter,

    /// A/B testing engine (optional)
    ab_engine: Option<Arc<ABTestingEngine>>,

    /// Ensemble engine (optional)
    ensemble_engine: Option<EnsembleEngine>,

    /// Configuration
    config: P3RouterConfig,

    /// Session ID for session-based user ID mode
    session_id: String,
}

impl P3IntelligentRouter {
    /// Create a new P3 router
    pub fn new(config: P3RouterConfig) -> Result<Self, P3RouterError> {
        let p2_router =
            P2IntelligentRouter::new(config.p2_config.clone()).map_err(P3RouterError::P2Error)?;

        let ab_engine = if config.ab_testing_enabled {
            Some(Arc::new(ABTestingEngine::new(vec![])))
        } else {
            None
        };

        let ensemble_engine = if config.ensemble_enabled {
            Some(EnsembleEngine::new(config.ensemble_config.clone()))
        } else {
            None
        };

        // Generate session ID
        let session_id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            p2_router,
            ab_engine,
            ensemble_engine,
            config,
            session_id,
        })
    }

    /// Create with custom A/B testing engine
    pub fn with_ab_engine(mut self, engine: Arc<ABTestingEngine>) -> Self {
        self.ab_engine = Some(engine);
        self
    }

    /// Pre-route: check cache, analyze, decide on A/B and ensemble
    pub async fn pre_route(
        &self,
        prompt: &str,
        intent: &TaskIntent,
        matcher: &ModelMatcher,
        user_id: Option<&str>,
        context: HashMap<String, String>,
    ) -> Result<P3PreRouteResult, P3RouterError> {
        let start_time = Instant::now();

        // Step 1: Try P2 pre-route (includes cache check and prompt analysis)
        let p2_result = self
            .p2_router
            .pre_route(prompt, intent, matcher)
            .await
            .map_err(P3RouterError::P2Error)?;

        // If cache hit, return immediately
        if let super::p2_router::PreRouteResult::CacheHit(hit) = p2_result {
            return Ok(P3PreRouteResult::CacheHit(hit));
        }

        // Get the P2 routing decision
        let p2_decision = match p2_result {
            super::p2_router::PreRouteResult::RoutingDecision(d) => d,
            _ => unreachable!(),
        };

        // Step 2: A/B Testing assignment
        let (experiment_assignment, ab_override, final_model, original_model) =
            self.apply_ab_testing(user_id, &p2_decision, &context);

        // Step 3: Ensemble decision
        if self.ensemble_engine.is_some() {
            let ensemble_decision = self.check_ensemble(&p2_decision, &final_model);

            if ensemble_decision.should_ensemble {
                // Extract config before moving ensemble_decision
                let ensemble_config = ensemble_decision.config.clone().unwrap_or_default();
                return Ok(P3PreRouteResult::Ensemble(P3EnsembleDecision {
                    p2_decision,
                    ensemble_decision,
                    ensemble_config,
                    experiment_assignment,
                    start_time,
                }));
            }
        }

        // Single model routing
        let mut final_decision = P3RoutingDecision {
            p2_decision,
            experiment_assignment,
            ab_override,
            original_model,
            start_time,
        };

        // Apply A/B override to the model
        if ab_override {
            final_decision.p2_decision.selected_model = final_model;
        }

        Ok(P3PreRouteResult::SingleModel(final_decision))
    }

    /// Post-route for single model: store cache and record outcome
    pub async fn post_route_single(
        &self,
        decision: &P3RoutingDecision,
        response: &CachedResponse,
        success: bool,
    ) -> Result<(), P3RouterError> {
        let latency_ms = decision.start_time.elapsed().as_millis() as u64;

        // Store in cache via P2
        self.p2_router
            .post_route(&decision.p2_decision, response)
            .await
            .map_err(P3RouterError::P2Error)?;

        // Record A/B outcome if assigned
        if let (Some(ref engine), Some(ref assignment)) =
            (&self.ab_engine, &decision.experiment_assignment)
        {
            self.record_ab_outcome(
                engine,
                assignment,
                latency_ms,
                response.content.len(),
                success,
            );
        }

        Ok(())
    }

    /// Post-route for ensemble: store cache and record outcome
    pub async fn post_route_ensemble(
        &self,
        decision: &P3EnsembleDecision,
        result: &EnsembleResult,
    ) -> Result<(), P3RouterError> {
        let latency_ms = decision.start_time.elapsed().as_millis() as u64;

        // Calculate total tokens from the selected model's result
        let tokens_used: u32 = result
            .all_results
            .iter()
            .find(|r| r.model_id == result.selected_model)
            .map(|r| r.tokens.total())
            .unwrap_or(0);

        // Create cached response from ensemble result
        let response = CachedResponse::new(
            result.response.clone(),
            tokens_used,
            result.total_latency_ms,
            result.total_cost_usd,
        );

        // Store in cache
        if let Some(ref cache) = self.p2_router_cache() {
            let metadata = CacheMetadata {
                task_intent: Some(decision.p2_decision.intent.clone()),
                features_hash: None,
                tags: vec!["ensemble".to_string()],
            };

            let _ = cache
                .store(
                    &decision.p2_decision.prompt,
                    &response,
                    &result.selected_model,
                    None,
                    Some(metadata),
                )
                .await;
        }

        // Record A/B outcome
        if let (Some(ref engine), Some(ref assignment)) =
            (&self.ab_engine, &decision.experiment_assignment)
        {
            self.record_ab_outcome(
                engine,
                assignment,
                latency_ms,
                result.response.len(),
                result.successful_count > 0,
            );
        }

        Ok(())
    }

    /// Get the underlying P2 router
    pub fn p2_router(&self) -> &P2IntelligentRouter {
        &self.p2_router
    }

    /// Get A/B testing engine if enabled
    pub fn ab_engine(&self) -> Option<&Arc<ABTestingEngine>> {
        self.ab_engine.as_ref()
    }

    /// Get ensemble engine if enabled
    pub fn ensemble_engine(&self) -> Option<&EnsembleEngine> {
        self.ensemble_engine.as_ref()
    }

    /// Get configuration
    pub fn config(&self) -> &P3RouterConfig {
        &self.config
    }

    // =========================================================================
    // Private Methods
    // =========================================================================

    /// Get cache reference from P2 router
    fn p2_router_cache(&self) -> Option<Arc<super::semantic_cache::SemanticCacheManager>> {
        // Note: This requires P2IntelligentRouter to expose cache
        // For now, return None - cache operations are done via P2 router
        None
    }

    /// Apply A/B testing and return assignment info
    fn apply_ab_testing(
        &self,
        user_id: Option<&str>,
        p2_decision: &RoutingDecision,
        _context: &HashMap<String, String>,
    ) -> (
        Option<VariantAssignment>,
        bool,
        ModelProfile,
        Option<ModelProfile>,
    ) {
        let original_model = p2_decision.selected_model.clone();

        let Some(ref engine) = self.ab_engine else {
            return (None, false, original_model, None);
        };

        // Determine effective user ID based on mode
        let effective_user_id = match &self.config.user_id_mode {
            UserIdMode::Provided => user_id.map(|s| s.to_string()),
            UserIdMode::SessionBased => Some(self.session_id.clone()),
            UserIdMode::RequestBased => Some(uuid::Uuid::new_v4().to_string()),
        };

        // Generate request ID for tracking
        let request_id = uuid::Uuid::new_v4().to_string();

        // Try to get assignment using the ABTestingEngine.assign() method
        let assignment = engine.assign(
            effective_user_id.as_deref(),
            Some(&self.session_id),
            &request_id,
            &p2_decision.intent,
            Some(&p2_decision.features),
        );

        let Some(assignment) = assignment else {
            return (None, false, original_model, None);
        };

        // Check if variant has model override (model_override is directly on VariantAssignment)
        if let Some(ref model_override) = assignment.model_override {
            // Create a modified model profile with the override
            let mut overridden_model = original_model.clone();
            overridden_model.id = model_override.clone();
            // Note: In production, would look up full profile from matcher
            (
                Some(assignment),
                true,
                overridden_model,
                Some(original_model),
            )
        } else {
            (Some(assignment), false, original_model, None)
        }
    }

    /// Check if ensemble execution should be used
    fn check_ensemble(
        &self,
        p2_decision: &RoutingDecision,
        _model: &ModelProfile,
    ) -> EnsembleDecision {
        let Some(ref engine) = self.ensemble_engine else {
            return EnsembleDecision {
                should_ensemble: false,
                config: None,
                reason: "Ensemble not enabled".to_string(),
                available_models: vec![],
            };
        };

        let request = EnsembleRequest::new(&p2_decision.prompt, p2_decision.intent.clone())
            .with_complexity(p2_decision.features.complexity_score);

        engine.should_ensemble(&request)
    }

    /// Record A/B experiment outcome
    fn record_ab_outcome(
        &self,
        engine: &ABTestingEngine,
        assignment: &VariantAssignment,
        latency_ms: u64,
        response_length: usize,
        success: bool,
    ) {
        // Use model_override if present, otherwise use variant_id as model identifier
        let model_used = assignment
            .model_override
            .as_ref()
            .unwrap_or(&assignment.variant_id);

        let request_id = uuid::Uuid::new_v4().to_string();

        let outcome = ExperimentOutcome::new(
            &assignment.experiment_id,
            &assignment.variant_id,
            &request_id,
            model_used,
        )
        .with_latency_ms(latency_ms)
        .with_success(success)
        .with_metric(
            TrackedMetric::Custom("response_length".to_string()),
            response_length as f64,
        );

        engine.record_outcome(outcome);
    }
}

// =============================================================================
// Errors
// =============================================================================

/// Errors from the P3 router
#[derive(Debug, thiserror::Error)]
pub enum P3RouterError {
    #[error("P2 router error: {0}")]
    P2Error(#[from] P2RouterError),

    #[error("A/B testing error: {0}")]
    ABTestingError(String),

    #[error("Ensemble execution error: {0}")]
    EnsembleError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_p3_router_config_default() {
        let config = P3RouterConfig::default();
        assert!(!config.ab_testing_enabled);
        assert!(!config.ensemble_enabled);
    }

    #[test]
    fn test_user_id_mode_default() {
        let mode = UserIdMode::default();
        assert!(matches!(mode, UserIdMode::Provided));
    }

    #[test]
    fn test_p3_pre_route_result_checks() {
        use super::super::semantic_cache::{CacheEntry, CacheHit, CacheHitType, CacheMetadata};

        // Create a test CacheEntry manually
        let entry = CacheEntry {
            id: "test-id".to_string(),
            prompt_hash: "hash".to_string(),
            prompt_preview: "prompt".to_string(),
            embedding: vec![0.1, 0.2, 0.3],
            response: CachedResponse::new("response".to_string(), 100, 50, 0.001),
            model_used: "model".to_string(),
            created_at: std::time::SystemTime::now(),
            expires_at: None,
            hit_count: 0,
            last_accessed: std::time::SystemTime::now(),
            metadata: CacheMetadata::default(),
        };

        let cache_hit = CacheHit {
            entry,
            similarity: 1.0,
            hit_type: CacheHitType::Exact,
        };

        let result = P3PreRouteResult::CacheHit(cache_hit);
        assert!(result.is_cache_hit());
        assert!(!result.requires_ensemble());
    }

    #[tokio::test]
    async fn test_p3_router_creation() {
        let config = P3RouterConfig::default();
        let router = P3IntelligentRouter::new(config);
        assert!(router.is_ok());

        let router = router.unwrap();
        assert!(router.ab_engine().is_none());
        assert!(router.ensemble_engine().is_none());
    }

    #[tokio::test]
    async fn test_p3_router_with_ab_enabled() {
        let config = P3RouterConfig {
            ab_testing_enabled: true,
            ..Default::default()
        };
        let router = P3IntelligentRouter::new(config).unwrap();
        assert!(router.ab_engine().is_some());
    }

    #[tokio::test]
    async fn test_p3_router_with_ensemble_enabled() {
        let config = P3RouterConfig {
            ensemble_enabled: true,
            ..Default::default()
        };
        let router = P3IntelligentRouter::new(config).unwrap();
        assert!(router.ensemble_engine().is_some());
    }

    #[tokio::test]
    async fn test_p3_router_with_both_enabled() {
        let config = P3RouterConfig {
            ab_testing_enabled: true,
            ensemble_enabled: true,
            ..Default::default()
        };
        let router = P3IntelligentRouter::new(config).unwrap();
        assert!(router.ab_engine().is_some());
        assert!(router.ensemble_engine().is_some());
    }

    #[test]
    fn test_p3_router_error_display() {
        let error = P3RouterError::EnsembleError("execution failed".to_string());
        assert!(error.to_string().contains("Ensemble execution error"));

        let error = P3RouterError::ABTestingError("assignment failed".to_string());
        assert!(error.to_string().contains("A/B testing error"));

        let error = P3RouterError::ConfigError("invalid config".to_string());
        assert!(error.to_string().contains("Configuration error"));
    }

    #[test]
    fn test_user_id_modes() {
        assert!(matches!(UserIdMode::Provided, UserIdMode::Provided));
        assert!(matches!(UserIdMode::SessionBased, UserIdMode::SessionBased));
        assert!(matches!(UserIdMode::RequestBased, UserIdMode::RequestBased));
    }

    #[test]
    fn test_p3_router_event_variants() {
        // Test that all event variants can be created
        let event1 = P3RouterEvent::CacheHit {
            prompt_hash: "abc123".to_string(),
            model_id: "claude-opus".to_string(),
        };
        assert!(matches!(event1, P3RouterEvent::CacheHit { .. }));

        let event2 = P3RouterEvent::ExperimentAssigned {
            experiment_id: "exp-1".to_string(),
            variant_id: "variant-a".to_string(),
            user_id: "user-123".to_string(),
        };
        assert!(matches!(event2, P3RouterEvent::ExperimentAssigned { .. }));

        let event3 = P3RouterEvent::EnsembleTriggered {
            models: vec!["model-a".to_string(), "model-b".to_string()],
            mode: "best_of_n".to_string(),
            reason: "high complexity".to_string(),
        };
        assert!(matches!(event3, P3RouterEvent::EnsembleTriggered { .. }));

        let event4 = P3RouterEvent::OutcomeRecorded {
            experiment_id: "exp-1".to_string(),
            variant_id: "variant-a".to_string(),
            latency_ms: 150,
            success: true,
        };
        assert!(matches!(event4, P3RouterEvent::OutcomeRecorded { .. }));
    }

    #[test]
    fn test_p3_pre_route_result_variants() {
        use super::super::semantic_cache::{CacheEntry, CacheHit, CacheHitType, CacheMetadata};

        // Test CacheHit variant
        let entry = CacheEntry {
            id: "test-id".to_string(),
            prompt_hash: "hash".to_string(),
            prompt_preview: "prompt".to_string(),
            embedding: vec![0.1, 0.2, 0.3],
            response: CachedResponse::new("response".to_string(), 100, 50, 0.001),
            model_used: "model".to_string(),
            created_at: std::time::SystemTime::now(),
            expires_at: None,
            hit_count: 0,
            last_accessed: std::time::SystemTime::now(),
            metadata: CacheMetadata::default(),
        };

        let cache_hit = CacheHit {
            entry,
            similarity: 1.0,
            hit_type: CacheHitType::Exact,
        };

        let result = P3PreRouteResult::CacheHit(cache_hit);
        assert!(result.is_cache_hit());
        assert!(!result.requires_ensemble());
    }
}

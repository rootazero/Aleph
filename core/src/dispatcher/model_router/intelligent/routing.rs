//! Intelligent Routing Extensions
//!
//! This module extends ModelMatcher with health-aware and metrics-aware routing
//! capabilities for optimal model selection.

use crate::dispatcher::model_router::{
    DynamicScorer, HealthManager, MetricsCollector, ModelMatcher, ModelProfile, ModelRouter,
    RoutingError, TaskIntent,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

// ============================================================================
// Routing Configuration
// ============================================================================

/// Configuration for intelligent routing
#[derive(Debug, Clone)]
pub struct IntelligentRoutingConfig {
    /// Weight for health status in final score (0.0-1.0)
    pub health_weight: f64,
    /// Weight for dynamic score in final score (0.0-1.0)
    pub score_weight: f64,
    /// Exploration rate for epsilon-greedy routing (0.0-1.0)
    pub exploration_rate: f64,
    /// Minimum score threshold to consider a model
    pub min_score_threshold: f64,
}

impl Default for IntelligentRoutingConfig {
    fn default() -> Self {
        Self {
            health_weight: 0.3,
            score_weight: 0.7,
            exploration_rate: 0.05,
            min_score_threshold: 0.1,
        }
    }
}

// ============================================================================
// Routing Results
// ============================================================================

/// Result of intelligent routing with detailed scores
#[derive(Debug, Clone)]
pub struct IntelligentRoutingResult {
    /// Selected model profile
    pub profile: ModelProfile,
    /// Final combined score
    pub final_score: f64,
    /// Health-based score component
    pub health_score: f64,
    /// Metrics-based score component
    pub metrics_score: f64,
    /// Whether this was an exploration selection
    pub is_exploration: bool,
    /// Reason for selection
    pub selection_reason: String,
}

/// Candidate model with scores for routing decision
#[derive(Debug, Clone)]
struct ScoredCandidate {
    profile: ModelProfile,
    health_score: f64,
    metrics_score: f64,
    final_score: f64,
}

// ============================================================================
// Intelligent Router
// ============================================================================

/// Intelligent router that combines health and metrics for optimal model selection
pub struct IntelligentRouter<'a> {
    matcher: &'a ModelMatcher,
    health_manager: &'a HealthManager,
    collector: &'a dyn MetricsCollector,
    scorer: &'a DynamicScorer,
    config: IntelligentRoutingConfig,
}

impl<'a> IntelligentRouter<'a> {
    /// Create a new intelligent router
    pub fn new(
        matcher: &'a ModelMatcher,
        health_manager: &'a HealthManager,
        collector: &'a dyn MetricsCollector,
        scorer: &'a DynamicScorer,
    ) -> Self {
        Self {
            matcher,
            health_manager,
            collector,
            scorer,
            config: IntelligentRoutingConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(mut self, config: IntelligentRoutingConfig) -> Self {
        self.config = config;
        self
    }

    /// Route using only health information
    pub async fn route_with_health(
        &self,
        intent: &TaskIntent,
    ) -> Result<IntelligentRoutingResult, RoutingError> {
        let candidates = self.get_health_filtered_candidates(intent).await;

        if candidates.is_empty() {
            return Err(RoutingError::NoModelAvailable {
                task_type: intent.to_task_type().to_string(),
            });
        }

        // Select best by health score
        let best = candidates
            .into_iter()
            .max_by(|a, b| a.health_score.partial_cmp(&b.health_score).unwrap())
            .unwrap();

        Ok(IntelligentRoutingResult {
            profile: best.profile,
            final_score: best.health_score,
            health_score: best.health_score,
            metrics_score: 0.0,
            is_exploration: false,
            selection_reason: "Selected by health status".to_string(),
        })
    }

    /// Route using only metrics information
    pub async fn route_with_metrics(
        &self,
        intent: &TaskIntent,
    ) -> Result<IntelligentRoutingResult, RoutingError> {
        let all_metrics = self.collector.all_metrics().await;
        let candidates = self
            .get_metrics_scored_candidates(intent, &all_metrics)
            .await;

        if candidates.is_empty() {
            // Fall back to basic intent routing
            return self
                .matcher
                .route_by_intent(intent)
                .map(|profile| IntelligentRoutingResult {
                    profile,
                    final_score: 0.5,
                    health_score: 1.0,
                    metrics_score: 0.5,
                    is_exploration: false,
                    selection_reason: "Fallback to intent-based routing".to_string(),
                })
                .ok_or_else(|| RoutingError::NoModelAvailable {
                    task_type: intent.to_task_type().to_string(),
                });
        }

        // Select best by metrics score
        let best = candidates
            .into_iter()
            .max_by(|a, b| a.metrics_score.partial_cmp(&b.metrics_score).unwrap())
            .unwrap();

        Ok(IntelligentRoutingResult {
            profile: best.profile,
            final_score: best.metrics_score,
            health_score: 1.0,
            metrics_score: best.metrics_score,
            is_exploration: false,
            selection_reason: "Selected by metrics score".to_string(),
        })
    }

    /// Route using combined health and metrics (intelligent routing)
    pub async fn route_intelligent(
        &self,
        intent: &TaskIntent,
    ) -> Result<IntelligentRoutingResult, RoutingError> {
        let all_metrics = self.collector.all_metrics().await;
        let candidates = self.get_combined_candidates(intent, &all_metrics).await;

        if candidates.is_empty() {
            // Fall back to basic intent routing
            return self
                .matcher
                .route_by_intent(intent)
                .map(|profile| IntelligentRoutingResult {
                    profile,
                    final_score: 0.5,
                    health_score: 1.0,
                    metrics_score: 0.5,
                    is_exploration: false,
                    selection_reason: "Fallback to intent-based routing (no healthy candidates)"
                        .to_string(),
                })
                .ok_or_else(|| RoutingError::NoModelAvailable {
                    task_type: intent.to_task_type().to_string(),
                });
        }

        // Check for exploration
        if self.should_explore() {
            if let Some(explored) = self.select_exploration_candidate(&candidates) {
                return Ok(IntelligentRoutingResult {
                    profile: explored.profile,
                    final_score: explored.final_score,
                    health_score: explored.health_score,
                    metrics_score: explored.metrics_score,
                    is_exploration: true,
                    selection_reason: "Exploration: testing underused model".to_string(),
                });
            }
        }

        // Select best by final score
        let best = candidates
            .into_iter()
            .max_by(|a, b| a.final_score.partial_cmp(&b.final_score).unwrap())
            .unwrap();

        Ok(IntelligentRoutingResult {
            profile: best.profile,
            final_score: best.final_score,
            health_score: best.health_score,
            metrics_score: best.metrics_score,
            is_exploration: false,
            selection_reason: format!(
                "Selected by combined score (health={:.2}, metrics={:.2})",
                best.health_score, best.metrics_score
            ),
        })
    }

    // ========================================================================
    // Helper Methods
    // ========================================================================

    /// Get candidates filtered by health status
    async fn get_health_filtered_candidates(&self, intent: &TaskIntent) -> Vec<ScoredCandidate> {
        let profiles = self.matcher.profiles();
        let mut candidates = Vec::new();

        for profile in profiles {
            let can_call = self.health_manager.can_call(&profile.id).await;
            if !can_call {
                continue;
            }

            let health_score = self.compute_health_score(&profile.id).await;
            candidates.push(ScoredCandidate {
                profile: profile.clone(),
                health_score,
                metrics_score: 0.0,
                final_score: health_score,
            });
        }

        // Filter by intent capability if needed
        if let Some(capability) = intent.required_capability() {
            candidates.retain(|c| c.profile.has_capability(capability));
        }

        candidates
    }

    /// Get candidates scored by metrics
    async fn get_metrics_scored_candidates(
        &self,
        intent: &TaskIntent,
        all_metrics: &HashMap<String, crate::dispatcher::model_router::MultiWindowMetrics>,
    ) -> Vec<ScoredCandidate> {
        let profiles = self.matcher.profiles();
        let mut candidates = Vec::new();

        for profile in profiles {
            let metrics = all_metrics.get(&profile.id);
            let metrics_score = self.scorer.score(profile, metrics, intent);

            if metrics_score < self.config.min_score_threshold {
                continue;
            }

            candidates.push(ScoredCandidate {
                profile: profile.clone(),
                health_score: 1.0,
                metrics_score,
                final_score: metrics_score,
            });
        }

        // Filter by intent capability if needed
        if let Some(capability) = intent.required_capability() {
            candidates.retain(|c| c.profile.has_capability(capability));
        }

        candidates
    }

    /// Get candidates with combined health and metrics scores
    async fn get_combined_candidates(
        &self,
        intent: &TaskIntent,
        all_metrics: &HashMap<String, crate::dispatcher::model_router::MultiWindowMetrics>,
    ) -> Vec<ScoredCandidate> {
        let profiles = self.matcher.profiles();
        let mut candidates = Vec::new();

        for profile in profiles {
            // Check health first
            let can_call = self.health_manager.can_call(&profile.id).await;
            if !can_call {
                continue;
            }

            // Compute scores
            let health_score = self.compute_health_score(&profile.id).await;
            let metrics = all_metrics.get(&profile.id);
            let metrics_score = self.scorer.score(profile, metrics, intent);

            // Compute final combined score
            let final_score =
                self.config.health_weight * health_score + self.config.score_weight * metrics_score;

            if final_score < self.config.min_score_threshold {
                continue;
            }

            candidates.push(ScoredCandidate {
                profile: profile.clone(),
                health_score,
                metrics_score,
                final_score,
            });
        }

        // Filter by intent capability if needed
        if let Some(capability) = intent.required_capability() {
            candidates.retain(|c| c.profile.has_capability(capability));
        }

        candidates
    }

    /// Compute health score for a model (0.0-1.0)
    async fn compute_health_score(&self, model_id: &str) -> f64 {
        use crate::dispatcher::model_router::HealthStatus;

        let status = self.health_manager.get_status(model_id).await;
        match status {
            HealthStatus::Healthy => 1.0,
            HealthStatus::Degraded => 0.7,
            HealthStatus::Unknown => 0.5,
            HealthStatus::HalfOpen => 0.3,
            HealthStatus::Unhealthy => 0.1,
            HealthStatus::CircuitOpen => 0.0,
        }
    }

    /// Check if we should explore (epsilon-greedy using simple counter)
    fn should_explore(&self) -> bool {
        // Simple deterministic exploration: explore every N calls based on rate
        // Using atomic counter for thread safety
        static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);

        // If exploration_rate is 0.05, explore every 20th call
        let explore_interval = (1.0 / self.config.exploration_rate) as u64;
        if explore_interval == 0 {
            return false;
        }
        count.is_multiple_of(explore_interval)
    }

    /// Select a candidate for exploration (prefer underused models)
    fn select_exploration_candidate(
        &self,
        candidates: &[ScoredCandidate],
    ) -> Option<ScoredCandidate> {
        if candidates.is_empty() {
            return None;
        }

        // Prefer models with lower metrics scores (underused)
        let mut sorted = candidates.to_vec();
        sorted.sort_by(|a, b| a.metrics_score.partial_cmp(&b.metrics_score).unwrap());

        // Pick from bottom half (less used models) - use deterministic selection
        let _half = (sorted.len() / 2).max(1);
        // Simple selection: pick the least used one
        Some(sorted[0].clone())
    }
}

// ============================================================================
// ModelMatcher Extensions
// ============================================================================

impl ModelMatcher {
    /// Create an intelligent router for this matcher
    pub fn intelligent_router<'a>(
        &'a self,
        health_manager: &'a HealthManager,
        collector: &'a dyn MetricsCollector,
        scorer: &'a DynamicScorer,
    ) -> IntelligentRouter<'a> {
        IntelligentRouter::new(self, health_manager, collector, scorer)
    }

    /// Route with health awareness (convenience method)
    pub async fn route_with_health(
        &self,
        intent: &TaskIntent,
        health_manager: &HealthManager,
    ) -> Result<ModelProfile, RoutingError> {
        // Get all profiles that are healthy
        let profiles = self.profiles();
        let mut healthy_profiles = Vec::new();

        for profile in profiles {
            if health_manager.can_call(&profile.id).await {
                healthy_profiles.push(profile.clone());
            }
        }

        // Try to find best from healthy profiles
        if healthy_profiles.is_empty() {
            // Fall back to intent-based routing without health check
            return self
                .route_by_intent(intent)
                .ok_or_else(|| RoutingError::NoModelAvailable {
                    task_type: intent.to_task_type().to_string(),
                });
        }

        // Try intent-based routing within healthy profiles
        if let Some(capability) = intent.required_capability() {
            for profile in &healthy_profiles {
                if profile.has_capability(capability) {
                    return Ok(profile.clone());
                }
            }
        }

        // Return first healthy profile
        Ok(healthy_profiles.into_iter().next().unwrap())
    }

    /// Route with metrics awareness (convenience method)
    pub async fn route_with_metrics(
        &self,
        intent: &TaskIntent,
        collector: &dyn MetricsCollector,
        scorer: &DynamicScorer,
    ) -> Result<ModelProfile, RoutingError> {
        let all_metrics = collector.all_metrics().await;
        let profiles = self.profiles();

        // Score all profiles
        let scored: Vec<(ModelProfile, f64)> = profiles
            .iter()
            .map(|p| {
                let metrics = all_metrics.get(&p.id);
                let score = scorer.score(p, metrics, intent);
                (p.clone(), score)
            })
            .collect();

        // Select best
        scored
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(p, _)| p)
            .ok_or_else(|| RoutingError::NoModelAvailable {
                task_type: intent.to_task_type().to_string(),
            })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::health::collector::{InMemoryMetricsCollector, MetricsConfig};
    use crate::dispatcher::model_router::health::status::{ErrorType, HealthConfig, HealthError};
    use crate::dispatcher::model_router::core::profiles::{Capability, CostTier, LatencyTier};
    use crate::dispatcher::model_router::core::rules::ModelRoutingRules;
    use crate::dispatcher::model_router::core::scoring::ScoringConfig;
    use std::time::Duration;

    fn create_test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile::new("model-a", "provider-a", "a")
                .with_capabilities(vec![Capability::CodeGeneration])
                .with_cost_tier(CostTier::Low)
                .with_latency_tier(LatencyTier::Fast),
            ModelProfile::new("model-b", "provider-b", "b")
                .with_capabilities(vec![Capability::CodeGeneration, Capability::Reasoning])
                .with_cost_tier(CostTier::Medium)
                .with_latency_tier(LatencyTier::Medium),
            ModelProfile::new("model-c", "provider-c", "c")
                .with_capabilities(vec![Capability::Reasoning])
                .with_cost_tier(CostTier::High)
                .with_latency_tier(LatencyTier::Slow),
        ]
    }

    fn create_matcher() -> ModelMatcher {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules::new("model-b");
        ModelMatcher::new(profiles, rules)
    }

    fn create_collector() -> InMemoryMetricsCollector {
        InMemoryMetricsCollector::new(MetricsConfig::default())
    }

    fn create_scorer() -> DynamicScorer {
        DynamicScorer::new(ScoringConfig::default())
    }

    #[tokio::test]
    async fn test_route_with_health_filters_unhealthy() {
        let matcher = create_matcher();
        let health_manager = HealthManager::new(HealthConfig::default());

        // Mark model-a as unhealthy
        let error = HealthError::new(ErrorType::ServerError, "Error");
        health_manager.record_failure("model-a", error).await;

        // Should not select model-a
        let result = matcher
            .route_with_health(&TaskIntent::CodeGeneration, &health_manager)
            .await
            .unwrap();

        assert_ne!(result.id, "model-a");
    }

    #[tokio::test]
    async fn test_route_with_health_prefers_healthy() {
        let matcher = create_matcher();
        let health_manager = HealthManager::new(HealthConfig::default());

        // Mark all models as healthy by recording success
        health_manager
            .record_success("model-a", Duration::from_millis(100), None)
            .await;
        health_manager
            .record_success("model-b", Duration::from_millis(100), None)
            .await;
        health_manager
            .record_success("model-c", Duration::from_millis(100), None)
            .await;

        // Should return a healthy model
        let result = matcher
            .route_with_health(&TaskIntent::CodeGeneration, &health_manager)
            .await
            .unwrap();

        assert!(!result.id.is_empty());
    }

    #[tokio::test]
    async fn test_route_with_metrics() {
        let matcher = create_matcher();
        let collector = create_collector();
        let scorer = create_scorer();

        // Should return best scored model
        let result = matcher
            .route_with_metrics(&TaskIntent::CodeGeneration, &collector, &scorer)
            .await
            .unwrap();

        assert!(!result.id.is_empty());
    }

    #[tokio::test]
    async fn test_intelligent_router_combined() {
        let matcher = create_matcher();
        let health_manager = HealthManager::new(HealthConfig::default());
        let collector = create_collector();
        let scorer = create_scorer();

        // Make all models healthy
        health_manager
            .record_success("model-a", Duration::from_millis(100), None)
            .await;
        health_manager
            .record_success("model-b", Duration::from_millis(100), None)
            .await;
        health_manager
            .record_success("model-c", Duration::from_millis(100), None)
            .await;

        let router = matcher.intelligent_router(&health_manager, &collector, &scorer);
        let result = router
            .route_intelligent(&TaskIntent::CodeGeneration)
            .await
            .unwrap();

        assert!(!result.profile.id.is_empty());
        assert!(result.final_score > 0.0);
    }

    #[tokio::test]
    async fn test_intelligent_router_filters_unhealthy() {
        let matcher = create_matcher();
        let health_manager = HealthManager::new(HealthConfig::default());
        let collector = create_collector();
        let scorer = create_scorer();

        // Mark model-a as unhealthy
        let error = HealthError::new(ErrorType::ServerError, "Error");
        health_manager.record_failure("model-a", error).await;

        // Make others healthy
        health_manager
            .record_success("model-b", Duration::from_millis(100), None)
            .await;
        health_manager
            .record_success("model-c", Duration::from_millis(100), None)
            .await;

        let router = matcher.intelligent_router(&health_manager, &collector, &scorer);
        let result = router
            .route_intelligent(&TaskIntent::CodeGeneration)
            .await
            .unwrap();

        // Should not select unhealthy model
        assert_ne!(result.profile.id, "model-a");
    }

    #[tokio::test]
    async fn test_routing_config() {
        let config = IntelligentRoutingConfig::default();

        assert!(config.health_weight > 0.0);
        assert!(config.score_weight > 0.0);
        assert!((config.health_weight + config.score_weight - 1.0).abs() < 0.01);
        assert!(config.exploration_rate >= 0.0 && config.exploration_rate <= 1.0);
    }

    #[tokio::test]
    async fn test_routing_result_fields() {
        let matcher = create_matcher();
        let health_manager = HealthManager::new(HealthConfig::default());
        let collector = create_collector();
        let scorer = create_scorer();

        health_manager
            .record_success("model-a", Duration::from_millis(100), None)
            .await;
        health_manager
            .record_success("model-b", Duration::from_millis(100), None)
            .await;

        let router = matcher.intelligent_router(&health_manager, &collector, &scorer);
        let result = router
            .route_intelligent(&TaskIntent::GeneralChat)
            .await
            .unwrap();

        // Verify result has all expected fields
        assert!(!result.selection_reason.is_empty());
        assert!(result.health_score >= 0.0 && result.health_score <= 1.0);
        assert!(result.metrics_score >= 0.0 && result.metrics_score <= 1.0);
        assert!(result.final_score >= 0.0 && result.final_score <= 1.0);
    }

    #[tokio::test]
    async fn test_fallback_when_all_unhealthy() {
        let matcher = create_matcher();
        let health_manager = HealthManager::new(HealthConfig::default());
        let collector = create_collector();
        let scorer = create_scorer();

        // Don't mark any as healthy (all unknown)
        // Intelligent router should fall back to intent-based routing

        let router = matcher.intelligent_router(&health_manager, &collector, &scorer);
        let result = router.route_intelligent(&TaskIntent::GeneralChat).await;

        // Should succeed with fallback
        assert!(result.is_ok());
    }
}

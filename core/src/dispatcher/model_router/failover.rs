//! Failover Chain Management
//!
//! This module provides failover chain configuration and model selection
//! strategies for resilient routing when primary models are unavailable.

use super::health::HealthStatus;
use super::health_manager::HealthManager;
use super::profiles::{Capability, CostTier, ModelProfile};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// =============================================================================
// Failover Selection Mode
// =============================================================================

/// Strategy for selecting failover models
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailoverSelectionMode {
    /// Use models in configured order (first available)
    Ordered,

    /// Prefer healthiest model
    #[default]
    HealthPriority,

    /// Prefer cheapest healthy model
    CostPriority,

    /// Balance health and cost with weights
    Balanced,
}

impl FailoverSelectionMode {
    /// Get display name
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ordered => "ordered",
            Self::HealthPriority => "health_priority",
            Self::CostPriority => "cost_priority",
            Self::Balanced => "balanced",
        }
    }
}

// =============================================================================
// Failover Chain
// =============================================================================

/// Chain of models for failover
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverChain {
    /// Primary model ID
    pub primary: String,

    /// Alternative models in preference order
    pub alternatives: Vec<String>,

    /// How to select from alternatives
    #[serde(default)]
    pub selection_mode: FailoverSelectionMode,

    /// Capability requirements that alternatives must satisfy
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,

    /// Weight for health score in balanced mode (0.0 - 1.0)
    #[serde(default = "default_health_weight")]
    pub health_weight: f64,

    /// Weight for cost score in balanced mode (0.0 - 1.0)
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f64,
}

fn default_health_weight() -> f64 {
    0.6
}

fn default_cost_weight() -> f64 {
    0.4
}

impl FailoverChain {
    /// Create a new failover chain with primary model only
    pub fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            alternatives: Vec::new(),
            selection_mode: FailoverSelectionMode::default(),
            required_capabilities: Vec::new(),
            health_weight: default_health_weight(),
            cost_weight: default_cost_weight(),
        }
    }

    /// Builder: set alternative models
    pub fn with_alternatives(mut self, alternatives: Vec<String>) -> Self {
        self.alternatives = alternatives;
        self
    }

    /// Builder: set selection mode
    pub fn with_selection_mode(mut self, mode: FailoverSelectionMode) -> Self {
        self.selection_mode = mode;
        self
    }

    /// Builder: set required capabilities
    pub fn with_required_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.required_capabilities = capabilities;
        self
    }

    /// Builder: set balanced weights
    pub fn with_weights(mut self, health_weight: f64, cost_weight: f64) -> Self {
        self.health_weight = health_weight.clamp(0.0, 1.0);
        self.cost_weight = cost_weight.clamp(0.0, 1.0);
        self
    }

    /// Build failover chain from profiles with capability overlap
    ///
    /// Finds alternative models that share at least one capability with the primary.
    pub fn from_profiles<'a>(
        primary_id: &str,
        profiles: impl Iterator<Item = &'a ModelProfile>,
        selection_mode: FailoverSelectionMode,
    ) -> Option<Self> {
        let profiles: Vec<_> = profiles.collect();
        let primary = profiles.iter().find(|p| p.id == primary_id)?;

        let primary_caps: HashSet<_> = primary.capabilities.iter().cloned().collect();

        // Find alternatives with overlapping capabilities
        let mut alternatives: Vec<_> = profiles
            .iter()
            .filter(|p| p.id != primary_id)
            .filter(|p| {
                // Must have at least one capability in common
                p.capabilities.iter().any(|c| primary_caps.contains(c))
            })
            .map(|p| p.id.clone())
            .collect();

        // Sort by cost tier (cheaper first) as initial order
        alternatives.sort_by(|a, b| {
            let a_profile = profiles.iter().find(|p| p.id == *a);
            let b_profile = profiles.iter().find(|p| p.id == *b);
            match (a_profile, b_profile) {
                (Some(a), Some(b)) => a.cost_tier.cmp(&b.cost_tier),
                _ => std::cmp::Ordering::Equal,
            }
        });

        Some(Self {
            primary: primary_id.to_string(),
            alternatives,
            selection_mode,
            required_capabilities: primary.capabilities.clone(),
            health_weight: default_health_weight(),
            cost_weight: default_cost_weight(),
        })
    }

    /// Get all models in the chain (primary first, then alternatives)
    pub fn all_models(&self) -> Vec<&str> {
        let mut models = vec![self.primary.as_str()];
        models.extend(self.alternatives.iter().map(|s| s.as_str()));
        models
    }

    /// Get total number of models in chain
    pub fn len(&self) -> usize {
        1 + self.alternatives.len()
    }

    /// Check if chain is empty (only has primary, no alternatives)
    pub fn is_empty(&self) -> bool {
        self.alternatives.is_empty()
    }

    /// Select next model to try after failure (synchronous version)
    ///
    /// # Arguments
    /// * `failed_models` - Models that have already failed
    /// * `health_statuses` - Map of model_id -> HealthStatus
    /// * `cost_tiers` - Map of model_id -> CostTier
    pub fn next_model_sync(
        &self,
        failed_models: &[String],
        health_statuses: &std::collections::HashMap<String, HealthStatus>,
        cost_tiers: &std::collections::HashMap<String, CostTier>,
    ) -> Option<String> {
        let candidates: Vec<_> = self
            .alternatives
            .iter()
            .filter(|m| !failed_models.contains(m))
            .filter(|m| {
                let status = health_statuses
                    .get(*m)
                    .copied()
                    .unwrap_or(HealthStatus::Unknown);
                status.can_call()
            })
            .cloned()
            .collect();

        if candidates.is_empty() {
            return None;
        }

        match self.selection_mode {
            FailoverSelectionMode::Ordered => {
                // Return first available in order
                candidates.into_iter().next()
            }

            FailoverSelectionMode::HealthPriority => {
                // Return healthiest model
                candidates
                    .into_iter()
                    .min_by_key(|m| {
                        health_statuses
                            .get(m)
                            .map(|s| s.priority())
                            .unwrap_or(u8::MAX)
                    })
            }

            FailoverSelectionMode::CostPriority => {
                // Return cheapest healthy model
                candidates.into_iter().min_by_key(|m| {
                    cost_tiers.get(m).copied().unwrap_or(CostTier::High)
                })
            }

            FailoverSelectionMode::Balanced => {
                // Score by health and cost
                candidates.into_iter().min_by(|a, b| {
                    let a_score = self.balanced_score(a, health_statuses, cost_tiers);
                    let b_score = self.balanced_score(b, health_statuses, cost_tiers);
                    a_score.partial_cmp(&b_score).unwrap_or(std::cmp::Ordering::Equal)
                })
            }
        }
    }

    /// Calculate balanced score (lower is better)
    fn balanced_score(
        &self,
        model_id: &str,
        health_statuses: &std::collections::HashMap<String, HealthStatus>,
        cost_tiers: &std::collections::HashMap<String, CostTier>,
    ) -> f64 {
        let health_score = health_statuses
            .get(model_id)
            .map(|s| s.priority() as f64)
            .unwrap_or(5.0);

        let cost_score = cost_tiers
            .get(model_id)
            .map(|c| c.cost_multiplier())
            .unwrap_or(10.0);

        // Normalize cost score (0.0 - 50.0 -> 0.0 - 5.0)
        let normalized_cost = cost_score / 10.0;

        // Weighted combination
        self.health_weight * health_score + self.cost_weight * normalized_cost
    }

    /// Select next model using async HealthManager
    pub async fn next_model(
        &self,
        failed_models: &[String],
        health_manager: &HealthManager,
        profiles: &[ModelProfile],
    ) -> Option<String> {
        // Build health status map
        let mut health_statuses = std::collections::HashMap::new();
        for model_id in &self.alternatives {
            let status = health_manager.get_status(model_id).await;
            health_statuses.insert(model_id.clone(), status);
        }

        // Build cost tier map
        let cost_tiers: std::collections::HashMap<String, CostTier> = profiles
            .iter()
            .map(|p| (p.id.clone(), p.cost_tier))
            .collect();

        self.next_model_sync(failed_models, &health_statuses, &cost_tiers)
    }

    /// Get the first healthy model in the chain (including primary)
    pub async fn first_healthy_model(&self, health_manager: &HealthManager) -> Option<String> {
        // Check primary first
        let primary_status = health_manager.get_status(&self.primary).await;
        if primary_status.can_call() {
            return Some(self.primary.clone());
        }

        // Check alternatives
        for alt in &self.alternatives {
            let status = health_manager.get_status(alt).await;
            if status.can_call() {
                return Some(alt.clone());
            }
        }

        None
    }
}

// =============================================================================
// Failover Configuration
// =============================================================================

/// Configuration for automatic failover chain building
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    /// Enable automatic failover
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default selection mode
    #[serde(default)]
    pub default_selection_mode: FailoverSelectionMode,

    /// Maximum alternatives to consider
    #[serde(default = "default_max_alternatives")]
    pub max_alternatives: usize,

    /// Require at least N shared capabilities
    #[serde(default = "default_min_shared_capabilities")]
    pub min_shared_capabilities: usize,

    /// Health weight for balanced mode
    #[serde(default = "default_health_weight")]
    pub health_weight: f64,

    /// Cost weight for balanced mode
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f64,
}

fn default_true() -> bool {
    true
}

fn default_max_alternatives() -> usize {
    5
}

fn default_min_shared_capabilities() -> usize {
    1
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_selection_mode: FailoverSelectionMode::HealthPriority,
            max_alternatives: 5,
            min_shared_capabilities: 1,
            health_weight: default_health_weight(),
            cost_weight: default_cost_weight(),
        }
    }
}

impl FailoverConfig {
    /// Build a failover chain using this configuration
    pub fn build_chain<'a>(
        &self,
        primary_id: &str,
        profiles: impl Iterator<Item = &'a ModelProfile>,
    ) -> Option<FailoverChain> {
        if !self.enabled {
            return Some(FailoverChain::new(primary_id));
        }

        let profiles: Vec<_> = profiles.collect();
        let primary = profiles.iter().find(|p| p.id == primary_id)?;
        let primary_caps: HashSet<_> = primary.capabilities.iter().cloned().collect();

        // Find alternatives with enough shared capabilities
        let mut alternatives: Vec<_> = profiles
            .iter()
            .filter(|p| p.id != primary_id)
            .filter(|p| {
                let shared = p
                    .capabilities
                    .iter()
                    .filter(|c| primary_caps.contains(c))
                    .count();
                shared >= self.min_shared_capabilities
            })
            .take(self.max_alternatives)
            .map(|p| p.id.clone())
            .collect();

        // Sort by cost tier initially
        alternatives.sort_by(|a, b| {
            let a_profile = profiles.iter().find(|p| p.id == *a);
            let b_profile = profiles.iter().find(|p| p.id == *b);
            match (a_profile, b_profile) {
                (Some(a), Some(b)) => a.cost_tier.cmp(&b.cost_tier),
                _ => std::cmp::Ordering::Equal,
            }
        });

        Some(FailoverChain {
            primary: primary_id.to_string(),
            alternatives,
            selection_mode: self.default_selection_mode,
            required_capabilities: primary.capabilities.clone(),
            health_weight: self.health_weight,
            cost_weight: self.cost_weight,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile::new("gpt-4o", "openai", "gpt-4o")
                .with_capabilities(vec![Capability::Reasoning, Capability::CodeGeneration])
                .with_cost_tier(CostTier::High),
            ModelProfile::new("claude-sonnet", "anthropic", "claude-sonnet")
                .with_capabilities(vec![Capability::Reasoning, Capability::CodeGeneration])
                .with_cost_tier(CostTier::Medium),
            ModelProfile::new("gpt-4o-mini", "openai", "gpt-4o-mini")
                .with_capabilities(vec![Capability::CodeGeneration, Capability::FastResponse])
                .with_cost_tier(CostTier::Low),
            ModelProfile::new("ollama-llama", "ollama", "llama3.2")
                .with_capabilities(vec![Capability::LocalPrivacy, Capability::FastResponse])
                .with_cost_tier(CostTier::Free)
                .as_local(),
        ]
    }

    #[test]
    fn test_failover_chain_new() {
        let chain = FailoverChain::new("gpt-4o");
        assert_eq!(chain.primary, "gpt-4o");
        assert!(chain.alternatives.is_empty());
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn test_failover_chain_builder() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec!["claude-sonnet".to_string(), "gpt-4o-mini".to_string()])
            .with_selection_mode(FailoverSelectionMode::CostPriority)
            .with_required_capabilities(vec![Capability::CodeGeneration]);

        assert_eq!(chain.alternatives.len(), 2);
        assert_eq!(chain.selection_mode, FailoverSelectionMode::CostPriority);
        assert!(!chain.is_empty());
        assert_eq!(chain.len(), 3);
    }

    #[test]
    fn test_failover_chain_all_models() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec!["claude-sonnet".to_string(), "gpt-4o-mini".to_string()]);

        let models = chain.all_models();
        assert_eq!(models, vec!["gpt-4o", "claude-sonnet", "gpt-4o-mini"]);
    }

    #[test]
    fn test_failover_chain_from_profiles() {
        let profiles = test_profiles();
        let chain = FailoverChain::from_profiles(
            "gpt-4o",
            profiles.iter(),
            FailoverSelectionMode::HealthPriority,
        )
        .unwrap();

        assert_eq!(chain.primary, "gpt-4o");
        // claude-sonnet and gpt-4o-mini share capabilities with gpt-4o
        assert!(chain.alternatives.contains(&"claude-sonnet".to_string()));
        assert!(chain.alternatives.contains(&"gpt-4o-mini".to_string()));
        // ollama-llama doesn't share capabilities
        assert!(!chain.alternatives.contains(&"ollama-llama".to_string()));
    }

    #[test]
    fn test_failover_chain_from_profiles_unknown_primary() {
        let profiles = test_profiles();
        let chain = FailoverChain::from_profiles(
            "unknown-model",
            profiles.iter(),
            FailoverSelectionMode::HealthPriority,
        );
        assert!(chain.is_none());
    }

    #[test]
    fn test_next_model_ordered() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec![
                "claude-sonnet".to_string(),
                "gpt-4o-mini".to_string(),
            ])
            .with_selection_mode(FailoverSelectionMode::Ordered);

        let health_statuses: HashMap<String, HealthStatus> = [
            ("claude-sonnet".to_string(), HealthStatus::Healthy),
            ("gpt-4o-mini".to_string(), HealthStatus::Healthy),
        ]
        .into_iter()
        .collect();

        let cost_tiers: HashMap<String, CostTier> = HashMap::new();

        let next = chain.next_model_sync(&[], &health_statuses, &cost_tiers);
        assert_eq!(next, Some("claude-sonnet".to_string())); // First in order
    }

    #[test]
    fn test_next_model_health_priority() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec![
                "claude-sonnet".to_string(),
                "gpt-4o-mini".to_string(),
            ])
            .with_selection_mode(FailoverSelectionMode::HealthPriority);

        let health_statuses: HashMap<String, HealthStatus> = [
            ("claude-sonnet".to_string(), HealthStatus::Degraded),
            ("gpt-4o-mini".to_string(), HealthStatus::Healthy),
        ]
        .into_iter()
        .collect();

        let cost_tiers: HashMap<String, CostTier> = HashMap::new();

        let next = chain.next_model_sync(&[], &health_statuses, &cost_tiers);
        assert_eq!(next, Some("gpt-4o-mini".to_string())); // Healthier
    }

    #[test]
    fn test_next_model_cost_priority() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec![
                "claude-sonnet".to_string(),
                "gpt-4o-mini".to_string(),
            ])
            .with_selection_mode(FailoverSelectionMode::CostPriority);

        let health_statuses: HashMap<String, HealthStatus> = [
            ("claude-sonnet".to_string(), HealthStatus::Healthy),
            ("gpt-4o-mini".to_string(), HealthStatus::Healthy),
        ]
        .into_iter()
        .collect();

        let cost_tiers: HashMap<String, CostTier> = [
            ("claude-sonnet".to_string(), CostTier::Medium),
            ("gpt-4o-mini".to_string(), CostTier::Low),
        ]
        .into_iter()
        .collect();

        let next = chain.next_model_sync(&[], &health_statuses, &cost_tiers);
        assert_eq!(next, Some("gpt-4o-mini".to_string())); // Cheaper
    }

    #[test]
    fn test_next_model_skips_failed() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec![
                "claude-sonnet".to_string(),
                "gpt-4o-mini".to_string(),
            ])
            .with_selection_mode(FailoverSelectionMode::Ordered);

        let health_statuses: HashMap<String, HealthStatus> = [
            ("claude-sonnet".to_string(), HealthStatus::Healthy),
            ("gpt-4o-mini".to_string(), HealthStatus::Healthy),
        ]
        .into_iter()
        .collect();

        let cost_tiers: HashMap<String, CostTier> = HashMap::new();
        let failed = vec!["claude-sonnet".to_string()];

        let next = chain.next_model_sync(&failed, &health_statuses, &cost_tiers);
        assert_eq!(next, Some("gpt-4o-mini".to_string())); // Skips failed
    }

    #[test]
    fn test_next_model_skips_unhealthy() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec![
                "claude-sonnet".to_string(),
                "gpt-4o-mini".to_string(),
            ])
            .with_selection_mode(FailoverSelectionMode::Ordered);

        let health_statuses: HashMap<String, HealthStatus> = [
            ("claude-sonnet".to_string(), HealthStatus::CircuitOpen),
            ("gpt-4o-mini".to_string(), HealthStatus::Healthy),
        ]
        .into_iter()
        .collect();

        let cost_tiers: HashMap<String, CostTier> = HashMap::new();

        let next = chain.next_model_sync(&[], &health_statuses, &cost_tiers);
        assert_eq!(next, Some("gpt-4o-mini".to_string())); // Skips unhealthy
    }

    #[test]
    fn test_next_model_all_exhausted() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec!["claude-sonnet".to_string()]);

        let health_statuses: HashMap<String, HealthStatus> = [
            ("claude-sonnet".to_string(), HealthStatus::CircuitOpen),
        ]
        .into_iter()
        .collect();

        let cost_tiers: HashMap<String, CostTier> = HashMap::new();

        let next = chain.next_model_sync(&[], &health_statuses, &cost_tiers);
        assert!(next.is_none());
    }

    #[test]
    fn test_failover_config_default() {
        let config = FailoverConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_selection_mode, FailoverSelectionMode::HealthPriority);
        assert_eq!(config.max_alternatives, 5);
    }

    #[test]
    fn test_failover_config_build_chain() {
        let profiles = test_profiles();
        let config = FailoverConfig::default();

        let chain = config.build_chain("gpt-4o", profiles.iter()).unwrap();
        assert_eq!(chain.primary, "gpt-4o");
        assert!(!chain.alternatives.is_empty());
    }

    #[test]
    fn test_failover_config_disabled() {
        let profiles = test_profiles();
        let config = FailoverConfig {
            enabled: false,
            ..Default::default()
        };

        let chain = config.build_chain("gpt-4o", profiles.iter()).unwrap();
        assert!(chain.alternatives.is_empty());
    }

    #[test]
    fn test_selection_mode_as_str() {
        assert_eq!(FailoverSelectionMode::Ordered.as_str(), "ordered");
        assert_eq!(FailoverSelectionMode::HealthPriority.as_str(), "health_priority");
        assert_eq!(FailoverSelectionMode::CostPriority.as_str(), "cost_priority");
        assert_eq!(FailoverSelectionMode::Balanced.as_str(), "balanced");
    }

    #[test]
    fn test_failover_chain_serialization() {
        let chain = FailoverChain::new("gpt-4o")
            .with_alternatives(vec!["claude-sonnet".to_string()])
            .with_selection_mode(FailoverSelectionMode::CostPriority);

        let json = serde_json::to_string(&chain).unwrap();
        assert!(json.contains("gpt-4o"));
        assert!(json.contains("cost_priority"));

        let parsed: FailoverChain = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.primary, "gpt-4o");
        assert_eq!(parsed.selection_mode, FailoverSelectionMode::CostPriority);
    }
}

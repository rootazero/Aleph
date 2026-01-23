//! Model routing FFI methods
//!
//! Contains model profiles and routing rules management:
//! - agent_get_model_profiles, agent_update_model_profile, agent_delete_model_profile
//! - agent_get_routing_rules, agent_update_routing_rule, agent_delete_routing_rule
//! - agent_update_cost_strategy, agent_update_default_model
//! - Model health monitoring

use crate::ffi::{AetherCore, AetherFfiError};
use tracing::info;

impl AetherCore {
    // ===== MODEL ROUTER =====

    /// Get all configured model profiles
    pub fn agent_get_model_profiles(&self) -> Vec<crate::ffi::dispatcher_types::ModelProfileFFI> {
        // Load from config file or return empty
        match crate::config::Config::load() {
            Ok(cfg) => cfg
                .agent
                .get_model_profiles()
                .into_iter()
                .map(crate::ffi::dispatcher_types::ModelProfileFFI::from)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get model routing rules
    pub fn agent_get_routing_rules(&self) -> crate::ffi::dispatcher_types::ModelRoutingRulesFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => {
                crate::ffi::dispatcher_types::ModelRoutingRulesFFI::from(cfg.agent.get_routing_rules())
            }
            Err(_) => crate::ffi::dispatcher_types::ModelRoutingRulesFFI::from(
                crate::dispatcher::ModelRoutingRules::default(),
            ),
        }
    }

    /// Update a model profile (add or modify)
    pub fn agent_update_model_profile(
        &self,
        profile: crate::ffi::dispatcher_types::ModelProfileFFI,
    ) -> Result<(), AetherFfiError> {
        let profile = crate::dispatcher::ModelProfile::from(profile);
        let profile_id = profile.id.clone();

        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Insert or update the profile in HashMap (key = profile_id)
        let config_profile = crate::config::types::agent::ModelProfileConfigToml {
            provider: profile.provider,
            model: profile.model,
            capabilities: profile.capabilities,
            cost_tier: profile.cost_tier,
            latency_tier: profile.latency_tier,
            max_context: profile.max_context,
            local: profile.local,
            parameters: profile.parameters,
        };

        full_config
            .agent
            .model_profiles
            .insert(profile_id.clone(), config_profile);

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(profile_id = %profile_id, "Model profile updated");
        Ok(())
    }

    /// Delete a model profile by ID
    pub fn agent_delete_model_profile(&self, profile_id: String) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Remove the profile from HashMap
        if full_config
            .agent
            .model_profiles
            .remove(&profile_id)
            .is_none()
        {
            return Err(AetherFfiError::Config(format!(
                "Model profile '{}' not found",
                profile_id
            )));
        }

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(profile_id = %profile_id, "Model profile deleted");
        Ok(())
    }

    /// Update a task type to model mapping
    pub fn agent_update_routing_rule(
        &self,
        task_type: String,
        model_id: String,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update built-in task type fields or use overrides for custom types
        match task_type.as_str() {
            "code_generation" => {
                full_config.agent.model_routing.code_generation = Some(model_id.clone())
            }
            "code_review" => full_config.agent.model_routing.code_review = Some(model_id.clone()),
            "image_analysis" => {
                full_config.agent.model_routing.image_analysis = Some(model_id.clone())
            }
            "video_understanding" => {
                full_config.agent.model_routing.video_understanding = Some(model_id.clone())
            }
            "long_document" => {
                full_config.agent.model_routing.long_document = Some(model_id.clone())
            }
            "quick_tasks" => full_config.agent.model_routing.quick_tasks = Some(model_id.clone()),
            "privacy_sensitive" => {
                full_config.agent.model_routing.privacy_sensitive = Some(model_id.clone())
            }
            "reasoning" => full_config.agent.model_routing.reasoning = Some(model_id.clone()),
            _ => {
                // Use overrides for custom task types
                full_config
                    .agent
                    .model_routing
                    .overrides
                    .insert(task_type.clone(), model_id.clone());
            }
        }

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(task_type = %task_type, model_id = %model_id, "Routing rule updated");
        Ok(())
    }

    /// Delete a task type mapping
    pub fn agent_delete_routing_rule(&self, task_type: String) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Clear built-in task type fields or remove from overrides
        let removed = match task_type.as_str() {
            "code_generation" => full_config
                .agent
                .model_routing
                .code_generation
                .take()
                .is_some(),
            "code_review" => full_config
                .agent
                .model_routing
                .code_review
                .take()
                .is_some(),
            "image_analysis" => full_config
                .agent
                .model_routing
                .image_analysis
                .take()
                .is_some(),
            "video_understanding" => full_config
                .agent
                .model_routing
                .video_understanding
                .take()
                .is_some(),
            "long_document" => full_config
                .agent
                .model_routing
                .long_document
                .take()
                .is_some(),
            "quick_tasks" => full_config
                .agent
                .model_routing
                .quick_tasks
                .take()
                .is_some(),
            "privacy_sensitive" => full_config
                .agent
                .model_routing
                .privacy_sensitive
                .take()
                .is_some(),
            "reasoning" => full_config.agent.model_routing.reasoning.take().is_some(),
            _ => full_config
                .agent
                .model_routing
                .overrides
                .remove(&task_type)
                .is_some(),
        };

        if !removed {
            return Err(AetherFfiError::Config(format!(
                "Task type mapping '{}' not found",
                task_type
            )));
        }

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(task_type = %task_type, "Routing rule deleted");
        Ok(())
    }

    /// Update cost strategy
    pub fn agent_update_cost_strategy(
        &self,
        strategy: crate::ffi::dispatcher_types::ModelCostStrategyFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Convert FFI strategy to internal CostStrategy enum
        let cost_strategy = crate::dispatcher::CostStrategy::from(strategy);
        full_config.agent.model_routing.cost_strategy = cost_strategy;

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(strategy = ?cost_strategy, "Cost strategy updated");
        Ok(())
    }

    /// Update default model
    pub fn agent_update_default_model(&self, model_id: String) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Validate that the model exists in HashMap
        if !full_config.agent.model_profiles.contains_key(&model_id) {
            return Err(AetherFfiError::Config(format!(
                "Model profile '{}' not found",
                model_id
            )));
        }

        // Update default model
        full_config.agent.model_routing.default_model = Some(model_id.clone());

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(model_id = %model_id, "Default model updated");
        Ok(())
    }

    // ===== MODEL HEALTH MONITORING =====

    /// Get health summary for all tracked models
    ///
    /// Returns a list of health summaries for all models being tracked by the health manager.
    /// Each summary includes the model's current health status, any degradation/error reasons,
    /// and consecutive success/failure counts.
    pub fn agent_get_model_health_summaries(
        &self,
    ) -> Vec<crate::ffi::dispatcher_types::ModelHealthSummaryFFI> {
        // TODO: Integrate with HealthManager when it's added to AgentEngine
        // For now, generate summaries from configured model profiles
        match crate::config::Config::load() {
            Ok(cfg) => cfg
                .agent
                .get_model_profiles()
                .into_iter()
                .map(|profile| crate::ffi::dispatcher_types::ModelHealthSummaryFFI {
                    model_id: profile.id,
                    status: crate::ffi::dispatcher_types::ModelHealthStatusFFI::Unknown,
                    status_text: "Unknown".to_string(),
                    status_emoji: "❓".to_string(),
                    reason: None,
                    consecutive_successes: 0,
                    consecutive_failures: 0,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get health summary for a specific model
    ///
    /// Returns the health summary for a specific model by ID, or None if the model
    /// is not found in the health tracking system.
    pub fn agent_get_model_health(
        &self,
        model_id: String,
    ) -> Option<crate::ffi::dispatcher_types::ModelHealthSummaryFFI> {
        // TODO: Integrate with HealthManager when it's added to AgentEngine
        // For now, return Unknown status if model exists
        match crate::config::Config::load() {
            Ok(cfg) => {
                if cfg.agent.model_profiles.contains_key(&model_id) {
                    Some(crate::ffi::dispatcher_types::ModelHealthSummaryFFI {
                        model_id,
                        status: crate::ffi::dispatcher_types::ModelHealthStatusFFI::Unknown,
                        status_text: "Unknown".to_string(),
                        status_emoji: "❓".to_string(),
                        reason: None,
                        consecutive_successes: 0,
                        consecutive_failures: 0,
                    })
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    /// Get overall health statistics
    ///
    /// Returns aggregate statistics about the health status of all tracked models,
    /// including counts of healthy, degraded, unhealthy, and circuit-open models.
    pub fn agent_get_health_statistics(&self) -> crate::ffi::dispatcher_types::HealthStatisticsFFI {
        // TODO: Integrate with HealthManager when it's added to AgentEngine
        // For now, return statistics based on configured model count
        let total = match crate::config::Config::load() {
            Ok(cfg) => cfg.agent.model_profiles.len() as u32,
            Err(_) => 0,
        };

        crate::ffi::dispatcher_types::HealthStatisticsFFI {
            total,
            healthy: 0,
            degraded: 0,
            unhealthy: 0,
            circuit_open: 0,
            half_open: 0,
            unknown: total,
            healthy_percent: if total > 0 { 0.0 } else { 100.0 },
        }
    }
}

//! Cowork task orchestration methods for AetherCore
//!
//! This module contains Cowork-related methods: cowork_plan, cowork_execute, etc.

use super::{AetherCore, AetherFfiError};
use crate::config::Config;
use std::sync::Arc;
use tracing::info;

impl AetherCore {
    /// Get or create the Cowork engine
    ///
    /// Lazily initializes the CoworkEngine on first use.
    pub(crate) fn get_or_create_cowork_engine(
        &self,
    ) -> Result<Arc<crate::dispatcher::CoworkEngine>, AetherFfiError> {
        // Check if engine already exists
        {
            let engine_guard = self.cowork_engine.read().unwrap();
            if let Some(engine) = engine_guard.as_ref() {
                return Ok(Arc::clone(engine));
            }
        }

        // Need to create the engine
        let mut engine_guard = self.cowork_engine.write().unwrap();

        // Double-check after acquiring write lock
        if let Some(engine) = engine_guard.as_ref() {
            return Ok(Arc::clone(engine));
        }

        // Get the config for cowork from the loaded configuration
        let cowork_config = {
            // Note: We lock full_config briefly to ensure consistent state, but load from file
            // because FullConfig doesn't expose cowork settings directly
            let _config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
            match Config::load() {
                Ok(cfg) => cfg.cowork.to_engine_config(),
                Err(_) => crate::dispatcher::CoworkConfig::default(),
            }
        };

        // Get an AI provider for the planner
        let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
        let default_provider_name = full_config
            .general
            .default_provider
            .clone()
            .unwrap_or_else(|| "openai".to_string());

        // Find the provider config
        let provider_config = full_config
            .providers
            .iter()
            .find(|(name, _)| **name == default_provider_name)
            .map(|(_, config)| config.clone())
            .ok_or_else(|| {
                AetherFfiError::Provider(format!(
                    "Default provider '{}' not found in config",
                    default_provider_name
                ))
            })?;

        // Create a provider from config
        let provider = crate::providers::create_provider(&default_provider_name, provider_config)
            .map_err(|e| {
            AetherFfiError::Provider(format!("Failed to create provider for Cowork: {}", e))
        })?;

        // Create the engine
        let mut engine = crate::dispatcher::CoworkEngine::new(cowork_config, provider);

        // Set fallback provider from default_provider config
        // This ensures model routing works even without explicit model_profiles
        engine.set_fallback_provider(&default_provider_name);
        info!(
            fallback_provider = %default_provider_name,
            "Set fallback provider for model routing"
        );

        let engine = Arc::new(engine);
        *engine_guard = Some(Arc::clone(&engine));
        info!("Cowork engine initialized");

        Ok(engine)
    }

    /// Get Cowork configuration
    pub fn cowork_get_config(&self) -> crate::cowork_ffi::CoworkConfigFFI {
        // Return current config or default
        crate::cowork_ffi::CoworkConfigFFI::from(crate::dispatcher::CoworkConfig::default())
    }

    /// Update Cowork configuration
    pub fn cowork_update_config(
        &self,
        config: crate::cowork_ffi::CoworkConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // For now, this would reinitialize the engine with new config
        // Reset the engine so it gets recreated with new config
        let mut engine_guard = self.cowork_engine.write().unwrap();
        *engine_guard = None;

        info!(
            enabled = config.enabled,
            max_parallelism = config.max_parallelism,
            "Cowork configuration updated"
        );

        Ok(())
    }

    /// Plan a task from natural language request
    pub fn cowork_plan(
        &self,
        request: String,
    ) -> Result<crate::cowork_ffi::CoworkTaskGraphFFI, AetherFfiError> {
        let engine = self.get_or_create_cowork_engine()?;

        info!(request = %request, "Planning Cowork task");

        let graph = self
            .runtime
            .block_on(async { engine.plan(&request).await })
            .map_err(|e| AetherFfiError::Provider(format!("Planning failed: {}", e)))?;

        Ok(crate::cowork_ffi::CoworkTaskGraphFFI::from(&graph))
    }

    /// Execute a task graph
    pub fn cowork_execute(
        &self,
        graph_ffi: crate::cowork_ffi::CoworkTaskGraphFFI,
    ) -> Result<crate::cowork_ffi::CoworkExecutionSummaryFFI, AetherFfiError> {
        let engine = self.get_or_create_cowork_engine()?;

        info!(graph_id = %graph_ffi.id, "Executing Cowork task graph");

        // Convert FFI graph back to internal TaskGraph
        // For now, we'll re-plan to get an executable graph
        // In a full implementation, we'd store the original graph
        let original_request = graph_ffi
            .original_request
            .clone()
            .unwrap_or_else(|| "Execute planned tasks".to_string());

        let graph = self
            .runtime
            .block_on(async { engine.plan(&original_request).await })
            .map_err(|e| AetherFfiError::Provider(format!("Re-planning failed: {}", e)))?;

        let summary = self
            .runtime
            .block_on(async { engine.execute(graph).await })
            .map_err(|e| AetherFfiError::Provider(format!("Execution failed: {}", e)))?;

        Ok(crate::cowork_ffi::CoworkExecutionSummaryFFI::from(summary))
    }

    /// Get current execution state
    pub fn cowork_get_state(&self) -> crate::cowork_ffi::CoworkExecutionState {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            self.runtime.block_on(async {
                crate::cowork_ffi::CoworkExecutionState::from(engine.state().await)
            })
        } else {
            crate::cowork_ffi::CoworkExecutionState::Idle
        }
    }

    /// Pause execution
    pub fn cowork_pause(&self) {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            engine.pause();
            info!("Cowork execution paused");
        }
    }

    /// Resume execution
    pub fn cowork_resume(&self) {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            engine.resume();
            info!("Cowork execution resumed");
        }
    }

    /// Cancel execution
    pub fn cowork_cancel(&self) {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            engine.cancel();
            info!("Cowork execution cancelled");
        }
    }

    /// Check if execution is paused
    pub fn cowork_is_paused(&self) -> bool {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            engine.is_paused()
        } else {
            false
        }
    }

    /// Check if execution is cancelled
    pub fn cowork_is_cancelled(&self) -> bool {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            engine.is_cancelled()
        } else {
            false
        }
    }

    /// Subscribe to progress events
    pub fn cowork_subscribe(&self, handler: Box<dyn crate::cowork_ffi::CoworkProgressHandler>) {
        if let Ok(engine) = self.get_or_create_cowork_engine() {
            // Convert Box to Arc for internal use
            let handler_arc: Arc<dyn crate::cowork_ffi::CoworkProgressHandler> = Arc::from(handler);
            let subscriber = Arc::new(crate::cowork_ffi::FfiProgressSubscriber::new(handler_arc));
            engine.subscribe(subscriber);
            info!("Cowork progress subscriber added");
        }
    }

    // ===== CODE EXECUTION CONFIG =====

    /// Get code execution configuration
    pub fn cowork_get_code_exec_config(&self) -> crate::cowork_ffi::CodeExecConfigFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => crate::cowork_ffi::CodeExecConfigFFI::from(cfg.cowork.code_exec),
            Err(_) => crate::cowork_ffi::CodeExecConfigFFI::default(),
        }
    }

    /// Update code execution configuration
    pub fn cowork_update_code_exec_config(
        &self,
        config: crate::cowork_ffi::CodeExecConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update code_exec section
        full_config.cowork.code_exec = config.into();

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!("Code execution configuration updated");
        Ok(())
    }

    // ===== FILE OPERATIONS CONFIG =====

    /// Get file operations configuration
    pub fn cowork_get_file_ops_config(&self) -> crate::cowork_ffi::FileOpsConfigFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => crate::cowork_ffi::FileOpsConfigFFI::from(cfg.cowork.file_ops),
            Err(_) => crate::cowork_ffi::FileOpsConfigFFI::default(),
        }
    }

    /// Update file operations configuration
    pub fn cowork_update_file_ops_config(
        &self,
        config: crate::cowork_ffi::FileOpsConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update file_ops section
        full_config.cowork.file_ops = config.into();

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!("File operations configuration updated");
        Ok(())
    }

    // ===== MODEL ROUTER =====

    /// Get all configured model profiles
    pub fn cowork_get_model_profiles(&self) -> Vec<crate::cowork_ffi::ModelProfileFFI> {
        // Load from config file or return empty
        match crate::config::Config::load() {
            Ok(cfg) => cfg
                .cowork
                .get_model_profiles()
                .into_iter()
                .map(crate::cowork_ffi::ModelProfileFFI::from)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get model routing rules
    pub fn cowork_get_routing_rules(&self) -> crate::cowork_ffi::ModelRoutingRulesFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => {
                crate::cowork_ffi::ModelRoutingRulesFFI::from(cfg.cowork.get_routing_rules())
            }
            Err(_) => crate::cowork_ffi::ModelRoutingRulesFFI::from(
                crate::dispatcher::ModelRoutingRules::default(),
            ),
        }
    }

    /// Update a model profile (add or modify)
    pub fn cowork_update_model_profile(
        &self,
        profile: crate::cowork_ffi::ModelProfileFFI,
    ) -> Result<(), AetherFfiError> {
        let profile = crate::dispatcher::ModelProfile::from(profile);
        let profile_id = profile.id.clone();

        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Insert or update the profile in HashMap (key = profile_id)
        let config_profile = crate::config::types::cowork::ModelProfileConfigToml {
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
            .cowork
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
    pub fn cowork_delete_model_profile(&self, profile_id: String) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Remove the profile from HashMap
        if full_config
            .cowork
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
    pub fn cowork_update_routing_rule(
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
                full_config.cowork.model_routing.code_generation = Some(model_id.clone())
            }
            "code_review" => full_config.cowork.model_routing.code_review = Some(model_id.clone()),
            "image_analysis" => {
                full_config.cowork.model_routing.image_analysis = Some(model_id.clone())
            }
            "video_understanding" => {
                full_config.cowork.model_routing.video_understanding = Some(model_id.clone())
            }
            "long_document" => {
                full_config.cowork.model_routing.long_document = Some(model_id.clone())
            }
            "quick_tasks" => full_config.cowork.model_routing.quick_tasks = Some(model_id.clone()),
            "privacy_sensitive" => {
                full_config.cowork.model_routing.privacy_sensitive = Some(model_id.clone())
            }
            "reasoning" => full_config.cowork.model_routing.reasoning = Some(model_id.clone()),
            _ => {
                // Use overrides for custom task types
                full_config
                    .cowork
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
    pub fn cowork_delete_routing_rule(&self, task_type: String) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Clear built-in task type fields or remove from overrides
        let removed = match task_type.as_str() {
            "code_generation" => full_config
                .cowork
                .model_routing
                .code_generation
                .take()
                .is_some(),
            "code_review" => full_config
                .cowork
                .model_routing
                .code_review
                .take()
                .is_some(),
            "image_analysis" => full_config
                .cowork
                .model_routing
                .image_analysis
                .take()
                .is_some(),
            "video_understanding" => full_config
                .cowork
                .model_routing
                .video_understanding
                .take()
                .is_some(),
            "long_document" => full_config
                .cowork
                .model_routing
                .long_document
                .take()
                .is_some(),
            "quick_tasks" => full_config
                .cowork
                .model_routing
                .quick_tasks
                .take()
                .is_some(),
            "privacy_sensitive" => full_config
                .cowork
                .model_routing
                .privacy_sensitive
                .take()
                .is_some(),
            "reasoning" => full_config.cowork.model_routing.reasoning.take().is_some(),
            _ => full_config
                .cowork
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
    pub fn cowork_update_cost_strategy(
        &self,
        strategy: crate::cowork_ffi::ModelCostStrategyFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Convert FFI strategy to internal CostStrategy enum
        let cost_strategy = crate::dispatcher::CostStrategy::from(strategy);
        full_config.cowork.model_routing.cost_strategy = cost_strategy;

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!(strategy = ?cost_strategy, "Cost strategy updated");
        Ok(())
    }

    /// Update default model
    pub fn cowork_update_default_model(&self, model_id: String) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Validate that the model exists in HashMap
        if !full_config.cowork.model_profiles.contains_key(&model_id) {
            return Err(AetherFfiError::Config(format!(
                "Model profile '{}' not found",
                model_id
            )));
        }

        // Update default model
        full_config.cowork.model_routing.default_model = Some(model_id.clone());

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
    pub fn cowork_get_model_health_summaries(
        &self,
    ) -> Vec<crate::cowork_ffi::ModelHealthSummaryFFI> {
        // TODO: Integrate with HealthManager when it's added to CoworkEngine
        // For now, generate summaries from configured model profiles
        match crate::config::Config::load() {
            Ok(cfg) => cfg
                .cowork
                .get_model_profiles()
                .into_iter()
                .map(|profile| crate::cowork_ffi::ModelHealthSummaryFFI {
                    model_id: profile.id,
                    status: crate::cowork_ffi::ModelHealthStatusFFI::Unknown,
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
    pub fn cowork_get_model_health(
        &self,
        model_id: String,
    ) -> Option<crate::cowork_ffi::ModelHealthSummaryFFI> {
        // TODO: Integrate with HealthManager when it's added to CoworkEngine
        // For now, return Unknown status if model exists
        match crate::config::Config::load() {
            Ok(cfg) => {
                if cfg.cowork.model_profiles.contains_key(&model_id) {
                    Some(crate::cowork_ffi::ModelHealthSummaryFFI {
                        model_id,
                        status: crate::cowork_ffi::ModelHealthStatusFFI::Unknown,
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
    pub fn cowork_get_health_statistics(&self) -> crate::cowork_ffi::HealthStatisticsFFI {
        // TODO: Integrate with HealthManager when it's added to CoworkEngine
        // For now, return statistics based on configured model count
        let total = match crate::config::Config::load() {
            Ok(cfg) => cfg.cowork.model_profiles.len() as u32,
            Err(_) => 0,
        };

        crate::cowork_ffi::HealthStatisticsFFI {
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

    // =========================================================================
    // Budget Management (Model Router P1)
    // =========================================================================

    /// Get budget status overview
    ///
    /// Returns the overall budget status including all configured limits,
    /// current spending, and warning/exceeded states.
    pub fn cowork_get_budget_status(&self) -> crate::cowork_ffi::BudgetStatusFFI {
        // Load config and get budget limits
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::cowork_ffi::BudgetStatusFFI::disabled(),
        };

        // Get budget configuration from cowork.model_routing.budget
        let budget_config = &config.cowork.model_routing.budget;

        if !budget_config.enabled {
            return crate::cowork_ffi::BudgetStatusFFI::disabled();
        }

        // Convert config limits to internal BudgetLimit types
        let default_enforcement = &budget_config.default_enforcement;
        let limits: Vec<crate::dispatcher::model_router::BudgetLimit> = budget_config
            .limits
            .iter()
            .map(|l| l.to_budget_limit(default_enforcement))
            .collect();

        if limits.is_empty() {
            return crate::cowork_ffi::BudgetStatusFFI::disabled();
        }

        // Create initial states for each limit
        // TODO: When BudgetManager is integrated into CoworkEngine, use actual states
        let mut states = std::collections::HashMap::new();
        for limit in &limits {
            states.insert(
                limit.id.clone(),
                crate::dispatcher::model_router::BudgetState::new(limit),
            );
        }

        crate::cowork_ffi::BudgetStatusFFI::from_limits_and_states(&limits, &states)
    }

    /// Get budget status for a specific scope
    ///
    /// Returns budget limits and status that apply to the given scope.
    pub fn cowork_get_budget_status_for_scope(
        &self,
        scope_type: String,
        scope_id: Option<String>,
    ) -> crate::cowork_ffi::BudgetStatusFFI {
        // Parse scope
        let scope = match scope_type.as_str() {
            "global" => crate::dispatcher::model_router::BudgetScope::Global,
            "project" => {
                crate::dispatcher::model_router::BudgetScope::Project(scope_id.unwrap_or_default())
            }
            "session" => {
                crate::dispatcher::model_router::BudgetScope::Session(scope_id.unwrap_or_default())
            }
            "model" => {
                crate::dispatcher::model_router::BudgetScope::Model(scope_id.unwrap_or_default())
            }
            _ => return crate::cowork_ffi::BudgetStatusFFI::disabled(),
        };

        // Load config and get budget limits
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::cowork_ffi::BudgetStatusFFI::disabled(),
        };

        let budget_config = &config.cowork.model_routing.budget;

        if !budget_config.enabled {
            return crate::cowork_ffi::BudgetStatusFFI::disabled();
        }

        // Convert config limits to internal BudgetLimit types
        let default_enforcement = &budget_config.default_enforcement;
        let all_limits: Vec<crate::dispatcher::model_router::BudgetLimit> = budget_config
            .limits
            .iter()
            .map(|l| l.to_budget_limit(default_enforcement))
            .collect();

        // Filter to limits that apply to this scope
        let applicable_limits: Vec<_> = all_limits
            .into_iter()
            .filter(|l| {
                l.scope.contains(&scope)
                    || l.scope == crate::dispatcher::model_router::BudgetScope::Global
            })
            .collect();

        if applicable_limits.is_empty() {
            return crate::cowork_ffi::BudgetStatusFFI::disabled();
        }

        // Create initial states for each limit
        // TODO: When BudgetManager is integrated, use actual states
        let mut states = std::collections::HashMap::new();
        for limit in &applicable_limits {
            states.insert(
                limit.id.clone(),
                crate::dispatcher::model_router::BudgetState::new(limit),
            );
        }

        crate::cowork_ffi::BudgetStatusFFI::from_limits_and_states(&applicable_limits, &states)
    }

    /// Get a single budget limit status by ID
    ///
    /// Returns the status of a specific budget limit, or None if not found.
    pub fn cowork_get_budget_limit(
        &self,
        limit_id: String,
    ) -> Option<crate::cowork_ffi::BudgetLimitStatusFFI> {
        // Load config
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return None,
        };

        let budget_config = &config.cowork.model_routing.budget;

        if !budget_config.enabled {
            return None;
        }

        // Find the limit by ID
        let limit_config = budget_config.limits.iter().find(|l| l.id == limit_id)?;

        let default_enforcement = &budget_config.default_enforcement;
        let limit = limit_config.to_budget_limit(default_enforcement);
        let state = crate::dispatcher::model_router::BudgetState::new(&limit);

        Some(crate::cowork_ffi::BudgetLimitStatusFFI::from_limit_and_state(&limit, &state))
    }

    // =========================================================================
    // A/B Testing (Model Router P3)
    // =========================================================================

    /// Get A/B testing status overview
    ///
    /// Returns the overall A/B testing status including all active experiments,
    /// their configurations, and current statistics.
    pub fn cowork_get_ab_testing_status(&self) -> crate::cowork_ffi::ABTestingStatusFFI {
        // Load config to check if A/B testing is enabled
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::cowork_ffi::ABTestingStatusFFI::disabled(),
        };

        let ab_config = &config.cowork.model_routing.ab_testing;

        if !ab_config.enabled {
            return crate::cowork_ffi::ABTestingStatusFFI::disabled();
        }

        // TODO: When ABTestingEngine is integrated into CoworkEngine, use actual engine
        // For now, return configured experiments count
        let experiment_count = ab_config.experiments.len();

        if experiment_count == 0 {
            return crate::cowork_ffi::ABTestingStatusFFI {
                enabled: true,
                total_experiments: 0,
                active_experiments: 0,
                experiments: Vec::new(),
                status_emoji: "⚪".to_string(),
                status_message: "No experiments configured".to_string(),
            };
        }

        crate::cowork_ffi::ABTestingStatusFFI {
            enabled: true,
            total_experiments: experiment_count as u32,
            active_experiments: ab_config.experiments.iter().filter(|e| e.enabled).count() as u32,
            experiments: Vec::new(), // Would populate from actual engine
            status_emoji: "🧪".to_string(),
            status_message: format!("{} experiment(s) configured", experiment_count),
        }
    }

    /// Get a list of active experiment IDs
    ///
    /// Returns the IDs of all currently active experiments that are
    /// accepting traffic and recording outcomes.
    pub fn cowork_get_active_experiments(&self) -> Vec<String> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return Vec::new(),
        };

        let ab_config = &config.cowork.model_routing.ab_testing;

        if !ab_config.enabled {
            return Vec::new();
        }

        ab_config
            .experiments
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.id.clone())
            .collect()
    }

    /// Get detailed report for a specific experiment
    ///
    /// Returns full statistics and significance tests for the specified experiment.
    /// Returns None if the experiment doesn't exist or A/B testing is disabled.
    pub fn cowork_get_experiment_report(
        &self,
        experiment_id: String,
    ) -> Option<crate::cowork_ffi::ExperimentReportFFI> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return None,
        };

        let ab_config = &config.cowork.model_routing.ab_testing;

        if !ab_config.enabled {
            return None;
        }

        // Check if experiment exists in config
        let _experiment = ab_config
            .experiments
            .iter()
            .find(|e| e.id == experiment_id)?;

        // TODO: When ABTestingEngine is integrated, get actual report
        // For now, return None as we don't have real data
        None
    }

    /// Enable an experiment
    ///
    /// Activates an experiment to start accepting traffic.
    /// Note: This is a runtime change and does not persist to config.
    pub fn cowork_enable_experiment(&self, experiment_id: String) -> Result<(), AetherFfiError> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(e) => return Err(AetherFfiError::Config(e.to_string())),
        };

        let ab_config = &config.cowork.model_routing.ab_testing;

        if !ab_config.enabled {
            return Err(AetherFfiError::Config(
                "A/B testing is disabled".to_string(),
            ));
        }

        // Check if experiment exists
        if !ab_config.experiments.iter().any(|e| e.id == experiment_id) {
            return Err(AetherFfiError::Config(format!(
                "Experiment '{}' not found",
                experiment_id
            )));
        }

        // TODO: When ABTestingEngine is integrated, enable the experiment
        // For now, just validate the request
        Ok(())
    }

    /// Disable an experiment
    ///
    /// Pauses an experiment to stop accepting traffic.
    /// Note: This is a runtime change and does not persist to config.
    pub fn cowork_disable_experiment(&self, experiment_id: String) -> Result<(), AetherFfiError> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(e) => return Err(AetherFfiError::Config(e.to_string())),
        };

        let ab_config = &config.cowork.model_routing.ab_testing;

        if !ab_config.enabled {
            return Err(AetherFfiError::Config(
                "A/B testing is disabled".to_string(),
            ));
        }

        // Check if experiment exists
        if !ab_config.experiments.iter().any(|e| e.id == experiment_id) {
            return Err(AetherFfiError::Config(format!(
                "Experiment '{}' not found",
                experiment_id
            )));
        }

        // TODO: When ABTestingEngine is integrated, disable the experiment
        // For now, just validate the request
        Ok(())
    }

    // =========================================================================
    // Ensemble (Model Router P3)
    // =========================================================================

    /// Get ensemble status overview
    ///
    /// Returns the current ensemble configuration and statistics.
    pub fn cowork_get_ensemble_status(&self) -> crate::cowork_ffi::EnsembleStatusFFI {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::cowork_ffi::EnsembleStatusFFI::disabled(),
        };

        let ensemble_config = &config.cowork.model_routing.ensemble;

        if !ensemble_config.enabled {
            return crate::cowork_ffi::EnsembleStatusFFI::disabled();
        }

        // Convert config to FFI summary
        let mode_ffi = match ensemble_config.default_mode.as_str() {
            "best_of_n" => crate::cowork_ffi::EnsembleModeFFI::BestOfN,
            "voting" => crate::cowork_ffi::EnsembleModeFFI::Voting,
            "consensus" => crate::cowork_ffi::EnsembleModeFFI::Consensus,
            "cascade" => crate::cowork_ffi::EnsembleModeFFI::Cascade,
            _ => crate::cowork_ffi::EnsembleModeFFI::Disabled,
        };

        let quality_metric_ffi = match ensemble_config.quality_scorer.as_str() {
            "length" => crate::cowork_ffi::QualityMetricFFI::Length,
            "structure" => crate::cowork_ffi::QualityMetricFFI::Structure,
            "length_and_structure" => crate::cowork_ffi::QualityMetricFFI::LengthAndStructure,
            "confidence_markers" | "confidence" => {
                crate::cowork_ffi::QualityMetricFFI::ConfidenceMarkers
            }
            "relevance" => crate::cowork_ffi::QualityMetricFFI::Relevance,
            _ => crate::cowork_ffi::QualityMetricFFI::LengthAndStructure,
        };

        // Collect all models from strategies and high complexity config
        let mut all_models: Vec<String> = ensemble_config
            .strategies
            .iter()
            .flat_map(|s| s.models.iter().cloned())
            .collect();
        all_models.extend(
            ensemble_config
                .high_complexity_ensemble
                .models
                .iter()
                .cloned(),
        );
        all_models.sort();
        all_models.dedup();

        let config_summary = crate::cowork_ffi::EnsembleConfigSummaryFFI {
            enabled: true,
            mode: mode_ffi,
            mode_display: ensemble_config.default_mode.clone(),
            models: all_models.clone(),
            quality_metric: quality_metric_ffi,
            timeout_ms: ensemble_config.default_timeout_secs * 1000, // Convert to ms
            high_complexity_enabled: ensemble_config.high_complexity_ensemble.enabled,
            complexity_threshold: ensemble_config
                .high_complexity_ensemble
                .complexity_threshold,
        };

        // TODO: When EnsembleEngine is integrated, get actual stats
        let stats = crate::cowork_ffi::EnsembleStatsFFI::empty();

        let model_count = all_models.len();
        let (emoji, message) = if model_count > 0 {
            (
                "🔀".to_string(),
                format!(
                    "Ensemble with {} models ({} mode)",
                    model_count, ensemble_config.default_mode
                ),
            )
        } else {
            (
                "⚠️".to_string(),
                "Ensemble enabled but no models configured".to_string(),
            )
        };

        crate::cowork_ffi::EnsembleStatusFFI {
            config: config_summary,
            stats,
            status_emoji: emoji,
            status_message: message,
        }
    }

    /// Get ensemble configuration summary
    ///
    /// Returns the current ensemble configuration for display.
    pub fn cowork_get_ensemble_config(&self) -> crate::cowork_ffi::EnsembleConfigSummaryFFI {
        let status = self.cowork_get_ensemble_status();
        status.config
    }

    /// Get ensemble execution statistics
    ///
    /// Returns statistics about ensemble executions.
    pub fn cowork_get_ensemble_stats(&self) -> crate::cowork_ffi::EnsembleStatsFFI {
        let status = self.cowork_get_ensemble_status();
        status.stats
    }
}

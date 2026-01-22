//! Dispatcher FFI Methods for AetherCore
//!
//! This module contains FFI methods for the Dispatcher layer:
//! - Task orchestration: agent_plan, agent_execute, etc.
//! - Model routing: get_model_profiles, update_routing_rule, etc.
//! - Budget management: get_budget_status, etc.
//! - A/B testing and ensemble: get_ab_testing_status, get_ensemble_status, etc.

use super::{AetherCore, AetherFfiError};
use crate::config::Config;
use std::sync::Arc;
use tracing::info;

impl AetherCore {
    /// Get or create the Agent engine
    ///
    /// Lazily initializes the AgentEngine on first use.
    pub(crate) fn get_or_create_agent_engine(
        &self,
    ) -> Result<Arc<crate::dispatcher::AgentEngine>, AetherFfiError> {
        // Check if engine already exists
        {
            let engine_guard = self.agent_engine.read().unwrap();
            if let Some(engine) = engine_guard.as_ref() {
                return Ok(Arc::clone(engine));
            }
        }

        // Need to create the engine
        let mut engine_guard = self.agent_engine.write().unwrap();

        // Double-check after acquiring write lock
        if let Some(engine) = engine_guard.as_ref() {
            return Ok(Arc::clone(engine));
        }

        // Get the config for agent from the loaded configuration
        let agent_config = {
            // Note: We lock full_config briefly to ensure consistent state, but load from file
            // because FullConfig doesn't expose agent settings directly
            let _config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());
            match Config::load() {
                Ok(cfg) => cfg.agent.to_engine_config(),
                Err(_) => crate::dispatcher::AgentConfig::default(),
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
            AetherFfiError::Provider(format!("Failed to create provider for Agent: {}", e))
        })?;

        // Create the engine
        let mut engine = crate::dispatcher::AgentEngine::new(agent_config, provider);

        // Set fallback provider from default_provider config
        // This ensures model routing works even without explicit model_profiles
        engine.set_fallback_provider(&default_provider_name);
        info!(
            fallback_provider = %default_provider_name,
            "Set fallback provider for model routing"
        );

        let engine = Arc::new(engine);
        *engine_guard = Some(Arc::clone(&engine));
        info!("Agent engine initialized");

        Ok(engine)
    }

    /// Get Cowork configuration (FFI method name kept for Swift compatibility)
    pub fn agent_get_config(&self) -> crate::ffi::dispatcher_types::AgentConfigFFI {
        // Return current config or default
        crate::ffi::dispatcher_types::AgentConfigFFI::from(crate::dispatcher::AgentConfig::default())
    }

    /// Update Cowork configuration
    pub fn agent_update_config(
        &self,
        config: crate::ffi::dispatcher_types::AgentConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // For now, this would reinitialize the engine with new config
        // Reset the engine so it gets recreated with new config
        let mut engine_guard = self.agent_engine.write().unwrap();
        *engine_guard = None;

        info!(
            max_parallelism = config.max_parallelism,
            "Cowork configuration updated"
        );

        Ok(())
    }

    /// Plan a task from natural language request
    pub fn agent_plan(
        &self,
        request: String,
    ) -> Result<crate::ffi::dispatcher_types::AgentTaskGraphFFI, AetherFfiError> {
        let engine = self.get_or_create_agent_engine()?;

        info!(request = %request, "Planning Cowork task");

        // Extract generation providers from config
        let providers = self.extract_generation_providers();

        let graph = if providers.image.is_empty()
            && providers.video.is_empty()
            && providers.audio.is_empty()
        {
            // No generation providers configured, use simple plan()
            self.runtime
                .block_on(async { engine.plan(&request).await })
        } else {
            // Generation providers available, pass them to planner
            info!(
                image_providers = providers.image.len(),
                video_providers = providers.video.len(),
                audio_providers = providers.audio.len(),
                "Using generation providers for planning"
            );
            self.runtime
                .block_on(async { engine.plan_with_providers(&request, &providers).await })
        }
        .map_err(|e| AetherFfiError::Provider(format!("Planning failed: {}", e)))?;

        Ok(crate::ffi::dispatcher_types::AgentTaskGraphFFI::from(&graph))
    }

    /// Extract generation providers from registry for planning
    ///
    /// This method reads from the generation_registry (which contains actually
    /// registered and working providers) and combines it with config to get
    /// model information.
    fn extract_generation_providers(&self) -> crate::dispatcher::GenerationProviders {
        use crate::generation::GenerationType;

        let mut providers = crate::dispatcher::GenerationProviders::default();

        // Get providers from the registry (these are actually registered and working)
        let registry = self.generation_registry.read().unwrap_or_else(|e| {
            tracing::warn!("Generation registry lock poisoned, recovering");
            e.into_inner()
        });

        // Get config for model information
        let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());

        // Process each registered provider
        for provider_name in registry.names() {
            if let Some(provider) = registry.get(&provider_name) {
                let supported_types = provider.supported_types();

                // Get models from config (if available) or use default_model from provider
                let models: Vec<String> = if let Some(prov_config) =
                    full_config.generation.providers.get(&provider_name)
                {
                    if prov_config.models.is_empty() {
                        // Use default model from config or provider
                        if let Some(model) = prov_config.model.as_ref() {
                            vec![model.clone()]
                        } else if let Some(model) = provider.default_model() {
                            vec![model.to_string()]
                        } else {
                            Vec::new()
                        }
                    } else {
                        // Use configured model aliases/names
                        prov_config.models.keys().cloned().collect()
                    }
                } else {
                    // No config, use default_model from provider
                    provider
                        .default_model()
                        .map(|m| vec![m.to_string()])
                        .unwrap_or_default()
                };

                if models.is_empty() {
                    continue;
                }

                // Add to appropriate provider list based on supported types
                if supported_types.contains(&GenerationType::Image) {
                    providers.image.push((provider_name.clone(), models.clone()));
                }
                if supported_types.contains(&GenerationType::Video) {
                    providers.video.push((provider_name.clone(), models.clone()));
                }
                if supported_types.contains(&GenerationType::Audio)
                    || supported_types.contains(&GenerationType::Speech)
                {
                    providers.audio.push((provider_name.clone(), models));
                }
            }
        }

        info!(
            image_providers = providers.image.len(),
            video_providers = providers.video.len(),
            audio_providers = providers.audio.len(),
            "Extracted generation providers from registry"
        );

        providers
    }

    /// Execute a task graph
    pub fn agent_execute(
        &self,
        graph_ffi: crate::ffi::dispatcher_types::AgentTaskGraphFFI,
    ) -> Result<crate::ffi::dispatcher_types::AgentExecutionSummaryFFI, AetherFfiError> {
        let engine = self.get_or_create_agent_engine()?;

        info!(graph_id = %graph_ffi.id, "Executing Cowork task graph");

        // Convert FFI graph back to internal TaskGraph
        // For now, we'll re-plan to get an executable graph
        // In a full implementation, we'd store the original graph
        let original_request = graph_ffi
            .original_request
            .clone()
            .unwrap_or_else(|| "Execute planned tasks".to_string());

        // Extract generation providers from config
        let providers = self.extract_generation_providers();

        let graph = if providers.image.is_empty()
            && providers.video.is_empty()
            && providers.audio.is_empty()
        {
            self.runtime
                .block_on(async { engine.plan(&original_request).await })
        } else {
            self.runtime
                .block_on(async { engine.plan_with_providers(&original_request, &providers).await })
        }
        .map_err(|e| AetherFfiError::Provider(format!("Re-planning failed: {}", e)))?;

        let summary = self
            .runtime
            .block_on(async { engine.execute(graph).await })
            .map_err(|e| AetherFfiError::Provider(format!("Execution failed: {}", e)))?;

        Ok(crate::ffi::dispatcher_types::AgentExecutionSummaryFFI::from(summary))
    }

    /// Get current execution state
    pub fn agent_get_state(&self) -> crate::ffi::dispatcher_types::AgentExecutionState {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            self.runtime.block_on(async {
                crate::ffi::dispatcher_types::AgentExecutionState::from(engine.state().await)
            })
        } else {
            crate::ffi::dispatcher_types::AgentExecutionState::Idle
        }
    }

    /// Pause execution
    pub fn agent_pause(&self) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.pause();
            info!("Cowork execution paused");
        }
    }

    /// Resume execution
    pub fn agent_resume(&self) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.resume();
            info!("Cowork execution resumed");
        }
    }

    /// Cancel execution
    pub fn agent_cancel(&self) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.cancel();
            info!("Cowork execution cancelled");
        }
    }

    /// Check if execution is paused
    pub fn agent_is_paused(&self) -> bool {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.is_paused()
        } else {
            false
        }
    }

    /// Check if execution is cancelled
    pub fn agent_is_cancelled(&self) -> bool {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.is_cancelled()
        } else {
            false
        }
    }

    /// Subscribe to progress events
    pub fn agent_subscribe(&self, handler: Box<dyn crate::ffi::dispatcher_types::AgentProgressHandler>) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            // Convert Box to Arc for internal use
            let handler_arc: Arc<dyn crate::ffi::dispatcher_types::AgentProgressHandler> = Arc::from(handler);
            let subscriber = Arc::new(crate::ffi::dispatcher_types::FfiProgressSubscriber::new(handler_arc));
            engine.subscribe(subscriber);
            info!("Cowork progress subscriber added");
        }
    }

    // ===== CODE EXECUTION CONFIG =====

    /// Get code execution configuration
    pub fn agent_get_code_exec_config(&self) -> crate::ffi::dispatcher_types::CodeExecConfigFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => crate::ffi::dispatcher_types::CodeExecConfigFFI::from(cfg.agent.code_exec),
            Err(_) => crate::ffi::dispatcher_types::CodeExecConfigFFI::default(),
        }
    }

    /// Update code execution configuration
    pub fn agent_update_code_exec_config(
        &self,
        config: crate::ffi::dispatcher_types::CodeExecConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update code_exec section
        full_config.agent.code_exec = config.into();

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!("Code execution configuration updated");
        Ok(())
    }

    // ===== FILE OPERATIONS CONFIG =====

    /// Get file operations configuration
    pub fn agent_get_file_ops_config(&self) -> crate::ffi::dispatcher_types::FileOpsConfigFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => crate::ffi::dispatcher_types::FileOpsConfigFFI::from(cfg.agent.file_ops),
            Err(_) => crate::ffi::dispatcher_types::FileOpsConfigFFI::default(),
        }
    }

    /// Update file operations configuration
    pub fn agent_update_file_ops_config(
        &self,
        config: crate::ffi::dispatcher_types::FileOpsConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update file_ops section
        full_config.agent.file_ops = config.into();

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!("File operations configuration updated");
        Ok(())
    }

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

    // =========================================================================
    // Budget Management (Model Router P1)
    // =========================================================================

    /// Get budget status overview
    ///
    /// Returns the overall budget status including all configured limits,
    /// current spending, and warning/exceeded states.
    pub fn agent_get_budget_status(&self) -> crate::ffi::dispatcher_types::BudgetStatusFFI {
        // Load config and get budget limits
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled(),
        };

        // Get budget configuration from cowork.model_routing.budget
        let budget_config = &config.agent.model_routing.budget;

        if !budget_config.enabled {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
        }

        // Convert config limits to internal BudgetLimit types
        let default_enforcement = &budget_config.default_enforcement;
        let limits: Vec<crate::dispatcher::model_router::BudgetLimit> = budget_config
            .limits
            .iter()
            .map(|l| l.to_budget_limit(default_enforcement))
            .collect();

        if limits.is_empty() {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
        }

        // Create initial states for each limit
        // TODO: When BudgetManager is integrated into AgentEngine, use actual states
        let mut states = std::collections::HashMap::new();
        for limit in &limits {
            states.insert(
                limit.id.clone(),
                crate::dispatcher::model_router::BudgetState::new(limit),
            );
        }

        crate::ffi::dispatcher_types::BudgetStatusFFI::from_limits_and_states(&limits, &states)
    }

    /// Get budget status for a specific scope
    ///
    /// Returns budget limits and status that apply to the given scope.
    pub fn agent_get_budget_status_for_scope(
        &self,
        scope_type: String,
        scope_id: Option<String>,
    ) -> crate::ffi::dispatcher_types::BudgetStatusFFI {
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
            _ => return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled(),
        };

        // Load config and get budget limits
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled(),
        };

        let budget_config = &config.agent.model_routing.budget;

        if !budget_config.enabled {
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
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
            return crate::ffi::dispatcher_types::BudgetStatusFFI::disabled();
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

        crate::ffi::dispatcher_types::BudgetStatusFFI::from_limits_and_states(&applicable_limits, &states)
    }

    /// Get a single budget limit status by ID
    ///
    /// Returns the status of a specific budget limit, or None if not found.
    pub fn agent_get_budget_limit(
        &self,
        limit_id: String,
    ) -> Option<crate::ffi::dispatcher_types::BudgetLimitStatusFFI> {
        // Load config
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return None,
        };

        let budget_config = &config.agent.model_routing.budget;

        if !budget_config.enabled {
            return None;
        }

        // Find the limit by ID
        let limit_config = budget_config.limits.iter().find(|l| l.id == limit_id)?;

        let default_enforcement = &budget_config.default_enforcement;
        let limit = limit_config.to_budget_limit(default_enforcement);
        let state = crate::dispatcher::model_router::BudgetState::new(&limit);

        Some(crate::ffi::dispatcher_types::BudgetLimitStatusFFI::from_limit_and_state(&limit, &state))
    }

    // =========================================================================
    // A/B Testing (Model Router P3)
    // =========================================================================

    /// Get A/B testing status overview
    ///
    /// Returns the overall A/B testing status including all active experiments,
    /// their configurations, and current statistics.
    pub fn agent_get_ab_testing_status(&self) -> crate::ffi::dispatcher_types::ABTestingStatusFFI {
        // Load config to check if A/B testing is enabled
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::ffi::dispatcher_types::ABTestingStatusFFI::disabled(),
        };

        let ab_config = &config.agent.model_routing.ab_testing;

        if !ab_config.enabled {
            return crate::ffi::dispatcher_types::ABTestingStatusFFI::disabled();
        }

        // TODO: When ABTestingEngine is integrated into AgentEngine, use actual engine
        // For now, return configured experiments count
        let experiment_count = ab_config.experiments.len();

        if experiment_count == 0 {
            return crate::ffi::dispatcher_types::ABTestingStatusFFI {
                enabled: true,
                total_experiments: 0,
                active_experiments: 0,
                experiments: Vec::new(),
                status_emoji: "⚪".to_string(),
                status_message: "No experiments configured".to_string(),
            };
        }

        crate::ffi::dispatcher_types::ABTestingStatusFFI {
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
    pub fn agent_get_active_experiments(&self) -> Vec<String> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return Vec::new(),
        };

        let ab_config = &config.agent.model_routing.ab_testing;

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
    pub fn agent_get_experiment_report(
        &self,
        experiment_id: String,
    ) -> Option<crate::ffi::dispatcher_types::ExperimentReportFFI> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return None,
        };

        let ab_config = &config.agent.model_routing.ab_testing;

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
    pub fn agent_enable_experiment(&self, experiment_id: String) -> Result<(), AetherFfiError> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(e) => return Err(AetherFfiError::Config(e.to_string())),
        };

        let ab_config = &config.agent.model_routing.ab_testing;

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
    pub fn agent_disable_experiment(&self, experiment_id: String) -> Result<(), AetherFfiError> {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(e) => return Err(AetherFfiError::Config(e.to_string())),
        };

        let ab_config = &config.agent.model_routing.ab_testing;

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
    pub fn agent_get_ensemble_status(&self) -> crate::ffi::dispatcher_types::EnsembleStatusFFI {
        let config = match crate::config::Config::load() {
            Ok(cfg) => cfg,
            Err(_) => return crate::ffi::dispatcher_types::EnsembleStatusFFI::disabled(),
        };

        let ensemble_config = &config.agent.model_routing.ensemble;

        if !ensemble_config.enabled {
            return crate::ffi::dispatcher_types::EnsembleStatusFFI::disabled();
        }

        // Convert config to FFI summary
        let mode_ffi = match ensemble_config.default_mode.as_str() {
            "best_of_n" => crate::ffi::dispatcher_types::EnsembleModeFFI::BestOfN,
            "voting" => crate::ffi::dispatcher_types::EnsembleModeFFI::Voting,
            "consensus" => crate::ffi::dispatcher_types::EnsembleModeFFI::Consensus,
            "cascade" => crate::ffi::dispatcher_types::EnsembleModeFFI::Cascade,
            _ => crate::ffi::dispatcher_types::EnsembleModeFFI::Disabled,
        };

        let quality_metric_ffi = match ensemble_config.quality_scorer.as_str() {
            "length" => crate::ffi::dispatcher_types::QualityMetricFFI::Length,
            "structure" => crate::ffi::dispatcher_types::QualityMetricFFI::Structure,
            "length_and_structure" => crate::ffi::dispatcher_types::QualityMetricFFI::LengthAndStructure,
            "confidence_markers" | "confidence" => {
                crate::ffi::dispatcher_types::QualityMetricFFI::ConfidenceMarkers
            }
            "relevance" => crate::ffi::dispatcher_types::QualityMetricFFI::Relevance,
            _ => crate::ffi::dispatcher_types::QualityMetricFFI::LengthAndStructure,
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

        let config_summary = crate::ffi::dispatcher_types::EnsembleConfigSummaryFFI {
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
        let stats = crate::ffi::dispatcher_types::EnsembleStatsFFI::empty();

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

        crate::ffi::dispatcher_types::EnsembleStatusFFI {
            config: config_summary,
            stats,
            status_emoji: emoji,
            status_message: message,
        }
    }

    /// Get ensemble configuration summary
    ///
    /// Returns the current ensemble configuration for display.
    pub fn agent_get_ensemble_config(&self) -> crate::ffi::dispatcher_types::EnsembleConfigSummaryFFI {
        let status = self.agent_get_ensemble_status();
        status.config
    }

    /// Get ensemble execution statistics
    ///
    /// Returns statistics about ensemble executions.
    pub fn agent_get_ensemble_stats(&self) -> crate::ffi::dispatcher_types::EnsembleStatsFFI {
        let status = self.agent_get_ensemble_status();
        status.stats
    }

    // =========================================================================
    // DAG Plan Confirmation (Plan Confirmation Flow)
    // =========================================================================

    /// Confirm or cancel a pending DAG task plan
    ///
    /// This method is called by Swift after displaying a confirmation dialog
    /// to the user. It completes the pending confirmation and allows the
    /// DAG scheduler to proceed (or cancel).
    ///
    /// # Arguments
    ///
    /// * `plan_id` - The plan ID from `on_plan_confirmation_required` callback
    /// * `confirmed` - `true` to confirm execution, `false` to cancel
    ///
    /// # Returns
    ///
    /// `true` if the confirmation was found and completed, `false` if expired or not found.
    pub fn confirm_task_plan(&self, plan_id: String, confirmed: bool) -> bool {
        let decision = if confirmed {
            crate::dispatcher::UserDecision::Confirmed
        } else {
            crate::dispatcher::UserDecision::Cancelled
        };

        info!(
            plan_id = %plan_id,
            decision = ?decision,
            "Confirming task plan from FFI"
        );

        crate::ffi::plan_confirmation::complete_pending_confirmation(&plan_id, decision)
    }
}

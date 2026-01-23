//! Core dispatcher FFI methods
//!
//! Contains agent engine lifecycle and basic task orchestration:
//! - get_or_create_agent_engine
//! - agent_plan, agent_execute
//! - state management (pause, resume, cancel)

use crate::config::Config;
use crate::ffi::{AetherCore, AetherFfiError};
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

    // Note: agent_get_config and agent_update_config have been removed.
    // Core execution parameters (require_confirmation, max_parallelism, max_task_retries)
    // are now hardcoded for security and stability.

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
    pub(crate) fn extract_generation_providers(&self) -> crate::dispatcher::GenerationProviders {
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
}

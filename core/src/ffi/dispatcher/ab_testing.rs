//! A/B Testing FFI methods
//!
//! Contains experiment management:
//! - agent_get_ab_testing_status
//! - agent_get_active_experiments
//! - agent_get_experiment_report
//! - agent_enable_experiment, agent_disable_experiment

use crate::ffi::{AetherCore, AetherFfiError};

impl AetherCore {
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
}

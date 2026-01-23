//! Ensemble FFI methods
//!
//! Contains ensemble status and configuration:
//! - agent_get_ensemble_status
//! - agent_get_ensemble_config
//! - agent_get_ensemble_stats

use crate::ffi::AetherCore;

impl AetherCore {
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
}

//! Prompt analysis configuration (P2)
//!
//! Contains PromptAnalysisConfigToml for configuring the prompt analyzer
//! for intelligent model routing based on prompt content features.

use serde::{Deserialize, Serialize};

// =============================================================================
// PromptAnalysisConfigToml
// =============================================================================

/// Prompt analysis configuration from TOML
///
/// Configures the prompt analyzer for intelligent model routing based on
/// prompt content features like complexity, language, and domain.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.prompt_analysis]
/// enabled = true
/// high_complexity_threshold = 0.7
/// low_complexity_threshold = 0.3
/// mixed_language_threshold = 0.3
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptAnalysisConfigToml {
    /// Enable prompt analysis for routing
    #[serde(default = "default_prompt_analysis_enabled")]
    pub enabled: bool,

    /// Threshold above which complexity is considered high
    #[serde(default = "default_high_complexity_threshold")]
    pub high_complexity_threshold: f64,

    /// Threshold below which complexity is considered low
    #[serde(default = "default_low_complexity_threshold")]
    pub low_complexity_threshold: f64,

    /// Threshold for mixed language detection (0.0 - 1.0)
    #[serde(default = "default_mixed_language_threshold")]
    pub mixed_language_threshold: f64,

    /// Complexity weight for text length
    #[serde(default = "default_complexity_length_weight")]
    pub complexity_length_weight: f64,

    /// Complexity weight for sentence structure
    #[serde(default = "default_complexity_structure_weight")]
    pub complexity_structure_weight: f64,

    /// Complexity weight for technical terms
    #[serde(default = "default_complexity_technical_weight")]
    pub complexity_technical_weight: f64,

    /// Complexity weight for multi-step indicators
    #[serde(default = "default_complexity_multi_step_weight")]
    pub complexity_multi_step_weight: f64,
}

impl Default for PromptAnalysisConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_prompt_analysis_enabled(),
            high_complexity_threshold: default_high_complexity_threshold(),
            low_complexity_threshold: default_low_complexity_threshold(),
            mixed_language_threshold: default_mixed_language_threshold(),
            complexity_length_weight: default_complexity_length_weight(),
            complexity_structure_weight: default_complexity_structure_weight(),
            complexity_technical_weight: default_complexity_technical_weight(),
            complexity_multi_step_weight: default_complexity_multi_step_weight(),
        }
    }
}

impl PromptAnalysisConfigToml {
    /// Convert to PromptAnalyzerConfig
    pub fn to_prompt_analyzer_config(
        &self,
    ) -> crate::dispatcher::model_router::PromptAnalyzerConfig {
        crate::dispatcher::model_router::PromptAnalyzerConfig {
            high_complexity_threshold: self.high_complexity_threshold,
            low_complexity_threshold: self.low_complexity_threshold,
            mixed_language_threshold: self.mixed_language_threshold,
            complexity_weights: crate::dispatcher::model_router::ComplexityWeights {
                length: self.complexity_length_weight,
                structure: self.complexity_structure_weight,
                technical: self.complexity_technical_weight,
                multi_step: self.complexity_multi_step_weight,
            },
            ..Default::default()
        }
    }

    /// Validate prompt analysis configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.high_complexity_threshold <= self.low_complexity_threshold {
            return Err(format!(
                "high_complexity_threshold ({}) must be greater than low_complexity_threshold ({})",
                self.high_complexity_threshold, self.low_complexity_threshold
            ));
        }

        if self.high_complexity_threshold > 1.0 || self.high_complexity_threshold < 0.0 {
            return Err(format!(
                "high_complexity_threshold must be between 0.0 and 1.0, got {}",
                self.high_complexity_threshold
            ));
        }

        if self.low_complexity_threshold > 1.0 || self.low_complexity_threshold < 0.0 {
            return Err(format!(
                "low_complexity_threshold must be between 0.0 and 1.0, got {}",
                self.low_complexity_threshold
            ));
        }

        if self.mixed_language_threshold > 1.0 || self.mixed_language_threshold < 0.0 {
            return Err(format!(
                "mixed_language_threshold must be between 0.0 and 1.0, got {}",
                self.mixed_language_threshold
            ));
        }

        Ok(())
    }
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_prompt_analysis_enabled() -> bool {
    true
}

fn default_high_complexity_threshold() -> f64 {
    0.7
}

fn default_low_complexity_threshold() -> f64 {
    0.3
}

fn default_mixed_language_threshold() -> f64 {
    0.3
}

fn default_complexity_length_weight() -> f64 {
    0.2
}

fn default_complexity_structure_weight() -> f64 {
    0.3
}

fn default_complexity_technical_weight() -> f64 {
    0.3
}

fn default_complexity_multi_step_weight() -> f64 {
    0.2
}

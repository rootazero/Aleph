//! Evolution configuration types
//!
//! Contains Skill Compiler configuration for Phase 10 (The Hands):
//! - EvolutionConfig: Main evolution settings
//! - SolidificationThresholds: Detection thresholds
//! - ToolGenerationConfig: Tool-backed skill generation settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// EvolutionConfig
// =============================================================================

/// Configuration for the skill evolution system (Skill Compiler)
///
/// The skill compiler detects repeated successful patterns and converts them
/// into reusable skills or tool-backed automations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EvolutionConfig {
    /// Enable the skill evolution system
    #[serde(default = "default_evolution_enabled")]
    pub enabled: bool,

    /// Database path for evolution tracker (relative to config dir or absolute)
    #[serde(default = "default_db_path")]
    pub db_path: String,

    /// Solidification detection thresholds
    #[serde(default)]
    pub thresholds: SolidificationThresholds,

    /// Tool-backed skill generation settings
    #[serde(default)]
    pub tool_generation: ToolGenerationConfig,

    /// Auto-commit generated skills to git
    #[serde(default = "default_auto_commit")]
    pub auto_commit: bool,

    /// Auto-push commits to remote (requires auto_commit)
    #[serde(default)]
    pub auto_push: bool,

    /// Git remote name for auto-push
    #[serde(default = "default_remote")]
    pub remote: String,

    /// Git branch for auto-push
    #[serde(default = "default_branch")]
    pub branch: String,

    /// Directory for generated skills (defaults to skills_dir from SkillsConfig)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_output_dir: Option<String>,

    /// AI provider for generating skill suggestions (uses default if not set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_provider: Option<String>,
}

// =============================================================================
// SolidificationThresholds
// =============================================================================

/// Thresholds for detecting solidification candidates
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SolidificationThresholds {
    /// Minimum successful executions before suggesting solidification
    #[serde(default = "default_min_success_count")]
    pub min_success_count: u32,

    /// Minimum success rate (0.0-1.0)
    #[serde(default = "default_min_success_rate")]
    pub min_success_rate: f32,

    /// Minimum days since first use before considering solidification
    #[serde(default = "default_min_age_days")]
    pub min_age_days: u32,

    /// Maximum days since last use (to avoid stale patterns)
    #[serde(default = "default_max_idle_days")]
    pub max_idle_days: u32,

    /// Minimum confidence score for suggestions (0.0-1.0)
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
}

// =============================================================================
// ToolGenerationConfig
// =============================================================================

/// Configuration for tool-backed skill generation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolGenerationConfig {
    /// Enable tool-backed skill generation
    #[serde(default)]
    pub enabled: bool,

    /// Directory for generated tool packages (relative to config dir or absolute)
    #[serde(default = "default_tools_output_dir")]
    pub tools_output_dir: String,

    /// Runtime for generated tools (python, node, etc.)
    #[serde(default = "default_runtime")]
    pub runtime: String,

    /// Require self-test pass before registration
    #[serde(default = "default_require_self_test")]
    pub require_self_test: bool,

    /// Require first-run confirmation for generated tools
    #[serde(default = "default_require_first_run_confirmation")]
    pub require_first_run_confirmation: bool,

    /// Maximum number of pending tool suggestions
    #[serde(default = "default_max_pending_suggestions")]
    pub max_pending_suggestions: u32,
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_evolution_enabled() -> bool {
    true // Enable evolution tracking by default
}

fn default_db_path() -> String {
    "evolution.db".to_string()
}

fn default_auto_commit() -> bool {
    false // Conservative: don't auto-commit by default
}

fn default_remote() -> String {
    "origin".to_string()
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_min_success_count() -> u32 {
    3 // Require 3 successful executions
}

fn default_min_success_rate() -> f32 {
    0.8 // 80% success rate
}

fn default_min_age_days() -> u32 {
    1 // Pattern must be at least 1 day old
}

fn default_max_idle_days() -> u32 {
    30 // Don't solidify patterns not used in 30 days
}

fn default_min_confidence() -> f32 {
    0.7 // 70% confidence threshold
}

fn default_tools_output_dir() -> String {
    "tools/compiled".to_string()
}

fn default_runtime() -> String {
    "python".to_string()
}

fn default_require_self_test() -> bool {
    true // Always self-test before registration
}

fn default_require_first_run_confirmation() -> bool {
    true // Always confirm first run
}

fn default_max_pending_suggestions() -> u32 {
    10 // Don't accumulate too many pending suggestions
}

// =============================================================================
// Default Implementations
// =============================================================================

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: default_evolution_enabled(),
            db_path: default_db_path(),
            thresholds: SolidificationThresholds::default(),
            tool_generation: ToolGenerationConfig::default(),
            auto_commit: default_auto_commit(),
            auto_push: false,
            remote: default_remote(),
            branch: default_branch(),
            skills_output_dir: None,
            ai_provider: None,
        }
    }
}

impl Default for SolidificationThresholds {
    fn default() -> Self {
        Self {
            min_success_count: default_min_success_count(),
            min_success_rate: default_min_success_rate(),
            min_age_days: default_min_age_days(),
            max_idle_days: default_max_idle_days(),
            min_confidence: default_min_confidence(),
        }
    }
}

impl Default for ToolGenerationConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Conservative: disable tool generation by default
            tools_output_dir: default_tools_output_dir(),
            runtime: default_runtime(),
            require_self_test: default_require_self_test(),
            require_first_run_confirmation: default_require_first_run_confirmation(),
            max_pending_suggestions: default_max_pending_suggestions(),
        }
    }
}

// =============================================================================
// Helper Methods
// =============================================================================

impl EvolutionConfig {
    /// Get the full path to the evolution database (cross-platform)
    pub fn get_db_path(&self) -> std::path::PathBuf {
        let path = std::path::Path::new(&self.db_path);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            crate::utils::paths::get_config_dir()
                .map(|d| d.join(&self.db_path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    }

    /// Get the skills output directory, falling back to SkillsConfig default
    pub fn get_skills_output_dir(&self, skills_config: &super::SkillsConfig) -> std::path::PathBuf {
        if let Some(ref dir) = self.skills_output_dir {
            let path = std::path::Path::new(dir);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                crate::utils::paths::get_config_dir()
                    .map(|d| d.join(dir))
                    .unwrap_or_else(|_| path.to_path_buf())
            }
        } else {
            skills_config.get_skills_dir_path()
        }
    }

}

impl ToolGenerationConfig {
    /// Get the full path to the tools output directory
    pub fn get_tools_output_dir(&self) -> std::path::PathBuf {
        let path = std::path::Path::new(&self.tools_output_dir);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            crate::utils::paths::get_config_dir()
                .map(|d| d.join(&self.tools_output_dir))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_evolution_config() {
        let config = EvolutionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.db_path, "evolution.db");
        assert!(!config.auto_commit);
        assert!(!config.auto_push);
    }

    #[test]
    fn test_default_thresholds() {
        let thresholds = SolidificationThresholds::default();
        assert_eq!(thresholds.min_success_count, 3);
        assert_eq!(thresholds.min_success_rate, 0.8);
        assert_eq!(thresholds.min_age_days, 1);
        assert_eq!(thresholds.max_idle_days, 30);
    }

    #[test]
    fn test_tool_generation_disabled_by_default() {
        let config = ToolGenerationConfig::default();
        assert!(!config.enabled);
        assert!(config.require_self_test);
        assert!(config.require_first_run_confirmation);
    }
}

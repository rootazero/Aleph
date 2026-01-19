//! Skills configuration types
//!
//! Contains Claude Agent Skills configuration:
//! - SkillsConfig: Skills directory and auto-matching settings

use serde::{Deserialize, Serialize};

// =============================================================================
// SkillsConfig
// =============================================================================

/// Skills configuration (Claude Agent Skills standard)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Enable skills capability
    #[serde(default = "default_skills_enabled")]
    pub enabled: bool,

    /// Skills directory path (relative to config dir or absolute)
    #[serde(default = "default_skills_dir")]
    pub skills_dir: String,

    /// Enable auto-matching skills based on user input
    #[serde(default = "default_auto_match_enabled")]
    pub auto_match_enabled: bool,
}

pub fn default_skills_enabled() -> bool {
    true
}

pub fn default_skills_dir() -> String {
    "skills".to_string()
}

pub fn default_auto_match_enabled() -> bool {
    false // Off by default, explicit /skill command required
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: default_skills_enabled(),
            skills_dir: default_skills_dir(),
            auto_match_enabled: default_auto_match_enabled(),
        }
    }
}

impl SkillsConfig {
    /// Get the full path to the skills directory (cross-platform)
    ///
    /// If skills_dir is relative, it's relative to the unified config directory:
    /// - All platforms: ~/.config/aether/
    ///
    /// If absolute, use as-is
    pub fn get_skills_dir_path(&self) -> std::path::PathBuf {
        let path = std::path::Path::new(&self.skills_dir);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative to config directory (platform-aware)
            crate::utils::paths::get_config_dir()
                .map(|d| d.join(&self.skills_dir))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    }
}

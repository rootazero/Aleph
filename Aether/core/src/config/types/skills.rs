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
    /// Get the full path to the skills directory
    ///
    /// If skills_dir is relative, it's relative to ~/.config/aether/
    /// If absolute, use as-is
    pub fn get_skills_dir_path(&self) -> std::path::PathBuf {
        let path = std::path::Path::new(&self.skills_dir);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative to config directory
            if let Some(home) = dirs::home_dir() {
                home.join(".config").join("aether").join(&self.skills_dir)
            } else {
                path.to_path_buf()
            }
        }
    }
}

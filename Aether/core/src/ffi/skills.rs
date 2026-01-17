//! Skills management methods for AetherCore
//!
//! This module contains skills-related methods: list_skills, install_skill, delete_skill, etc.
//!
//! # Hot-Reload Support
//!
//! All skill modification methods (install, delete) automatically notify the UI
//! of tool registry changes via the `on_tools_changed` callback.

use super::{AetherCore, AetherFfiError};
use tracing::info;

impl AetherCore {
    /// List all installed skills
    pub fn list_skills(&self) -> Result<Vec<crate::skills::SkillInfo>, AetherFfiError> {
        crate::initialization::list_installed_skills()
            .map_err(|e| AetherFfiError::Config(e.to_string()))
    }

    /// Install a skill from a GitHub URL
    ///
    /// After successful installation, notifies UI of tool registry change.
    pub fn install_skill(&self, url: String) -> Result<crate::skills::SkillInfo, AetherFfiError> {
        let skill_info = crate::initialization::install_skill_from_url(url)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(skill_id = %skill_info.id, "Skill installed");

        // Notify UI of tool registry change (hot-reload)
        self.notify_tools_changed();

        Ok(skill_info)
    }

    /// Install skills from a local ZIP file
    ///
    /// After successful installation, notifies UI of tool registry change.
    pub fn install_skills_from_zip(&self, zip_path: String) -> Result<Vec<String>, AetherFfiError> {
        let skill_ids = crate::initialization::install_skills_from_zip(zip_path)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(count = skill_ids.len(), "Skills installed from ZIP");

        // Notify UI of tool registry change (hot-reload)
        self.notify_tools_changed();

        Ok(skill_ids)
    }

    /// Delete a skill by ID
    ///
    /// After successful deletion, notifies UI of tool registry change.
    pub fn delete_skill(&self, skill_id: String) -> Result<(), AetherFfiError> {
        crate::initialization::delete_skill(skill_id.clone())
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(skill_id = %skill_id, "Skill deleted");

        // Notify UI of tool registry change (hot-reload)
        self.notify_tools_changed();

        Ok(())
    }

    /// Get the skills directory path
    pub fn get_skills_dir(&self) -> Result<String, AetherFfiError> {
        crate::initialization::get_skills_dir_string()
            .map_err(|e| AetherFfiError::Config(e.to_string()))
    }

    /// Refresh skills and notify UI
    ///
    /// Reloads the skills registry from disk and notifies the UI of the change.
    /// This can be called manually when skills are modified outside the normal
    /// install/delete flow (e.g., manual file system changes).
    pub fn refresh_skills(&self) {
        info!("Skills refresh requested");

        // Reload skills from disk
        match crate::initialization::list_installed_skills() {
            Ok(skills) => {
                info!(count = skills.len(), "Skills refreshed from disk");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to refresh skills");
            }
        }

        // Notify UI of potential tool registry change
        self.notify_tools_changed();
    }
}

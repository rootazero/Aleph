//! Skills management methods for AetherCore
//!
//! This module contains skills-related methods: list_skills, install_skill, delete_skill, etc.

use super::{AetherCore, AetherFfiError};
use tracing::info;

impl AetherCore {
    /// List all installed skills
    pub fn list_skills(&self) -> Result<Vec<crate::skills::SkillInfo>, AetherFfiError> {
        crate::initialization::list_installed_skills()
            .map_err(|e| AetherFfiError::Config(e.to_string()))
    }

    /// Install a skill from a GitHub URL
    pub fn install_skill(&self, url: String) -> Result<crate::skills::SkillInfo, AetherFfiError> {
        let skill_info = crate::initialization::install_skill_from_url(url)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(skill_id = %skill_info.id, "Skill installed");
        Ok(skill_info)
    }

    /// Install skills from a local ZIP file
    pub fn install_skills_from_zip(&self, zip_path: String) -> Result<Vec<String>, AetherFfiError> {
        let skill_ids = crate::initialization::install_skills_from_zip(zip_path)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(count = skill_ids.len(), "Skills installed from ZIP");
        Ok(skill_ids)
    }

    /// Delete a skill by ID
    pub fn delete_skill(&self, skill_id: String) -> Result<(), AetherFfiError> {
        crate::initialization::delete_skill(skill_id.clone())
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(skill_id = %skill_id, "Skill deleted");
        Ok(())
    }

    /// Get the skills directory path
    pub fn get_skills_dir(&self) -> Result<String, AetherFfiError> {
        crate::initialization::get_skills_dir_string()
            .map_err(|e| AetherFfiError::Config(e.to_string()))
    }

    /// Refresh skills (placeholder for V2)
    ///
    /// In V2, this is a no-op since tool registry is managed differently.
    pub fn refresh_skills(&self) {
        info!("Skills refresh requested");
    }
}

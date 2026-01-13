//! Skill operations for AetherCore
//!
//! This module contains all skill management methods:
//! - Skill installation (URL, ZIP)
//! - Skill deletion
//! - Skill listing
//!
//! All methods automatically refresh the tool registry after changes.

use super::AetherCore;
use crate::error::Result;
use crate::skills::SkillInfo;
use tracing::info;

impl AetherCore {
    // ========================================================================
    // SKILL MANAGEMENT METHODS
    // ========================================================================

    /// List all installed skills
    ///
    /// Returns information about all skills in the skills directory.
    pub fn list_skills(&self) -> Result<Vec<SkillInfo>> {
        crate::initialization::list_installed_skills()
    }

    /// Install a skill from a GitHub URL
    ///
    /// Downloads and installs a skill from a GitHub repository URL.
    /// Automatically refreshes the tool registry after installation.
    ///
    /// # Arguments
    /// * `url` - GitHub repository URL (e.g., https://github.com/user/repo)
    ///
    /// # Returns
    /// * `Ok(SkillInfo)` - Information about the installed skill
    /// * `Err(AetherError)` - Installation error
    pub fn install_skill(&self, url: String) -> Result<SkillInfo> {
        let skill_info = crate::initialization::install_skill_from_url(url)?;

        // Refresh tool registry to pick up new skill
        self.refresh_tool_registry();

        info!(skill_id = %skill_info.id, "Skill installed and registry refreshed");
        Ok(skill_info)
    }

    /// Install skills from a local ZIP file
    ///
    /// Extracts and installs skills from a ZIP file.
    /// Automatically refreshes the tool registry after installation.
    ///
    /// # Arguments
    /// * `zip_path` - Path to the ZIP file
    ///
    /// # Returns
    /// * `Ok(Vec<String>)` - List of installed skill IDs
    /// * `Err(AetherError)` - Installation error
    pub fn install_skills_from_zip(&self, zip_path: String) -> Result<Vec<String>> {
        let skill_ids = crate::initialization::install_skills_from_zip(zip_path)?;

        // Refresh tool registry to pick up new skills
        self.refresh_tool_registry();

        info!(count = skill_ids.len(), "Skills installed from ZIP and registry refreshed");
        Ok(skill_ids)
    }

    /// Delete a skill by ID
    ///
    /// Removes a skill from the skills directory.
    /// Automatically refreshes the tool registry after deletion.
    ///
    /// # Arguments
    /// * `skill_id` - The skill ID to delete
    ///
    /// # Returns
    /// * `Ok(())` - Skill deleted successfully
    /// * `Err(AetherError)` - Deletion error (skill not found, etc.)
    pub fn delete_skill(&self, skill_id: String) -> Result<()> {
        crate::initialization::delete_skill(skill_id.clone())?;

        // Refresh tool registry to remove deleted skill
        self.refresh_tool_registry();

        info!(skill_id = %skill_id, "Skill deleted and registry refreshed");
        Ok(())
    }

    /// Get the skills directory path
    ///
    /// Returns the path where skills are stored.
    pub fn get_skills_dir(&self) -> Result<String> {
        crate::initialization::get_skills_dir_string()
    }

    /// Refresh skills in the tool registry
    ///
    /// Forces a refresh of the tool registry to pick up any skill changes.
    /// This is useful when skills are modified outside of AetherCore.
    pub fn refresh_skills(&self) {
        self.refresh_tool_registry();
        info!("Skills refreshed in tool registry");
    }
}

#[cfg(test)]
mod tests {
    // Tests would require setting up skill directories and mock skills
    // which is complex. For now, rely on integration tests.
}

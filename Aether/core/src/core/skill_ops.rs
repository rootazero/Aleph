//! Skill operations for AetherCore
//!
//! This module contains all skill management methods:
//! - Skill installation (URL, ZIP)
//! - Skill deletion
//! - Skill listing
//!
//! All methods automatically refresh the tool registry after changes.
//! Uses scoped refresh (RefreshScope::SkillsOnly) for incremental updates.

use super::tools::RefreshScope;
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
    /// Triggers a background refresh of the tool registry after installation.
    /// When complete, `on_tools_changed()` callback will be invoked.
    ///
    /// # Arguments
    /// * `url` - GitHub repository URL (e.g., https://github.com/user/repo)
    ///
    /// # Returns
    /// * `Ok(SkillInfo)` - Information about the installed skill
    /// * `Err(AetherError)` - Installation error
    pub fn install_skill(&self, url: String) -> Result<SkillInfo> {
        let skill_info = crate::initialization::install_skill_from_url(url)?;

        // Scoped refresh: only update skills (non-blocking, incremental)
        self.refresh_tool_registry_scoped(RefreshScope::SkillsOnly);

        info!(skill_id = %skill_info.id, "Skill installed, scoped refresh initiated");
        Ok(skill_info)
    }

    /// Install skills from a local ZIP file
    ///
    /// Extracts and installs skills from a ZIP file.
    /// Triggers a background refresh of the tool registry after installation.
    /// When complete, `on_tools_changed()` callback will be invoked.
    ///
    /// # Arguments
    /// * `zip_path` - Path to the ZIP file
    ///
    /// # Returns
    /// * `Ok(Vec<String>)` - List of installed skill IDs
    /// * `Err(AetherError)` - Installation error
    pub fn install_skills_from_zip(&self, zip_path: String) -> Result<Vec<String>> {
        let skill_ids = crate::initialization::install_skills_from_zip(zip_path)?;

        // Scoped refresh: only update skills (non-blocking, incremental)
        self.refresh_tool_registry_scoped(RefreshScope::SkillsOnly);

        info!(count = skill_ids.len(), "Skills installed from ZIP, scoped refresh initiated");
        Ok(skill_ids)
    }

    /// Delete a skill by ID
    ///
    /// Removes a skill from the skills directory.
    /// Triggers a background refresh of the tool registry after deletion.
    /// When complete, `on_tools_changed()` callback will be invoked.
    ///
    /// # Arguments
    /// * `skill_id` - The skill ID to delete
    ///
    /// # Returns
    /// * `Ok(())` - Skill deleted successfully
    /// * `Err(AetherError)` - Deletion error (skill not found, etc.)
    pub fn delete_skill(&self, skill_id: String) -> Result<()> {
        crate::initialization::delete_skill(skill_id.clone())?;

        // Scoped refresh: only update skills (non-blocking, incremental)
        self.refresh_tool_registry_scoped(RefreshScope::SkillsOnly);

        info!(skill_id = %skill_id, "Skill deleted, scoped refresh initiated");
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
    /// Triggers a scoped refresh of the tool registry to pick up any skill changes.
    /// This is useful when skills are modified outside of AetherCore.
    /// When complete, `on_tools_changed()` callback will be invoked.
    pub fn refresh_skills(&self) {
        self.refresh_tool_registry_scoped(RefreshScope::SkillsOnly);
        info!("Skills scoped refresh initiated in background");
    }
}

#[cfg(test)]
mod tests {
    // Tests would require setting up skill directories and mock skills
    // which is complex. For now, rely on integration tests.
}

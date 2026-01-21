//! Plugin management methods for AetherCore
//!
//! This module provides FFI methods for managing Claude Code compatible plugins:
//! - List installed plugins
//! - Enable/disable plugins
//! - Load plugins from custom paths
//! - Execute plugin skills

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tracing::info;

use super::{AetherCore, AetherFfiError};
use crate::plugins::{
    default_plugins_dir, PluginInfo, PluginManager, PluginSkill,
};

// ============================================================================
// FFI Types
// ============================================================================

/// Plugin information for FFI/UI display
#[derive(Debug, Clone)]
pub struct PluginInfoFFI {
    /// Plugin name
    pub name: String,
    /// Plugin version
    pub version: String,
    /// Plugin description
    pub description: String,
    /// Whether plugin is enabled
    pub enabled: bool,
    /// Plugin root path
    pub path: String,
    /// Number of skills
    pub skills_count: u32,
    /// Number of agents
    pub agents_count: u32,
    /// Number of hook events
    pub hooks_count: u32,
    /// Number of MCP servers
    pub mcp_servers_count: u32,
}

impl From<PluginInfo> for PluginInfoFFI {
    fn from(info: PluginInfo) -> Self {
        Self {
            name: info.name,
            version: info.version.unwrap_or_default(),
            description: info.description.unwrap_or_default(),
            enabled: info.enabled,
            path: info.path,
            skills_count: info.skills_count as u32,
            agents_count: info.agents_count as u32,
            hooks_count: info.hooks_count as u32,
            mcp_servers_count: info.mcp_servers_count as u32,
        }
    }
}

/// Plugin skill information for FFI/UI display
#[derive(Debug, Clone)]
pub struct PluginSkillFFI {
    /// Fully qualified skill name (plugin:skill)
    pub qualified_name: String,
    /// Plugin name
    pub plugin_name: String,
    /// Skill name
    pub skill_name: String,
    /// Skill description
    pub description: String,
    /// Whether this is a command (user-triggered) or skill (auto-invocable)
    pub is_command: bool,
}

impl From<&PluginSkill> for PluginSkillFFI {
    fn from(skill: &PluginSkill) -> Self {
        Self {
            qualified_name: skill.qualified_name(),
            plugin_name: skill.plugin_name.clone(),
            skill_name: skill.skill_name.clone(),
            description: skill.description.clone(),
            is_command: skill.skill_type == crate::plugins::SkillType::Command,
        }
    }
}

// ============================================================================
// AetherCore Plugin Extensions
// ============================================================================

impl AetherCore {
    /// Get or initialize the plugin manager
    fn get_plugin_manager(&self) -> Arc<RwLock<PluginManager>> {
        // Check if already initialized
        if let Some(manager) = self.try_get_plugin_manager() {
            return manager;
        }

        // Initialize plugin manager
        let plugins_dir = default_plugins_dir();
        let manager = PluginManager::new(plugins_dir);

        // Store in AetherCore (we'll need to add this field)
        // For now, create a new one each time
        Arc::new(RwLock::new(manager))
    }

    /// Try to get existing plugin manager (placeholder for future caching)
    fn try_get_plugin_manager(&self) -> Option<Arc<RwLock<PluginManager>>> {
        None
    }

    /// List all installed plugins
    ///
    /// Returns information about all plugins in the plugins directory.
    pub fn list_plugins(&self) -> Result<Vec<PluginInfoFFI>, AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        // Load all plugins
        if let Err(e) = manager.load_all() {
            tracing::warn!(error = %e, "Error loading some plugins");
        }

        let plugins = manager
            .list_plugins()
            .into_iter()
            .map(PluginInfoFFI::from)
            .collect();

        Ok(plugins)
    }

    /// Enable a plugin
    ///
    /// Enables a previously disabled plugin. The plugin's skills, hooks, and agents
    /// will become active.
    pub fn enable_plugin(&self, name: String) -> Result<(), AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        // Ensure plugins are loaded
        let _ = manager.load_all();

        manager
            .set_enabled(&name, true)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(plugin = %name, "Plugin enabled");

        // Notify UI of potential tool registry change
        self.notify_tools_changed();

        Ok(())
    }

    /// Disable a plugin
    ///
    /// Disables a plugin. The plugin's skills, hooks, and agents will be deactivated
    /// but the plugin will remain installed.
    pub fn disable_plugin(&self, name: String) -> Result<(), AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        // Ensure plugins are loaded
        let _ = manager.load_all();

        manager
            .set_enabled(&name, false)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(plugin = %name, "Plugin disabled");

        // Notify UI of potential tool registry change
        self.notify_tools_changed();

        Ok(())
    }

    /// Load a plugin from a custom path
    ///
    /// Useful for development: load a plugin from a path outside the standard
    /// plugins directory.
    pub fn load_plugin_from_path(&self, path: String) -> Result<PluginInfoFFI, AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        let info = manager
            .load_plugin(&PathBuf::from(&path))
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(plugin = %info.name, path = %path, "Plugin loaded from custom path");

        // Notify UI of tool registry change
        self.notify_tools_changed();

        Ok(PluginInfoFFI::from(info))
    }

    /// List all skills from enabled plugins
    ///
    /// Returns information about all skills (commands and auto-invocable skills)
    /// from enabled plugins.
    pub fn list_plugin_skills(&self) -> Result<Vec<PluginSkillFFI>, AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        // Ensure plugins are loaded
        let _ = manager.load_all();

        let skills = manager
            .get_all_skills()
            .iter()
            .map(PluginSkillFFI::from)
            .collect();

        Ok(skills)
    }

    /// Execute a plugin skill
    ///
    /// Prepares a skill for execution by substituting $ARGUMENTS and returns
    /// the processed content for LLM processing.
    ///
    /// # Arguments
    ///
    /// * `plugin_name` - The plugin containing the skill
    /// * `skill_name` - The skill to execute
    /// * `arguments` - Arguments to substitute for $ARGUMENTS
    ///
    /// # Returns
    ///
    /// The processed skill content ready for LLM evaluation.
    pub fn execute_plugin_skill(
        &self,
        plugin_name: String,
        skill_name: String,
        arguments: String,
    ) -> Result<String, AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        // Ensure plugins are loaded
        let _ = manager.load_all();

        let content = manager
            .prepare_skill_execution(&plugin_name, &skill_name, &arguments)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(
            plugin = %plugin_name,
            skill = %skill_name,
            "Executing plugin skill"
        );

        Ok(content)
    }

    /// Get the plugins directory path
    ///
    /// Returns the path where plugins are stored.
    pub fn get_plugins_dir(&self) -> String {
        default_plugins_dir().to_string_lossy().to_string()
    }

    /// Refresh plugins
    ///
    /// Reloads all plugins from disk. Useful after manually adding or removing
    /// plugin files.
    pub fn refresh_plugins(&self) -> Result<u32, AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        let plugins = manager
            .load_all()
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(count = plugins.len(), "Plugins refreshed");

        // Notify UI of potential tool registry change
        self.notify_tools_changed();

        Ok(plugins.len() as u32)
    }

    /// Get skill instructions for prompt injection
    ///
    /// Returns formatted markdown instructions for all auto-invocable skills
    /// from enabled plugins. This should be appended to the system prompt.
    pub fn get_plugin_skill_instructions(&self) -> Result<String, AetherFfiError> {
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        // Ensure plugins are loaded
        let _ = manager.load_all();

        let skills = manager.get_auto_invocable_skills();
        let instructions = crate::plugins::build_skill_instructions(&skills);

        Ok(instructions)
    }

    /// Install a plugin from a Git repository URL
    ///
    /// Clones the repository into the plugins directory. The repository
    /// must contain a valid plugin structure with `.claude-plugin/plugin.json`.
    ///
    /// # Arguments
    ///
    /// * `url` - Git repository URL (e.g., "https://github.com/user/plugin.git")
    ///
    /// # Returns
    ///
    /// Information about the installed plugin.
    pub fn install_plugin_from_git(&self, url: String) -> Result<PluginInfoFFI, AetherFfiError> {
        use std::process::Command;

        let plugins_dir = default_plugins_dir();

        // Ensure plugins directory exists
        if !plugins_dir.exists() {
            std::fs::create_dir_all(&plugins_dir)
                .map_err(|e| AetherFfiError::Config(format!("Failed to create plugins directory: {}", e)))?;
        }

        // Extract plugin name from URL
        let plugin_name = extract_repo_name(&url)
            .ok_or_else(|| AetherFfiError::Config("Invalid Git URL".to_string()))?;

        let plugin_path = plugins_dir.join(&plugin_name);

        // Check if plugin already exists
        if plugin_path.exists() {
            return Err(AetherFfiError::Config(format!(
                "Plugin '{}' already exists. Uninstall it first.",
                plugin_name
            )));
        }

        info!(url = %url, name = %plugin_name, "Installing plugin from Git");

        // Clone the repository
        let output = Command::new("git")
            .args(["clone", "--depth", "1", &url, plugin_path.to_str().unwrap()])
            .output()
            .map_err(|e| AetherFfiError::Config(format!("Failed to run git: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AetherFfiError::Config(format!("Git clone failed: {}", stderr)));
        }

        // Validate plugin structure
        if !crate::plugins::is_valid_plugin_dir(&plugin_path) {
            // Clean up invalid plugin
            let _ = std::fs::remove_dir_all(&plugin_path);
            return Err(AetherFfiError::Config(
                "Repository does not contain a valid plugin structure (missing .claude-plugin/plugin.json)".to_string()
            ));
        }

        // Load the installed plugin
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();

        let info = manager
            .load_plugin(&plugin_path)
            .map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(plugin = %info.name, "Plugin installed from Git");

        // Notify UI of tool registry change
        self.notify_tools_changed();

        Ok(PluginInfoFFI::from(info))
    }

    /// Install plugins from a ZIP file
    ///
    /// Extracts the ZIP file into the plugins directory. The ZIP should
    /// contain one or more plugin directories with valid plugin structure.
    ///
    /// # Arguments
    ///
    /// * `zip_path` - Path to the ZIP file
    ///
    /// # Returns
    ///
    /// List of installed plugin names.
    pub fn install_plugins_from_zip(&self, zip_path: String) -> Result<Vec<String>, AetherFfiError> {
        use std::fs::File;
        use std::io::BufReader;

        let plugins_dir = default_plugins_dir();

        // Ensure plugins directory exists
        if !plugins_dir.exists() {
            std::fs::create_dir_all(&plugins_dir)
                .map_err(|e| AetherFfiError::Config(format!("Failed to create plugins directory: {}", e)))?;
        }

        let zip_file = File::open(&zip_path)
            .map_err(|e| AetherFfiError::Config(format!("Failed to open ZIP file: {}", e)))?;

        let reader = BufReader::new(zip_file);
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| AetherFfiError::Config(format!("Failed to read ZIP archive: {}", e)))?;

        info!(path = %zip_path, "Installing plugins from ZIP");

        // Extract to a temporary directory first
        let temp_dir = tempfile::tempdir()
            .map_err(|e| AetherFfiError::Config(format!("Failed to create temp directory: {}", e)))?;

        archive.extract(temp_dir.path())
            .map_err(|e| AetherFfiError::Config(format!("Failed to extract ZIP: {}", e)))?;

        // Find valid plugins in extracted content
        let mut installed_plugins = Vec::new();

        // Check if root is a plugin
        if crate::plugins::is_valid_plugin_dir(temp_dir.path()) {
            // Single plugin at root
            let plugin_name = get_plugin_name_from_manifest(temp_dir.path())
                .unwrap_or_else(|| "unknown-plugin".to_string());
            let dest_path = plugins_dir.join(&plugin_name);

            if dest_path.exists() {
                return Err(AetherFfiError::Config(format!(
                    "Plugin '{}' already exists. Uninstall it first.",
                    plugin_name
                )));
            }

            copy_dir_recursive(temp_dir.path(), &dest_path)
                .map_err(|e| AetherFfiError::Config(format!("Failed to copy plugin: {}", e)))?;

            installed_plugins.push(plugin_name);
        } else {
            // Check subdirectories for plugins
            let entries = std::fs::read_dir(temp_dir.path())
                .map_err(|e| AetherFfiError::Config(format!("Failed to read temp directory: {}", e)))?;

            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && crate::plugins::is_valid_plugin_dir(&path) {
                    let plugin_name = get_plugin_name_from_manifest(&path)
                        .or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()))
                        .unwrap_or_else(|| "unknown-plugin".to_string());

                    let dest_path = plugins_dir.join(&plugin_name);

                    if dest_path.exists() {
                        tracing::warn!(plugin = %plugin_name, "Plugin already exists, skipping");
                        continue;
                    }

                    copy_dir_recursive(&path, &dest_path)
                        .map_err(|e| AetherFfiError::Config(format!("Failed to copy plugin: {}", e)))?;

                    installed_plugins.push(plugin_name);
                }
            }
        }

        if installed_plugins.is_empty() {
            return Err(AetherFfiError::Config(
                "No valid plugins found in ZIP file".to_string()
            ));
        }

        // Refresh plugins to load newly installed ones
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();
        let _ = manager.load_all();

        info!(count = installed_plugins.len(), "Plugins installed from ZIP");

        // Notify UI of tool registry change
        self.notify_tools_changed();

        Ok(installed_plugins)
    }

    /// Uninstall a plugin
    ///
    /// Removes the plugin directory from the plugins folder.
    ///
    /// # Arguments
    ///
    /// * `name` - Plugin name to uninstall
    pub fn uninstall_plugin(&self, name: String) -> Result<(), AetherFfiError> {
        let plugins_dir = default_plugins_dir();
        let plugin_path = plugins_dir.join(&name);

        if !plugin_path.exists() {
            return Err(AetherFfiError::Config(format!(
                "Plugin '{}' not found",
                name
            )));
        }

        // Verify it's actually a plugin
        if !crate::plugins::is_valid_plugin_dir(&plugin_path) {
            return Err(AetherFfiError::Config(format!(
                "'{}' is not a valid plugin",
                name
            )));
        }

        info!(plugin = %name, "Uninstalling plugin");

        // Remove from registry first
        let manager = self.get_plugin_manager();
        let mut manager = manager.write().unwrap();
        let _ = manager.unload_plugin(&name);

        // Delete the directory
        std::fs::remove_dir_all(&plugin_path)
            .map_err(|e| AetherFfiError::Config(format!("Failed to delete plugin: {}", e)))?;

        info!(plugin = %name, "Plugin uninstalled");

        // Notify UI of tool registry change
        self.notify_tools_changed();

        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract repository name from Git URL
fn extract_repo_name(url: &str) -> Option<String> {
    // Handle various URL formats:
    // https://github.com/user/repo.git -> repo
    // https://github.com/user/repo -> repo
    // git@github.com:user/repo.git -> repo

    let url = url.trim_end_matches('/');
    let url = url.trim_end_matches(".git");

    url.rsplit('/')
        .next()
        .or_else(|| url.rsplit(':').next())
        .map(|s| s.to_string())
}

/// Get plugin name from manifest
fn get_plugin_name_from_manifest(path: &std::path::Path) -> Option<String> {
    let manifest_path = path.join(".claude-plugin").join("plugin.json");
    let content = std::fs::read_to_string(manifest_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value.get("name")?.as_str().map(|s| s.to_string())
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_info_ffi_conversion() {
        let info = PluginInfo {
            name: "test-plugin".to_string(),
            version: Some("1.0.0".to_string()),
            description: Some("A test plugin".to_string()),
            enabled: true,
            path: "/path/to/plugin".to_string(),
            skills_count: 3,
            agents_count: 1,
            hooks_count: 2,
            mcp_servers_count: 0,
        };

        let ffi: PluginInfoFFI = info.into();
        assert_eq!(ffi.name, "test-plugin");
        assert_eq!(ffi.version, "1.0.0");
        assert_eq!(ffi.skills_count, 3);
        assert!(ffi.enabled);
    }
}

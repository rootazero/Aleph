//! Plugin registry
//!
//! Manages loaded plugins and their state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::{
    LoadedPlugin, PluginAgent, PluginInfo, PluginSkill, PluginState, PluginStateFile,
};

/// Plugin registry for managing loaded plugins
#[derive(Debug)]
pub struct PluginRegistry {
    /// Loaded plugins by name
    plugins: RwLock<HashMap<String, LoadedPlugin>>,
    /// State file path
    state_path: PathBuf,
}

impl PluginRegistry {
    /// Create a new plugin registry
    pub fn new(state_path: PathBuf) -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            state_path,
        }
    }

    /// Register a loaded plugin
    pub fn register(&self, mut plugin: LoadedPlugin) -> PluginResult<()> {
        let name = plugin.manifest.name.clone();

        // Load persisted state
        let state = self.load_state()?;
        if let Some(plugin_state) = state.plugins.get(&name) {
            plugin.enabled = plugin_state.enabled;
        }

        let mut plugins = self.plugins.write().unwrap();
        if plugins.contains_key(&name) {
            return Err(PluginError::AlreadyLoaded(name));
        }

        tracing::info!(
            "Registered plugin: {} (enabled: {})",
            name,
            plugin.enabled
        );
        plugins.insert(name, plugin);
        Ok(())
    }

    /// Unregister a plugin
    pub fn unregister(&self, name: &str) -> PluginResult<LoadedPlugin> {
        let mut plugins = self.plugins.write().unwrap();
        plugins
            .remove(name)
            .ok_or_else(|| PluginError::PluginNotFound(name.to_string()))
    }

    /// Get a plugin by name
    pub fn get(&self, name: &str) -> Option<LoadedPlugin> {
        let plugins = self.plugins.read().unwrap();
        plugins.get(name).cloned()
    }

    /// Check if a plugin exists
    pub fn contains(&self, name: &str) -> bool {
        let plugins = self.plugins.read().unwrap();
        plugins.contains_key(name)
    }

    /// List all plugins
    pub fn list(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().unwrap();
        plugins.values().map(PluginInfo::from).collect()
    }

    /// List all enabled plugins
    pub fn list_enabled(&self) -> Vec<LoadedPlugin> {
        let plugins = self.plugins.read().unwrap();
        plugins
            .values()
            .filter(|p| p.enabled)
            .cloned()
            .collect()
    }

    /// Set plugin enabled state
    pub fn set_enabled(&self, name: &str, enabled: bool) -> PluginResult<()> {
        {
            let mut plugins = self.plugins.write().unwrap();
            let plugin = plugins
                .get_mut(name)
                .ok_or_else(|| PluginError::PluginNotFound(name.to_string()))?;
            plugin.enabled = enabled;
        }

        // Persist state
        self.save_plugin_state(name, enabled)?;

        tracing::info!("Plugin '{}' enabled: {}", name, enabled);
        Ok(())
    }

    /// Get all skills from enabled plugins
    pub fn get_all_skills(&self) -> Vec<PluginSkill> {
        let plugins = self.plugins.read().unwrap();
        plugins
            .values()
            .filter(|p| p.enabled)
            .flat_map(|p| p.skills.clone())
            .collect()
    }

    /// Get auto-invocable skills from enabled plugins
    pub fn get_auto_invocable_skills(&self) -> Vec<PluginSkill> {
        self.get_all_skills()
            .into_iter()
            .filter(|s| s.is_auto_invocable())
            .collect()
    }

    /// Get a specific skill
    pub fn get_skill(&self, plugin: &str, skill: &str) -> Option<PluginSkill> {
        let plugins = self.plugins.read().unwrap();
        plugins.get(plugin).and_then(|p| {
            p.skills
                .iter()
                .find(|s| s.skill_name == skill)
                .cloned()
        })
    }

    /// Get all agents from enabled plugins
    pub fn get_all_agents(&self) -> Vec<PluginAgent> {
        let plugins = self.plugins.read().unwrap();
        plugins
            .values()
            .filter(|p| p.enabled)
            .flat_map(|p| p.agents.clone())
            .collect()
    }

    /// Get a specific agent
    pub fn get_agent(&self, plugin: &str, agent: &str) -> Option<PluginAgent> {
        let plugins = self.plugins.read().unwrap();
        plugins.get(plugin).and_then(|p| {
            p.agents
                .iter()
                .find(|a| a.agent_name == agent)
                .cloned()
        })
    }

    /// Load persisted state
    fn load_state(&self) -> PluginResult<PluginStateFile> {
        if !self.state_path.exists() {
            return Ok(PluginStateFile::default());
        }

        let content = std::fs::read_to_string(&self.state_path)?;
        serde_json::from_str(&content).map_err(|e| {
            PluginError::StatePersistenceError(format!("Failed to parse state file: {}", e))
        })
    }

    /// Save a plugin's enabled state
    fn save_plugin_state(&self, name: &str, enabled: bool) -> PluginResult<()> {
        let mut state = self.load_state()?;

        // Get current version
        let version = {
            let plugins = self.plugins.read().unwrap();
            plugins
                .get(name)
                .and_then(|p| p.manifest.version.clone())
        };

        state.plugins.insert(
            name.to_string(),
            PluginState { enabled, version },
        );

        self.save_state(&state)
    }

    /// Save state to file
    fn save_state(&self, state: &PluginStateFile) -> PluginResult<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(state).map_err(|e| {
            PluginError::StatePersistenceError(format!("Failed to serialize state: {}", e))
        })?;

        std::fs::write(&self.state_path, content)?;
        Ok(())
    }

    /// Clear all plugins (for testing)
    #[cfg(test)]
    pub fn clear(&self) {
        let mut plugins = self.plugins.write().unwrap();
        plugins.clear();
    }
}

/// Get the default state file path
pub fn default_state_path() -> PathBuf {
    crate::utils::paths::get_config_dir()
        .map(|p| p.join("plugins.json"))
        .unwrap_or_else(|_| PathBuf::from(".").join("aether").join("plugins.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::types::{PluginManifest, PluginHooksConfig, PluginMcpConfig};
    use tempfile::TempDir;

    fn create_test_plugin(name: &str) -> LoadedPlugin {
        LoadedPlugin {
            manifest: PluginManifest {
                name: name.to_string(),
                version: Some("1.0.0".to_string()),
                description: None,
                author: None,
                homepage: None,
                repository: None,
                license: None,
                keywords: None,
                commands: None,
                skills: None,
                agents: None,
                hooks: None,
                mcp_servers: None,
                lsp_servers: None,
            },
            path: PathBuf::from("/test"),
            enabled: true,
            skills: vec![],
            hooks: PluginHooksConfig::default(),
            agents: vec![],
            mcp_servers: PluginMcpConfig::default(),
        }
    }

    #[test]
    fn test_register_and_get() {
        let temp = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp.path().join("plugins.json"));

        let plugin = create_test_plugin("test-plugin");
        registry.register(plugin).unwrap();

        assert!(registry.contains("test-plugin"));
        let retrieved = registry.get("test-plugin").unwrap();
        assert_eq!(retrieved.manifest.name, "test-plugin");
    }

    #[test]
    fn test_register_duplicate() {
        let temp = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp.path().join("plugins.json"));

        let plugin1 = create_test_plugin("test-plugin");
        let plugin2 = create_test_plugin("test-plugin");

        registry.register(plugin1).unwrap();
        let result = registry.register(plugin2);

        assert!(matches!(result, Err(PluginError::AlreadyLoaded(_))));
    }

    #[test]
    fn test_unregister() {
        let temp = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp.path().join("plugins.json"));

        let plugin = create_test_plugin("test-plugin");
        registry.register(plugin).unwrap();

        let removed = registry.unregister("test-plugin").unwrap();
        assert_eq!(removed.manifest.name, "test-plugin");
        assert!(!registry.contains("test-plugin"));
    }

    #[test]
    fn test_set_enabled() {
        let temp = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp.path().join("plugins.json"));

        let plugin = create_test_plugin("test-plugin");
        registry.register(plugin).unwrap();

        registry.set_enabled("test-plugin", false).unwrap();
        let plugin = registry.get("test-plugin").unwrap();
        assert!(!plugin.enabled);

        // Check state was persisted
        let state = registry.load_state().unwrap();
        assert!(!state.plugins.get("test-plugin").unwrap().enabled);
    }

    #[test]
    fn test_list_enabled() {
        let temp = TempDir::new().unwrap();
        let registry = PluginRegistry::new(temp.path().join("plugins.json"));

        let plugin1 = create_test_plugin("plugin-a");
        let plugin2 = create_test_plugin("plugin-b");

        registry.register(plugin1).unwrap();
        registry.register(plugin2).unwrap();

        registry.set_enabled("plugin-b", false).unwrap();

        let enabled = registry.list_enabled();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].manifest.name, "plugin-a");
    }
}

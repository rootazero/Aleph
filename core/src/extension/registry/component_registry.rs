//! Component registry for managing loaded extensions

use crate::extension::types::*;
use std::collections::HashMap;

/// Component registry - stores and manages all loaded components
#[derive(Debug, Default)]
pub struct ComponentRegistry {
    /// Loaded skills by qualified name
    skills: HashMap<String, ExtensionSkill>,

    /// Loaded commands by name
    commands: HashMap<String, ExtensionCommand>,

    /// Loaded agents by qualified name
    agents: HashMap<String, ExtensionAgent>,

    /// Loaded plugins by name
    plugins: HashMap<String, ExtensionPlugin>,
}

impl ComponentRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    // =========================================================================
    // Skill Management
    // =========================================================================

    /// Register a skill
    pub fn register_skill(&mut self, skill: ExtensionSkill) {
        let name = skill.qualified_name();
        tracing::debug!("Registering skill: {}", name);
        self.skills.insert(name, skill);
    }

    /// Get a skill by qualified name
    pub fn get_skill(&self, qualified_name: &str) -> Option<ExtensionSkill> {
        self.skills.get(qualified_name).cloned()
    }

    /// Get all skills
    pub fn get_all_skills(&self) -> Vec<ExtensionSkill> {
        self.skills.values().cloned().collect()
    }

    /// Get auto-invocable skills (for LLM prompt injection)
    pub fn get_auto_invocable_skills(&self) -> Vec<ExtensionSkill> {
        self.skills
            .values()
            .filter(|s| s.is_auto_invocable())
            .cloned()
            .collect()
    }

    /// Remove a skill
    pub fn unregister_skill(&mut self, qualified_name: &str) -> Option<ExtensionSkill> {
        self.skills.remove(qualified_name)
    }

    // =========================================================================
    // Command Management
    // =========================================================================

    /// Register a command
    pub fn register_command(&mut self, command: ExtensionCommand) {
        let name = command.qualified_name();
        tracing::debug!("Registering command: {}", name);
        self.commands.insert(name, command);
    }

    /// Get a command by name
    pub fn get_command(&self, name: &str) -> Option<ExtensionCommand> {
        self.commands.get(name).cloned()
    }

    /// Get all commands
    pub fn get_all_commands(&self) -> Vec<ExtensionCommand> {
        self.commands.values().cloned().collect()
    }

    /// Remove a command
    pub fn unregister_command(&mut self, name: &str) -> Option<ExtensionCommand> {
        self.commands.remove(name)
    }

    // =========================================================================
    // Agent Management
    // =========================================================================

    /// Register an agent
    pub fn register_agent(&mut self, agent: ExtensionAgent) {
        let name = agent.qualified_name();
        tracing::debug!("Registering agent: {}", name);
        self.agents.insert(name, agent);
    }

    /// Get an agent by qualified name
    pub fn get_agent(&self, qualified_name: &str) -> Option<ExtensionAgent> {
        self.agents.get(qualified_name).cloned()
    }

    /// Get all agents
    pub fn get_all_agents(&self) -> Vec<ExtensionAgent> {
        self.agents.values().cloned().collect()
    }

    /// Get primary agents
    pub fn get_primary_agents(&self) -> Vec<ExtensionAgent> {
        self.agents
            .values()
            .filter(|a| a.is_primary())
            .cloned()
            .collect()
    }

    /// Get sub-agents
    pub fn get_subagents(&self) -> Vec<ExtensionAgent> {
        self.agents
            .values()
            .filter(|a| a.is_subagent())
            .cloned()
            .collect()
    }

    /// Remove an agent
    pub fn unregister_agent(&mut self, qualified_name: &str) -> Option<ExtensionAgent> {
        self.agents.remove(qualified_name)
    }

    // =========================================================================
    // Plugin Management
    // =========================================================================

    /// Register a plugin
    pub fn register_plugin(&mut self, plugin: ExtensionPlugin) {
        tracing::debug!("Registering plugin: {}", plugin.name);
        self.plugins.insert(plugin.name.clone(), plugin);
    }

    /// Get a plugin by name
    pub fn get_plugin(&self, name: &str) -> Option<&ExtensionPlugin> {
        self.plugins.get(name)
    }

    /// Get all plugins
    pub fn get_all_plugins(&self) -> Vec<&ExtensionPlugin> {
        self.plugins.values().collect()
    }

    /// Get plugin info for all plugins
    pub fn get_plugin_info_list(&self) -> Vec<PluginInfo> {
        self.plugins.values().map(|p| p.info()).collect()
    }

    /// Enable/disable a plugin
    pub fn set_plugin_enabled(&mut self, name: &str, enabled: bool) -> bool {
        if let Some(plugin) = self.plugins.get_mut(name) {
            plugin.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Remove a plugin and all its components
    pub fn unregister_plugin(&mut self, name: &str) -> Option<ExtensionPlugin> {
        if let Some(plugin) = self.plugins.remove(name) {
            // Remove all components from this plugin
            let prefix = format!("{}:", name);
            self.skills.retain(|k, _| !k.starts_with(&prefix));
            self.commands.retain(|k, _| !k.starts_with(&prefix));
            self.agents.retain(|k, _| !k.starts_with(&prefix));
            Some(plugin)
        } else {
            None
        }
    }

    // =========================================================================
    // Statistics
    // =========================================================================

    /// Get total skill count
    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }

    /// Get total command count
    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    /// Get total agent count
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Get total plugin count
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Clear all components
    pub fn clear(&mut self) {
        self.skills.clear();
        self.commands.clear();
        self.agents.clear();
        self.plugins.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoverySource;
    use std::path::PathBuf;

    fn create_test_skill(name: &str, plugin: Option<&str>) -> ExtensionSkill {
        ExtensionSkill {
            name: name.to_string(),
            plugin_name: plugin.map(|s| s.to_string()),
            skill_type: SkillType::Skill,
            description: "Test skill".to_string(),
            content: "Test content".to_string(),
            disable_model_invocation: false,
            source_path: PathBuf::from("/test"),
            source: DiscoverySource::AetherGlobal,
        }
    }

    #[test]
    fn test_register_and_get_skill() {
        let mut registry = ComponentRegistry::new();
        let skill = create_test_skill("hello", Some("my-plugin"));

        registry.register_skill(skill);

        let retrieved = registry.get_skill("my-plugin:hello");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "hello");
    }

    #[test]
    fn test_get_auto_invocable_skills() {
        let mut registry = ComponentRegistry::new();

        let skill1 = create_test_skill("auto", None);
        let mut skill2 = create_test_skill("manual", None);
        skill2.disable_model_invocation = true;

        registry.register_skill(skill1);
        registry.register_skill(skill2);

        let auto = registry.get_auto_invocable_skills();
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].name, "auto");
    }

    #[test]
    fn test_unregister_plugin() {
        let mut registry = ComponentRegistry::new();

        // Register skills from a plugin
        let skill = create_test_skill("hello", Some("my-plugin"));
        registry.register_skill(skill);

        // Register the plugin
        let plugin = ExtensionPlugin {
            name: "my-plugin".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            path: PathBuf::from("/test"),
            enabled: true,
            skills: vec![],
            commands: vec![],
            agents: vec![],
            hooks: vec![],
            mcp_servers: HashMap::new(),
        };
        registry.register_plugin(plugin);

        assert_eq!(registry.skill_count(), 1);
        assert_eq!(registry.plugin_count(), 1);

        // Unregister plugin
        registry.unregister_plugin("my-plugin");

        assert_eq!(registry.skill_count(), 0);
        assert_eq!(registry.plugin_count(), 0);
    }
}

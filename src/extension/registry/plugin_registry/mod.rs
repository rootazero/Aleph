//! Plugin Registry Implementation
//!
//! Central storage for all plugin registrations. The `PluginRegistry` maintains
//! a comprehensive registry of all plugins and their registered components:
//! tools, hooks, services, in-chat commands, skills, agents.

use std::collections::{HashMap, HashSet};

use super::types::{
    AgentRegistration, HookRegistration, PluginDiagnostic, ServiceRegistration, SkillRegistration,
    ToolRegistration,
};
use crate::extension::types::{HookEvent, PluginRecord, PluginStatus};

/// Central registry for all plugin registrations.
///
/// The `PluginRegistry` provides:
/// - Plugin lifecycle management (register, enable, disable, unregister)
/// - Component registration (tools, hooks, services, commands, skills, agents)
/// - Query methods for accessing registered components
/// - Automatic tracking of which plugin registered each component
/// - Priority-ordered hook execution
#[derive(Debug, Default)]
pub struct PluginRegistry {
    /// Registered plugins by ID
    plugins: HashMap<String, PluginRecord>,

    /// Registered tools by name
    tools: HashMap<String, ToolRegistration>,

    /// Registered hooks (sorted by priority)
    hooks: Vec<HookRegistration>,

    /// Registered background services by ID
    services: HashMap<String, ServiceRegistration>,

    /// Registered skills by name. Plugin `commands/` markdown also lives here,
    /// as entries tagged `skill_type = SkillType::Command` — see
    /// [`crate::extension::ExtensionManager::get_all_commands`].
    skills: HashMap<String, SkillRegistration>,

    /// Registered agents by name
    agents: HashMap<String, AgentRegistration>,

    /// Accumulated diagnostics from plugins
    diagnostics: Vec<PluginDiagnostic>,
}

impl PluginRegistry {
    /// Create a new empty plugin registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all registrations from the registry.
    ///
    /// This removes all plugins and their associated components.
    pub fn clear(&mut self) {
        self.plugins.clear();
        self.tools.clear();
        self.hooks.clear();
        self.services.clear();
        self.skills.clear();
        self.agents.clear();
        self.diagnostics.clear();
    }

    // =========================================================================
    // Plugin Management
    // =========================================================================

    /// Register a plugin in the registry.
    ///
    /// If a plugin with the same ID already exists, it will be replaced.
    pub fn register_plugin(&mut self, record: PluginRecord) {
        if self.plugins.contains_key(&record.id) {
            self.unregister_plugin(&record.id);
        }
        self.plugins.insert(record.id.clone(), record);
    }

    /// Get a plugin by ID.
    #[must_use]
    pub fn get_plugin(&self, id: &str) -> Option<&PluginRecord> {
        self.plugins.get(id)
    }

    /// Get a mutable reference to a plugin by ID.
    pub fn get_plugin_mut(&mut self, id: &str) -> Option<&mut PluginRecord> {
        self.plugins.get_mut(id)
    }

    /// List all registered plugins.
    #[must_use]
    pub fn list_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins.values().collect()
    }

    /// List all active (loaded) plugins.
    #[must_use]
    pub fn list_active_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins
            .values()
            .filter(|p| p.status.is_active())
            .collect()
    }

    /// Disable a plugin by ID.
    ///
    /// Returns `true` if the plugin was found and disabled, `false` otherwise.
    pub fn disable_plugin(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.status = PluginStatus::Disabled;
            true
        } else {
            false
        }
    }

    /// Enable a previously disabled plugin by ID.
    ///
    /// Returns `true` if the plugin was found and enabled, `false` otherwise.
    /// Note: This does not re-run plugin initialization; it only changes the status.
    pub fn enable_plugin(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            if matches!(plugin.status, PluginStatus::Disabled) {
                plugin.status = PluginStatus::Loaded;
                return true;
            }
        }
        false
    }

    // =========================================================================
    // Tool Registration
    // =========================================================================

    /// Register a tool.
    ///
    /// The tool is stored under both its namespaced key (`plugin_id:name`) and its
    /// short name (`name`) for backward compatibility. If two plugins register a tool
    /// with the same short name, first-come wins for the short key; both are always
    /// accessible via their namespaced keys.
    pub fn register_tool(&mut self, tool: ToolRegistration) {
        let plugin_id = tool.plugin_id.clone();
        let namespaced_key = format!("{}:{}", tool.plugin_id, tool.name);
        let short_key = tool.name.clone();

        // Update plugin record (track by namespaced key)
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.tool_names.contains(&namespaced_key) {
                plugin.tool_names.push(namespaced_key.clone());
            }
        }

        // Register under short name for backward compat. First-come wins across
        // *different* plugins, but the owning plugin must be able to refresh its
        // own short-key entry on re-registration — otherwise the short key would
        // serve stale metadata/handler while the namespaced key is updated.
        let short_owner = self
            .tools
            .get(&short_key)
            .map(|existing| existing.plugin_id.clone())
            .filter(|owner| owner != &plugin_id);
        match short_owner {
            Some(owner) => {
                tracing::warn!(
                    "Tool short name '{}' from plugin '{}' conflicts with existing tool from plugin '{}'",
                    short_key,
                    plugin_id,
                    owner
                );
            }
            None => {
                self.tools.insert(short_key, tool.clone());
            }
        }
        // Always register under namespaced key
        self.tools.insert(namespaced_key, tool);
    }

    /// Get a tool by name.
    #[must_use]
    pub fn get_tool(&self, name: &str) -> Option<&ToolRegistration> {
        if name.contains(':') {
            return self.tools.get(name);
        }
        self.tools
            .values()
            .filter(|tool| tool.name == name)
            .filter(|tool| {
                self.plugins
                    .get(&tool.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
            .min_by(|a, b| a.plugin_id.cmp(&b.plugin_id))
    }

    /// List all registered tools.
    #[must_use]
    pub fn list_tools(&self) -> Vec<&ToolRegistration> {
        let mut seen = HashSet::new();
        self.tools
            .values()
            .filter(|tool| seen.insert((tool.plugin_id.as_str(), tool.name.as_str())))
            .collect()
    }

    /// List tools from a specific plugin.
    #[must_use]
    pub fn list_tools_for_plugin(&self, plugin_id: &str) -> Vec<&ToolRegistration> {
        let mut seen = HashSet::new();
        self.tools
            .values()
            .filter(|tool| tool.plugin_id == plugin_id)
            .filter(|tool| seen.insert(tool.name.as_str()))
            .collect()
    }

    // =========================================================================
    // Hook Registration
    // =========================================================================

    /// Register a hook.
    ///
    /// Hooks are maintained in priority order (lower priority value = earlier execution).
    pub fn register_hook(&mut self, hook: HookRegistration) {
        let plugin_id = hook.plugin_id.clone();

        self.hooks.push(hook);
        // Sort by priority (stable sort to preserve insertion order for equal priorities)
        self.hooks.sort_by_key(|h| h.priority);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            plugin.hook_count += 1;
        }
    }

    /// Get all hooks registered for a specific event.
    ///
    /// Returns hooks in priority order (lower priority = earlier in list).
    #[must_use]
    pub fn get_hooks_for_event(&self, event: HookEvent) -> Vec<&HookRegistration> {
        self.hooks.iter().filter(|h| h.event == event).collect()
    }

    /// List all registered hooks.
    #[must_use]
    pub fn list_hooks(&self) -> Vec<&HookRegistration> {
        self.hooks.iter().collect()
    }

    // =========================================================================
    // Service Registration
    // =========================================================================

    /// Register a background service.
    pub fn register_service(&mut self, service: ServiceRegistration) {
        let plugin_id = service.plugin_id.clone();
        let service_id = service.id.clone();
        let namespaced_key = format!("{plugin_id}:{service_id}");

        self.services.insert(namespaced_key, service);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.service_ids.contains(&service_id) {
                plugin.service_ids.push(service_id);
            }
        }
    }

    /// Get a service by ID.
    #[must_use]
    pub fn get_service(&self, id: &str) -> Option<&ServiceRegistration> {
        if let Some(service) = self.services.get(id) {
            return Some(service);
        }
        let mut matches = self.services.values().filter(|service| service.id == id);
        let first = matches.next()?;
        if matches.next().is_some() {
            None
        } else {
            Some(first)
        }
    }

    /// List all registered services.
    #[must_use]
    pub fn list_services(&self) -> Vec<&ServiceRegistration> {
        self.services.values().collect()
    }

    // =========================================================================
    // In-Chat Command Registration
    // =========================================================================

    // =========================================================================
    // Skill Registration
    // =========================================================================

    /// Register a skill.
    ///
    /// The skill is stored under both its namespaced key (`plugin_id:name`) and its
    /// short name (`name`) for backward compatibility. First-come wins for the short key.
    pub fn register_skill(&mut self, skill: SkillRegistration) {
        let namespaced_key = format!("{}:{}", skill.plugin_id, skill.name);
        let short_key = skill.name.clone();

        // Register under short name for backward compat (first-come wins)
        self.skills
            .entry(short_key)
            .or_insert_with(|| skill.clone());
        // Always register under namespaced key
        self.skills.insert(namespaced_key, skill);
    }

    /// Get a skill by name.
    #[must_use]
    pub fn get_skill(&self, name: &str) -> Option<&SkillRegistration> {
        if name.contains(':') {
            return self.skills.get(name);
        }
        self.skills
            .values()
            .filter(|skill| skill.name == name)
            .filter(|skill| {
                self.plugins
                    .get(&skill.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
            .min_by(|a, b| a.plugin_id.cmp(&b.plugin_id))
    }

    /// List all registered skills.
    #[must_use]
    pub fn list_skills(&self) -> Vec<&SkillRegistration> {
        let mut seen = HashSet::new();
        self.skills
            .values()
            .filter(|skill| seen.insert((skill.plugin_id.as_str(), skill.name.as_str())))
            .collect()
    }

    // =========================================================================
    // Agent Registration
    // =========================================================================

    /// Register an agent.
    ///
    /// The agent is stored under both its namespaced key (`plugin_id:name`) and its
    /// short name (`name`) for backward compatibility. First-come wins for the short key.
    pub fn register_agent(&mut self, agent: AgentRegistration) {
        let namespaced_key = format!("{}:{}", agent.plugin_id, agent.name);
        let short_key = agent.name.clone();

        // Register under short name for backward compat (first-come wins)
        self.agents
            .entry(short_key)
            .or_insert_with(|| agent.clone());
        // Always register under namespaced key
        self.agents.insert(namespaced_key, agent);
    }

    /// Get an agent by name.
    #[must_use]
    pub fn get_agent(&self, name: &str) -> Option<&AgentRegistration> {
        if name.contains(':') {
            return self.agents.get(name);
        }
        self.agents
            .values()
            .filter(|agent| agent.name == name)
            .filter(|agent| {
                self.plugins
                    .get(&agent.plugin_id)
                    .is_some_and(|plugin| plugin.status.is_active())
            })
            .min_by(|a, b| a.plugin_id.cmp(&b.plugin_id))
    }

    /// List all registered agents.
    #[must_use]
    pub fn list_agents(&self) -> Vec<&AgentRegistration> {
        let mut seen = HashSet::new();
        self.agents
            .values()
            .filter(|agent| seen.insert((agent.plugin_id.as_str(), agent.name.as_str())))
            .collect()
    }

    // =========================================================================
    // Diagnostics
    // =========================================================================

    /// Add a diagnostic message.
    pub fn add_diagnostic(&mut self, diagnostic: PluginDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Get all diagnostic messages.
    #[must_use]
    pub fn diagnostics(&self) -> &[PluginDiagnostic] {
        &self.diagnostics
    }

    /// Clear all diagnostic messages.
    pub fn clear_diagnostics(&mut self) {
        self.diagnostics.clear();
    }

    // =========================================================================
    // Unregistration
    // =========================================================================

    /// Unregister a plugin and all its associated components.
    ///
    /// This removes:
    /// - The plugin record
    /// - All tools registered by this plugin
    /// - All hooks registered by this plugin
    /// - All services registered by this plugin
    /// - All in-chat commands registered by this plugin
    /// - All skills registered by this plugin
    /// - All agents registered by this plugin
    /// - All diagnostics from this plugin
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        // Remove the plugin record
        self.plugins.remove(plugin_id);

        // Remove all tools from this plugin
        self.tools.retain(|_, t| t.plugin_id != plugin_id);

        // Remove all hooks from this plugin
        self.hooks.retain(|h| h.plugin_id != plugin_id);

        // Remove all services from this plugin
        self.services.retain(|_, s| s.plugin_id != plugin_id);

        // Remove all skills (and `commands/`-derived Command-typed skills) from
        // this plugin
        self.skills.retain(|_, s| s.plugin_id != plugin_id);

        // Remove all agents from this plugin
        self.agents.retain(|_, a| a.plugin_id != plugin_id);

        // Remove all diagnostics from this plugin
        self.diagnostics
            .retain(|d| d.plugin_id.as_deref() != Some(plugin_id));
    }

    // =========================================================================
    // Statistics
    // =========================================================================

    /// Get registry statistics.
    #[must_use]
    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            plugins: self.plugins.len(),
            active_plugins: self.list_active_plugins().len(),
            tools: self.list_tools().len(),
            hooks: self.hooks.len(),
            services: self.services.len(),
            commands: self
                .list_skills()
                .into_iter()
                .filter(|s| s.skill_type == crate::extension::types::SkillType::Command)
                .count(),
            skills: self
                .list_skills()
                .into_iter()
                .filter(|s| s.skill_type == crate::extension::types::SkillType::Skill)
                .count(),
            agents: self.list_agents().len(),
            diagnostics: self.diagnostics.len(),
        }
    }
}

/// Registry statistics.
#[derive(Debug, Clone, Default)]
pub struct RegistryStats {
    pub plugins: usize,
    pub active_plugins: usize,
    pub tools: usize,
    pub hooks: usize,
    pub services: usize,
    pub commands: usize,
    pub skills: usize,
    pub agents: usize,
    pub diagnostics: usize,
}

//! Plugin Registry Implementation
//!
//! Central storage for all plugin registrations. The `PluginRegistry` maintains
//! a comprehensive registry of all plugins and their registered components:
//! tools, hooks, services, in-chat commands, skills, agents.

use std::collections::{HashMap, HashSet, VecDeque};

use super::types::{
    AgentRegistration, ContextRegistration, HookRegistration, PluginDiagnostic,
    ServiceRegistration, SkillRegistration, ToolRegistration,
};
use crate::extension::capability::ContextGrantAudit;
use crate::extension::types::{PluginRecord, PluginStatus};
use crate::extension::visibility::{PluginInstanceKey, ScopeKey};

mod context;

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
    /// Group exact instances by logical ID for bare-name resolution.
    plugins: HashMap<String, HashMap<ScopeKey, PluginRecord>>,

    /// Registered tools by name
    tools: HashMap<String, ToolRegistration>,

    /// Registered hooks (sorted by priority)
    hooks: Vec<HookRegistration>,

    /// Registered background services by ID
    services: HashMap<String, ServiceRegistration>,

    /// Registered skills by name. Plugin `commands/` markdown also lives here,
    /// as entries tagged `skill_type = SkillType::Command` — see
    /// `extension/slash_effect.rs`.
    skills: HashMap<String, SkillRegistration>,

    /// Registered agents by name
    agents: HashMap<String, AgentRegistration>,

    /// Accumulated diagnostics from plugins
    diagnostics: Vec<PluginDiagnostic>,

    /// Context capability registrations, keyed by one namespaced component key.
    contexts: HashMap<String, ContextRegistration>,

    /// Auditable grant validation decisions.
    context_audit: VecDeque<ContextGrantAudit>,

    /// Total audit decisions since construction/clear; retained records are bounded.
    context_audit_total: u64,

    /// Plugin sources the load that built these rows could not read
    /// (`discovery::PluginDiscovery::unreadable`). Non-empty means the rows
    /// are INCOMPLETE: a plugin missing from them may be unknown rather than
    /// uninstalled. Held here, beside the rows, so a reader gets both from
    /// one guard and they are cleared together.
    unreadable_sources: Vec<std::path::PathBuf>,
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
        self.contexts.clear();
        self.context_audit.clear();
        self.context_audit_total = 0;
        self.diagnostics.clear();
        self.unreadable_sources.clear();
    }

    /// Record the sources the current load could not read (see the field).
    pub fn set_unreadable_sources(&mut self, sources: Vec<std::path::PathBuf>) {
        self.unreadable_sources = sources;
    }

    /// The sources the load behind these rows could not read. Non-empty ⇒
    /// the plugin list is incomplete, not authoritative.
    #[must_use]
    pub fn unreadable_sources(&self) -> &[std::path::PathBuf] {
        &self.unreadable_sources
    }

    // =========================================================================
    // Plugin Management
    // =========================================================================

    /// Register or replace the record for one exact instance.
    pub fn register_plugin(&mut self, record: PluginRecord) {
        let instance = record.instance_key();
        if self.get_plugin_instance(&instance).is_some() {
            self.unregister_plugin_instance(&instance);
        }
        self.plugins
            .entry(instance.plugin_id)
            .or_default()
            .insert(instance.scope, record);
    }

    pub(crate) fn get_plugin_instance(
        &self,
        instance: &PluginInstanceKey,
    ) -> Option<&PluginRecord> {
        self.plugins.get(&instance.plugin_id)?.get(&instance.scope)
    }

    pub(crate) fn get_plugin_instance_mut(
        &mut self,
        instance: &PluginInstanceKey,
    ) -> Option<&mut PluginRecord> {
        self.plugins
            .get_mut(&instance.plugin_id)?
            .get_mut(&instance.scope)
    }

    pub(crate) fn get_plugin_by_runtime_key(&self, runtime_key: &str) -> Option<&PluginRecord> {
        let id = PluginInstanceKey::runtime_group(runtime_key);
        self.plugin_instances(id)
            .find(|record| record.instance_key().runtime_key() == runtime_key)
    }

    pub(crate) fn active_plugin(&self, runtime_key: &str) -> Option<&PluginRecord> {
        self.get_plugin_by_runtime_key(runtime_key)
            .filter(|record| record.status.is_active())
    }

    fn get_plugin_by_runtime_key_mut(&mut self, runtime_key: &str) -> Option<&mut PluginRecord> {
        let instance = self.get_plugin_by_runtime_key(runtime_key)?.instance_key();
        self.get_plugin_instance_mut(&instance)
    }

    pub(crate) fn plugin_instances(&self, id: &str) -> impl Iterator<Item = &PluginRecord> {
        self.plugins
            .get(id)
            .into_iter()
            .flat_map(|rows| rows.values())
    }

    /// Legacy lookup for an unambiguous logical ID.
    #[must_use]
    pub fn get_plugin(&self, id: &str) -> Option<&PluginRecord> {
        let rows = self.plugins.get(id)?;
        (rows.len() == 1).then(|| rows.values().next()).flatten()
    }

    /// Legacy mutable lookup for an unambiguous logical ID.
    pub fn get_plugin_mut(&mut self, id: &str) -> Option<&mut PluginRecord> {
        let rows = self.plugins.get_mut(id)?;
        (rows.len() == 1)
            .then(|| rows.values_mut().next())
            .flatten()
    }

    /// List all registered plugins.
    #[must_use]
    pub fn list_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins
            .values()
            .flat_map(|rows| rows.values())
            .collect()
    }

    /// List all active (loaded) plugins.
    #[must_use]
    pub fn list_active_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins
            .values()
            .flat_map(|rows| rows.values())
            .filter(|p| p.status.is_active())
            .collect()
    }

    /// Disable a plugin by ID.
    ///
    /// Returns `true` if the plugin was found and disabled, `false` otherwise.
    /// There is no `enable_plugin` twin: re-enabling is a `mount`, which
    /// writes `Loaded` through `register_plugin_row`.
    pub fn disable_plugin(&mut self, id: &str) -> bool {
        self.get_plugin(id)
            .map(PluginRecord::instance_key)
            .is_some_and(|instance| self.disable_plugin_instance(&instance))
    }

    pub(crate) fn disable_plugin_instance(&mut self, instance: &PluginInstanceKey) -> bool {
        let Some(plugin) = self.get_plugin_instance_mut(instance) else {
            return false;
        };
        let was_active = plugin.status.is_active();
        plugin.status = PluginStatus::Disabled;
        let id = &instance.plugin_id;
        if was_active || !self.plugin_instances(id).any(|row| row.status.is_active()) {
            self.contexts.retain(|_, registration| {
                registration.plugin_id.is_empty() || registration.plugin_id != *id
            });
        }
        true
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
        let namespaced_key =
            crate::extension::namespaced_component_key(&tool.plugin_id, &tool.name);
        let short_key = tool.name.clone();

        if let Some(plugin) = self.get_plugin_by_runtime_key_mut(&plugin_id) {
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
            .filter(|tool| self.active_plugin(&tool.plugin_id).is_some())
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

        if let Some(plugin) = self.get_plugin_by_runtime_key_mut(&plugin_id) {
            plugin.hook_count += 1;
        }
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
        let namespaced_key = crate::extension::namespaced_component_key(&plugin_id, &service_id);

        self.services.insert(namespaced_key, service);

        if let Some(plugin) = self.get_plugin_by_runtime_key_mut(&plugin_id) {
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
    /// short name (`name`) for backward compatibility. First-come wins for the short key,
    /// except that the registering plugin may refresh its own short-key entry.
    pub fn register_skill(&mut self, skill: SkillRegistration) {
        let namespaced_key =
            crate::extension::namespaced_component_key(&skill.plugin_id, &skill.name);
        let short_key = skill.name.clone();

        // Register under short name: first-come wins, but the owning plugin
        // may refresh its own entry (same-owner refresh rule).
        let short_entry = self
            .skills
            .entry(short_key)
            .or_insert_with(|| skill.clone());
        if short_entry.plugin_id == skill.plugin_id {
            *short_entry = skill.clone();
        }
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
            .filter(|skill| self.active_plugin(&skill.plugin_id).is_some())
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
        let namespaced_key =
            crate::extension::namespaced_component_key(&agent.plugin_id, &agent.name);
        let short_key = agent.name.clone();

        // Only claim the short key when it is unowned or already owned by this plugin.
        match self.agents.get(&short_key) {
            Some(existing) if existing.plugin_id != agent.plugin_id => {}
            _ => {
                self.agents.insert(short_key, agent.clone());
            }
        }
        // Always register under namespaced key
        self.agents.insert(namespaced_key, agent);
    }

    /// Get an agent by name.
    ///
    /// Test-only: production resolves agents through the registrar
    /// (`AgentRegistration::qualified_name` / the registry map) — this lookup
    /// by bare name has no production caller.
    #[cfg(test)]
    #[must_use]
    pub fn get_agent(&self, name: &str) -> Option<&AgentRegistration> {
        if name.contains(':') {
            return self.agents.get(name);
        }
        self.agents
            .values()
            .filter(|agent| agent.name == name)
            .filter(|agent| self.active_plugin(&agent.plugin_id).is_some())
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
        if let Some(instance) = self.get_plugin(plugin_id).map(PluginRecord::instance_key) {
            self.unregister_plugin_instance(&instance);
        }
    }

    pub(crate) fn unregister_plugin_instance(&mut self, instance: &PluginInstanceKey) {
        let Some(rows) = self.plugins.get_mut(&instance.plugin_id) else {
            return;
        };
        if rows.remove(&instance.scope).is_none() {
            return;
        }
        if rows.is_empty() {
            self.plugins.remove(&instance.plugin_id);
        }

        let owner = instance.runtime_key();
        self.tools.retain(|_, tool| tool.plugin_id != owner);
        self.hooks.retain(|hook| hook.plugin_id != owner);
        self.services
            .retain(|_, service| service.plugin_id != owner);
        self.skills.retain(|_, skill| skill.plugin_id != owner);
        self.agents.retain(|_, agent| agent.plugin_id != owner);
        self.contexts
            .retain(|_, registration| registration.plugin_id != owner);
        self.diagnostics
            .retain(|diagnostic| diagnostic.plugin_id.as_deref() != Some(owner.as_str()));
    }
}

#[cfg(test)]
mod agent_refresh_tests {
    use super::*;

    #[test]
    fn agent_refresh_updates_own_short_key_but_not_another_plugins() {
        let mut registry = PluginRegistry::default();
        let mut agent = AgentRegistration {
            plugin_id: "owner".into(),
            name: "reviewer".into(),
            content: "old".into(),
            ..Default::default()
        };
        registry.register_agent(agent.clone());
        agent.content = "new".into();
        registry.register_agent(agent.clone());
        assert_eq!(registry.agents["reviewer"].content, "new");
        assert_eq!(registry.agents["owner:reviewer"].content, "new");
        agent.plugin_id = "other".into();
        agent.content = "other content".into();
        registry.register_agent(agent);
        assert_eq!(registry.agents["reviewer"].plugin_id, "owner");
        assert_eq!(registry.agents["reviewer"].content, "new");
        assert_eq!(registry.agents["other:reviewer"].content, "other content");
    }
}

//! Agent registry for managing agent definitions.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::agents::types::{AgentDef, AgentMode};

/// Registry for managing agent definitions
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentDef>>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with built-in agents
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        for agent in builtin_agents() {
            registry.register(agent);
        }
        registry
    }

    /// Register an agent definition
    pub fn register(&self, agent: AgentDef) {
        let mut agents = self.agents.write().unwrap();
        agents.insert(agent.id.clone(), agent);
    }

    /// Get an agent by ID
    pub fn get(&self, id: &str) -> Option<AgentDef> {
        let agents = self.agents.read().unwrap();
        agents.get(id).cloned()
    }

    /// List all registered agent IDs
    pub fn list_ids(&self) -> Vec<String> {
        let agents = self.agents.read().unwrap();
        agents.keys().cloned().collect()
    }

    /// List all sub-agents (excluding primary)
    pub fn list_subagents(&self) -> Vec<AgentDef> {
        let agents = self.agents.read().unwrap();
        agents
            .values()
            .filter(|a| a.mode == AgentMode::SubAgent)
            .cloned()
            .collect()
    }

    /// Remove an agent by ID
    pub fn unregister(&self, id: &str) -> Option<AgentDef> {
        let mut agents = self.agents.write().unwrap();
        agents.remove(id)
    }
}

/// Returns the built-in agent definitions
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        // Main agent - full access
        AgentDef::new("main", AgentMode::Primary, include_str!("prompts/main.md")),
        // Explore agent - read-only tools
        AgentDef::new(
            "explore",
            AgentMode::SubAgent,
            include_str!("prompts/explore.md"),
        )
        .with_allowed_tools(vec![
            "glob".into(),
            "grep".into(),
            "read_file".into(),
            "web_fetch".into(),
            "search".into(),
        ])
        .with_denied_tools(vec![
            "write_file".into(),
            "edit_file".into(),
            "bash".into(),
        ])
        .with_max_iterations(20),
        // Coder agent - file operations
        AgentDef::new(
            "coder",
            AgentMode::SubAgent,
            include_str!("prompts/coder.md"),
        )
        .with_allowed_tools(vec![
            "read_file".into(),
            "write_file".into(),
            "edit_file".into(),
            "glob".into(),
            "grep".into(),
        ])
        .with_max_iterations(30),
        // Researcher agent - search and web
        AgentDef::new(
            "researcher",
            AgentMode::SubAgent,
            include_str!("prompts/researcher.md"),
        )
        .with_allowed_tools(vec![
            "search".into(),
            "web_fetch".into(),
            "read_file".into(),
        ])
        .with_denied_tools(vec![
            "write_file".into(),
            "edit_file".into(),
            "bash".into(),
        ])
        .with_max_iterations(15),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = AgentRegistry::new();
        assert!(registry.list_ids().is_empty());
    }

    #[test]
    fn test_registry_register_and_get() {
        let registry = AgentRegistry::new();
        let agent = AgentDef::new("test", AgentMode::SubAgent, "Test prompt");

        registry.register(agent);

        let retrieved = registry.get("test").unwrap();
        assert_eq!(retrieved.id, "test");
        assert_eq!(retrieved.system_prompt, "Test prompt");
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = AgentRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_list_ids() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("a", AgentMode::SubAgent, ""));
        registry.register(AgentDef::new("b", AgentMode::SubAgent, ""));

        let ids = registry.list_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"a".to_string()));
        assert!(ids.contains(&"b".to_string()));
    }

    #[test]
    fn test_registry_list_subagents() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("main", AgentMode::Primary, ""));
        registry.register(AgentDef::new("explore", AgentMode::SubAgent, ""));
        registry.register(AgentDef::new("coder", AgentMode::SubAgent, ""));

        let subagents = registry.list_subagents();
        assert_eq!(subagents.len(), 2);
        assert!(subagents.iter().all(|a| a.mode == AgentMode::SubAgent));
    }

    #[test]
    fn test_registry_unregister() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("test", AgentMode::SubAgent, ""));

        let removed = registry.unregister("test");
        assert!(removed.is_some());
        assert!(registry.get("test").is_none());
    }

    #[test]
    fn test_with_builtins() {
        let registry = AgentRegistry::with_builtins();

        assert!(registry.get("main").is_some());
        assert!(registry.get("explore").is_some());
        assert!(registry.get("coder").is_some());
        assert!(registry.get("researcher").is_some());
    }

    #[test]
    fn test_builtin_agents_count() {
        let agents = builtin_agents();
        assert_eq!(agents.len(), 4);
    }

    #[test]
    fn test_explore_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let explore = registry.get("explore").unwrap();

        assert_eq!(explore.mode, AgentMode::SubAgent);
        assert!(explore.is_tool_allowed("glob"));
        assert!(explore.is_tool_allowed("grep"));
        assert!(!explore.is_tool_allowed("write_file"));
        assert!(!explore.is_tool_allowed("bash"));
        assert_eq!(explore.max_iterations, Some(20));
    }

    #[test]
    fn test_coder_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let coder = registry.get("coder").unwrap();

        assert!(coder.is_tool_allowed("write_file"));
        assert!(coder.is_tool_allowed("edit_file"));
        assert_eq!(coder.max_iterations, Some(30));
    }

    #[test]
    fn test_researcher_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let researcher = registry.get("researcher").unwrap();

        assert!(researcher.is_tool_allowed("search"));
        assert!(researcher.is_tool_allowed("web_fetch"));
        assert!(!researcher.is_tool_allowed("write_file"));
        assert_eq!(researcher.max_iterations, Some(15));
    }
}

//! Agent registry for managing agent definitions.

use crate::sync_primitives::RwLock;
use std::collections::HashMap;

use crate::agents::types::{AgentDef, AgentMode, ContextMode};

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
        let mut agents = self.agents.write().unwrap_or_else(|e| e.into_inner());
        agents.insert(agent.id.clone(), agent);
    }

    /// Get an agent by ID
    pub fn get(&self, id: &str) -> Option<AgentDef> {
        let agents = self.agents.read().unwrap_or_else(|e| e.into_inner());
        agents.get(id).cloned()
    }

    /// List all registered agent IDs (sorted for deterministic output)
    pub fn list_ids(&self) -> Vec<String> {
        let agents = self.agents.read().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<String> = agents.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// List all sub-agents (excluding primary, sorted by id)
    pub fn list_subagents(&self) -> Vec<AgentDef> {
        let agents = self.agents.read().unwrap_or_else(|e| e.into_inner());
        let mut result: Vec<AgentDef> = agents
            .values()
            .filter(|a| a.mode == AgentMode::SubAgent)
            .cloned()
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Remove an agent by ID
    pub fn unregister(&self, id: &str) -> Option<AgentDef> {
        let mut agents = self.agents.write().unwrap_or_else(|e| e.into_inner());
        agents.remove(id)
    }
}

/// Returns the built-in agent definitions
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        // Main agent - full access
        AgentDef::new("main", AgentMode::Primary)
            .with_description("Primary agent that responds directly to user"),
        // Explore agent - read-only tools
        AgentDef::new("explore", AgentMode::SubAgent)
            .with_description("Read-only codebase exploration specialist")
            .with_when_to_use("When you need to search, read, or understand code without modifying anything")
            .with_prompt_sections(vec!["explore_constraints".into()])
            .with_allowed_tools(vec![
                "glob".into(),
                "grep".into(),
                "read_file".into(),
                "web_fetch".into(),
                "search".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(20),
        // Coder agent - file operations
        AgentDef::new("coder", AgentMode::SubAgent)
            .with_description("Code writing specialist with file operations")
            .with_when_to_use("When you need to write, edit, or create code files")
            .with_prompt_sections(vec!["coder_guidelines".into()])
            .with_allowed_tools(vec![
                "read_file".into(),
                "write_file".into(),
                "edit_file".into(),
                "glob".into(),
                "grep".into(),
                "bash".into(),
            ])
            .with_max_iterations(30),
        // Researcher agent - search and web
        AgentDef::new("researcher", AgentMode::SubAgent)
            .with_description("Web and document research specialist")
            .with_when_to_use("When you need to search the web, fetch URLs, or gather external information")
            .with_prompt_sections(vec!["researcher_protocol".into()])
            .with_allowed_tools(vec![
                "search".into(),
                "web_fetch".into(),
                "read_file".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(15),
        // Default agent - general-purpose sub-agent
        AgentDef::new("default", AgentMode::SubAgent)
            .with_description("General-purpose sub-agent")
            .with_when_to_use("When no specialized agent fits the task")
            .with_context_mode(ContextMode::Summary),
        // Plan agent - read-only planner
        AgentDef::new("plan", AgentMode::SubAgent)
            .with_description("Read-only planning and analysis specialist")
            .with_when_to_use("When you need to analyze requirements, design architecture, or create plans")
            .with_prompt_sections(vec!["plan_protocol".into()])
            .with_allowed_tools(vec![
                "glob".into(),
                "grep".into(),
                "read_file".into(),
                "bash".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into()])
            .with_max_iterations(20)
            .with_context_mode(ContextMode::Summary),
        // Verify agent - adversarial verifier (read-only)
        AgentDef::new("verify", AgentMode::SubAgent)
            .with_description("Adversarial verification specialist")
            .with_when_to_use("When you need to independently verify that work was done correctly")
            .with_prompt_sections(vec!["verify_protocol".into()])
            .with_allowed_tools(vec!["*".into()])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into()])
            .with_max_iterations(25)
            .with_context_mode(ContextMode::Summary),
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
        let agent = AgentDef::new("test", AgentMode::SubAgent);

        registry.register(agent);

        let retrieved = registry.get("test").unwrap();
        assert_eq!(retrieved.id, "test");
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = AgentRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_list_ids() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("a", AgentMode::SubAgent));
        registry.register(AgentDef::new("b", AgentMode::SubAgent));

        let ids = registry.list_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"a".to_string()));
        assert!(ids.contains(&"b".to_string()));
    }

    #[test]
    fn test_registry_list_subagents() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("main", AgentMode::Primary));
        registry.register(AgentDef::new("explore", AgentMode::SubAgent));
        registry.register(AgentDef::new("coder", AgentMode::SubAgent));

        let subagents = registry.list_subagents();
        assert_eq!(subagents.len(), 2);
        assert!(subagents.iter().all(|a| a.mode == AgentMode::SubAgent));
    }

    #[test]
    fn test_registry_unregister() {
        let registry = AgentRegistry::new();
        registry.register(AgentDef::new("test", AgentMode::SubAgent));

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
        assert_eq!(agents.len(), 7);
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
        assert!(coder.is_tool_allowed("bash"));
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

    #[test]
    fn test_verify_agent_config() {
        let registry = AgentRegistry::with_builtins();
        let verify = registry.get("verify").unwrap();
        assert_eq!(verify.mode, AgentMode::SubAgent);
        assert!(verify.is_tool_allowed("glob"));
        assert!(verify.is_tool_allowed("bash"));
        assert!(!verify.is_tool_allowed("write_file")); // read-only: write denied
        assert!(!verify.is_tool_allowed("edit_file")); // read-only: edit denied
        assert_eq!(verify.max_iterations, Some(25));
        assert_eq!(verify.prompt_sections, vec!["verify_protocol"]);
    }

    #[test]
    fn test_builtin_subagents_have_descriptions() {
        let registry = AgentRegistry::with_builtins();
        let subagents = registry.list_subagents();
        for agent in &subagents {
            assert!(!agent.description.is_empty(), "Agent '{}' should have a description", agent.id);
        }
    }

    #[test]
    fn test_builtin_subagents_have_when_to_use() {
        let registry = AgentRegistry::with_builtins();
        let subagents = registry.list_subagents();
        for agent in &subagents {
            assert!(agent.when_to_use.is_some(), "Agent '{}' should have when_to_use", agent.id);
        }
    }

    #[test]
    fn test_builtin_agents_have_prompt_sections() {
        let agents = builtin_agents();
        let by_id: std::collections::HashMap<&str, &AgentDef> =
            agents.iter().map(|a| (a.id.as_str(), a)).collect();

        // Main has no sections (uses default prompt builder flow)
        assert!(by_id["main"].prompt_sections.is_empty());

        // Sub-agents each have their specific section
        assert_eq!(
            by_id["explore"].prompt_sections,
            vec!["explore_constraints"]
        );
        assert_eq!(by_id["coder"].prompt_sections, vec!["coder_guidelines"]);
        assert_eq!(
            by_id["researcher"].prompt_sections,
            vec!["researcher_protocol"]
        );
        assert_eq!(by_id["verify"].prompt_sections, vec!["verify_protocol"]);
    }
}

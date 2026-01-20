//! Agent type definitions.

use serde::{Deserialize, Serialize};

/// Mode of an agent
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentMode {
    /// Main agent that responds directly to user
    Primary,
    /// Sub-agent called by other agents
    SubAgent,
}

/// Definition of an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    /// Unique identifier (e.g., "explore", "coder", "researcher")
    pub id: String,
    /// Agent mode
    pub mode: AgentMode,
    /// System prompt for the agent
    pub system_prompt: String,
    /// Tools this agent is allowed to use ("*" for all)
    pub allowed_tools: Vec<String>,
    /// Tools this agent is denied from using
    pub denied_tools: Vec<String>,
    /// Maximum iterations (overrides default loop limit)
    pub max_iterations: Option<u32>,
}

impl AgentDef {
    /// Create a new agent definition
    pub fn new(id: impl Into<String>, mode: AgentMode, system_prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            mode,
            system_prompt: system_prompt.into(),
            allowed_tools: vec!["*".into()],
            denied_tools: vec![],
            max_iterations: None,
        }
    }

    /// Set allowed tools
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    /// Set denied tools
    pub fn with_denied_tools(mut self, tools: Vec<String>) -> Self {
        self.denied_tools = tools;
        self
    }

    /// Set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Check if a tool is allowed for this agent
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Check denied list first
        if self.denied_tools.contains(&tool_name.to_string()) {
            return false;
        }

        // Check allowed list
        if self.allowed_tools.contains(&"*".to_string()) {
            return true;
        }

        self.allowed_tools.contains(&tool_name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_def_new() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "Test prompt");
        assert_eq!(agent.id, "test");
        assert_eq!(agent.mode, AgentMode::SubAgent);
        assert_eq!(agent.system_prompt, "Test prompt");
        assert_eq!(agent.allowed_tools, vec!["*"]);
        assert!(agent.denied_tools.is_empty());
        assert!(agent.max_iterations.is_none());
    }

    #[test]
    fn test_is_tool_allowed_wildcard() {
        let agent = AgentDef::new("test", AgentMode::Primary, "");
        assert!(agent.is_tool_allowed("any_tool"));
        assert!(agent.is_tool_allowed("another_tool"));
    }

    #[test]
    fn test_is_tool_allowed_specific() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "")
            .with_allowed_tools(vec!["read_file".into(), "glob".into()]);

        assert!(agent.is_tool_allowed("read_file"));
        assert!(agent.is_tool_allowed("glob"));
        assert!(!agent.is_tool_allowed("write_file"));
    }

    #[test]
    fn test_is_tool_denied() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "")
            .with_denied_tools(vec!["bash".into(), "write_file".into()]);

        assert!(!agent.is_tool_allowed("bash"));
        assert!(!agent.is_tool_allowed("write_file"));
        assert!(agent.is_tool_allowed("read_file"));
    }

    #[test]
    fn test_denied_overrides_allowed() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "")
            .with_allowed_tools(vec!["bash".into()])
            .with_denied_tools(vec!["bash".into()]);

        // Denied takes precedence
        assert!(!agent.is_tool_allowed("bash"));
    }

    #[test]
    fn test_with_max_iterations() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "").with_max_iterations(20);

        assert_eq!(agent.max_iterations, Some(20));
    }
}

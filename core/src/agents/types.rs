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

/// Context mode for sub-agents
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextMode {
    /// Start with a fresh context (no parent history)
    Fresh,
    /// Receive a summary of parent context
    Summary,
}

impl Default for ContextMode {
    fn default() -> Self {
        Self::Fresh
    }
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
    /// Token budget override for this agent's loop
    pub token_budget: Option<u32>,
    /// Suggested model to use (e.g., "fast", "deep")
    pub model_hint: Option<String>,
    /// Context mode: whether sub-agent gets parent context
    pub context_mode: ContextMode,
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
            token_budget: None,
            model_hint: None,
            context_mode: ContextMode::default(),
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

    /// Set context mode
    pub fn with_context_mode(mut self, mode: ContextMode) -> Self {
        self.context_mode = mode;
        self
    }

    /// Set token budget
    pub fn with_token_budget(mut self, budget: u32) -> Self {
        self.token_budget = Some(budget);
        self
    }

    /// Set model hint
    pub fn with_model_hint(mut self, hint: impl Into<String>) -> Self {
        self.model_hint = Some(hint.into());
        self
    }

    /// Check if a tool is allowed for this agent
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Check denied list first
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // Check allowed list
        if self.allowed_tools.iter().any(|t| t == "*") {
            return true;
        }

        self.allowed_tools.iter().any(|t| t == tool_name)
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

    #[test]
    fn test_context_mode_default() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt");
        assert_eq!(agent.context_mode, ContextMode::Fresh);
        assert!(agent.token_budget.is_none());
        assert!(agent.model_hint.is_none());
    }

    #[test]
    fn test_with_context_mode() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt")
            .with_context_mode(ContextMode::Summary);
        assert_eq!(agent.context_mode, ContextMode::Summary);
    }

    #[test]
    fn test_with_token_budget() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt")
            .with_token_budget(50_000);
        assert_eq!(agent.token_budget, Some(50_000));
    }

    #[test]
    fn test_with_model_hint() {
        let agent = AgentDef::new("test", AgentMode::SubAgent, "prompt")
            .with_model_hint("fast");
        assert_eq!(agent.model_hint.as_deref(), Some("fast"));
    }
}

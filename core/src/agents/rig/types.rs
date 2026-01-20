//! Agent Types for Tool Calling Loop
//!
//! Core data structures for the agent loop execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// =============================================================================
// Agent Configuration
// =============================================================================

/// Configuration for the agent loop
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// System prompt for the agent
    pub system_prompt: Option<String>,

    /// Maximum number of turns (LLM calls) allowed
    pub max_turns: usize,

    /// Timeout per turn in milliseconds
    pub turn_timeout_ms: u64,

    /// Whether to stop on first tool error
    pub stop_on_error: bool,

    /// Whether to include tool results in conversation history
    pub include_tool_results: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            max_turns: 50, // Allows complex multi-step tasks
            turn_timeout_ms: 30_000,
            stop_on_error: false,
            include_tool_results: true,
        }
    }
}

impl AgentConfig {
    /// Create a new config with system prompt
    pub fn with_system_prompt(prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: Some(prompt.into()),
            ..Default::default()
        }
    }

    /// Builder: set max turns
    pub fn max_turns(mut self, max: usize) -> Self {
        self.max_turns = max;
        self
    }

    /// Builder: set turn timeout
    pub fn turn_timeout_ms(mut self, timeout: u64) -> Self {
        self.turn_timeout_ms = timeout;
        self
    }

    /// Builder: set stop on error
    pub fn stop_on_error(mut self, stop: bool) -> Self {
        self.stop_on_error = stop;
        self
    }
}

// =============================================================================
// Tool Call Types
// =============================================================================

/// How a tool call relates to the current goal
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GoalRelation {
    /// Directly achieves the goal
    DirectlyAchieves,
    /// Gathers information for subsequent decisions
    GathersInformation,
    /// Validates previous results
    Validates,
    /// Prepares for subsequent steps
    Prepares,
}

/// Information about a tool call from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    /// Unique ID for this tool call
    pub id: String,

    /// Tool name to execute
    pub name: String,

    /// Arguments for the tool (JSON)
    pub arguments: Value,

    // === New fields for context retention ===
    /// Purpose of this call (LLM generated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,

    /// Expected outcome type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_outcome: Option<String>,

    /// Relation to current goal
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_relation: Option<GoalRelation>,
}

impl ToolCallInfo {
    /// Create a new tool call info
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            purpose: None,
            expected_outcome: None,
            goal_relation: None,
        }
    }

    /// Set the purpose of this tool call
    pub fn with_purpose(mut self, purpose: impl Into<String>) -> Self {
        self.purpose = Some(purpose.into());
        self
    }

    /// Set the expected outcome
    pub fn with_expected_outcome(mut self, outcome: impl Into<String>) -> Self {
        self.expected_outcome = Some(outcome.into());
        self
    }

    /// Set the goal relation
    pub fn with_goal_relation(mut self, relation: GoalRelation) -> Self {
        self.goal_relation = Some(relation);
        self
    }
}

/// Result of executing a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Tool call ID (matches ToolCallInfo.id)
    pub tool_call_id: String,

    /// Tool name
    pub name: String,

    /// Result content (string or JSON)
    pub content: String,

    /// Whether execution was successful
    pub success: bool,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    // === New fields for context retention ===
    /// Result summary (human-readable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Contribution to goal
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_contribution: Option<String>,

    /// Extracted knowledge fragments
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extracted_knowledge: Vec<crate::components::Knowledge>,
}

impl ToolCallResult {
    /// Create a successful result
    pub fn success(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.into(),
            content: content.into(),
            success: true,
            error: None,
            duration_ms,
            summary: None,
            goal_contribution: None,
            extracted_knowledge: Vec::new(),
        }
    }

    /// Create a failed result
    pub fn failure(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        error: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.into(),
            content: String::new(),
            success: false,
            error: Some(error.into()),
            duration_ms,
            summary: None,
            goal_contribution: None,
            extracted_knowledge: Vec::new(),
        }
    }

    /// Set result summary
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set goal contribution
    pub fn with_goal_contribution(mut self, contribution: impl Into<String>) -> Self {
        self.goal_contribution = Some(contribution.into());
        self
    }

    /// Add extracted knowledge
    pub fn with_knowledge(mut self, knowledge: crate::components::Knowledge) -> Self {
        self.extracted_knowledge.push(knowledge);
        self
    }
}

// =============================================================================
// Agent Result
// =============================================================================

/// Result of running the agent loop
#[derive(Debug, Clone)]
pub struct AgentResult {
    /// Final response from the agent
    pub response: String,

    /// Number of tool calls made
    pub tool_calls_made: usize,

    /// Total turns (LLM calls) made
    pub turns: usize,

    /// Total execution time in milliseconds
    pub total_duration_ms: u64,

    /// Whether the agent completed successfully
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Tool call history
    pub tool_history: Vec<ToolCallResult>,
}

impl AgentResult {
    /// Create a successful result
    pub fn success(
        response: String,
        tool_calls_made: usize,
        turns: usize,
        total_duration_ms: u64,
        tool_history: Vec<ToolCallResult>,
    ) -> Self {
        Self {
            response,
            tool_calls_made,
            turns,
            total_duration_ms,
            success: true,
            error: None,
            tool_history,
        }
    }

    /// Create a failed result
    pub fn failure(
        error: impl Into<String>,
        turns: usize,
        total_duration_ms: u64,
        tool_history: Vec<ToolCallResult>,
    ) -> Self {
        Self {
            response: String::new(),
            tool_calls_made: tool_history.len(),
            turns,
            total_duration_ms,
            success: false,
            error: Some(error.into()),
            tool_history,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.max_turns, 50); // 50 turns for complex tasks
        assert_eq!(config.turn_timeout_ms, 30_000);
        assert!(!config.stop_on_error);
    }

    #[test]
    fn test_agent_config_builder() {
        let config = AgentConfig::with_system_prompt("You are a helper")
            .max_turns(5)
            .turn_timeout_ms(10_000)
            .stop_on_error(true);

        assert_eq!(config.system_prompt, Some("You are a helper".to_string()));
        assert_eq!(config.max_turns, 5);
        assert_eq!(config.turn_timeout_ms, 10_000);
        assert!(config.stop_on_error);
    }

    #[test]
    fn test_tool_call_info() {
        let info = ToolCallInfo::new("call_123", "search", serde_json::json!({"query": "test"}));

        assert_eq!(info.id, "call_123");
        assert_eq!(info.name, "search");
    }

    #[test]
    fn test_tool_call_result_success() {
        let result = ToolCallResult::success("call_123", "search", "Found results", 150);

        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.content, "Found results");
    }

    #[test]
    fn test_tool_call_result_failure() {
        let result = ToolCallResult::failure("call_123", "search", "Connection timeout", 5000);

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_agent_result_success() {
        let result = AgentResult::success("Done!".to_string(), 2, 3, 5000, vec![]);

        assert!(result.success);
        assert_eq!(result.response, "Done!");
        assert_eq!(result.tool_calls_made, 2);
        assert_eq!(result.turns, 3);
    }

    #[test]
    fn test_agent_result_failure() {
        let result = AgentResult::failure("Max turns exceeded", 10, 30000, vec![]);

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_tool_call_info_with_purpose() {
        let info = ToolCallInfo::new("call_123", "search_files", serde_json::json!({"pattern": "*.toml"}))
            .with_purpose("Find configuration files to determine build method")
            .with_expected_outcome("List of config file paths")
            .with_goal_relation(GoalRelation::GathersInformation);

        assert_eq!(info.purpose, Some("Find configuration files to determine build method".to_string()));
        assert_eq!(info.expected_outcome, Some("List of config file paths".to_string()));
        assert_eq!(info.goal_relation, Some(GoalRelation::GathersInformation));
    }

    #[test]
    fn test_tool_call_result_with_summary() {
        use crate::components::Knowledge;

        let result = ToolCallResult::success("call_123", "search_files", "Found 3 files", 150)
            .with_summary("Located config files: Cargo.toml, .env, settings.json")
            .with_goal_contribution("Config file locations confirmed")
            .with_knowledge(Knowledge::new("config_path", "./Cargo.toml", "search_files"));

        assert_eq!(result.summary, Some("Located config files: Cargo.toml, .env, settings.json".to_string()));
        assert_eq!(result.goal_contribution, Some("Config file locations confirmed".to_string()));
        assert_eq!(result.extracted_knowledge.len(), 1);
    }
}

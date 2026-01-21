//! Skill definition types for the Skill DAG layer

use crate::three_layer::safety::Capability;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Definition of a Skill in the middle layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description of what this skill does
    pub description: String,
    /// Input JSON schema
    #[serde(default)]
    pub input_schema: Option<Value>,
    /// Output JSON schema
    #[serde(default)]
    pub output_schema: Option<Value>,
    /// Required capabilities
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    /// Cost estimate
    #[serde(default)]
    pub cost_estimate: CostEstimate,
    /// Retry policy
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// DAG nodes
    #[serde(default)]
    pub nodes: Vec<SkillNode>,
    /// DAG edges (from_id, to_id)
    #[serde(default)]
    pub edges: Vec<(String, String)>,
}

impl SkillDefinition {
    /// Create a new skill definition
    pub fn new(id: String, name: String, description: String) -> Self {
        Self {
            id,
            name,
            description,
            input_schema: None,
            output_schema: None,
            required_capabilities: Vec::new(),
            cost_estimate: CostEstimate::default(),
            retry_policy: RetryPolicy::default(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Add required capabilities
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.required_capabilities = capabilities;
        self
    }

    /// Add nodes
    pub fn with_nodes(mut self, nodes: Vec<SkillNode>) -> Self {
        self.nodes = nodes;
        self
    }

    /// Add edges
    pub fn with_edges(mut self, edges: Vec<(String, String)>) -> Self {
        self.edges = edges;
        self
    }
}

/// Cost estimate for a skill execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Estimated max tokens
    pub max_tokens: u64,
    /// Estimated max tool calls
    pub max_tool_calls: u32,
}

impl Default for CostEstimate {
    fn default() -> Self {
        Self {
            max_tokens: 10_000,
            max_tool_calls: 10,
        }
    }
}

/// Retry policy for skill execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum retries
    pub max_retries: u32,
    /// Initial backoff in seconds
    pub initial_backoff_secs: u64,
    /// Maximum backoff in seconds
    pub max_backoff_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_secs: 1,
            max_backoff_secs: 30,
        }
    }
}

impl RetryPolicy {
    /// Get initial backoff as Duration
    pub fn initial_backoff(&self) -> Duration {
        Duration::from_secs(self.initial_backoff_secs)
    }

    /// Get max backoff as Duration
    pub fn max_backoff(&self) -> Duration {
        Duration::from_secs(self.max_backoff_secs)
    }
}

/// A node in the Skill DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillNode {
    /// Node ID (unique within skill)
    pub id: String,
    /// Node type
    pub node_type: SkillNodeType,
}

impl SkillNode {
    /// Create a tool invocation node
    pub fn tool(id: &str, tool_id: &str, args_template: Value) -> Self {
        Self {
            id: id.to_string(),
            node_type: SkillNodeType::Tool {
                tool_id: tool_id.to_string(),
                args_template,
            },
        }
    }

    /// Create an LLM processing node
    pub fn llm(id: &str, prompt_template: &str) -> Self {
        Self {
            id: id.to_string(),
            node_type: SkillNodeType::LlmProcess {
                prompt_template: prompt_template.to_string(),
            },
        }
    }

    /// Create a skill invocation node (nested skill)
    pub fn skill(id: &str, skill_id: &str) -> Self {
        Self {
            id: id.to_string(),
            node_type: SkillNodeType::Skill {
                skill_id: skill_id.to_string(),
            },
        }
    }
}

/// Type of skill node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SkillNodeType {
    /// Invoke a tool
    Tool {
        tool_id: String,
        args_template: Value,
    },
    /// Invoke another skill
    Skill {
        skill_id: String,
    },
    /// LLM processing
    LlmProcess {
        prompt_template: String,
    },
    /// Conditional branch
    Condition {
        expression: String,
    },
    /// Parallel fan-out
    Parallel {
        branches: Vec<String>,
    },
    /// Aggregate fan-in
    Aggregate {
        strategy: AggregateStrategy,
    },
}

/// Strategy for aggregating parallel results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AggregateStrategy {
    /// Collect all results into array
    CollectAll,
    /// Take first successful result
    FirstSuccess,
    /// Merge objects
    MergeObjects,
    /// Custom aggregation via LLM
    LlmMerge { prompt: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_definition_basic() {
        let skill = SkillDefinition::new(
            "research".to_string(),
            "Research Skill".to_string(),
            "Research and collect information".to_string(),
        );

        assert_eq!(skill.id, "research");
        assert_eq!(skill.name, "Research Skill");
        assert!(skill.required_capabilities.is_empty());
    }

    #[test]
    fn test_skill_definition_with_capabilities() {
        let skill = SkillDefinition::new(
            "file_analyzer".to_string(),
            "File Analyzer".to_string(),
            "Analyze files".to_string(),
        )
        .with_capabilities(vec![
            Capability::FileRead,
            Capability::LlmCall,
        ]);

        assert_eq!(skill.required_capabilities.len(), 2);
    }

    #[test]
    fn test_skill_node_types() {
        let tool_node = SkillNode::tool("search", "web_search", serde_json::json!({}));
        assert!(matches!(tool_node.node_type, SkillNodeType::Tool { .. }));

        let llm_node = SkillNode::llm("summarize", "Summarize: {{ input }}");
        assert!(matches!(llm_node.node_type, SkillNodeType::LlmProcess { .. }));
    }
}

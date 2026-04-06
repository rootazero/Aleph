//! AgentInfoTool — return full agent definition details for a given agent ID.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::AgentRegistry;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentInfoArgs {
    /// Agent ID to look up (e.g., "explore", "coder", "researcher")
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfoOutput {
    pub id: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    pub mode: String,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    pub context_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u32>,
}

impl fmt::Display for AgentInfoOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Agent: {} ({})", self.id, self.mode)?;
        writeln!(f, "  description: {}", self.description)?;
        if let Some(ref when) = self.when_to_use {
            writeln!(f, "  when_to_use: {}", when)?;
        }
        writeln!(f, "  allowed_tools: {}", self.allowed_tools.join(", "))?;
        if !self.denied_tools.is_empty() {
            writeln!(f, "  denied_tools: {}", self.denied_tools.join(", "))?;
        }
        if let Some(max) = self.max_iterations {
            writeln!(f, "  max_iterations: {}", max)?;
        }
        writeln!(f, "  context_mode: {}", self.context_mode)?;
        Ok(())
    }
}

// =============================================================================
// Tool
// =============================================================================

#[derive(Clone)]
pub struct AgentInfoTool {
    registry: Arc<AgentRegistry>,
}

impl AgentInfoTool {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl AlephTool for AgentInfoTool {
    const NAME: &'static str = "agent_info";
    const DESCRIPTION: &'static str =
        "Get detailed capabilities and configuration of a registered agent. \
         Returns allowed/denied tools, iteration limits, context mode, and usage hints.";

    type Args = AgentInfoArgs;
    type Output = AgentInfoOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"agent_info({"agent_id": "explore"})"#.to_string(),
            r#"agent_info({"agent_id": "coder"})"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "agent_info requested");

        let agent_def = self.registry.get(&args.agent_id).ok_or_else(|| {
            let available = self.registry.list_ids().join(", ");
            AlephError::NotFound(format!(
                "Agent '{}' not found. Available agents: {}",
                args.agent_id, available
            ))
        })?;

        Ok(AgentInfoOutput {
            id: agent_def.id,
            description: agent_def.description,
            when_to_use: agent_def.when_to_use,
            mode: format!("{:?}", agent_def.mode),
            allowed_tools: agent_def.allowed_tools,
            denied_tools: agent_def.denied_tools,
            max_iterations: agent_def.max_iterations,
            context_mode: format!("{:?}", agent_def.context_mode),
            model_hint: agent_def.model_hint,
            token_budget: agent_def.token_budget,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentRegistry;

    fn test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    #[tokio::test]
    async fn test_info_existing_agent() {
        let tool = AgentInfoTool::new(test_registry());
        let result = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await;
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.id, "explore");
        assert!(!info.description.is_empty());
        assert!(info.when_to_use.is_some());
        assert_eq!(info.mode, "SubAgent");
        assert!(info.allowed_tools.contains(&"glob".to_string()));
        assert!(info.denied_tools.contains(&"bash".to_string()));
    }

    #[tokio::test]
    async fn test_info_not_found() {
        let tool = AgentInfoTool::new(test_registry());
        let result = tool
            .call(AgentInfoArgs {
                agent_id: "nonexistent".to_string(),
            })
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_info_tool_definition() {
        let tool = AgentInfoTool::new(test_registry());
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_info");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_info_display_format() {
        let tool = AgentInfoTool::new(test_registry());
        let info = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await
            .unwrap();
        let display = info.to_string();
        assert!(display.contains("explore"));
        assert!(display.contains("SubAgent"));
        assert!(display.contains("context_mode"));
    }
}

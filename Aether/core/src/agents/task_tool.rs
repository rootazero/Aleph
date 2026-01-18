//! TaskTool for calling sub-agents.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::agents::registry::AgentRegistry;
use crate::agents::AgentMode;
use crate::event::{AetherEvent, EventBus, SubAgentRequest};

/// Error type for TaskTool operations
#[derive(Debug, thiserror::Error)]
pub enum TaskToolError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),
    #[error("Cannot call primary agent as sub-agent")]
    CannotCallPrimary,
    #[error("Event bus error: {0}")]
    EventBusError(String),
    #[error("Sub-agent execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),
}

/// Result of a TaskTool execution
#[derive(Debug, Clone)]
pub struct TaskToolResult {
    pub agent_id: String,
    pub child_session_id: String,
    pub summary: String,
    pub success: bool,
}

/// Tool for calling sub-agents
pub struct TaskTool {
    registry: Arc<AgentRegistry>,
    bus: Arc<EventBus>,
}

impl TaskTool {
    /// Create a new TaskTool
    pub fn new(registry: Arc<AgentRegistry>, bus: Arc<EventBus>) -> Self {
        Self { registry, bus }
    }

    /// Get the tool name
    pub fn name(&self) -> &str {
        "task"
    }

    /// Get the tool description
    pub fn description(&self) -> &str {
        "Run a task with a specialized sub-agent. Available agents: explore, coder, researcher"
    }

    /// Get the JSON Schema for parameters
    pub fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "enum": self.available_agents(),
                    "description": "The sub-agent to use"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task description for the sub-agent"
                }
            },
            "required": ["agent", "prompt"]
        })
    }

    /// List available sub-agent IDs
    pub fn available_agents(&self) -> Vec<String> {
        self.registry
            .list_subagents()
            .into_iter()
            .map(|a| a.id)
            .collect()
    }

    /// Execute the TaskTool
    pub async fn execute(
        &self,
        args: Value,
        parent_session_id: &str,
    ) -> Result<TaskToolResult, TaskToolError> {
        // Parse parameters
        let agent_id = args["agent"]
            .as_str()
            .ok_or_else(|| TaskToolError::InvalidParameters("Missing 'agent' parameter".into()))?;
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| TaskToolError::InvalidParameters("Missing 'prompt' parameter".into()))?;

        // Validate agent exists and is a sub-agent
        let agent = self
            .registry
            .get(agent_id)
            .ok_or_else(|| TaskToolError::AgentNotFound(agent_id.into()))?;

        if agent.mode == AgentMode::Primary {
            return Err(TaskToolError::CannotCallPrimary);
        }

        // Generate child session ID
        let child_session_id = Uuid::new_v4().to_string();

        // Create the sub-agent request event
        let request = SubAgentRequest {
            agent_id: agent_id.into(),
            prompt: prompt.into(),
            parent_session_id: parent_session_id.into(),
            child_session_id: child_session_id.clone(),
        };

        // Publish SubAgentStarted event
        self.bus
            .publish(AetherEvent::SubAgentStarted(request))
            .await;

        // Note: In a real implementation, we would wait for SubAgentCompleted
        // For now, return a placeholder result indicating the event was published
        Ok(TaskToolResult {
            agent_id: agent_id.into(),
            child_session_id: child_session_id.clone(),
            summary: format!(
                "Sub-agent '{}' started with session '{}'",
                agent_id, child_session_id
            ),
            success: true,
        })
    }

    /// Execute and wait for completion using a completion channel
    pub async fn execute_and_wait(
        &self,
        args: Value,
        parent_session_id: &str,
        completion_rx: oneshot::Receiver<TaskToolResult>,
    ) -> Result<TaskToolResult, TaskToolError> {
        // Start the sub-agent
        let _ = self.execute(args, parent_session_id).await?;

        // Wait for completion
        completion_rx
            .await
            .map_err(|e| TaskToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn create_test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    fn create_test_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    #[test]
    fn test_task_tool_name() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        assert_eq!(tool.name(), "task");
    }

    #[test]
    fn test_task_tool_description() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        assert!(tool.description().contains("sub-agent"));
    }

    #[test]
    fn test_parameters_schema() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent"].is_object());
        assert!(schema["properties"]["prompt"].is_object());
    }

    #[test]
    fn test_available_agents() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let agents = tool.available_agents();
        assert!(agents.contains(&"explore".to_string()));
        assert!(agents.contains(&"coder".to_string()));
        assert!(agents.contains(&"researcher".to_string()));
        // Main should not be in the list (it's Primary, not SubAgent)
        assert!(!agents.contains(&"main".to_string()));
    }

    #[tokio::test]
    async fn test_execute_invalid_agent() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let args = json!({
            "agent": "nonexistent",
            "prompt": "test"
        });

        let result = tool.execute(args, "session-1").await;
        assert!(matches!(result, Err(TaskToolError::AgentNotFound(_))));
    }

    #[tokio::test]
    async fn test_execute_primary_agent_rejected() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let args = json!({
            "agent": "main",
            "prompt": "test"
        });

        let result = tool.execute(args, "session-1").await;
        assert!(matches!(result, Err(TaskToolError::CannotCallPrimary)));
    }

    #[tokio::test]
    async fn test_execute_missing_agent_param() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let args = json!({
            "prompt": "test"
        });

        let result = tool.execute(args, "session-1").await;
        assert!(matches!(result, Err(TaskToolError::InvalidParameters(_))));
    }

    #[tokio::test]
    async fn test_execute_missing_prompt_param() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let args = json!({
            "agent": "explore"
        });

        let result = tool.execute(args, "session-1").await;
        assert!(matches!(result, Err(TaskToolError::InvalidParameters(_))));
    }

    #[tokio::test]
    async fn test_execute_success() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let tool = TaskTool::new(registry, bus);

        let args = json!({
            "agent": "explore",
            "prompt": "Find all Rust files"
        });

        let result = tool.execute(args, "session-1").await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert_eq!(result.agent_id, "explore");
        assert!(result.success);
        assert!(!result.child_session_id.is_empty());
    }

    #[tokio::test]
    async fn test_execute_publishes_event() {
        let registry = create_test_registry();
        let bus = create_test_bus();
        let mut subscriber = bus.subscribe();
        let tool = TaskTool::new(registry, Arc::clone(&bus));

        let args = json!({
            "agent": "coder",
            "prompt": "Write a test file"
        });

        tool.execute(args, "parent-session").await.unwrap();

        // Check that event was published
        let event = subscriber.recv().await.unwrap();
        match event.event {
            AetherEvent::SubAgentStarted(req) => {
                assert_eq!(req.agent_id, "coder");
                assert_eq!(req.prompt, "Write a test file");
                assert_eq!(req.parent_session_id, "parent-session");
                assert!(!req.child_session_id.is_empty());
            }
            _ => panic!("Expected SubAgentStarted event"),
        }
    }
}

# Phase 4: Sub-agent System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the sub-agent system that allows the main agent to delegate tasks to specialized sub-agents (explore, coder, researcher).

**Architecture:** Create an AgentRegistry to manage agent definitions, implement TaskTool for calling sub-agents, and add a SubAgentHandler component to handle sub-agent lifecycle events. Sub-agents run in forked sessions with filtered tool access.

**Tech Stack:** Rust, async-trait, tokio, serde, uuid

---

## Task 1: Create agents module structure

**Files:**
- Create: `Aether/core/src/agents/mod.rs`
- Create: `Aether/core/src/agents/types.rs`
- Modify: `Aether/core/src/lib.rs`

**Step 1: Create agents module file**

Create `Aether/core/src/agents/mod.rs`:

```rust
//! Agent system for sub-agent delegation.
//!
//! This module provides:
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//! - `TaskTool`: Tool for calling sub-agents

mod registry;
mod task_tool;
mod types;

pub use registry::AgentRegistry;
pub use task_tool::TaskTool;
pub use types::{AgentDef, AgentMode};
```

**Step 2: Create types file**

Create `Aether/core/src/agents/types.rs`:

```rust
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
        let agent = AgentDef::new("test", AgentMode::SubAgent, "")
            .with_max_iterations(20);

        assert_eq!(agent.max_iterations, Some(20));
    }
}
```

**Step 3: Add module to lib.rs**

Add to `Aether/core/src/lib.rs` after `pub mod agent;`:

```rust
pub mod agents; // NEW: Sub-agent system
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test agents::types`
Expected: 6 tests passing

**Step 5: Commit**

```bash
git add Aether/core/src/agents/
git add Aether/core/src/lib.rs
git commit -m "feat(agents): add agent types module with AgentDef and AgentMode"
```

---

## Task 2: Implement AgentRegistry

**Files:**
- Create: `Aether/core/src/agents/registry.rs`
- Modify: `Aether/core/src/agents/mod.rs`

**Step 1: Create registry file**

Create `Aether/core/src/agents/registry.rs`:

```rust
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
        AgentDef::new(
            "main",
            AgentMode::Primary,
            include_str!("prompts/main.md"),
        ),
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
```

**Step 2: Create agent prompt files**

Create directory and prompt files:

`Aether/core/src/agents/prompts/main.md`:
```markdown
You are the main assistant agent. You have access to all available tools and can delegate tasks to specialized sub-agents.

When facing complex tasks, consider delegating to:
- **explore**: For searching and reading files, web research
- **coder**: For writing and editing code files
- **researcher**: For in-depth research and information gathering

Always think step by step and use the most appropriate tools for each task.
```

`Aether/core/src/agents/prompts/explore.md`:
```markdown
You are an exploration agent specialized in searching and reading.

Your capabilities:
- Search files using glob patterns
- Search content using grep
- Read file contents
- Fetch web pages
- Search the web

You cannot modify files. Focus on gathering information and reporting findings.
```

`Aether/core/src/agents/prompts/coder.md`:
```markdown
You are a coding agent specialized in writing and editing code.

Your capabilities:
- Read existing files
- Write new files
- Edit existing files
- Search for files and content

Focus on clean, well-documented code. Follow the project's coding conventions.
```

`Aether/core/src/agents/prompts/researcher.md`:
```markdown
You are a research agent specialized in information gathering.

Your capabilities:
- Search the web
- Fetch and analyze web pages
- Read local files for context

You cannot modify files. Focus on comprehensive research and clear summaries.
```

**Step 3: Update mod.rs**

Update `Aether/core/src/agents/mod.rs` to include registry:

```rust
//! Agent system for sub-agent delegation.
//!
//! This module provides:
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//! - `TaskTool`: Tool for calling sub-agents

mod registry;
mod types;

pub use registry::{builtin_agents, AgentRegistry};
pub use types::{AgentDef, AgentMode};
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test agents::registry`
Expected: 11 tests passing

**Step 5: Commit**

```bash
git add Aether/core/src/agents/
git commit -m "feat(agents): add AgentRegistry with built-in agent definitions"
```

---

## Task 3: Implement TaskTool

**Files:**
- Create: `Aether/core/src/agents/task_tool.rs`
- Modify: `Aether/core/src/agents/mod.rs`

**Step 1: Create task_tool file**

Create `Aether/core/src/agents/task_tool.rs`:

```rust
//! TaskTool for calling sub-agents.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::agents::registry::AgentRegistry;
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

        if agent.mode == crate::agents::AgentMode::Primary {
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
            .await
            .map_err(|e| TaskToolError::EventBusError(e.to_string()))?;

        // Note: In a real implementation, we would wait for SubAgentCompleted
        // For now, return a placeholder result indicating the event was published
        Ok(TaskToolResult {
            agent_id: agent_id.into(),
            summary: format!("Sub-agent '{}' started with session '{}'", agent_id, child_session_id),
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
    use crate::agents::AgentMode;
    use crate::event::EventBusConfig;

    fn create_test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    fn create_test_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new(EventBusConfig::default()))
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
```

**Step 2: Update mod.rs**

Update `Aether/core/src/agents/mod.rs`:

```rust
//! Agent system for sub-agent delegation.
//!
//! This module provides:
//! - `AgentDef`: Agent definition with tools and limits
//! - `AgentMode`: Primary vs SubAgent distinction
//! - `AgentRegistry`: Registry for managing agents
//! - `TaskTool`: Tool for calling sub-agents

mod registry;
mod task_tool;
mod types;

pub use registry::{builtin_agents, AgentRegistry};
pub use task_tool::{TaskTool, TaskToolError, TaskToolResult};
pub use types::{AgentDef, AgentMode};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test agents::task_tool`
Expected: 10 tests passing

**Step 4: Commit**

```bash
git add Aether/core/src/agents/
git commit -m "feat(agents): add TaskTool for calling sub-agents"
```

---

## Task 4: Implement SubAgentHandler Component

**Files:**
- Create: `Aether/core/src/components/subagent_handler.rs`
- Modify: `Aether/core/src/components/mod.rs`

**Step 1: Create subagent_handler file**

Create `Aether/core/src/components/subagent_handler.rs`:

```rust
//! Sub-agent handler component for managing sub-agent lifecycle.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::agents::{AgentDef, AgentRegistry};
use crate::components::types::{ExecutionSession, SessionStatus, SubAgentPart};
use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, SubAgentRequest,
    SubAgentResult,
};

/// Tracks active sub-agent sessions
#[derive(Debug)]
struct SubAgentSession {
    agent_def: AgentDef,
    parent_session_id: String,
    iteration_count: u32,
}

/// Handler for sub-agent lifecycle events
pub struct SubAgentHandler {
    registry: Arc<AgentRegistry>,
    active_sessions: RwLock<HashMap<String, SubAgentSession>>,
}

impl SubAgentHandler {
    /// Create a new SubAgentHandler
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            active_sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Get the agent definition for a sub-agent
    pub fn get_agent(&self, agent_id: &str) -> Option<AgentDef> {
        self.registry.get(agent_id)
    }

    /// Check if a session is active
    pub async fn is_session_active(&self, session_id: &str) -> bool {
        let sessions = self.active_sessions.read().await;
        sessions.contains_key(session_id)
    }

    /// Get the parent session ID for a sub-agent session
    pub async fn get_parent_session(&self, child_session_id: &str) -> Option<String> {
        let sessions = self.active_sessions.read().await;
        sessions
            .get(child_session_id)
            .map(|s| s.parent_session_id.clone())
    }

    /// Get the current iteration count for a sub-agent session
    pub async fn get_iteration_count(&self, session_id: &str) -> Option<u32> {
        let sessions = self.active_sessions.read().await;
        sessions.get(session_id).map(|s| s.iteration_count)
    }

    /// Increment the iteration count for a sub-agent session
    pub async fn increment_iteration(&self, session_id: &str) -> Option<u32> {
        let mut sessions = self.active_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.iteration_count += 1;
            Some(session.iteration_count)
        } else {
            None
        }
    }

    /// Check if a sub-agent has exceeded its max iterations
    pub async fn has_exceeded_max_iterations(&self, session_id: &str) -> bool {
        let sessions = self.active_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            if let Some(max) = session.agent_def.max_iterations {
                return session.iteration_count >= max;
            }
        }
        false
    }

    /// Handle SubAgentStarted event
    async fn handle_started(
        &self,
        request: &SubAgentRequest,
        ctx: &EventContext,
    ) -> Result<(), HandlerError> {
        // Get the agent definition
        let agent_def = self
            .registry
            .get(&request.agent_id)
            .ok_or_else(|| HandlerError::InvalidEvent(format!(
                "Agent not found: {}",
                request.agent_id
            )))?;

        // Create the sub-agent session tracking
        let session = SubAgentSession {
            agent_def,
            parent_session_id: request.parent_session_id.clone(),
            iteration_count: 0,
        };

        // Store the session
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(request.child_session_id.clone(), session);
        }

        tracing::info!(
            agent_id = %request.agent_id,
            child_session_id = %request.child_session_id,
            parent_session_id = %request.parent_session_id,
            "Sub-agent started"
        );

        Ok(())
    }

    /// Handle SubAgentCompleted event
    async fn handle_completed(
        &self,
        result: &SubAgentResult,
        ctx: &EventContext,
    ) -> Result<(), HandlerError> {
        // Remove the session from tracking
        let session = {
            let mut sessions = self.active_sessions.write().await;
            sessions.remove(&result.child_session_id)
        };

        if let Some(session) = session {
            tracing::info!(
                agent_id = %result.agent_id,
                child_session_id = %result.child_session_id,
                success = %result.success,
                iterations = %session.iteration_count,
                "Sub-agent completed"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl EventHandler for SubAgentHandler {
    fn name(&self) -> &str {
        "SubAgentHandler"
    }

    fn subscribed_events(&self) -> Vec<EventType> {
        vec![EventType::SubAgentStarted, EventType::SubAgentCompleted]
    }

    async fn handle(&self, event: &AetherEvent, ctx: &EventContext) -> Result<(), HandlerError> {
        match event {
            AetherEvent::SubAgentStarted(request) => {
                self.handle_started(request, ctx).await
            }
            AetherEvent::SubAgentCompleted(result) => {
                self.handle_completed(result, ctx).await
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventBus, EventBusConfig};

    fn create_test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    fn create_test_context() -> EventContext {
        let bus = Arc::new(EventBus::new(EventBusConfig::default()));
        EventContext {
            bus,
            session_id: "test-session".into(),
        }
    }

    #[tokio::test]
    async fn test_handler_name() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        assert_eq!(handler.name(), "SubAgentHandler");
    }

    #[tokio::test]
    async fn test_subscribed_events() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let events = handler.subscribed_events();

        assert!(events.contains(&EventType::SubAgentStarted));
        assert!(events.contains(&EventType::SubAgentCompleted));
    }

    #[tokio::test]
    async fn test_get_agent() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);

        assert!(handler.get_agent("explore").is_some());
        assert!(handler.get_agent("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_handle_started() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };

        let event = AetherEvent::SubAgentStarted(request);
        handler.handle(&event, &ctx).await.unwrap();

        assert!(handler.is_session_active("child-1").await);
        assert_eq!(
            handler.get_parent_session("child-1").await,
            Some("parent-1".into())
        );
    }

    #[tokio::test]
    async fn test_handle_started_invalid_agent() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        let request = SubAgentRequest {
            agent_id: "nonexistent".into(),
            prompt: "test".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };

        let event = AetherEvent::SubAgentStarted(request);
        let result = handler.handle(&event, &ctx).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_completed() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        // First start a session
        let start_request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AetherEvent::SubAgentStarted(start_request), &ctx)
            .await
            .unwrap();

        assert!(handler.is_session_active("child-1").await);

        // Now complete it
        let result = SubAgentResult {
            agent_id: "explore".into(),
            child_session_id: "child-1".into(),
            summary: "Found 5 files".into(),
            success: true,
            error: None,
        };
        handler
            .handle(&AetherEvent::SubAgentCompleted(result), &ctx)
            .await
            .unwrap();

        assert!(!handler.is_session_active("child-1").await);
    }

    #[tokio::test]
    async fn test_iteration_tracking() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "test".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AetherEvent::SubAgentStarted(request), &ctx)
            .await
            .unwrap();

        assert_eq!(handler.get_iteration_count("child-1").await, Some(0));

        handler.increment_iteration("child-1").await;
        assert_eq!(handler.get_iteration_count("child-1").await, Some(1));

        handler.increment_iteration("child-1").await;
        assert_eq!(handler.get_iteration_count("child-1").await, Some(2));
    }

    #[tokio::test]
    async fn test_max_iterations_check() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        // explore agent has max_iterations = 20
        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "test".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AetherEvent::SubAgentStarted(request), &ctx)
            .await
            .unwrap();

        assert!(!handler.has_exceeded_max_iterations("child-1").await);

        // Simulate 20 iterations
        for _ in 0..20 {
            handler.increment_iteration("child-1").await;
        }

        assert!(handler.has_exceeded_max_iterations("child-1").await);
    }

    #[tokio::test]
    async fn test_increment_nonexistent_session() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);

        assert!(handler.increment_iteration("nonexistent").await.is_none());
    }
}
```

**Step 2: Update components/mod.rs**

Add to `Aether/core/src/components/mod.rs`:

```rust
mod subagent_handler;
pub use subagent_handler::SubAgentHandler;
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test components::subagent_handler`
Expected: 11 tests passing

**Step 4: Commit**

```bash
git add Aether/core/src/components/
git commit -m "feat(components): add SubAgentHandler for sub-agent lifecycle management"
```

---

## Task 5: Add lib.rs exports for agents module

**Files:**
- Modify: `Aether/core/src/lib.rs`

**Step 1: Add agents module declaration and exports**

Add to `Aether/core/src/lib.rs` after `pub mod agent;`:

```rust
pub mod agents; // NEW: Sub-agent system (AgentDef, AgentRegistry, TaskTool)
```

Add to exports section:

```rust
// Agent system exports
pub use crate::agents::{
    builtin_agents, AgentDef, AgentMode, AgentRegistry, TaskTool, TaskToolError, TaskToolResult,
};
```

Add SubAgentHandler to components exports:

```rust
// In the components exports section, add:
SubAgentHandler,
```

**Step 2: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test`
Expected: All tests passing

**Step 3: Commit**

```bash
git add Aether/core/src/lib.rs
git commit -m "feat(lib): add agents module exports"
```

---

## Task 6: Integration tests for sub-agent system

**Files:**
- Create: `Aether/core/src/agents/integration_test.rs`
- Modify: `Aether/core/src/agents/mod.rs`

**Step 1: Create integration test file**

Create `Aether/core/src/agents/integration_test.rs`:

```rust
//! Integration tests for the sub-agent system.

use std::sync::Arc;

use crate::agents::{AgentMode, AgentRegistry, TaskTool};
use crate::components::SubAgentHandler;
use crate::event::{AetherEvent, EventBus, EventBusConfig, EventHandler, EventContext, SubAgentRequest, SubAgentResult};

fn create_test_setup() -> (Arc<AgentRegistry>, Arc<EventBus>, SubAgentHandler, TaskTool) {
    let registry = Arc::new(AgentRegistry::with_builtins());
    let bus = Arc::new(EventBus::new(EventBusConfig::default()));
    let handler = SubAgentHandler::new(Arc::clone(&registry));
    let tool = TaskTool::new(Arc::clone(&registry), Arc::clone(&bus));
    (registry, bus, handler, tool)
}

fn create_context(bus: Arc<EventBus>) -> EventContext {
    EventContext {
        bus,
        session_id: "test-session".into(),
    }
}

#[tokio::test]
async fn test_full_subagent_lifecycle() {
    let (registry, bus, handler, tool) = create_test_setup();
    let mut subscriber = bus.subscribe();
    let ctx = create_context(Arc::clone(&bus));

    // 1. Call TaskTool to start a sub-agent
    let args = serde_json::json!({
        "agent": "explore",
        "prompt": "Find all Rust files"
    });
    let result = tool.execute(args, "parent-session").await.unwrap();
    assert!(result.success);

    // 2. Receive the SubAgentStarted event
    let event = subscriber.recv().await.unwrap();
    let request = match &event.event {
        AetherEvent::SubAgentStarted(req) => req.clone(),
        _ => panic!("Expected SubAgentStarted"),
    };

    // 3. Handler processes the event
    handler.handle(&event.event, &ctx).await.unwrap();
    assert!(handler.is_session_active(&request.child_session_id).await);

    // 4. Simulate sub-agent iterations
    for i in 0..5 {
        let count = handler.increment_iteration(&request.child_session_id).await;
        assert_eq!(count, Some(i + 1));
    }

    // 5. Sub-agent completes
    let completion = SubAgentResult {
        agent_id: request.agent_id.clone(),
        child_session_id: request.child_session_id.clone(),
        summary: "Found 10 Rust files".into(),
        success: true,
        error: None,
    };

    bus.publish(AetherEvent::SubAgentCompleted(completion.clone()))
        .await
        .unwrap();

    // 6. Handler processes completion
    let complete_event = subscriber.recv().await.unwrap();
    handler.handle(&complete_event.event, &ctx).await.unwrap();

    // 7. Session is no longer active
    assert!(!handler.is_session_active(&request.child_session_id).await);
}

#[tokio::test]
async fn test_tool_filter_by_agent() {
    let (registry, _, _, _) = create_test_setup();

    // Test explore agent tool filtering
    let explore = registry.get("explore").unwrap();
    assert!(explore.is_tool_allowed("glob"));
    assert!(explore.is_tool_allowed("grep"));
    assert!(explore.is_tool_allowed("read_file"));
    assert!(!explore.is_tool_allowed("write_file"));
    assert!(!explore.is_tool_allowed("bash"));

    // Test coder agent tool filtering
    let coder = registry.get("coder").unwrap();
    assert!(coder.is_tool_allowed("write_file"));
    assert!(coder.is_tool_allowed("edit_file"));

    // Test researcher agent tool filtering
    let researcher = registry.get("researcher").unwrap();
    assert!(researcher.is_tool_allowed("search"));
    assert!(researcher.is_tool_allowed("web_fetch"));
    assert!(!researcher.is_tool_allowed("bash"));
}

#[tokio::test]
async fn test_max_iterations_enforcement() {
    let (registry, bus, handler, tool) = create_test_setup();
    let ctx = create_context(Arc::clone(&bus));

    // Start explore agent (max_iterations = 20)
    let args = serde_json::json!({
        "agent": "explore",
        "prompt": "test"
    });
    tool.execute(args, "parent").await.unwrap();

    // Get the child session ID from the event
    let mut subscriber = bus.subscribe();
    // Note: We need to subscribe before executing to catch the event
    // Re-subscribe
    drop(subscriber);

    let request = SubAgentRequest {
        agent_id: "explore".into(),
        prompt: "test".into(),
        parent_session_id: "parent".into(),
        child_session_id: "test-child".into(),
    };
    handler.handle(&AetherEvent::SubAgentStarted(request.clone()), &ctx).await.unwrap();

    // Iterate up to max
    for _ in 0..19 {
        assert!(!handler.has_exceeded_max_iterations("test-child").await);
        handler.increment_iteration("test-child").await;
    }

    // At iteration 19, not yet exceeded
    assert!(!handler.has_exceeded_max_iterations("test-child").await);

    // At iteration 20, exceeded
    handler.increment_iteration("test-child").await;
    assert!(handler.has_exceeded_max_iterations("test-child").await);
}

#[tokio::test]
async fn test_nested_subagent_tracking() {
    let (registry, bus, handler, _) = create_test_setup();
    let ctx = create_context(Arc::clone(&bus));

    // Start first sub-agent
    let request1 = SubAgentRequest {
        agent_id: "explore".into(),
        prompt: "Find files".into(),
        parent_session_id: "main-session".into(),
        child_session_id: "child-1".into(),
    };
    handler.handle(&AetherEvent::SubAgentStarted(request1), &ctx).await.unwrap();

    // Start second sub-agent from the first
    let request2 = SubAgentRequest {
        agent_id: "researcher".into(),
        prompt: "Research topic".into(),
        parent_session_id: "child-1".into(),
        child_session_id: "child-2".into(),
    };
    handler.handle(&AetherEvent::SubAgentStarted(request2), &ctx).await.unwrap();

    // Both sessions active
    assert!(handler.is_session_active("child-1").await);
    assert!(handler.is_session_active("child-2").await);

    // Parent tracking
    assert_eq!(
        handler.get_parent_session("child-1").await,
        Some("main-session".into())
    );
    assert_eq!(
        handler.get_parent_session("child-2").await,
        Some("child-1".into())
    );

    // Complete inner first
    let result2 = SubAgentResult {
        agent_id: "researcher".into(),
        child_session_id: "child-2".into(),
        summary: "Research complete".into(),
        success: true,
        error: None,
    };
    handler.handle(&AetherEvent::SubAgentCompleted(result2), &ctx).await.unwrap();

    assert!(!handler.is_session_active("child-2").await);
    assert!(handler.is_session_active("child-1").await);

    // Complete outer
    let result1 = SubAgentResult {
        agent_id: "explore".into(),
        child_session_id: "child-1".into(),
        summary: "Exploration complete".into(),
        success: true,
        error: None,
    };
    handler.handle(&AetherEvent::SubAgentCompleted(result1), &ctx).await.unwrap();

    assert!(!handler.is_session_active("child-1").await);
}

#[tokio::test]
async fn test_subagent_failure_tracking() {
    let (registry, bus, handler, _) = create_test_setup();
    let ctx = create_context(Arc::clone(&bus));

    let request = SubAgentRequest {
        agent_id: "coder".into(),
        prompt: "Write code".into(),
        parent_session_id: "parent".into(),
        child_session_id: "child".into(),
    };
    handler.handle(&AetherEvent::SubAgentStarted(request), &ctx).await.unwrap();

    // Sub-agent fails
    let result = SubAgentResult {
        agent_id: "coder".into(),
        child_session_id: "child".into(),
        summary: "".into(),
        success: false,
        error: Some("Tool execution failed".into()),
    };
    handler.handle(&AetherEvent::SubAgentCompleted(result), &ctx).await.unwrap();

    // Session still cleaned up on failure
    assert!(!handler.is_session_active("child").await);
}

#[tokio::test]
async fn test_builtin_agent_prompts_loaded() {
    let (registry, _, _, _) = create_test_setup();

    let main = registry.get("main").unwrap();
    assert!(!main.system_prompt.is_empty());
    assert!(main.system_prompt.contains("main assistant"));

    let explore = registry.get("explore").unwrap();
    assert!(!explore.system_prompt.is_empty());
    assert!(explore.system_prompt.contains("exploration"));

    let coder = registry.get("coder").unwrap();
    assert!(!coder.system_prompt.is_empty());
    assert!(coder.system_prompt.contains("coding"));

    let researcher = registry.get("researcher").unwrap();
    assert!(!researcher.system_prompt.is_empty());
    assert!(researcher.system_prompt.contains("research"));
}
```

**Step 2: Update agents/mod.rs**

Add to `Aether/core/src/agents/mod.rs`:

```rust
#[cfg(test)]
mod integration_test;
```

**Step 3: Run integration tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test agents::integration_test`
Expected: 7 tests passing

**Step 4: Commit**

```bash
git add Aether/core/src/agents/
git commit -m "test(agents): add integration tests for sub-agent system"
```

---

## Task 7: Final verification

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test`
Expected: All tests passing

**Step 2: Run clippy**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo clippy -- -D warnings`
Expected: No new warnings (existing warnings acceptable)

**Step 3: Build release**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo build --release`
Expected: Build succeeds

**Step 4: Document completion**

Create summary of implemented components:
- `agents/types.rs`: AgentDef, AgentMode
- `agents/registry.rs`: AgentRegistry, builtin_agents()
- `agents/task_tool.rs`: TaskTool for calling sub-agents
- `agents/prompts/`: main.md, explore.md, coder.md, researcher.md
- `components/subagent_handler.rs`: SubAgentHandler lifecycle management

---

## Summary

Phase 4 implements the sub-agent system with:

1. **AgentDef & AgentMode**: Type-safe agent definitions with tool filtering
2. **AgentRegistry**: Central registry with built-in agents (main, explore, coder, researcher)
3. **TaskTool**: Tool interface for calling sub-agents
4. **SubAgentHandler**: Component for managing sub-agent lifecycle and iterations
5. **Integration tests**: Full lifecycle, tool filtering, max iterations, nested agents

This enables the main agent to delegate complex tasks to specialized sub-agents with proper isolation and tool access control.

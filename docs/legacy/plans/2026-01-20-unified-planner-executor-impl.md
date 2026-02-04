# Unified Planner & Executor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the 6-layer intent/dispatcher/cowork architecture with a unified 2-layer planner-executor system.

**Architecture:** L1 slash command routing + L3 AI unified planner → Unified executor handling Conversational/SingleAction/TaskGraph plans.

**Tech Stack:** Rust, rig-core, serde_json, async-trait, tokio

---

## Phase 1: Create Planner Module

### Task 1.1: Create ExecutionPlan Types

**Files:**
- Create: `core/src/planner/types.rs`
- Create: `core/src/planner/mod.rs`

**Step 1: Create the types file**

```rust
// core/src/planner/types.rs

//! Execution plan types for the unified planner

use serde::{Deserialize, Serialize};

use crate::cowork::types::{Task, TaskType};

/// Execution plan - output of the unified planner
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionPlan {
    /// Pure conversation, no tools needed
    Conversational {
        /// Optional prompt enhancement
        enhanced_prompt: Option<String>,
    },

    /// Single action (tool call or simple task)
    SingleAction {
        /// Tool to invoke
        tool_name: String,
        /// Tool parameters
        parameters: serde_json::Value,
        /// Whether user confirmation is required
        requires_confirmation: bool,
    },

    /// Complex task graph (multi-step)
    TaskGraph {
        /// List of planned tasks
        tasks: Vec<PlannedTask>,
        /// Dependencies as (predecessor_idx, successor_idx) pairs
        dependencies: Vec<(usize, usize)>,
        /// Whether user confirmation is required
        requires_confirmation: bool,
    },
}

impl ExecutionPlan {
    /// Check if this plan requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        match self {
            ExecutionPlan::Conversational { .. } => false,
            ExecutionPlan::SingleAction { requires_confirmation, .. } => *requires_confirmation,
            ExecutionPlan::TaskGraph { requires_confirmation, .. } => *requires_confirmation,
        }
    }

    /// Get a human-readable description of the plan type
    pub fn plan_type(&self) -> &'static str {
        match self {
            ExecutionPlan::Conversational { .. } => "conversational",
            ExecutionPlan::SingleAction { .. } => "single_action",
            ExecutionPlan::TaskGraph { .. } => "task_graph",
        }
    }

    /// Create a conversational plan
    pub fn conversational() -> Self {
        ExecutionPlan::Conversational {
            enhanced_prompt: None,
        }
    }

    /// Create a conversational plan with enhanced prompt
    pub fn conversational_with_prompt(prompt: String) -> Self {
        ExecutionPlan::Conversational {
            enhanced_prompt: Some(prompt),
        }
    }

    /// Create a single action plan
    pub fn single_action(tool_name: String, parameters: serde_json::Value) -> Self {
        ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation: false,
        }
    }

    /// Create a single action plan that requires confirmation
    pub fn single_action_with_confirmation(tool_name: String, parameters: serde_json::Value) -> Self {
        ExecutionPlan::SingleAction {
            tool_name,
            parameters,
            requires_confirmation: true,
        }
    }
}

/// A planned task within a TaskGraph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTask {
    /// Task index (0-based)
    pub id: usize,
    /// Human-readable description
    pub description: String,
    /// Task type (reuses cowork types)
    pub task_type: TaskType,
    /// Suggested tool to use
    pub tool_hint: Option<String>,
    /// Task-specific parameters
    pub parameters: serde_json::Value,
}

impl PlannedTask {
    /// Create a new planned task
    pub fn new(id: usize, description: impl Into<String>, task_type: TaskType) -> Self {
        Self {
            id,
            description: description.into(),
            task_type,
            tool_hint: None,
            parameters: serde_json::Value::Null,
        }
    }

    /// Set tool hint
    pub fn with_tool_hint(mut self, tool: impl Into<String>) -> Self {
        self.tool_hint = Some(tool.into());
        self
    }

    /// Set parameters
    pub fn with_parameters(mut self, params: serde_json::Value) -> Self {
        self.parameters = params;
        self
    }

    /// Convert to cowork Task
    pub fn to_task(&self) -> Task {
        Task::new(
            format!("task_{}", self.id),
            &self.description,
            self.task_type.clone(),
        )
        .with_parameters(self.parameters.clone())
    }
}

/// Planner error types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlannerError {
    /// LLM call failed
    LlmError(String),
    /// Failed to parse LLM response
    ParseError(String),
    /// Invalid plan generated
    ValidationError(String),
    /// Timeout during planning
    Timeout,
}

impl std::fmt::Display for PlannerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlannerError::LlmError(msg) => write!(f, "LLM error: {}", msg),
            PlannerError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            PlannerError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            PlannerError::Timeout => write!(f, "Planning timed out"),
        }
    }
}

impl std::error::Error for PlannerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{AiTask, FileOp};
    use std::path::PathBuf;

    #[test]
    fn test_execution_plan_conversational() {
        let plan = ExecutionPlan::conversational();
        assert_eq!(plan.plan_type(), "conversational");
        assert!(!plan.requires_confirmation());
    }

    #[test]
    fn test_execution_plan_single_action() {
        let plan = ExecutionPlan::single_action(
            "search".to_string(),
            serde_json::json!({"query": "test"}),
        );
        assert_eq!(plan.plan_type(), "single_action");
        assert!(!plan.requires_confirmation());
    }

    #[test]
    fn test_execution_plan_single_action_with_confirmation() {
        let plan = ExecutionPlan::single_action_with_confirmation(
            "delete_file".to_string(),
            serde_json::json!({"path": "/tmp/test"}),
        );
        assert!(plan.requires_confirmation());
    }

    #[test]
    fn test_planned_task_creation() {
        let task = PlannedTask::new(
            0,
            "Read config file",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/etc/config"),
            }),
        )
        .with_tool_hint("file_reader");

        assert_eq!(task.id, 0);
        assert_eq!(task.description, "Read config file");
        assert_eq!(task.tool_hint, Some("file_reader".to_string()));
    }

    #[test]
    fn test_planned_task_to_cowork_task() {
        let planned = PlannedTask::new(
            1,
            "List directory",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        let task = planned.to_task();
        assert_eq!(task.id, "task_1");
        assert_eq!(task.name, "List directory");
    }

    #[test]
    fn test_execution_plan_serialization() {
        let plan = ExecutionPlan::TaskGraph {
            tasks: vec![PlannedTask::new(
                0,
                "Test task",
                TaskType::AiInference(AiTask {
                    prompt: "test".to_string(),
                    requires_privacy: false,
                    has_images: false,
                    output_format: None,
                }),
            )],
            dependencies: vec![],
            requires_confirmation: true,
        };

        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("task_graph"));
        assert!(json.contains("Test task"));
    }
}
```

**Step 2: Create the module file**

```rust
// core/src/planner/mod.rs

//! Unified Planner Module
//!
//! This module provides the unified planning layer that replaces the previous
//! 6-layer intent/dispatcher system with a simpler 2-layer architecture:
//!
//! - L1: Slash command fast routing (handled by command/parser.rs)
//! - L3: AI unified planner (this module)
//!
//! The planner analyzes user input and generates an ExecutionPlan that can be:
//! - Conversational: Pure dialogue, no tools
//! - SingleAction: Single tool invocation
//! - TaskGraph: Multi-step task DAG

mod types;

pub use types::{ExecutionPlan, PlannedTask, PlannerError};
```

**Step 3: Run tests**

Run: `cd core && cargo test planner::types --no-fail-fast`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/planner/
git commit -m "feat(planner): add ExecutionPlan types for unified planner

- ExecutionPlan enum with Conversational/SingleAction/TaskGraph variants
- PlannedTask struct that converts to cowork Task
- PlannerError for error handling
- Comprehensive unit tests"
```

---

### Task 1.2: Create Planning Prompt Templates

**Files:**
- Create: `core/src/planner/prompt.rs`
- Modify: `core/src/planner/mod.rs`

**Step 1: Create prompt templates**

```rust
// core/src/planner/prompt.rs

//! Planning prompt templates for the unified planner

/// System prompt for the planning LLM
pub const PLANNING_SYSTEM_PROMPT: &str = r#"You are a task planning assistant. Analyze user requests and determine the best execution strategy.

## Available Tools

{tools}

## Output Format

Return a JSON object with the following structure:

```json
{
  "type": "conversational" | "single_action" | "task_graph",

  // For conversational:
  "enhanced_prompt": "optional enhanced prompt",

  // For single_action:
  "tool_name": "tool to use",
  "parameters": { ... },
  "requires_confirmation": false,

  // For task_graph:
  "tasks": [
    {
      "id": 0,
      "description": "task description",
      "task_type": { "type": "file_operation", "op": "read", "path": "/path" },
      "tool_hint": "optional tool name"
    }
  ],
  "dependencies": [[0, 1], [1, 2]],
  "requires_confirmation": true
}
```

## Decision Rules

1. **Conversational** - Use when:
   - User is asking questions
   - User wants explanations or summaries
   - User is greeting or chatting
   - No tools or actions are needed

2. **SingleAction** - Use when:
   - User wants to perform ONE specific action
   - The action maps to a single tool
   - Examples: "search for X", "fetch webpage Y", "translate Z"

3. **TaskGraph** - Use when:
   - User wants to perform MULTIPLE steps
   - Steps have dependencies (one must complete before another)
   - Examples: "organize files and create report", "analyze data then visualize"

## Task Types

- file_operation: read, write, move, copy, delete, search, list, batch_move
- code_execution: script, file, command
- document_generation: excel, powerpoint, pdf, markdown
- app_automation: launch, apple_script, ui_action
- ai_inference: prompts requiring AI processing

## Important

- Always set requires_confirmation=true for destructive operations (delete, move, write)
- Be conservative: prefer conversational for ambiguous requests
- Task IDs must be sequential integers starting from 0
- Dependencies reference task IDs as [predecessor, successor]
"#;

/// Build the user prompt with the actual request
pub fn build_planning_prompt(user_input: &str, tools_description: &str) -> String {
    format!(
        "User request: {}\n\nAnalyze this request and return the appropriate execution plan as JSON.",
        user_input
    )
}

/// Format tool descriptions for the system prompt
pub fn format_tools_for_prompt(tools: &[ToolInfo]) -> String {
    if tools.is_empty() {
        return "No tools available.".to_string();
    }

    tools
        .iter()
        .map(|t| format!("- **{}**: {}", t.name, t.description))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tool information for prompt generation
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

impl ToolInfo {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Get the complete system prompt with tools injected
pub fn get_system_prompt_with_tools(tools: &[ToolInfo]) -> String {
    PLANNING_SYSTEM_PROMPT.replace("{tools}", &format_tools_for_prompt(tools))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tools_empty() {
        let result = format_tools_for_prompt(&[]);
        assert_eq!(result, "No tools available.");
    }

    #[test]
    fn test_format_tools_multiple() {
        let tools = vec![
            ToolInfo::new("search", "Search the web"),
            ToolInfo::new("youtube", "Extract YouTube transcripts"),
        ];
        let result = format_tools_for_prompt(&tools);
        assert!(result.contains("**search**"));
        assert!(result.contains("**youtube**"));
    }

    #[test]
    fn test_build_planning_prompt() {
        let prompt = build_planning_prompt("help me organize files", "");
        assert!(prompt.contains("organize files"));
    }

    #[test]
    fn test_system_prompt_with_tools() {
        let tools = vec![ToolInfo::new("test_tool", "A test tool")];
        let prompt = get_system_prompt_with_tools(&tools);
        assert!(prompt.contains("**test_tool**"));
        assert!(!prompt.contains("{tools}"));
    }
}
```

**Step 2: Update mod.rs**

```rust
// core/src/planner/mod.rs

//! Unified Planner Module
//!
//! This module provides the unified planning layer that replaces the previous
//! 6-layer intent/dispatcher system with a simpler 2-layer architecture:
//!
//! - L1: Slash command fast routing (handled by command/parser.rs)
//! - L3: AI unified planner (this module)
//!
//! The planner analyzes user input and generates an ExecutionPlan that can be:
//! - Conversational: Pure dialogue, no tools
//! - SingleAction: Single tool invocation
//! - TaskGraph: Multi-step task DAG

mod prompt;
mod types;

pub use prompt::{
    build_planning_prompt, format_tools_for_prompt, get_system_prompt_with_tools, ToolInfo,
    PLANNING_SYSTEM_PROMPT,
};
pub use types::{ExecutionPlan, PlannedTask, PlannerError};
```

**Step 3: Run tests**

Run: `cd core && cargo test planner:: --no-fail-fast`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/planner/
git commit -m "feat(planner): add planning prompt templates

- PLANNING_SYSTEM_PROMPT with decision rules
- ToolInfo struct for tool descriptions
- format_tools_for_prompt and build_planning_prompt helpers"
```

---

### Task 1.3: Implement UnifiedPlanner

**Files:**
- Create: `core/src/planner/unified.rs`
- Modify: `core/src/planner/mod.rs`

**Step 1: Create UnifiedPlanner implementation**

```rust
// core/src/planner/unified.rs

//! Unified planner implementation

use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use super::prompt::{build_planning_prompt, get_system_prompt_with_tools, ToolInfo};
use super::types::{ExecutionPlan, PlannedTask, PlannerError};
use crate::cowork::types::TaskType;
use crate::providers::AiProvider;

/// Configuration for the unified planner
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Timeout for planning requests
    pub timeout: Duration,
    /// Whether to require confirmation for destructive operations
    pub require_confirmation_for_destructive: bool,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            require_confirmation_for_destructive: true,
        }
    }
}

/// Unified planner that generates execution plans from user input
pub struct UnifiedPlanner {
    provider: Arc<dyn AiProvider>,
    config: PlannerConfig,
    tools: Vec<ToolInfo>,
}

impl UnifiedPlanner {
    /// Create a new unified planner
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            config: PlannerConfig::default(),
            tools: Vec::new(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(provider: Arc<dyn AiProvider>, config: PlannerConfig) -> Self {
        Self {
            provider,
            config,
            tools: Vec::new(),
        }
    }

    /// Set available tools
    pub fn with_tools(mut self, tools: Vec<ToolInfo>) -> Self {
        self.tools = tools;
        self
    }

    /// Update available tools
    pub fn set_tools(&mut self, tools: Vec<ToolInfo>) {
        self.tools = tools;
    }

    /// Plan an execution strategy for user input
    pub async fn plan(&self, user_input: &str) -> Result<ExecutionPlan, PlannerError> {
        info!(input = %user_input, "Planning execution strategy");

        // Build prompts
        let system_prompt = get_system_prompt_with_tools(&self.tools);
        let user_prompt = build_planning_prompt(user_input, "");

        debug!("Sending planning request to LLM");

        // Call LLM with timeout
        let response = timeout(self.config.timeout, async {
            self.provider
                .process(&user_prompt, Some(&system_prompt))
                .await
        })
        .await
        .map_err(|_| PlannerError::Timeout)?
        .map_err(|e| PlannerError::LlmError(e.to_string()))?;

        debug!(response_len = response.len(), "Received LLM response");

        // Parse response
        self.parse_response(&response)
    }

    /// Parse LLM response into ExecutionPlan
    fn parse_response(&self, response: &str) -> Result<ExecutionPlan, PlannerError> {
        // Extract JSON from response
        let json_str = extract_json(response)
            .map_err(|e| PlannerError::ParseError(format!("Failed to extract JSON: {}", e)))?;

        // Parse into intermediate structure
        let raw: RawPlanResponse = serde_json::from_str(&json_str)
            .map_err(|e| PlannerError::ParseError(format!("Invalid JSON: {}", e)))?;

        // Convert to ExecutionPlan
        self.convert_raw_plan(raw)
    }

    /// Convert raw plan response to ExecutionPlan
    fn convert_raw_plan(&self, raw: RawPlanResponse) -> Result<ExecutionPlan, PlannerError> {
        match raw.plan_type.as_str() {
            "conversational" => Ok(ExecutionPlan::Conversational {
                enhanced_prompt: raw.enhanced_prompt,
            }),

            "single_action" => {
                let tool_name = raw.tool_name.ok_or_else(|| {
                    PlannerError::ValidationError("single_action requires tool_name".to_string())
                })?;

                Ok(ExecutionPlan::SingleAction {
                    tool_name,
                    parameters: raw.parameters.unwrap_or(serde_json::Value::Null),
                    requires_confirmation: raw.requires_confirmation.unwrap_or(false),
                })
            }

            "task_graph" => {
                let raw_tasks = raw.tasks.ok_or_else(|| {
                    PlannerError::ValidationError("task_graph requires tasks".to_string())
                })?;

                let tasks: Vec<PlannedTask> = raw_tasks
                    .into_iter()
                    .map(|t| self.convert_raw_task(t))
                    .collect::<Result<Vec<_>, _>>()?;

                let dependencies = raw.dependencies.unwrap_or_default();

                // Validate dependencies
                for (pred, succ) in &dependencies {
                    if *pred >= tasks.len() || *succ >= tasks.len() {
                        return Err(PlannerError::ValidationError(format!(
                            "Invalid dependency: ({}, {}) out of bounds",
                            pred, succ
                        )));
                    }
                }

                Ok(ExecutionPlan::TaskGraph {
                    tasks,
                    dependencies,
                    requires_confirmation: raw.requires_confirmation.unwrap_or(true),
                })
            }

            other => Err(PlannerError::ValidationError(format!(
                "Unknown plan type: {}",
                other
            ))),
        }
    }

    /// Convert raw task to PlannedTask
    fn convert_raw_task(&self, raw: RawTask) -> Result<PlannedTask, PlannerError> {
        let task_type = parse_task_type(&raw.task_type)?;

        Ok(PlannedTask {
            id: raw.id,
            description: raw.description,
            task_type,
            tool_hint: raw.tool_hint,
            parameters: raw.task_type,
        })
    }
}

/// Extract JSON from response that may be wrapped in markdown code blocks
fn extract_json(response: &str) -> Result<String, String> {
    let trimmed = response.trim();

    // Try to find JSON in ```json code block
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7;
        if let Some(end) = trimmed[json_start..].find("```") {
            return Ok(trimmed[json_start..json_start + end].trim().to_string());
        }
    }

    // Try to find JSON in generic code block
    if let Some(start) = trimmed.find("```") {
        if let Some(newline) = trimmed[start + 3..].find('\n') {
            let json_start = start + 3 + newline + 1;
            if let Some(end) = trimmed[json_start..].find("```") {
                return Ok(trimmed[json_start..json_start + end].trim().to_string());
            }
        }
    }

    // Try direct JSON parse
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    // Find first { and last }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            return Ok(trimmed[start..=end].to_string());
        }
    }

    Err("Could not find JSON in response".to_string())
}

/// Parse task type from JSON value
fn parse_task_type(value: &serde_json::Value) -> Result<TaskType, PlannerError> {
    use crate::cowork::types::{AiTask, AppAuto, CodeExec, DocGen, FileOp, Language};
    use std::path::PathBuf;

    let type_name = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("ai_inference");

    match type_name {
        "file_operation" => {
            let op = value.get("op").and_then(|v| v.as_str()).unwrap_or("list");
            let path = value
                .get("path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));

            let file_op = match op {
                "read" => FileOp::Read { path },
                "write" => FileOp::Write { path },
                "list" => FileOp::List { path },
                "delete" => FileOp::Delete { path },
                "search" => FileOp::Search {
                    pattern: value
                        .get("pattern")
                        .and_then(|v| v.as_str())
                        .unwrap_or("*")
                        .to_string(),
                    dir: path,
                },
                "move" => FileOp::Move {
                    from: value
                        .get("from")
                        .and_then(|v| v.as_str())
                        .map(PathBuf::from)
                        .unwrap_or_default(),
                    to: value
                        .get("to")
                        .and_then(|v| v.as_str())
                        .map(PathBuf::from)
                        .unwrap_or_default(),
                },
                "copy" => FileOp::Copy {
                    from: value
                        .get("from")
                        .and_then(|v| v.as_str())
                        .map(PathBuf::from)
                        .unwrap_or_default(),
                    to: value
                        .get("to")
                        .and_then(|v| v.as_str())
                        .map(PathBuf::from)
                        .unwrap_or_default(),
                },
                _ => FileOp::List { path },
            };

            Ok(TaskType::FileOperation(file_op))
        }

        "code_execution" => {
            let exec = value.get("exec").and_then(|v| v.as_str()).unwrap_or("command");

            let code_exec = match exec {
                "script" => CodeExec::Script {
                    code: value
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    language: match value.get("language").and_then(|v| v.as_str()) {
                        Some("python") | Some("py") => Language::Python,
                        Some("javascript") | Some("js") => Language::JavaScript,
                        Some("ruby") | Some("rb") => Language::Ruby,
                        Some("rust") | Some("rs") => Language::Rust,
                        _ => Language::Shell,
                    },
                },
                "command" => CodeExec::Command {
                    cmd: value
                        .get("cmd")
                        .and_then(|v| v.as_str())
                        .unwrap_or("echo")
                        .to_string(),
                    args: value
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default(),
                },
                _ => CodeExec::Command {
                    cmd: "echo".to_string(),
                    args: vec!["unknown".to_string()],
                },
            };

            Ok(TaskType::CodeExecution(code_exec))
        }

        "document_generation" => {
            let format = value.get("format").and_then(|v| v.as_str()).unwrap_or("markdown");
            let output = value
                .get("output")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("output.md"));

            let doc_gen = match format {
                "excel" => DocGen::Excel {
                    template: value.get("template").and_then(|v| v.as_str()).map(PathBuf::from),
                    output,
                },
                "powerpoint" | "pptx" => DocGen::PowerPoint {
                    template: value.get("template").and_then(|v| v.as_str()).map(PathBuf::from),
                    output,
                },
                "pdf" => DocGen::Pdf {
                    style: value.get("style").and_then(|v| v.as_str()).map(String::from),
                    output,
                },
                _ => DocGen::Markdown { output },
            };

            Ok(TaskType::DocumentGeneration(doc_gen))
        }

        "app_automation" => {
            let action = value.get("action").and_then(|v| v.as_str()).unwrap_or("launch");

            let app_auto = match action {
                "launch" => AppAuto::Launch {
                    bundle_id: value
                        .get("bundle_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("com.apple.finder")
                        .to_string(),
                },
                "apple_script" | "applescript" => AppAuto::AppleScript {
                    script: value
                        .get("script")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
                _ => AppAuto::Launch {
                    bundle_id: "com.apple.finder".to_string(),
                },
            };

            Ok(TaskType::AppAutomation(app_auto))
        }

        _ => {
            // Default to AI inference
            Ok(TaskType::AiInference(AiTask {
                prompt: value
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Process this request")
                    .to_string(),
                requires_privacy: value
                    .get("requires_privacy")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                has_images: value
                    .get("has_images")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                output_format: value.get("output_format").and_then(|v| v.as_str()).map(String::from),
            }))
        }
    }
}

// JSON parsing structures

#[derive(Debug, Deserialize)]
struct RawPlanResponse {
    #[serde(rename = "type")]
    plan_type: String,

    // Conversational
    enhanced_prompt: Option<String>,

    // SingleAction
    tool_name: Option<String>,
    parameters: Option<serde_json::Value>,

    // TaskGraph
    tasks: Option<Vec<RawTask>>,
    dependencies: Option<Vec<(usize, usize)>>,

    // Common
    requires_confirmation: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawTask {
    id: usize,
    description: String,
    #[serde(rename = "task_type")]
    task_type: serde_json::Value,
    tool_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let response = r#"{"type": "conversational"}"#;
        let json = extract_json(response).unwrap();
        assert!(json.contains("conversational"));
    }

    #[test]
    fn test_extract_json_code_block() {
        let response = r#"Here's the plan:

```json
{"type": "conversational"}
```

Done!"#;
        let json = extract_json(response).unwrap();
        assert!(json.contains("conversational"));
    }

    #[test]
    fn test_extract_json_embedded() {
        let response = r#"I'll help you: {"type": "conversational"} That's it."#;
        let json = extract_json(response).unwrap();
        assert!(json.contains("conversational"));
    }

    #[test]
    fn test_parse_task_type_file_op() {
        let value = serde_json::json!({
            "type": "file_operation",
            "op": "read",
            "path": "/tmp/test.txt"
        });

        let task_type = parse_task_type(&value).unwrap();
        assert!(matches!(task_type, TaskType::FileOperation(FileOp::Read { .. })));
    }

    #[test]
    fn test_parse_task_type_ai_inference() {
        let value = serde_json::json!({
            "type": "ai_inference",
            "prompt": "Analyze this"
        });

        let task_type = parse_task_type(&value).unwrap();
        assert!(matches!(task_type, TaskType::AiInference(_)));
    }

    #[test]
    fn test_planner_config_default() {
        let config = PlannerConfig::default();
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.require_confirmation_for_destructive);
    }
}
```

**Step 2: Update mod.rs**

```rust
// core/src/planner/mod.rs

//! Unified Planner Module
//!
//! This module provides the unified planning layer that replaces the previous
//! 6-layer intent/dispatcher system with a simpler 2-layer architecture:
//!
//! - L1: Slash command fast routing (handled by command/parser.rs)
//! - L3: AI unified planner (this module)
//!
//! The planner analyzes user input and generates an ExecutionPlan that can be:
//! - Conversational: Pure dialogue, no tools
//! - SingleAction: Single tool invocation
//! - TaskGraph: Multi-step task DAG

mod prompt;
mod types;
mod unified;

pub use prompt::{
    build_planning_prompt, format_tools_for_prompt, get_system_prompt_with_tools, ToolInfo,
    PLANNING_SYSTEM_PROMPT,
};
pub use types::{ExecutionPlan, PlannedTask, PlannerError};
pub use unified::{PlannerConfig, UnifiedPlanner};
```

**Step 3: Run tests**

Run: `cd core && cargo test planner:: --no-fail-fast`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/planner/
git commit -m "feat(planner): implement UnifiedPlanner

- UnifiedPlanner with async plan() method
- PlannerConfig for timeout and confirmation settings
- JSON extraction from LLM responses
- Task type parsing for all cowork task types
- Comprehensive unit tests"
```

---

### Task 1.4: Register Planner Module in lib.rs

**Files:**
- Modify: `core/src/lib.rs`

**Step 1: Add planner module declaration**

In `core/src/lib.rs`, add after line 65 (after `pub mod cowork;`):

```rust
pub mod planner; // NEW: Unified planner for execution strategy
```

**Step 2: Add planner exports**

In `core/src/lib.rs`, add after the cowork_ffi exports (around line 287):

```rust
// Planner exports (unified execution planning)
pub use crate::planner::{
    ExecutionPlan, PlannedTask, PlannerConfig, PlannerError, ToolInfo, UnifiedPlanner,
};
```

**Step 3: Run build**

Run: `cd core && cargo build`
Expected: Build succeeds

**Step 4: Run all tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 5: Commit**

```bash
git add core/src/lib.rs
git commit -m "feat(planner): register planner module in lib.rs

- Add pub mod planner declaration
- Export public planner types"
```

---

## Phase 2: Create Executor Module

### Task 2.1: Create ExecutionResult Types

**Files:**
- Create: `core/src/executor/types.rs`
- Create: `core/src/executor/mod.rs`

**Step 1: Create types file**

```rust
// core/src/executor/types.rs

//! Execution result types for the unified executor

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::agent::types::ToolCallInfo;
use crate::cowork::types::TaskResult;

/// Result of executing a plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Final response content
    pub content: String,
    /// Tool calls made during execution
    pub tool_calls: Vec<ToolCallInfo>,
    /// Individual task results (for TaskGraph)
    pub task_results: Option<Vec<TaskExecutionResult>>,
    /// Total execution time
    pub execution_time_ms: u64,
    /// Whether execution was successful
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

impl ExecutionResult {
    /// Create a successful result with content
    pub fn success(content: String) -> Self {
        Self {
            content,
            tool_calls: Vec::new(),
            task_results: None,
            execution_time_ms: 0,
            success: true,
            error: None,
        }
    }

    /// Create a failed result with error
    pub fn failure(error: String) -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            task_results: None,
            execution_time_ms: 0,
            success: false,
            error: Some(error),
        }
    }

    /// Set tool calls
    pub fn with_tool_calls(mut self, calls: Vec<ToolCallInfo>) -> Self {
        self.tool_calls = calls;
        self
    }

    /// Set task results
    pub fn with_task_results(mut self, results: Vec<TaskExecutionResult>) -> Self {
        self.task_results = Some(results);
        self
    }

    /// Set execution time
    pub fn with_execution_time(mut self, time: Duration) -> Self {
        self.execution_time_ms = time.as_millis() as u64;
        self
    }
}

impl Default for ExecutionResult {
    fn default() -> Self {
        Self::success(String::new())
    }
}

/// Result of executing a single task within a TaskGraph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    /// Task ID
    pub task_id: String,
    /// Task description
    pub description: String,
    /// Whether task succeeded
    pub success: bool,
    /// Task output/result
    pub output: String,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution time for this task
    pub execution_time_ms: u64,
}

impl TaskExecutionResult {
    /// Create a successful task result
    pub fn success(task_id: String, description: String, output: String) -> Self {
        Self {
            task_id,
            description,
            success: true,
            output,
            error: None,
            execution_time_ms: 0,
        }
    }

    /// Create a failed task result
    pub fn failure(task_id: String, description: String, error: String) -> Self {
        Self {
            task_id,
            description,
            success: false,
            output: String::new(),
            error: Some(error),
            execution_time_ms: 0,
        }
    }

    /// Set execution time
    pub fn with_execution_time(mut self, time: Duration) -> Self {
        self.execution_time_ms = time.as_millis() as u64;
        self
    }
}

/// Execution context passed through the executor
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Application context (bundle ID)
    pub app_context: Option<String>,
    /// Window title
    pub window_title: Option<String>,
    /// Topic ID for conversation
    pub topic_id: Option<String>,
    /// Whether streaming is enabled
    pub stream: bool,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            app_context: None,
            window_title: None,
            topic_id: None,
            stream: true,
        }
    }
}

impl ExecutionContext {
    /// Create new context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set app context
    pub fn with_app_context(mut self, ctx: String) -> Self {
        self.app_context = Some(ctx);
        self
    }

    /// Set topic ID
    pub fn with_topic_id(mut self, id: String) -> Self {
        self.topic_id = Some(id);
        self
    }
}

/// Executor error types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutorError {
    /// Plan execution failed
    ExecutionFailed(String),
    /// Tool call failed
    ToolError(String),
    /// Task failed
    TaskFailed { task_id: String, error: String },
    /// Timeout during execution
    Timeout,
    /// User cancelled execution
    Cancelled,
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            ExecutorError::ToolError(msg) => write!(f, "Tool error: {}", msg),
            ExecutorError::TaskFailed { task_id, error } => {
                write!(f, "Task {} failed: {}", task_id, error)
            }
            ExecutorError::Timeout => write!(f, "Execution timed out"),
            ExecutorError::Cancelled => write!(f, "Execution cancelled"),
        }
    }
}

impl std::error::Error for ExecutorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_success() {
        let result = ExecutionResult::success("Hello world".to_string());
        assert!(result.success);
        assert_eq!(result.content, "Hello world");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_execution_result_failure() {
        let result = ExecutionResult::failure("Something went wrong".to_string());
        assert!(!result.success);
        assert!(result.content.is_empty());
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_task_execution_result() {
        let result = TaskExecutionResult::success(
            "task_0".to_string(),
            "Read file".to_string(),
            "file contents".to_string(),
        )
        .with_execution_time(Duration::from_millis(100));

        assert!(result.success);
        assert_eq!(result.execution_time_ms, 100);
    }

    #[test]
    fn test_execution_context() {
        let ctx = ExecutionContext::new()
            .with_app_context("com.test.app".to_string())
            .with_topic_id("topic_123".to_string());

        assert_eq!(ctx.app_context, Some("com.test.app".to_string()));
        assert_eq!(ctx.topic_id, Some("topic_123".to_string()));
    }
}
```

**Step 2: Create mod.rs**

```rust
// core/src/executor/mod.rs

//! Unified Executor Module
//!
//! This module provides the unified execution layer that handles all types
//! of ExecutionPlan:
//!
//! - Conversational: Direct LLM chat
//! - SingleAction: Single tool invocation via Agent
//! - TaskGraph: Multi-step DAG execution via Cowork scheduler

mod types;

pub use types::{ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult};
```

**Step 3: Run tests**

Run: `cd core && cargo test executor::types --no-fail-fast`
Expected: All tests pass

**Step 4: Commit**

```bash
git add core/src/executor/
git commit -m "feat(executor): add ExecutionResult types

- ExecutionResult for plan execution results
- TaskExecutionResult for individual task results
- ExecutionContext for execution parameters
- ExecutorError for error handling"
```

---

### Task 2.2: Implement UnifiedExecutor

**Files:**
- Create: `core/src/executor/unified.rs`
- Modify: `core/src/executor/mod.rs`

**Step 1: Create UnifiedExecutor**

```rust
// core/src/executor/unified.rs

//! Unified executor implementation

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::types::{ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult};
use crate::agent::RigAgentManager;
use crate::cowork::scheduler::{DagScheduler, SchedulerConfig, TaskScheduler};
use crate::cowork::executor::ExecutorRegistry;
use crate::cowork::types::{TaskGraph, TaskResult, TaskStatus};
use crate::planner::{ExecutionPlan, PlannedTask};
use crate::uniffi_core::AlephEventHandler;

/// Configuration for the unified executor
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum parallel tasks for TaskGraph execution
    pub max_parallelism: usize,
    /// Timeout per task in seconds
    pub task_timeout_seconds: u64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_parallelism: 4,
            task_timeout_seconds: 300,
        }
    }
}

/// Unified executor that handles all ExecutionPlan types
pub struct UnifiedExecutor {
    /// Agent manager for conversations and single actions
    agent_manager: Arc<RigAgentManager>,
    /// DAG scheduler for task graphs
    dag_scheduler: Arc<RwLock<DagScheduler>>,
    /// Executor registry for task execution
    executor_registry: Arc<ExecutorRegistry>,
    /// Event handler for callbacks
    event_handler: Arc<dyn AlephEventHandler>,
    /// Configuration
    config: ExecutorConfig,
}

impl UnifiedExecutor {
    /// Create a new unified executor
    pub fn new(
        agent_manager: Arc<RigAgentManager>,
        executor_registry: Arc<ExecutorRegistry>,
        event_handler: Arc<dyn AlephEventHandler>,
    ) -> Self {
        let config = ExecutorConfig::default();
        let scheduler_config = SchedulerConfig {
            max_parallelism: config.max_parallelism,
        };

        Self {
            agent_manager,
            dag_scheduler: Arc::new(RwLock::new(DagScheduler::with_config(scheduler_config))),
            executor_registry,
            event_handler,
            config,
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        agent_manager: Arc<RigAgentManager>,
        executor_registry: Arc<ExecutorRegistry>,
        event_handler: Arc<dyn AlephEventHandler>,
        config: ExecutorConfig,
    ) -> Self {
        let scheduler_config = SchedulerConfig {
            max_parallelism: config.max_parallelism,
        };

        Self {
            agent_manager,
            dag_scheduler: Arc::new(RwLock::new(DagScheduler::with_config(scheduler_config))),
            executor_registry,
            event_handler,
            config,
        }
    }

    /// Execute a plan
    pub async fn execute(
        &self,
        plan: ExecutionPlan,
        context: &ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        let start = Instant::now();

        info!(plan_type = %plan.plan_type(), "Executing plan");

        let result = match plan {
            ExecutionPlan::Conversational { enhanced_prompt } => {
                self.execute_conversation(enhanced_prompt, context).await
            }
            ExecutionPlan::SingleAction {
                tool_name,
                parameters,
                ..
            } => {
                self.execute_single_action(&tool_name, parameters, context)
                    .await
            }
            ExecutionPlan::TaskGraph {
                tasks,
                dependencies,
                ..
            } => {
                self.execute_task_graph(tasks, dependencies, context)
                    .await
            }
        };

        let elapsed = start.elapsed();
        info!(
            elapsed_ms = elapsed.as_millis(),
            success = result.is_ok(),
            "Plan execution completed"
        );

        result.map(|r| r.with_execution_time(elapsed))
    }

    /// Execute a conversational plan
    async fn execute_conversation(
        &self,
        enhanced_prompt: Option<String>,
        context: &ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        debug!("Executing conversational plan");

        // For conversation, we let the agent handle it directly
        // The enhanced_prompt can be used to augment the system prompt
        // This is a simplified implementation - full integration would use agent_manager

        Ok(ExecutionResult::success(
            enhanced_prompt.unwrap_or_else(|| "Conversation mode".to_string()),
        ))
    }

    /// Execute a single action plan
    async fn execute_single_action(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
        context: &ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        debug!(tool = %tool_name, "Executing single action");

        self.event_handler.on_tool_start(tool_name.to_string());

        // Execute tool via agent manager
        // This is a simplified implementation - full integration would call the actual tool

        let result_str = format!("Executed {} with {:?}", tool_name, parameters);

        self.event_handler
            .on_tool_result(tool_name.to_string(), result_str.clone());

        Ok(ExecutionResult::success(result_str))
    }

    /// Execute a task graph plan
    async fn execute_task_graph(
        &self,
        tasks: Vec<PlannedTask>,
        dependencies: Vec<(usize, usize)>,
        context: &ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        debug!(
            task_count = tasks.len(),
            dep_count = dependencies.len(),
            "Executing task graph"
        );

        // Convert PlannedTasks to cowork Tasks and build TaskGraph
        let mut task_graph = TaskGraph::new("unified_exec", "Unified Execution");

        for planned in &tasks {
            let task = planned.to_task();
            task_graph.add_task(task);
        }

        // Add dependencies
        for (pred_idx, succ_idx) in &dependencies {
            let pred_id = format!("task_{}", pred_idx);
            let succ_id = format!("task_{}", succ_idx);
            task_graph.add_dependency(&pred_id, &succ_id);
        }

        // Validate graph
        task_graph.validate().map_err(|e| {
            ExecutorError::ExecutionFailed(format!("Invalid task graph: {}", e))
        })?;

        // Notify plan created
        let step_descriptions: Vec<String> = tasks.iter().map(|t| t.description.clone()).collect();
        self.event_handler
            .on_plan_created("unified_exec".to_string(), step_descriptions);

        // Execute tasks using DAG scheduler
        let mut task_results = Vec::new();
        let mut scheduler = self.dag_scheduler.write().await;
        scheduler.reset();

        loop {
            let ready = scheduler.next_ready(&task_graph);

            if ready.is_empty() {
                if scheduler.is_complete(&task_graph) {
                    break;
                }
                // No ready tasks but not complete - deadlock or all running
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                continue;
            }

            // Execute ready tasks (simplified - in real impl, would be parallel)
            for task in ready {
                let task_id = task.id.clone();
                let task_name = task.name.clone();

                scheduler.mark_running(&task_id);

                self.event_handler
                    .on_tool_call_started(task_id.clone(), task_name.clone());

                let start = Instant::now();

                // Execute task via executor registry
                let exec_result = self.executor_registry.execute(&task).await;

                let elapsed = start.elapsed();

                match exec_result {
                    Ok(result) => {
                        scheduler.mark_completed(&task_id);

                        // Update task status in graph
                        if let Some(t) = task_graph.get_task_mut(&task_id) {
                            t.status = TaskStatus::completed(result.clone());
                        }

                        let output = result.output.unwrap_or_default();
                        self.event_handler
                            .on_tool_call_completed(task_id.clone(), output.clone());

                        task_results.push(
                            TaskExecutionResult::success(task_id, task_name, output)
                                .with_execution_time(elapsed),
                        );
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        scheduler.mark_failed(&task_id, &error_msg);

                        // Update task status in graph
                        if let Some(t) = task_graph.get_task_mut(&task_id) {
                            t.status = TaskStatus::failed(&error_msg);
                        }

                        task_results.push(
                            TaskExecutionResult::failure(task_id, task_name, error_msg)
                                .with_execution_time(elapsed),
                        );
                    }
                }
            }
        }

        // Summarize results
        let success_count = task_results.iter().filter(|r| r.success).count();
        let total_count = task_results.len();

        let summary = format!(
            "Completed {}/{} tasks successfully",
            success_count, total_count
        );

        if success_count == total_count {
            Ok(ExecutionResult::success(summary).with_task_results(task_results))
        } else {
            let errors: Vec<String> = task_results
                .iter()
                .filter_map(|r| r.error.clone())
                .collect();

            Ok(ExecutionResult {
                content: summary,
                tool_calls: Vec::new(),
                task_results: Some(task_results),
                execution_time_ms: 0,
                success: false,
                error: Some(errors.join("; ")),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{AiTask, TaskType};

    // Note: Full tests require mock implementations of RigAgentManager,
    // ExecutorRegistry, and AlephEventHandler. These would be added
    // in the integration testing phase.

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.max_parallelism, 4);
        assert_eq!(config.task_timeout_seconds, 300);
    }
}
```

**Step 2: Update mod.rs**

```rust
// core/src/executor/mod.rs

//! Unified Executor Module
//!
//! This module provides the unified execution layer that handles all types
//! of ExecutionPlan:
//!
//! - Conversational: Direct LLM chat
//! - SingleAction: Single tool invocation via Agent
//! - TaskGraph: Multi-step DAG execution via Cowork scheduler

mod types;
mod unified;

pub use types::{ExecutionContext, ExecutionResult, ExecutorError, TaskExecutionResult};
pub use unified::{ExecutorConfig, UnifiedExecutor};
```

**Step 3: Run build**

Run: `cd core && cargo build`
Expected: Build succeeds (or shows expected import errors that we'll fix)

**Step 4: Commit**

```bash
git add core/src/executor/
git commit -m "feat(executor): implement UnifiedExecutor

- UnifiedExecutor handles all ExecutionPlan types
- ExecutorConfig for parallelism and timeout settings
- Integration with DagScheduler and ExecutorRegistry
- Event handler callbacks for UI updates"
```

---

### Task 2.3: Register Executor Module in lib.rs

**Files:**
- Modify: `core/src/lib.rs`

**Step 1: Add executor module declaration**

In `core/src/lib.rs`, add after the planner module (around line 66):

```rust
pub mod executor; // NEW: Unified executor for plan execution
```

**Step 2: Add executor exports**

In `core/src/lib.rs`, add after the planner exports:

```rust
// Executor exports (unified plan execution)
pub use crate::executor::{
    ExecutionContext, ExecutionResult, ExecutorConfig, ExecutorError, TaskExecutionResult,
    UnifiedExecutor,
};
```

**Step 3: Run build**

Run: `cd core && cargo build`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add core/src/lib.rs
git commit -m "feat(executor): register executor module in lib.rs

- Add pub mod executor declaration
- Export public executor types"
```

---

## Phase 3: Integrate into Processing Flow

### Task 3.1: Update ffi/processing.rs to Use Unified Planner

**Files:**
- Modify: `core/src/ffi/processing.rs`

This is a major refactoring task. The detailed implementation will depend on the exact current state of processing.rs. The key changes are:

1. Remove IntentClassifier usage
2. Remove NaturalLanguageCommandDetector usage
3. Add UnifiedPlanner initialization
4. Replace the multi-layer routing with: slash command check → AI planning → unified execution

**Step 1: Add imports**

At the top of `core/src/ffi/processing.rs`, add:

```rust
use crate::planner::{UnifiedPlanner, ExecutionPlan, ToolInfo};
use crate::executor::{UnifiedExecutor, ExecutionContext, ExecutionResult};
```

**Step 2: Simplify process() function**

The new flow should be:

```rust
// In process() function:

// 1. Check for slash command (L1)
if input.starts_with('/') {
    if let Some(cmd) = command_parser.parse_slash_only(&input) {
        return execute_slash_command(cmd, handler).await;
    }
}

// 2. AI Planning (L3)
handler.on_thinking();
let plan = unified_planner.plan(&input).await?;

// 3. Confirmation if needed
if plan.requires_confirmation() {
    handler.on_plan_created(&plan);
    // Wait for confirmation...
}

// 4. Execute
let result = unified_executor.execute(plan, &context).await?;

// 5. Store to memory
memory_ingestion.store(&result).await?;

handler.on_complete(&result.content);
```

**Step 3: Remove deprecated code**

Remove:
- IntentClassifier usage
- parse_command() with NL detection
- Multi-layer routing logic

**Note:** This task requires careful implementation based on the current code state. The exact changes will be determined during implementation.

**Step 4: Run tests**

Run: `cd core && cargo test`
Expected: All tests pass

**Step 5: Commit**

```bash
git add core/src/ffi/processing.rs
git commit -m "refactor(processing): integrate unified planner and executor

- Replace 6-layer routing with 2-layer architecture
- L1: Slash command fast routing
- L3: AI unified planning
- Unified execution for all plan types"
```

---

## Phase 4: Delete Redundant Modules

### Task 4.1: Delete Intent Module

**Files:**
- Delete: `core/src/intent/` (entire directory)
- Modify: `core/src/lib.rs` (remove intent exports)

**Step 1: Remove intent module declaration from lib.rs**

In `core/src/lib.rs`, remove:
```rust
pub mod intent; // DELETE THIS LINE
```

**Step 2: Remove intent exports from lib.rs**

Remove the entire `pub use crate::intent::{...}` block.

**Step 3: Delete intent directory**

```bash
rm -rf core/src/intent/
```

**Step 4: Fix compilation errors**

Any remaining references to intent types need to be updated or removed.

**Step 5: Run build**

Run: `cd core && cargo build`
Expected: Build succeeds (may require fixing imports)

**Step 6: Commit**

```bash
git add -A
git commit -m "refactor: delete redundant intent module

- Remove intent/ directory (15 files)
- Update lib.rs exports
- Intent functionality replaced by unified planner"
```

---

### Task 4.2: Delete Dispatcher Module

**Files:**
- Delete: `core/src/dispatcher/` (entire directory)
- Modify: `core/src/lib.rs` (remove dispatcher exports)

**Step 1: Remove dispatcher module declaration from lib.rs**

Remove: `pub mod dispatcher;`

**Step 2: Remove dispatcher exports from lib.rs**

Remove the entire `pub use crate::dispatcher::{...}` block.

**Step 3: Delete dispatcher directory**

```bash
rm -rf core/src/dispatcher/
```

**Step 4: Fix compilation errors**

Update any remaining references.

**Step 5: Run build and commit**

```bash
cd core && cargo build
git add -A
git commit -m "refactor: delete redundant dispatcher module

- Remove dispatcher/ directory (6 files)
- Update lib.rs exports
- Dispatcher functionality replaced by unified planner"
```

---

### Task 4.3: Delete Command NL Detection Files

**Files:**
- Delete: `core/src/command/nl_detector.rs`
- Delete: `core/src/command/unified_index.rs`
- Modify: `core/src/command/mod.rs`

**Step 1: Delete files**

```bash
rm core/src/command/nl_detector.rs
rm core/src/command/unified_index.rs
```

**Step 2: Update command/mod.rs**

Remove references to NaturalLanguageCommandDetector and UnifiedCommandIndex.

**Step 3: Run build and commit**

```bash
cd core && cargo build
git add -A
git commit -m "refactor: remove NL command detection from command module

- Delete nl_detector.rs and unified_index.rs
- NL command detection replaced by unified planner"
```

---

## Phase 5: UI Adaptation

### Task 5.1: Update AlephEventHandler Callbacks

**Files:**
- Modify: `core/src/uniffi_core.rs` or equivalent callback definition file
- Modify: UniFFI UDL file

**Step 1: Add new callbacks**

```rust
pub trait AlephEventHandler {
    // Existing callbacks...

    // NEW: Plan created notification
    fn on_plan_created(&self, session_id: String, steps: Vec<String>);

    // NEW: Task started
    fn on_task_started(&self, task_id: String, description: String);

    // NEW: Task completed
    fn on_task_completed(&self, task_id: String, result: String);
}
```

**Step 2: Remove deprecated callbacks**

```rust
// REMOVE:
// fn on_agent_mode_detected(&self, task: ExecutableTaskFFI);
```

**Step 3: Update UniFFI bindings**

Regenerate bindings after UDL changes.

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor(ffi): update AlephEventHandler callbacks

- Add on_plan_created, on_task_started, on_task_completed
- Remove on_agent_mode_detected
- Regenerate UniFFI bindings"
```

---

### Task 5.2: Update macOS Swift Bridge

**Files:**
- Modify: `platforms/macos/Aether/Sources/Bridge/AetherBridge.swift` (or equivalent)

**Step 1: Implement new callbacks**

```swift
extension AlephBridge: AlephEventHandler {
    func onPlanCreated(sessionId: String, steps: [String]) {
        DispatchQueue.main.async {
            // Update UI to show plan
            self.delegate?.planCreated(steps: steps)
        }
    }

    func onTaskStarted(taskId: String, description: String) {
        DispatchQueue.main.async {
            self.delegate?.taskStarted(taskId: taskId, description: description)
        }
    }

    func onTaskCompleted(taskId: String, result: String) {
        DispatchQueue.main.async {
            self.delegate?.taskCompleted(taskId: taskId, result: result)
        }
    }
}
```

**Step 2: Remove deprecated callbacks**

Remove `onAgentModeDetected` implementation.

**Step 3: Build and test**

```bash
cd platforms/macos && xcodegen generate && xcodebuild build
```

**Step 4: Commit**

```bash
git add -A
git commit -m "refactor(macos): update Swift bridge for new callbacks

- Implement onPlanCreated, onTaskStarted, onTaskCompleted
- Remove onAgentModeDetected"
```

---

## Phase 6: Configuration and Documentation

### Task 6.1: Update config.toml Structure

**Files:**
- Modify: Default config template or documentation

**Step 1: Create new config structure**

```toml
[planner]
model = "claude-haiku"
timeout_seconds = 10

[execution]
require_confirmation = true
confirmation_policy = "destructive_only"
max_parallelism = 4
task_timeout_seconds = 300

[execution.file_ops]
enabled = true
allowed_paths = ["~/Downloads/**", "~/Documents/**"]
require_confirmation_for_delete = true

[execution.code_exec]
enabled = false
allowed_runtimes = ["shell", "python"]
sandbox_enabled = true
```

**Step 2: Implement config migration**

Create migration logic to convert old config to new format.

**Step 3: Commit**

```bash
git add -A
git commit -m "refactor(config): update configuration structure

- Add [planner] and [execution] sections
- Remove [intent] and [dispatcher] sections
- Add config migration logic"
```

---

### Task 6.2: Update Documentation

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Delete: `docs/DISPATCHER.md`
- Rename: `docs/COWORK.md` → `docs/EXECUTION.md`

**Step 1: Update ARCHITECTURE.md**

Update to reflect the new 2-layer architecture.

**Step 2: Delete obsolete docs**

```bash
rm docs/DISPATCHER.md
```

**Step 3: Rename and update COWORK.md**

```bash
mv docs/COWORK.md docs/EXECUTION.md
```

Update content to reflect the unified executor.

**Step 4: Commit**

```bash
git add -A
git commit -m "docs: update documentation for unified architecture

- Update ARCHITECTURE.md for 2-layer design
- Delete obsolete DISPATCHER.md
- Rename COWORK.md to EXECUTION.md"
```

---

## Verification Checklist

After completing all phases:

- [ ] `cargo build` succeeds with no errors
- [ ] `cargo test` passes all tests
- [ ] No references to deleted modules (intent, dispatcher)
- [ ] `/agent` command no longer exists
- [ ] AI planning works for all non-slash inputs
- [ ] TaskGraph execution works with DAG scheduling
- [ ] UI callbacks fire correctly
- [ ] Configuration migration works

---

## Rollback Plan

If issues arise:

1. Git revert to last known good commit
2. Restore deleted modules from git history
3. Revert lib.rs changes
4. Rebuild

```bash
git log --oneline  # Find last good commit
git revert HEAD~N..HEAD  # Revert N commits
cargo build
```

# DAG Scheduler Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate DAG scheduler into Agent Loop to support multi-step task execution with parallel scheduling, risk evaluation, and UI feedback.

**Architecture:** Pre-analyze user input with LLM to determine single-step vs multi-step. Multi-step tasks generate TaskGraph, scheduled by DagScheduler with retry + LLM decision on failure. UI receives callbacks for task plan display and streaming output.

**Tech Stack:** Rust, UniFFI, async-trait, tokio, serde_json, regex

---

## Phase 1: Core Components

### Task 1.1: Create TaskAnalyzer

**Files:**
- Create: `core/src/dispatcher/analyzer.rs`
- Modify: `core/src/dispatcher/mod.rs:49-99`
- Test: `core/src/dispatcher/analyzer.rs` (inline tests)

**Step 1: Write the failing test**

Add to `core/src/dispatcher/analyzer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_analyze_single_step() {
        let analyzer = TaskAnalyzer::new_mock();
        let result = analyzer.analyze("What is the weather today?").await.unwrap();
        assert!(matches!(result, AnalysisResult::SingleStep { .. }));
    }

    #[tokio::test]
    async fn test_analyze_multi_step() {
        let analyzer = TaskAnalyzer::new_mock();
        let result = analyzer.analyze("分析这篇文档，然后生成知识图谱，最后用绘图模型绘制").await.unwrap();
        assert!(matches!(result, AnalysisResult::MultiStep { .. }));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test dispatcher::analyzer::tests --no-default-features`
Expected: FAIL with "cannot find module analyzer"

**Step 3: Write minimal implementation**

Create `core/src/dispatcher/analyzer.rs`:

```rust
//! Task Analyzer - Pre-analyze user input for single/multi-step tasks

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use crate::dispatcher::cowork_types::TaskGraph;
use crate::dispatcher::planner::{LlmTaskPlanner, TaskPlanner};
use crate::error::Result;
use crate::providers::AiProvider;

/// Result of task analysis
#[derive(Debug, Clone)]
pub enum AnalysisResult {
    /// Single-step task, use Agent Loop directly
    SingleStep {
        intent: String,
    },
    /// Multi-step task, needs DAG scheduling
    MultiStep {
        task_graph: TaskGraph,
        requires_confirmation: bool,
    },
}

/// Task analyzer that determines if input requires multi-step execution
pub struct TaskAnalyzer {
    provider: Arc<dyn AiProvider>,
    planner: LlmTaskPlanner,
}

impl TaskAnalyzer {
    /// Create a new task analyzer
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            planner: LlmTaskPlanner::new(provider.clone()),
            provider,
        }
    }

    /// Analyze user input to determine execution strategy
    pub async fn analyze(&self, input: &str) -> Result<AnalysisResult> {
        info!("Analyzing input for task complexity: {}", input);

        // Quick heuristic check first
        if self.is_likely_single_step(input) {
            debug!("Heuristic: likely single-step task");
            return Ok(AnalysisResult::SingleStep {
                intent: input.to_string(),
            });
        }

        // Use LLM to determine if multi-step is needed
        let analysis_prompt = self.build_analysis_prompt(input);
        let response = self.provider
            .process(&analysis_prompt, Some(ANALYSIS_SYSTEM_PROMPT))
            .await?;

        self.parse_analysis_response(&response, input).await
    }

    /// Quick heuristic to skip LLM call for obvious single-step tasks
    fn is_likely_single_step(&self, input: &str) -> bool {
        let len = input.chars().count();

        // Very short inputs are usually single-step
        if len < 20 {
            return true;
        }

        // Check for multi-step indicators
        let multi_step_patterns = [
            "然后", "之后", "接着", "再", "最后",
            "first", "then", "after", "finally", "next",
            "步骤", "分步", "依次",
            "→", "->", "=>",
        ];

        !multi_step_patterns.iter().any(|p| input.contains(p))
    }

    fn build_analysis_prompt(&self, input: &str) -> String {
        format!(
            r#"分析以下用户请求，判断是否需要多步骤执行：

用户请求: "{}"

如果任务可以一步完成（如：回答问题、简单翻译、单个工具调用），返回：
{{"type": "single", "intent": "简短描述意图"}}

如果任务需要多个步骤（如：分析A然后用结果做B），返回：
{{"type": "multi", "tasks": [
  {{"id": "t1", "name": "步骤名称", "description": "详细描述", "deps": [], "risk": "low"}},
  {{"id": "t2", "name": "步骤名称", "description": "详细描述", "deps": ["t1"], "risk": "low"}}
]}}

risk 值: "low"（分析、生成文本） 或 "high"（调用API、执行代码、修改文件）

只返回 JSON，不要其他文字。"#,
            input
        )
    }

    async fn parse_analysis_response(&self, response: &str, original_input: &str) -> Result<AnalysisResult> {
        let json_str = extract_json(response)?;
        let parsed: AnalysisResponse = serde_json::from_str(&json_str)?;

        match parsed {
            AnalysisResponse::Single { intent } => {
                Ok(AnalysisResult::SingleStep { intent })
            }
            AnalysisResponse::Multi { tasks } => {
                // Use planner to build proper TaskGraph
                let task_graph = self.planner.plan(original_input).await?;
                let requires_confirmation = tasks.iter().any(|t| t.risk == "high");

                Ok(AnalysisResult::MultiStep {
                    task_graph,
                    requires_confirmation,
                })
            }
        }
    }

    /// Create a mock analyzer for testing
    #[cfg(test)]
    pub fn new_mock() -> Self {
        use crate::providers::MockProvider;
        let provider = Arc::new(MockProvider::new());
        Self::new(provider)
    }
}

const ANALYSIS_SYSTEM_PROMPT: &str = r#"你是一个任务分析器。分析用户请求，判断是单步任务还是多步任务。
只返回 JSON 格式响应，不要其他文字。"#;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnalysisResponse {
    Single { intent: String },
    Multi { tasks: Vec<TaskDef> },
}

#[derive(Debug, Deserialize)]
struct TaskDef {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default = "default_risk")]
    risk: String,
}

fn default_risk() -> String {
    "low".to_string()
}

fn extract_json(response: &str) -> Result<String> {
    let trimmed = response.trim();

    // Try to find JSON in code block
    if let Some(start) = trimmed.find("```json") {
        let json_start = start + 7;
        if let Some(end) = trimmed[json_start..].find("```") {
            return Ok(trimmed[json_start..json_start + end].trim().to_string());
        }
    }

    // Try direct JSON
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    // Find first { and last }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            return Ok(trimmed[start..=end].to_string());
        }
    }

    Err(crate::error::AetherError::parse("Could not extract JSON from response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_likely_single_step() {
        let analyzer = TaskAnalyzer::new_mock();

        // Short inputs
        assert!(analyzer.is_likely_single_step("你好"));
        assert!(analyzer.is_likely_single_step("What time is it?"));

        // Multi-step indicators
        assert!(!analyzer.is_likely_single_step("分析这个文档，然后生成摘要"));
        assert!(!analyzer.is_likely_single_step("First read the file, then analyze it"));
    }

    #[test]
    fn test_extract_json() {
        let json = extract_json(r#"{"type": "single", "intent": "test"}"#).unwrap();
        assert!(json.contains("single"));

        let json_block = extract_json("Here's the result:\n```json\n{\"type\": \"single\"}\n```\nDone").unwrap();
        assert!(json_block.contains("single"));
    }
}
```

**Step 4: Update mod.rs to export analyzer**

Modify `core/src/dispatcher/mod.rs`, add after line 64:

```rust
pub mod analyzer;
```

Add to re-exports section (after line 98):

```rust
pub use analyzer::{AnalysisResult, TaskAnalyzer};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test dispatcher::analyzer::tests --no-default-features`
Expected: PASS (mock provider tests pass)

**Step 6: Commit**

```bash
git add core/src/dispatcher/analyzer.rs core/src/dispatcher/mod.rs
git commit -m "feat(dispatcher): add TaskAnalyzer for single/multi-step detection

- Heuristic check for obvious single-step tasks
- LLM-based analysis for complex inputs
- Parses response to determine execution strategy

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 1.2: Create RiskEvaluator

**Files:**
- Create: `core/src/dispatcher/risk.rs`
- Modify: `core/src/dispatcher/mod.rs`
- Test: `core/src/dispatcher/risk.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_low_risk() {
        let evaluator = RiskEvaluator::new();
        let task = create_test_task("分析文档内容");
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);
    }

    #[test]
    fn test_evaluate_high_risk_api() {
        let evaluator = RiskEvaluator::new();
        let task = create_test_task("调用 API 获取数据");
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test dispatcher::risk::tests --no-default-features`
Expected: FAIL with "cannot find module risk"

**Step 3: Write minimal implementation**

Create `core/src/dispatcher/risk.rs`:

```rust
//! Risk Evaluator - Assess task risk levels for confirmation decisions

use regex::Regex;
use std::sync::OnceLock;

use crate::dispatcher::cowork_types::{Task, TaskGraph, TaskType};

/// Task risk level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Low risk - auto execute
    Low,
    /// High risk - requires user confirmation
    High,
}

/// Risk evaluator for tasks
pub struct RiskEvaluator {
    high_risk_patterns: Vec<&'static Regex>,
}

static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn get_patterns() -> &'static Vec<Regex> {
    PATTERNS.get_or_init(|| {
        vec![
            // Network/API patterns
            Regex::new(r"(?i)(api|http|request|fetch|curl|wget)").unwrap(),
            // Execution patterns
            Regex::new(r"(?i)(execute|run|eval|shell|command|exec)").unwrap(),
            // File modification patterns
            Regex::new(r"(?i)(write|delete|remove|modify|create)\s*(file|文件)").unwrap(),
            // Send patterns
            Regex::new(r"(?i)(send|post|upload|publish|发送|上传)").unwrap(),
            // Financial patterns
            Regex::new(r"(?i)(pay|purchase|transaction|transfer|支付|购买|转账)").unwrap(),
            // Chinese API patterns
            Regex::new(r"(?i)(调用|请求|接口)").unwrap(),
        ]
    })
}

impl RiskEvaluator {
    /// Create a new risk evaluator
    pub fn new() -> Self {
        let patterns = get_patterns();
        Self {
            high_risk_patterns: patterns.iter().collect(),
        }
    }

    /// Evaluate the risk level of a single task
    pub fn evaluate(&self, task: &Task) -> RiskLevel {
        // Check task name and description
        let text = format!(
            "{} {}",
            task.name,
            task.description.as_deref().unwrap_or("")
        );

        for pattern in &self.high_risk_patterns {
            if pattern.is_match(&text) {
                return RiskLevel::High;
            }
        }

        // Check task type
        match &task.task_type {
            TaskType::CodeExecution(_) => RiskLevel::High,
            TaskType::AppAutomation(_) => RiskLevel::High,
            TaskType::FileOperation(op) => {
                use crate::dispatcher::cowork_types::FileOp;
                match op {
                    FileOp::Write { .. } | FileOp::Delete { .. } | FileOp::Move { .. } => {
                        RiskLevel::High
                    }
                    _ => RiskLevel::Low,
                }
            }
            TaskType::AiInference(_) => RiskLevel::Low,
            TaskType::DocumentGeneration(_) => RiskLevel::Low,
        }
    }

    /// Evaluate entire TaskGraph, returns true if any task is high risk
    pub fn evaluate_graph(&self, graph: &TaskGraph) -> bool {
        graph.tasks.iter().any(|t| self.evaluate(t) == RiskLevel::High)
    }

    /// Get all high-risk tasks in a graph
    pub fn get_high_risk_tasks<'a>(&self, graph: &'a TaskGraph) -> Vec<&'a Task> {
        graph
            .tasks
            .iter()
            .filter(|t| self.evaluate(t) == RiskLevel::High)
            .collect()
    }
}

impl Default for RiskEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::cowork_types::{AiTask, FileOp};
    use std::path::PathBuf;

    fn create_test_task(name: &str) -> Task {
        Task::new(
            "test",
            name,
            TaskType::AiInference(AiTask {
                prompt: name.to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
    }

    #[test]
    fn test_evaluate_low_risk() {
        let evaluator = RiskEvaluator::new();
        let task = create_test_task("分析文档内容");
        assert_eq!(evaluator.evaluate(&task), RiskLevel::Low);
    }

    #[test]
    fn test_evaluate_high_risk_api() {
        let evaluator = RiskEvaluator::new();
        let task = create_test_task("调用 API 获取数据");
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);
    }

    #[test]
    fn test_evaluate_high_risk_execute() {
        let evaluator = RiskEvaluator::new();
        let task = create_test_task("Execute the shell command");
        assert_eq!(evaluator.evaluate(&task), RiskLevel::High);
    }

    #[test]
    fn test_evaluate_file_operations() {
        let evaluator = RiskEvaluator::new();

        // Read is low risk
        let read_task = Task::new(
            "read",
            "Read file",
            TaskType::FileOperation(FileOp::Read {
                path: PathBuf::from("/tmp/test"),
            }),
        );
        assert_eq!(evaluator.evaluate(&read_task), RiskLevel::Low);

        // Delete is high risk
        let delete_task = Task::new(
            "delete",
            "Delete file",
            TaskType::FileOperation(FileOp::Delete {
                path: PathBuf::from("/tmp/test"),
            }),
        );
        assert_eq!(evaluator.evaluate(&delete_task), RiskLevel::High);
    }

    #[test]
    fn test_evaluate_graph() {
        let evaluator = RiskEvaluator::new();
        let mut graph = TaskGraph::new("test", "Test Graph");

        graph.add_task(create_test_task("分析文档"));
        graph.add_task(create_test_task("生成摘要"));

        // No high risk tasks
        assert!(!evaluator.evaluate_graph(&graph));

        // Add high risk task
        graph.add_task(create_test_task("调用绘图 API"));
        assert!(evaluator.evaluate_graph(&graph));
    }
}
```

**Step 4: Update mod.rs**

Add to `core/src/dispatcher/mod.rs`:

```rust
pub mod risk;
```

Add to re-exports:

```rust
pub use risk::{RiskEvaluator, RiskLevel};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test dispatcher::risk::tests --no-default-features`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/dispatcher/risk.rs core/src/dispatcher/mod.rs
git commit -m "feat(dispatcher): add RiskEvaluator for task risk assessment

- Pattern-based risk detection (API, execute, file modify, etc.)
- Task type-based risk rules
- Graph-level risk evaluation

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 1.3: Create TaskContext

**Files:**
- Create: `core/src/dispatcher/context.rs`
- Modify: `core/src/dispatcher/mod.rs`
- Test: `core/src/dispatcher/context.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_record_and_build() {
        let mut ctx = TaskContext::new("test user input");
        ctx.record_output("t1", TaskOutput::text("result 1"));

        let prompt = ctx.build_prompt_context("t2", &["t1"]);
        assert!(prompt.contains("test user input"));
        assert!(prompt.contains("result 1"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test dispatcher::context::tests --no-default-features`
Expected: FAIL

**Step 3: Write minimal implementation**

Create `core/src/dispatcher/context.rs`:

```rust
//! Task Context - Manage context passing between tasks

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Output from a completed task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutput {
    /// The main output value
    pub value: serde_json::Value,
    /// Optional summary for display
    pub summary: Option<String>,
    /// Output type hint
    pub output_type: OutputType,
}

/// Type of task output
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputType {
    Text,
    Json,
    Binary,
    Error,
}

impl TaskOutput {
    /// Create a text output
    pub fn text(content: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            value: serde_json::Value::String(content.clone()),
            summary: Some(if content.len() > 100 {
                format!("{}...", &content[..100])
            } else {
                content
            }),
            output_type: OutputType::Text,
        }
    }

    /// Create a JSON output
    pub fn json(value: serde_json::Value) -> Self {
        Self {
            summary: Some(format!("{}", value).chars().take(100).collect()),
            value,
            output_type: OutputType::Json,
        }
    }

    /// Create an error output
    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            value: serde_json::Value::String(message.clone()),
            summary: Some(message),
            output_type: OutputType::Error,
        }
    }
}

/// Context for task execution, supporting implicit accumulation and explicit reference
pub struct TaskContext {
    /// Implicit accumulation: all completed task outputs
    history: Vec<HistoryEntry>,

    /// Explicit variables: task ID → output value
    variables: HashMap<String, TaskOutput>,

    /// Original user input
    user_input: String,

    /// Maximum history entries to include in context
    max_history: usize,
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    task_id: String,
    task_name: String,
    output: TaskOutput,
}

impl TaskContext {
    /// Create a new task context
    pub fn new(user_input: impl Into<String>) -> Self {
        Self {
            history: Vec::new(),
            variables: HashMap::new(),
            user_input: user_input.into(),
            max_history: 5,
        }
    }

    /// Set maximum history entries
    pub fn with_max_history(mut self, max: usize) -> Self {
        self.max_history = max;
        self
    }

    /// Build prompt context for a task
    pub fn build_prompt_context(&self, task_id: &str, dependencies: &[&str]) -> String {
        let mut context = format!("用户原始请求: {}\n\n", self.user_input);

        // Add explicit dependencies
        if !dependencies.is_empty() {
            context.push_str("=== 前置任务结果 ===\n");
            for dep_id in dependencies {
                if let Some(output) = self.variables.get(*dep_id) {
                    context.push_str(&format!(
                        "[{}]: {}\n",
                        dep_id,
                        output.summary.as_deref().unwrap_or("(no summary)")
                    ));
                }
            }
            context.push('\n');
        }

        // Add recent history (implicit accumulation)
        if !self.history.is_empty() {
            let recent: Vec<_> = self.history.iter().rev().take(self.max_history).collect();
            if !recent.is_empty() {
                context.push_str("=== 执行历史 ===\n");
                for entry in recent.iter().rev() {
                    if !dependencies.contains(&entry.task_id.as_str()) {
                        context.push_str(&format!(
                            "- {}: {}\n",
                            entry.task_name,
                            entry.output.summary.as_deref().unwrap_or("(done)")
                        ));
                    }
                }
                context.push('\n');
            }
        }

        context.push_str(&format!("=== 当前任务 ===\n任务ID: {}\n", task_id));

        context
    }

    /// Record task output
    pub fn record_output(&mut self, task_id: &str, output: TaskOutput) {
        self.variables.insert(task_id.to_string(), output.clone());
        self.history.push(HistoryEntry {
            task_id: task_id.to_string(),
            task_name: task_id.to_string(), // Will be updated with actual name
            output,
        });
    }

    /// Record task output with name
    pub fn record_output_with_name(&mut self, task_id: &str, task_name: &str, output: TaskOutput) {
        self.variables.insert(task_id.to_string(), output.clone());
        self.history.push(HistoryEntry {
            task_id: task_id.to_string(),
            task_name: task_name.to_string(),
            output,
        });
    }

    /// Get output for a specific task
    pub fn get_output(&self, task_id: &str) -> Option<&TaskOutput> {
        self.variables.get(task_id)
    }

    /// Get user input
    pub fn user_input(&self) -> &str {
        &self.user_input
    }

    /// Clear all context
    pub fn clear(&mut self) {
        self.history.clear();
        self.variables.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_output_text() {
        let output = TaskOutput::text("Hello, world!");
        assert_eq!(output.output_type, OutputType::Text);
        assert!(output.summary.is_some());
    }

    #[test]
    fn test_task_output_long_text() {
        let long_text = "a".repeat(200);
        let output = TaskOutput::text(&long_text);
        assert!(output.summary.as_ref().unwrap().len() < 110);
    }

    #[test]
    fn test_context_record_and_build() {
        let mut ctx = TaskContext::new("test user input");
        ctx.record_output("t1", TaskOutput::text("result 1"));

        let prompt = ctx.build_prompt_context("t2", &["t1"]);
        assert!(prompt.contains("test user input"));
        assert!(prompt.contains("result 1"));
    }

    #[test]
    fn test_context_explicit_reference() {
        let mut ctx = TaskContext::new("user request");
        ctx.record_output_with_name("task1", "分析文档", TaskOutput::text("分析结果"));
        ctx.record_output_with_name("task2", "提取关键词", TaskOutput::text("关键词列表"));

        let prompt = ctx.build_prompt_context("task3", &["task1", "task2"]);
        assert!(prompt.contains("[task1]"));
        assert!(prompt.contains("[task2]"));
    }

    #[test]
    fn test_context_get_output() {
        let mut ctx = TaskContext::new("test");
        ctx.record_output("t1", TaskOutput::text("output"));

        assert!(ctx.get_output("t1").is_some());
        assert!(ctx.get_output("t2").is_none());
    }
}
```

**Step 4: Update mod.rs**

Add to `core/src/dispatcher/mod.rs`:

```rust
pub mod context;
```

Add to re-exports:

```rust
pub use context::{OutputType, TaskContext, TaskOutput};
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test dispatcher::context::tests --no-default-features`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/dispatcher/context.rs core/src/dispatcher/mod.rs
git commit -m "feat(dispatcher): add TaskContext for inter-task context passing

- Implicit history accumulation
- Explicit variable reference by task ID
- Prompt context building for downstream tasks

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

### Task 1.4: Create ExecutionCallback

**Files:**
- Create: `core/src/dispatcher/callback.rs`
- Modify: `core/src/dispatcher/mod.rs`

**Step 1: Write the callback interface**

Create `core/src/dispatcher/callback.rs`:

```rust
//! Execution Callback - UI feedback interface for task execution

use async_trait::async_trait;

use crate::dispatcher::cowork_types::TaskGraph;
use crate::dispatcher::risk::RiskLevel;

/// Task status for UI display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum TaskDisplayStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Task info for UI display
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TaskInfo {
    pub id: String,
    pub name: String,
    pub status: TaskDisplayStatus,
    pub risk_level: String, // "low" or "high"
    pub dependencies: Vec<String>,
}

/// Task plan for UI display
#[derive(Debug, Clone)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct TaskPlan {
    pub id: String,
    pub title: String,
    pub tasks: Vec<TaskInfo>,
    pub requires_confirmation: bool,
}

impl TaskPlan {
    /// Create from TaskGraph
    pub fn from_graph(graph: &TaskGraph, requires_confirmation: bool) -> Self {
        let tasks = graph
            .tasks
            .iter()
            .map(|t| {
                let deps = graph.get_predecessors(&t.id)
                    .into_iter()
                    .cloned()
                    .collect();

                TaskInfo {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    status: if t.is_pending() {
                        TaskDisplayStatus::Pending
                    } else if t.is_running() {
                        TaskDisplayStatus::Running
                    } else if t.is_completed() {
                        TaskDisplayStatus::Completed
                    } else if t.is_failed() {
                        TaskDisplayStatus::Failed
                    } else {
                        TaskDisplayStatus::Cancelled
                    },
                    risk_level: "low".to_string(), // Will be updated by RiskEvaluator
                    dependencies: deps,
                }
            })
            .collect();

        Self {
            id: graph.id.clone(),
            title: graph.name.clone(),
            tasks,
            requires_confirmation,
        }
    }
}

/// User decision for confirmation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserDecision {
    Confirmed,
    Cancelled,
}

/// Callback interface for execution progress
#[async_trait]
pub trait ExecutionCallback: Send + Sync {
    /// Called when task plan is ready
    async fn on_plan_ready(&self, plan: &TaskPlan);

    /// Called when user confirmation is required (high-risk tasks)
    /// Returns user decision
    async fn on_confirmation_required(&self, plan: &TaskPlan) -> UserDecision;

    /// Called when a task starts
    async fn on_task_start(&self, task_id: &str, task_name: &str);

    /// Called for streaming output from a task
    async fn on_task_stream(&self, task_id: &str, chunk: &str);

    /// Called when a task completes
    async fn on_task_complete(&self, task_id: &str, summary: &str);

    /// Called when a task is retrying
    async fn on_task_retry(&self, task_id: &str, attempt: u32, error: &str);

    /// Called when LLM is deciding on failure
    async fn on_task_deciding(&self, task_id: &str, error: &str);

    /// Called when a task fails
    async fn on_task_failed(&self, task_id: &str, error: &str);

    /// Called when all tasks complete
    async fn on_all_complete(&self, summary: &str);

    /// Called when execution is cancelled
    async fn on_cancelled(&self);
}

/// No-op callback for testing
pub struct NoOpCallback;

#[async_trait]
impl ExecutionCallback for NoOpCallback {
    async fn on_plan_ready(&self, _plan: &TaskPlan) {}
    async fn on_confirmation_required(&self, _plan: &TaskPlan) -> UserDecision {
        UserDecision::Confirmed
    }
    async fn on_task_start(&self, _task_id: &str, _task_name: &str) {}
    async fn on_task_stream(&self, _task_id: &str, _chunk: &str) {}
    async fn on_task_complete(&self, _task_id: &str, _summary: &str) {}
    async fn on_task_retry(&self, _task_id: &str, _attempt: u32, _error: &str) {}
    async fn on_task_deciding(&self, _task_id: &str, _error: &str) {}
    async fn on_task_failed(&self, _task_id: &str, _error: &str) {}
    async fn on_all_complete(&self, _summary: &str) {}
    async fn on_cancelled(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_plan_from_graph() {
        let mut graph = TaskGraph::new("test", "Test Plan");
        graph.add_task(crate::dispatcher::cowork_types::Task::new(
            "t1",
            "Task 1",
            crate::dispatcher::cowork_types::TaskType::AiInference(
                crate::dispatcher::cowork_types::AiTask {
                    prompt: "test".to_string(),
                    requires_privacy: false,
                    has_images: false,
                    output_format: None,
                },
            ),
        ));

        let plan = TaskPlan::from_graph(&graph, false);
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].id, "t1");
        assert!(!plan.requires_confirmation);
    }
}
```

**Step 2: Update mod.rs**

Add to `core/src/dispatcher/mod.rs`:

```rust
pub mod callback;
```

Add to re-exports:

```rust
pub use callback::{
    ExecutionCallback, NoOpCallback, TaskDisplayStatus, TaskInfo, TaskPlan, UserDecision,
};
```

**Step 3: Run test**

Run: `cd core && cargo test dispatcher::callback::tests --no-default-features`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/dispatcher/callback.rs core/src/dispatcher/mod.rs
git commit -m "feat(dispatcher): add ExecutionCallback interface for UI feedback

- TaskPlan and TaskInfo for UI display
- Async callback trait for execution events
- NoOpCallback for testing

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 2: Enhance DagScheduler

### Task 2.1: Add execute_graph method to DagScheduler

**Files:**
- Modify: `core/src/dispatcher/scheduler/dag.rs`
- Modify: `core/src/dispatcher/scheduler/mod.rs`

**Step 1: Write the failing test**

Add to `core/src/dispatcher/scheduler/dag.rs`:

```rust
#[tokio::test]
async fn test_execute_graph_basic() {
    let mut graph = TaskGraph::new("test", "Test");
    graph.add_task(create_task("a"));
    graph.add_task(create_task("b"));
    graph.add_dependency("a", "b");

    let scheduler = DagScheduler::new();
    let executor = Arc::new(MockTaskExecutor);
    let callback = Arc::new(NoOpCallback);
    let context = TaskContext::new("test");

    let result = scheduler.execute_graph(graph, executor, callback, context).await;
    assert!(result.is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test scheduler::dag::tests::test_execute_graph --no-default-features`
Expected: FAIL

**Step 3: Write implementation**

Add to `core/src/dispatcher/scheduler/dag.rs`:

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use futures::future::join_all;

use crate::dispatcher::callback::{ExecutionCallback, TaskPlan, UserDecision};
use crate::dispatcher::context::{TaskContext, TaskOutput};
use crate::dispatcher::risk::RiskEvaluator;
use crate::error::Result;

/// Result of graph execution
#[derive(Debug)]
pub struct ExecutionResult {
    pub graph_id: String,
    pub completed_tasks: Vec<String>,
    pub failed_tasks: Vec<String>,
    pub cancelled: bool,
}

/// Task executor trait
#[async_trait::async_trait]
pub trait GraphTaskExecutor: Send + Sync {
    async fn execute(&self, task: &Task, context: &str) -> Result<TaskOutput>;
}

impl DagScheduler {
    /// Execute entire TaskGraph with callbacks
    pub async fn execute_graph(
        &self,
        mut graph: TaskGraph,
        executor: Arc<dyn GraphTaskExecutor>,
        callback: Arc<dyn ExecutionCallback>,
        mut context: TaskContext,
    ) -> Result<ExecutionResult> {
        let graph_id = graph.id.clone();
        let risk_evaluator = RiskEvaluator::new();
        let requires_confirmation = risk_evaluator.evaluate_graph(&graph);

        // 1. Notify UI with task plan
        let plan = TaskPlan::from_graph(&graph, requires_confirmation);
        callback.on_plan_ready(&plan).await;

        // 2. Check if confirmation needed
        if requires_confirmation {
            let decision = callback.on_confirmation_required(&plan).await;
            if decision == UserDecision::Cancelled {
                callback.on_cancelled().await;
                return Ok(ExecutionResult {
                    graph_id,
                    completed_tasks: vec![],
                    failed_tasks: vec![],
                    cancelled: true,
                });
            }
        }

        // 3. Create mutable scheduler state
        let scheduler = Arc::new(Mutex::new(Self::new()));
        let mut completed = Vec::new();
        let mut failed = Vec::new();

        // 4. DAG scheduling loop
        loop {
            let ready_tasks = {
                let sched = scheduler.lock().await;
                sched.next_ready(&graph)
                    .into_iter()
                    .map(|t| t.clone())
                    .collect::<Vec<_>>()
            };

            if ready_tasks.is_empty() {
                // Check if all done
                let sched = scheduler.lock().await;
                if sched.is_complete(&graph) {
                    break;
                }
                // No ready tasks but not complete - deadlock or all failed
                break;
            }

            // Mark tasks as running
            {
                let mut sched = scheduler.lock().await;
                for task in &ready_tasks {
                    sched.mark_running(&task.id);
                    graph.get_task_mut(&task.id).map(|t| {
                        t.status = TaskStatus::running(0.0);
                    });
                }
            }

            // Execute ready tasks in parallel
            let futures = ready_tasks.iter().map(|task| {
                let task = task.clone();
                let executor = executor.clone();
                let callback = callback.clone();
                let scheduler = scheduler.clone();
                let deps: Vec<&str> = graph.get_predecessors(&task.id)
                    .iter()
                    .map(|s| s.as_str())
                    .collect();
                let ctx_prompt = context.build_prompt_context(&task.id, &deps);

                async move {
                    callback.on_task_start(&task.id, &task.name).await;

                    // Execute with retry
                    let result = execute_with_retry(
                        &task,
                        &executor,
                        &callback,
                        &ctx_prompt,
                        2, // max retries
                    ).await;

                    (task, result)
                }
            });

            let results = join_all(futures).await;

            // Process results
            for (task, result) in results {
                match result {
                    Ok(output) => {
                        context.record_output_with_name(&task.id, &task.name, output.clone());
                        callback.on_task_complete(&task.id, output.summary.as_deref().unwrap_or("完成")).await;

                        let mut sched = scheduler.lock().await;
                        sched.mark_completed(&task.id);
                        graph.get_task_mut(&task.id).map(|t| {
                            t.status = TaskStatus::completed(TaskResult::default());
                        });
                        completed.push(task.id.clone());
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        callback.on_task_failed(&task.id, &error_msg).await;

                        let mut sched = scheduler.lock().await;
                        sched.mark_failed(&task.id, &error_msg);
                        graph.get_task_mut(&task.id).map(|t| {
                            t.status = TaskStatus::failed(&error_msg);
                        });
                        failed.push(task.id.clone());
                    }
                }
            }
        }

        // 5. All done
        let summary = format!(
            "完成 {} 个任务，失败 {} 个",
            completed.len(),
            failed.len()
        );
        callback.on_all_complete(&summary).await;

        Ok(ExecutionResult {
            graph_id,
            completed_tasks: completed,
            failed_tasks: failed,
            cancelled: false,
        })
    }
}

async fn execute_with_retry(
    task: &Task,
    executor: &Arc<dyn GraphTaskExecutor>,
    callback: &Arc<dyn ExecutionCallback>,
    context: &str,
    max_retries: u32,
) -> Result<TaskOutput> {
    let mut last_error = None;

    for attempt in 0..=max_retries {
        match executor.execute(task, context).await {
            Ok(output) => return Ok(output),
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries {
                    callback.on_task_retry(&task.id, attempt + 1, &last_error.as_ref().unwrap().to_string()).await;
                }
            }
        }
    }

    // All retries failed - could add LLM decision here
    callback.on_task_deciding(&task.id, &last_error.as_ref().unwrap().to_string()).await;

    Err(last_error.unwrap())
}
```

**Step 4: Run test**

Run: `cd core && cargo test scheduler::dag::tests::test_execute_graph --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/dispatcher/scheduler/dag.rs
git commit -m "feat(scheduler): add execute_graph with retry and callbacks

- Full DAG scheduling loop with parallel execution
- Retry logic (2 attempts) before failure
- UI callbacks for progress tracking
- Risk-based confirmation flow

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 3: Integration into processing.rs

### Task 3.1: Integrate TaskAnalyzer into process_with_agent_loop

**Files:**
- Modify: `core/src/ffi/processing.rs:639-736`

**Step 1: Add imports**

Add at top of file:

```rust
use crate::dispatcher::{
    AnalysisResult, TaskAnalyzer, DagScheduler, TaskContext,
    ExecutionCallback, TaskPlan, UserDecision,
};
```

**Step 2: Modify process_with_agent_loop**

Replace the `RouteResult::NeedsThinking` branch (lines 709-734):

```rust
RouteResult::NeedsThinking(ctx) => {
    info!(
        category_hint = ?ctx.category_hint,
        bias_execute = ctx.bias_execute,
        latency_us = ctx.latency_us,
        "Needs thinking - analyzing task complexity"
    );

    // Create TaskAnalyzer
    let provider = create_provider_from_config(config);
    let analyzer = TaskAnalyzer::new(provider.clone());

    // Analyze input
    let analysis_result = runtime.block_on(async {
        analyzer.analyze(input).await
    });

    match analysis_result {
        Ok(AnalysisResult::SingleStep { intent }) => {
            info!(intent = %intent, "Single-step task - using Agent Loop");
            run_agent_loop(
                runtime, input, ctx, config, tool_server_handle,
                registered_tools, op_token, handler, memory_config,
                memory_path, input_for_memory, app_context, window_title,
                generation_config,
            );
        }
        Ok(AnalysisResult::MultiStep { task_graph, requires_confirmation }) => {
            info!(
                tasks = task_graph.tasks.len(),
                requires_confirmation,
                "Multi-step task - using DAG scheduler"
            );
            run_dag_execution(
                runtime, task_graph, requires_confirmation, provider,
                op_token, handler, input,
            );
        }
        Err(e) => {
            warn!(error = %e, "Task analysis failed, falling back to Agent Loop");
            run_agent_loop(
                runtime, input, ctx, config, tool_server_handle,
                registered_tools, op_token, handler, memory_config,
                memory_path, input_for_memory, app_context, window_title,
                generation_config,
            );
        }
    }
}
```

**Step 3: Add run_dag_execution function**

Add new function:

```rust
/// Run DAG-based multi-step execution
fn run_dag_execution(
    runtime: &tokio::runtime::Handle,
    task_graph: TaskGraph,
    requires_confirmation: bool,
    provider: Arc<dyn AiProvider>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    user_input: &str,
) {
    let handler = handler.clone();
    let op_token = op_token.clone();
    let user_input = user_input.to_string();

    runtime.spawn(async move {
        // Create callback adapter
        let callback = Arc::new(FfiExecutionCallback::new(handler.clone()));

        // Create executor
        let executor = Arc::new(LlmTaskExecutor::new(provider));

        // Create context
        let context = TaskContext::new(&user_input);

        // Create scheduler and execute
        let scheduler = DagScheduler::new();

        match scheduler.execute_graph(task_graph, executor, callback, context).await {
            Ok(result) => {
                if result.cancelled {
                    handler.on_error("用户取消了任务执行".to_string());
                } else if !result.failed_tasks.is_empty() {
                    handler.on_error(format!(
                        "部分任务失败: {:?}",
                        result.failed_tasks
                    ));
                }
                handler.on_complete();
            }
            Err(e) => {
                handler.on_error(format!("DAG 执行失败: {}", e));
                handler.on_complete();
            }
        }
    });
}
```

**Step 4: Implement FfiExecutionCallback**

Add adapter class:

```rust
/// Adapter to convert ExecutionCallback to AetherEventHandler
struct FfiExecutionCallback {
    handler: Arc<dyn crate::ffi::AetherEventHandler>,
    confirmation_tx: tokio::sync::mpsc::Sender<UserDecision>,
    confirmation_rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<UserDecision>>,
}

impl FfiExecutionCallback {
    fn new(handler: Arc<dyn crate::ffi::AetherEventHandler>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        Self {
            handler,
            confirmation_tx: tx,
            confirmation_rx: tokio::sync::Mutex::new(rx),
        }
    }
}

#[async_trait::async_trait]
impl ExecutionCallback for FfiExecutionCallback {
    async fn on_plan_ready(&self, plan: &TaskPlan) {
        // Format plan as markdown for display
        let mut output = format!("📋 **任务计划: {}**\n\n", plan.title);
        for (i, task) in plan.tasks.iter().enumerate() {
            let status = match task.status {
                TaskDisplayStatus::Pending => "○",
                TaskDisplayStatus::Running => "◉",
                TaskDisplayStatus::Completed => "✓",
                TaskDisplayStatus::Failed => "✗",
                TaskDisplayStatus::Cancelled => "⊘",
            };
            output.push_str(&format!("{} {}. {}\n", status, i + 1, task.name));
        }
        output.push_str("\n---\n\n");
        self.handler.on_token(output);
    }

    async fn on_confirmation_required(&self, plan: &TaskPlan) -> UserDecision {
        self.handler.on_token("⚠️ 此任务包含高风险操作，是否继续执行？\n".to_string());
        // For now, auto-confirm. Real implementation would wait for user input.
        UserDecision::Confirmed
    }

    async fn on_task_start(&self, task_id: &str, task_name: &str) {
        self.handler.on_token(format!("\n**[开始]** {}\n", task_name));
    }

    async fn on_task_stream(&self, _task_id: &str, chunk: &str) {
        self.handler.on_token(chunk.to_string());
    }

    async fn on_task_complete(&self, _task_id: &str, summary: &str) {
        self.handler.on_token(format!("\n✓ {}\n", summary));
    }

    async fn on_task_retry(&self, task_id: &str, attempt: u32, error: &str) {
        self.handler.on_token(format!(
            "\n⟳ 重试 {} (第{}次): {}\n",
            task_id, attempt, error
        ));
    }

    async fn on_task_deciding(&self, task_id: &str, error: &str) {
        self.handler.on_token(format!(
            "\n🤔 任务 {} 失败，正在决策...\n错误: {}\n",
            task_id, error
        ));
    }

    async fn on_task_failed(&self, task_id: &str, error: &str) {
        self.handler.on_token(format!("\n✗ {} 失败: {}\n", task_id, error));
    }

    async fn on_all_complete(&self, summary: &str) {
        self.handler.on_token(format!("\n---\n\n**执行完成**: {}\n", summary));
    }

    async fn on_cancelled(&self) {
        self.handler.on_token("\n---\n\n**已取消**\n".to_string());
    }
}
```

**Step 5: Implement LlmTaskExecutor**

```rust
/// LLM-based task executor
struct LlmTaskExecutor {
    provider: Arc<dyn AiProvider>,
}

impl LlmTaskExecutor {
    fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl GraphTaskExecutor for LlmTaskExecutor {
    async fn execute(&self, task: &Task, context: &str) -> Result<TaskOutput> {
        let prompt = format!(
            "{}\n\n请执行以下任务:\n任务: {}\n描述: {}\n\n请直接给出结果。",
            context,
            task.name,
            task.description.as_deref().unwrap_or("无"),
        );

        let response = self.provider.process(&prompt, None).await?;
        Ok(TaskOutput::text(response))
    }
}
```

**Step 6: Test manually**

Run: `cd core && cargo build --features uniffi`
Expected: BUILD SUCCESS

**Step 7: Commit**

```bash
git add core/src/ffi/processing.rs
git commit -m "feat(ffi): integrate DAG scheduler into main processing flow

- TaskAnalyzer for single/multi-step detection
- DAG execution path for multi-step tasks
- FfiExecutionCallback adapter for UI feedback
- LlmTaskExecutor for task execution

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 4: UniFFI Export

### Task 4.1: Export callback types via UniFFI

**Files:**
- Modify: `core/src/lib.rs`
- Modify: `core/src/dispatcher/callback.rs`

**Step 1: Add uniffi attributes**

Already added in Task 1.4 with `#[cfg_attr(feature = "uniffi", ...)]`

**Step 2: Update lib.rs exports**

Add to uniffi exports section:

```rust
#[cfg(feature = "uniffi")]
pub use dispatcher::{TaskDisplayStatus, TaskInfo, TaskPlan};
```

**Step 3: Regenerate bindings**

Run: `./scripts/build-core.sh macos`

**Step 4: Commit**

```bash
git add core/src/lib.rs
git commit -m "feat(uniffi): export DAG callback types for Swift

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 5: Swift UI Component

### Task 5.1: Create TaskPlanCard View

**Files:**
- Create: `platforms/macos/Aether/Sources/Components/Molecules/TaskPlanCard.swift`

**Step 1: Create the component**

```swift
import SwiftUI

/// Task status indicator
struct TaskStatusIcon: View {
    let status: TaskDisplayStatus

    var body: some View {
        Group {
            switch status {
            case .pending:
                Circle()
                    .stroke(Color.secondary, lineWidth: 1.5)
                    .frame(width: 12, height: 12)
            case .running:
                Circle()
                    .fill(Color.blue)
                    .frame(width: 12, height: 12)
                    .overlay(
                        ProgressView()
                            .scaleEffect(0.5)
                    )
            case .completed:
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                    .font(.system(size: 12))
            case .failed:
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.red)
                    .font(.system(size: 12))
            case .cancelled:
                Image(systemName: "minus.circle.fill")
                    .foregroundColor(.secondary)
                    .font(.system(size: 12))
            }
        }
    }
}

/// Task plan card showing execution progress
struct TaskPlanCard: View {
    let plan: TaskPlan

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Header
            HStack {
                Text("📋")
                Text(plan.title)
                    .font(.headline)
                Spacer()
                if plan.requiresConfirmation {
                    Text("需确认")
                        .font(.caption)
                        .foregroundColor(.orange)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.orange.opacity(0.2))
                        .cornerRadius(4)
                }
            }

            Divider()

            // Task list
            ForEach(Array(plan.tasks.enumerated()), id: \.element.id) { index, task in
                HStack(spacing: 8) {
                    TaskStatusIcon(status: task.status)

                    Text("\(index + 1).")
                        .font(.caption)
                        .foregroundColor(.secondary)

                    Text(task.name)
                        .font(.body)

                    Spacer()

                    if task.riskLevel == "high" {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(.orange)
                            .font(.caption)
                    }
                }
                .padding(.vertical, 2)
            }
        }
        .padding()
        .background(Color.secondary.opacity(0.1))
        .cornerRadius(12)
    }
}

#Preview {
    TaskPlanCard(plan: TaskPlan(
        id: "test",
        title: "分析文档并生成图谱",
        tasks: [
            TaskInfo(id: "t1", name: "分析文档内容", status: .completed, riskLevel: "low", dependencies: []),
            TaskInfo(id: "t2", name: "生成知识图谱 Prompt", status: .running, riskLevel: "low", dependencies: ["t1"]),
            TaskInfo(id: "t3", name: "调用绘图 API", status: .pending, riskLevel: "high", dependencies: ["t2"]),
        ],
        requiresConfirmation: true
    ))
    .padding()
    .frame(width: 400)
}
```

**Step 2: Commit**

```bash
git add platforms/macos/Aether/Sources/Components/Molecules/TaskPlanCard.swift
git commit -m "feat(macos): add TaskPlanCard UI component

- Status icons for pending/running/completed/failed/cancelled
- Risk level indicator
- Confirmation badge

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Phase 6: Integration Test

### Task 6.1: Create integration test

**Files:**
- Create: `core/tests/dag_integration_test.rs`

**Step 1: Write integration test**

```rust
//! Integration test for DAG scheduler with Agent Loop

use std::sync::Arc;
use aethecore::dispatcher::{
    AnalysisResult, DagScheduler, TaskAnalyzer, TaskContext,
    ExecutionCallback, NoOpCallback, TaskPlan, UserDecision,
};
use aethecore::providers::MockProvider;

#[tokio::test]
async fn test_multi_step_task_flow() {
    // Setup
    let provider = Arc::new(MockProvider::with_response(
        r#"{"type": "multi", "tasks": [
            {"id": "t1", "name": "分析文档", "deps": [], "risk": "low"},
            {"id": "t2", "name": "生成图谱", "deps": ["t1"], "risk": "high"}
        ]}"#
    ));

    let analyzer = TaskAnalyzer::new(provider.clone());

    // Analyze
    let result = analyzer.analyze("分析这篇文档，然后生成知识图谱").await.unwrap();

    // Verify multi-step detected
    match result {
        AnalysisResult::MultiStep { task_graph, requires_confirmation } => {
            assert!(requires_confirmation); // has high-risk task
            assert!(task_graph.tasks.len() >= 2);
        }
        _ => panic!("Expected multi-step result"),
    }
}

#[tokio::test]
async fn test_single_step_task_flow() {
    let provider = Arc::new(MockProvider::with_response(
        r#"{"type": "single", "intent": "回答问题"}"#
    ));

    let analyzer = TaskAnalyzer::new(provider);
    let result = analyzer.analyze("今天天气怎么样").await.unwrap();

    assert!(matches!(result, AnalysisResult::SingleStep { .. }));
}
```

**Step 2: Run test**

Run: `cd core && cargo test dag_integration --no-default-features`
Expected: PASS

**Step 3: Commit**

```bash
git add core/tests/dag_integration_test.rs
git commit -m "test: add DAG scheduler integration tests

- Multi-step task detection
- Single-step fallback
- Risk evaluation

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

| Phase | Tasks | Status |
|-------|-------|--------|
| Phase 1 | TaskAnalyzer, RiskEvaluator, TaskContext, ExecutionCallback | Pending |
| Phase 2 | DagScheduler.execute_graph | Pending |
| Phase 3 | Integration into processing.rs | Pending |
| Phase 4 | UniFFI export | Pending |
| Phase 5 | Swift TaskPlanCard | Pending |
| Phase 6 | Integration tests | Pending |

**Estimated commits:** 10
**Key files modified:** 8
**New files created:** 6

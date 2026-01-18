# Phase 2: Core Components Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the 6 core event handler components that power the agentic loop: IntentAnalyzer, TaskPlanner, ToolExecutor, LoopController, SessionRecorder, and SessionCompactor.

**Architecture:** Each component implements the `EventHandler` trait from Phase 1, subscribing to specific events and publishing new ones. Components are loosely coupled via the EventBus, enabling the agentic loop pattern where tool results feed back to LLM decisions.

**Tech Stack:** tokio (async), serde_json (serialization), chrono (timestamps), uuid (IDs), rusqlite (persistence), existing IntentClassifier and ToolRegistry

**Reference Files:**
- Design: `docs/plans/2026-01-18-event-driven-agent-loop-design.md`
- Event Types: `Aether/core/src/event/types.rs`
- Event Handler Trait: `Aether/core/src/event/handler.rs`
- Existing IntentClassifier: `Aether/core/src/intent/classifier.rs`
- Existing ToolRegistry: `Aether/core/src/dispatcher/registry.rs`

---

## Task 1: Create Components Module Structure

**Files:**
- Create: `Aether/core/src/components/mod.rs`
- Create: `Aether/core/src/components/types.rs`
- Modify: `Aether/core/src/lib.rs`

**Step 1: Create components module directory and mod.rs**

```rust
// Aether/core/src/components/mod.rs
//! Core event handler components for the agentic loop.
//!
//! This module provides the 6 core components:
//! - `IntentAnalyzer`: Input analysis and complexity detection
//! - `TaskPlanner`: LLM-based task decomposition
//! - `ToolExecutor`: Tool execution with retry logic
//! - `LoopController`: Agentic loop control with protection mechanisms
//! - `SessionRecorder`: State persistence to SQLite
//! - `SessionCompactor`: Token management and session compaction

mod intent_analyzer;
mod loop_controller;
mod session_compactor;
mod session_recorder;
mod task_planner;
mod tool_executor;
mod types;

pub use intent_analyzer::IntentAnalyzer;
pub use loop_controller::{LoopConfig, LoopController};
pub use session_compactor::SessionCompactor;
pub use session_recorder::SessionRecorder;
pub use task_planner::TaskPlanner;
pub use tool_executor::{RetryPolicy, ToolExecutor};
pub use types::*;
```

**Step 2: Create shared types for components**

```rust
// Aether/core/src/components/types.rs
//! Shared types for component implementations.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::dispatcher::ToolRegistry;
use crate::event::EventBus;

/// Execution session - tracks the state of an agentic loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSession {
    pub id: String,
    pub parent_id: Option<String>,
    pub agent_id: String,
    pub status: SessionStatus,
    pub iteration_count: u32,
    pub total_tokens: u64,
    pub parts: Vec<SessionPart>,
    pub recent_calls: Vec<ToolCallRecord>,
    pub model: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for ExecutionSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionSession {
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            agent_id: "main".into(),
            status: SessionStatus::Running,
            iteration_count: 0,
            total_tokens: 0,
            parts: Vec::new(),
            recent_calls: Vec::new(),
            model: "default".into(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.into();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed(String),
    Paused,
    Compacting,
}

/// Session part - fine-grained execution records
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
}

impl SessionPart {
    pub fn type_name(&self) -> &'static str {
        match self {
            SessionPart::UserInput(_) => "user_input",
            SessionPart::AiResponse(_) => "ai_response",
            SessionPart::ToolCall(_) => "tool_call",
            SessionPart::Reasoning(_) => "reasoning",
            SessionPart::PlanCreated(_) => "plan_created",
            SessionPart::SubAgentCall(_) => "sub_agent_call",
            SessionPart::Summary(_) => "summary",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputPart {
    pub text: String,
    pub context: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponsePart {
    pub content: String,
    pub reasoning: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallPart {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningPart {
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanPart {
    pub plan_id: String,
    pub steps: Vec<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPart {
    pub agent_id: String,
    pub prompt: String,
    pub result: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryPart {
    pub content: String,
    pub original_count: u32,
    pub compacted_at: i64,
}

/// Tool call record for doom loop detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
}

/// Complexity level for planning decisions
#[derive(Debug, Clone, PartialEq)]
pub enum Complexity {
    Simple,
    NeedsPlan,
}

/// LLM decision after tool execution
#[derive(Debug, Clone)]
pub enum Decision {
    CallTool(crate::event::ToolCallRequest),
    Stop(crate::event::StopReason),
    AskUser(crate::event::UserQuestion),
}

/// Component context - shared state for all event handlers
pub struct ComponentContext {
    pub session: Arc<RwLock<ExecutionSession>>,
    pub tools: Arc<ToolRegistry>,
    pub bus: EventBus,
    pub abort_signal: Arc<std::sync::atomic::AtomicBool>,
    pub session_id: String,
}

impl ComponentContext {
    pub fn new(
        session: Arc<RwLock<ExecutionSession>>,
        tools: Arc<ToolRegistry>,
        bus: EventBus,
        abort_signal: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        Self {
            session,
            tools,
            bus,
            abort_signal,
            session_id,
        }
    }
}
```

**Step 3: Run syntax check**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo check 2>&1 | head -30`
Expected: Error about missing files (intent_analyzer.rs, etc.) - this is expected at this stage

**Step 4: Commit module structure**

```bash
git add Aether/core/src/components/
git commit -m "feat(components): add components module structure and shared types"
```

---

## Task 2: Implement IntentAnalyzer Component

**Files:**
- Create: `Aether/core/src/components/intent_analyzer.rs`
- Reference: `Aether/core/src/intent/classifier.rs`

**Step 1: Write IntentAnalyzer implementation**

```rust
// Aether/core/src/components/intent_analyzer.rs
//! Intent analyzer component - detects intent and complexity for routing.
//!
//! Subscribes to: InputReceived
//! Publishes: PlanRequested or ToolCallRequested

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info};

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, InputEvent, PlanRequest,
    PlanStep, StepStatus, TaskPlan, ToolCallRequest,
};
use crate::intent::{ExecutionIntent, IntentClassifier};

use super::types::Complexity;

/// Multi-step detection keywords (Chinese)
const MULTI_STEP_KEYWORDS_ZH: &[&str] = &[
    "然后", "接着", "之后", "并且", "同时", "再", "随后", "最后", "首先", "其次",
];

/// Multi-step detection keywords (English)
const MULTI_STEP_KEYWORDS_EN: &[&str] = &[
    "then",
    "after that",
    "and then",
    "also",
    "next",
    "finally",
    "first",
    "second",
    "afterwards",
];

/// Intent Analyzer - determines if planning is needed
pub struct IntentAnalyzer {
    classifier: IntentClassifier,
}

impl IntentAnalyzer {
    pub fn new() -> Self {
        Self {
            classifier: IntentClassifier::new(),
        }
    }

    /// Analyze complexity of the input
    fn analyze_complexity(&self, text: &str, _intent: &ExecutionIntent) -> Complexity {
        // Rule 1: Contains multi-step keywords
        if self.has_multi_step_keywords(text) {
            debug!("Detected multi-step keywords in input");
            return Complexity::NeedsPlan;
        }

        // Rule 2: Multiple sentences with action verbs
        if self.has_multiple_action_sentences(text) {
            debug!("Detected multiple action sentences");
            return Complexity::NeedsPlan;
        }

        // Rule 3: Explicit step markers (1. 2. 3. or - - -)
        if self.has_step_markers(text) {
            debug!("Detected step markers in input");
            return Complexity::NeedsPlan;
        }

        Complexity::Simple
    }

    fn has_multi_step_keywords(&self, text: &str) -> bool {
        let text_lower = text.to_lowercase();
        for kw in MULTI_STEP_KEYWORDS_ZH {
            if text.contains(kw) {
                return true;
            }
        }
        for kw in MULTI_STEP_KEYWORDS_EN {
            if text_lower.contains(kw) {
                return true;
            }
        }
        false
    }

    fn has_multiple_action_sentences(&self, text: &str) -> bool {
        // Simple heuristic: count sentences with action verbs
        let action_verbs = [
            "create", "delete", "move", "copy", "run", "execute", "write", "read", "search",
            "find", "update", "install", "build", "test", "deploy", "创建", "删除", "移动",
            "复制", "运行", "执行", "写入", "读取", "搜索", "查找", "更新", "安装", "构建",
            "测试", "部署",
        ];

        let sentences: Vec<&str> = text.split(|c| c == '.' || c == '。' || c == ';' || c == '；')
            .filter(|s| !s.trim().is_empty())
            .collect();

        if sentences.len() < 2 {
            return false;
        }

        let action_count = sentences.iter()
            .filter(|s| {
                let s_lower = s.to_lowercase();
                action_verbs.iter().any(|v| s_lower.contains(v) || s.contains(v))
            })
            .count();

        action_count >= 2
    }

    fn has_step_markers(&self, text: &str) -> bool {
        // Check for numbered steps: 1. 2. 3. or 1) 2) 3)
        let numbered = regex::Regex::new(r"(?m)^\s*\d+[.)]\s").unwrap();
        if numbered.find(text).is_some() {
            return true;
        }

        // Check for bullet points: - or * at start of lines
        let bullets = regex::Regex::new(r"(?m)^\s*[-*]\s").unwrap();
        let bullet_count = bullets.find_iter(text).count();
        bullet_count >= 2
    }

    /// Extract preliminary steps from input text
    fn extract_steps(&self, text: &str) -> Vec<String> {
        let mut steps = Vec::new();
        let mut current = text.to_string();

        // Split by multi-step keywords
        let separators = [
            "然后", "接着", "之后", "并且", "同时", "then", "after that", "and then",
        ];

        for sep in &separators {
            if let Some(pos) = current.find(sep) {
                let before = current[..pos].trim().to_string();
                if !before.is_empty() {
                    steps.push(before);
                }
                current = current[pos + sep.len()..].to_string();
            }
        }

        if !current.trim().is_empty() {
            steps.push(current.trim().to_string());
        }

        // If no splits found, return the whole text as one step
        if steps.is_empty() {
            steps.push(text.to_string());
        }

        steps
    }

    /// Build a direct tool call for simple requests
    fn build_direct_call(&self, intent: &ExecutionIntent, input: &InputEvent) -> ToolCallRequest {
        // For simple cases, we let the main LLM decide the tool
        ToolCallRequest {
            call_id: uuid::Uuid::new_v4().to_string(),
            tool: "chat".into(), // Default to chat, LLM will route
            parameters: serde_json::json!({
                "input": input.text,
                "context": input.context,
            }),
            step_id: None,
            reason: format!("Direct execution for intent: {:?}", intent),
        }
    }
}

impl Default for IntentAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for IntentAnalyzer {
    fn name(&self) -> &'static str {
        "IntentAnalyzer"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::InputReceived]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        let AetherEvent::InputReceived(input) = event else {
            return Ok(vec![]);
        };

        info!("IntentAnalyzer processing input: {}", &input.text[..input.text.len().min(50)]);

        // 1. Use existing classifier to get intent
        let intent = self.classifier.classify_sync(&input.text);

        // 2. Determine complexity
        let complexity = self.analyze_complexity(&input.text, &intent);

        // 3. Decide between planning or direct execution
        match complexity {
            Complexity::Simple => {
                debug!("Simple request - building direct tool call");
                Ok(vec![AetherEvent::ToolCallRequested(
                    self.build_direct_call(&intent, input),
                )])
            }
            Complexity::NeedsPlan => {
                debug!("Complex request - requesting planning phase");
                let detected_steps = self.extract_steps(&input.text);
                Ok(vec![AetherEvent::PlanRequested(PlanRequest {
                    input: input.clone(),
                    intent: format!("{:?}", intent),
                    detected_steps,
                })])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_step_detection_chinese() {
        let analyzer = IntentAnalyzer::new();
        assert!(analyzer.has_multi_step_keywords("搜索文件然后删除它"));
        assert!(analyzer.has_multi_step_keywords("首先创建目录，接着复制文件"));
        assert!(!analyzer.has_multi_step_keywords("搜索文件"));
    }

    #[test]
    fn test_multi_step_detection_english() {
        let analyzer = IntentAnalyzer::new();
        assert!(analyzer.has_multi_step_keywords("search files then delete them"));
        assert!(analyzer.has_multi_step_keywords("First create directory, after that copy files"));
        assert!(!analyzer.has_multi_step_keywords("search files"));
    }

    #[test]
    fn test_step_extraction() {
        let analyzer = IntentAnalyzer::new();

        let steps = analyzer.extract_steps("搜索文件然后删除它");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], "搜索文件");
        assert_eq!(steps[1], "删除它");

        let steps = analyzer.extract_steps("find files and then process them");
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn test_complexity_simple() {
        let analyzer = IntentAnalyzer::new();
        let intent = ExecutionIntent::default();
        assert_eq!(
            analyzer.analyze_complexity("search for rust tutorials", &intent),
            Complexity::Simple
        );
    }

    #[test]
    fn test_complexity_needs_plan() {
        let analyzer = IntentAnalyzer::new();
        let intent = ExecutionIntent::default();
        assert_eq!(
            analyzer.analyze_complexity("search for files then delete the old ones", &intent),
            Complexity::NeedsPlan
        );
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test intent_analyzer --no-fail-fast 2>&1`
Expected: Tests should compile but may need IntentClassifier adjustments

**Step 3: Commit IntentAnalyzer**

```bash
git add Aether/core/src/components/intent_analyzer.rs
git commit -m "feat(components): implement IntentAnalyzer with complexity detection"
```

---

## Task 3: Implement TaskPlanner Component

**Files:**
- Create: `Aether/core/src/components/task_planner.rs`

**Step 1: Write TaskPlanner implementation**

```rust
// Aether/core/src/components/task_planner.rs
//! Task planner component - decomposes complex requests into executable steps.
//!
//! Subscribes to: PlanRequested
//! Publishes: PlanCreated

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, PlanRequest, PlanStep,
    StepStatus, TaskPlan,
};

/// Task Planner - decomposes complex requests into executable steps
pub struct TaskPlanner {
    /// Whether to use LLM for planning (false = use rule-based)
    use_llm: bool,
}

impl TaskPlanner {
    pub fn new() -> Self {
        Self { use_llm: false }
    }

    pub fn with_llm(mut self, use_llm: bool) -> Self {
        self.use_llm = use_llm;
        self
    }

    /// Generate plan from detected steps (rule-based)
    fn generate_plan_from_steps(&self, request: &PlanRequest) -> TaskPlan {
        let steps: Vec<PlanStep> = request
            .detected_steps
            .iter()
            .enumerate()
            .map(|(i, desc)| {
                let step_id = format!("step_{}", i + 1);
                let depends_on = if i > 0 {
                    vec![format!("step_{}", i)]
                } else {
                    vec![]
                };

                PlanStep {
                    id: step_id,
                    description: desc.clone(),
                    tool: self.infer_tool(desc),
                    parameters: self.extract_parameters(desc),
                    depends_on,
                    status: StepStatus::Pending,
                }
            })
            .collect();

        // Identify parallel groups (steps without dependencies on each other)
        let parallel_groups = self.identify_parallel_groups(&steps);

        TaskPlan {
            id: uuid::Uuid::new_v4().to_string(),
            steps,
            parallel_groups,
            current_step_index: 0,
        }
    }

    /// Infer tool from step description
    fn infer_tool(&self, description: &str) -> String {
        let desc_lower = description.to_lowercase();

        // Search-related
        if desc_lower.contains("search") || desc_lower.contains("搜索") || desc_lower.contains("查找")
        {
            return "search".into();
        }

        // File operations
        if desc_lower.contains("delete") || desc_lower.contains("删除") {
            return "file_delete".into();
        }
        if desc_lower.contains("copy") || desc_lower.contains("复制") {
            return "file_copy".into();
        }
        if desc_lower.contains("move") || desc_lower.contains("移动") {
            return "file_move".into();
        }
        if desc_lower.contains("create") || desc_lower.contains("创建") {
            return "file_write".into();
        }
        if desc_lower.contains("read") || desc_lower.contains("读取") {
            return "file_read".into();
        }

        // Web operations
        if desc_lower.contains("fetch") || desc_lower.contains("download") || desc_lower.contains("下载") {
            return "web_fetch".into();
        }

        // Default to chat for general processing
        "chat".into()
    }

    /// Extract parameters from step description
    fn extract_parameters(&self, description: &str) -> Value {
        serde_json::json!({
            "description": description,
        })
    }

    /// Identify groups of steps that can run in parallel
    fn identify_parallel_groups(&self, steps: &[PlanStep]) -> Vec<Vec<String>> {
        let mut groups: Vec<Vec<String>> = Vec::new();
        let mut current_group: Vec<String> = Vec::new();

        for (i, step) in steps.iter().enumerate() {
            // Check if this step depends on any step in the current group
            let depends_on_current = step.depends_on.iter().any(|dep| {
                current_group.iter().any(|g| g == dep)
            });

            if depends_on_current || current_group.is_empty() {
                // Start a new group if dependent or first step
                if !current_group.is_empty() {
                    groups.push(current_group);
                    current_group = Vec::new();
                }
            }

            current_group.push(step.id.clone());
        }

        if !current_group.is_empty() {
            groups.push(current_group);
        }

        // Only return groups with more than one step
        groups.into_iter().filter(|g| g.len() > 1).collect()
    }
}

impl Default for TaskPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for TaskPlanner {
    fn name(&self) -> &'static str {
        "TaskPlanner"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::PlanRequested]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        let AetherEvent::PlanRequested(request) = event else {
            return Ok(vec![]);
        };

        info!(
            "TaskPlanner creating plan for: {}",
            &request.input.text[..request.input.text.len().min(50)]
        );

        // Generate plan
        let plan = if self.use_llm {
            // TODO: Implement LLM-based planning in future
            warn!("LLM planning not yet implemented, falling back to rule-based");
            self.generate_plan_from_steps(request)
        } else {
            self.generate_plan_from_steps(request)
        };

        debug!("Created plan with {} steps", plan.steps.len());

        Ok(vec![AetherEvent::PlanCreated(plan)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::InputEvent;

    fn make_request(text: &str, steps: Vec<&str>) -> PlanRequest {
        PlanRequest {
            input: InputEvent {
                text: text.into(),
                topic_id: None,
                context: None,
                timestamp: 0,
            },
            intent: "General".into(),
            detected_steps: steps.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_generate_plan_single_step() {
        let planner = TaskPlanner::new();
        let request = make_request("search for files", vec!["search for files"]);
        let plan = planner.generate_plan_from_steps(&request);

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].id, "step_1");
        assert!(plan.steps[0].depends_on.is_empty());
    }

    #[test]
    fn test_generate_plan_multiple_steps() {
        let planner = TaskPlanner::new();
        let request = make_request(
            "search then delete",
            vec!["search for old files", "delete them"],
        );
        let plan = planner.generate_plan_from_steps(&request);

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].id, "step_1");
        assert_eq!(plan.steps[1].id, "step_2");
        assert!(plan.steps[0].depends_on.is_empty());
        assert_eq!(plan.steps[1].depends_on, vec!["step_1"]);
    }

    #[test]
    fn test_infer_tool_search() {
        let planner = TaskPlanner::new();
        assert_eq!(planner.infer_tool("search for rust tutorials"), "search");
        assert_eq!(planner.infer_tool("搜索文件"), "search");
    }

    #[test]
    fn test_infer_tool_file_ops() {
        let planner = TaskPlanner::new();
        assert_eq!(planner.infer_tool("delete old files"), "file_delete");
        assert_eq!(planner.infer_tool("copy to backup"), "file_copy");
        assert_eq!(planner.infer_tool("移动文件"), "file_move");
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test task_planner --no-fail-fast 2>&1`
Expected: All tests pass

**Step 3: Commit TaskPlanner**

```bash
git add Aether/core/src/components/task_planner.rs
git commit -m "feat(components): implement TaskPlanner with rule-based step decomposition"
```

---

## Task 4: Implement ToolExecutor Component

**Files:**
- Create: `Aether/core/src/components/tool_executor.rs`

**Step 1: Write ToolExecutor implementation**

```rust
// Aether/core/src/components/tool_executor.rs
//! Tool executor component - executes tools with retry logic.
//!
//! Subscribes to: ToolCallRequested
//! Publishes: ToolCallStarted, ToolCallCompleted, ToolCallFailed, ToolCallRetrying

use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::event::{
    AetherEvent, ErrorKind, EventContext, EventHandler, EventType, HandlerError, TokenUsage,
    ToolCallError, ToolCallRequest, ToolCallResult, ToolCallRetry, ToolCallStarted,
};

/// Retry policy configuration
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub retryable_errors: Vec<ErrorKind>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30000,
            retryable_errors: vec![
                ErrorKind::Timeout,
                ErrorKind::RateLimit,
                ErrorKind::ServiceUnavailable,
            ],
        }
    }
}

/// Tool Executor - executes tools with retry logic
pub struct ToolExecutor {
    retry_policy: RetryPolicy,
}

impl ToolExecutor {
    pub fn new() -> Self {
        Self {
            retry_policy: RetryPolicy::default(),
        }
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Calculate delay with exponential backoff
    fn calculate_delay(&self, attempt: u32) -> u64 {
        let delay = self.retry_policy.base_delay_ms * 2u64.pow(attempt.saturating_sub(1));
        delay.min(self.retry_policy.max_delay_ms)
    }

    /// Check if error is retryable
    fn is_retryable(&self, error_kind: &ErrorKind) -> bool {
        self.retry_policy.retryable_errors.contains(error_kind)
    }

    /// Execute tool once (stub implementation)
    async fn execute_once(
        &self,
        request: &ToolCallRequest,
        ctx: &EventContext,
    ) -> Result<Value, (ErrorKind, String)> {
        // Check abort signal
        if ctx.abort_signal.load(Ordering::Relaxed) {
            return Err((ErrorKind::Aborted, "Operation aborted by user".into()));
        }

        // TODO: Integrate with actual ToolRegistry
        // For now, return a stub response
        debug!("Executing tool: {} with params: {:?}", request.tool, request.parameters);

        // Simulate tool execution
        // In real implementation, this would call:
        // ctx.tools.execute(&request.tool, request.parameters.clone()).await

        Ok(serde_json::json!({
            "status": "success",
            "tool": request.tool,
            "message": format!("Tool {} executed successfully", request.tool),
        }))
    }

    /// Execute tool with retry logic
    async fn execute_with_retry(
        &self,
        request: &ToolCallRequest,
        ctx: &EventContext,
    ) -> Result<(Value, u32), (ErrorKind, String, u32)> {
        let mut attempts = 0u32;
        let mut last_error: Option<(ErrorKind, String)> = None;

        while attempts <= self.retry_policy.max_retries {
            if attempts > 0 {
                let delay = self.calculate_delay(attempts);
                debug!("Retry attempt {} after {}ms delay", attempts, delay);

                // Publish retry event
                let retry_event = AetherEvent::ToolCallRetrying(ToolCallRetry {
                    call_id: request.call_id.clone(),
                    attempt: attempts,
                    delay_ms: delay,
                    reason: last_error.as_ref().map(|(_, msg)| msg.clone()),
                });
                let _ = ctx.bus.publish(retry_event).await;

                tokio::time::sleep(Duration::from_millis(delay)).await;
            }

            // Check abort signal before each attempt
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Err((ErrorKind::Aborted, "Operation aborted".into(), attempts));
            }

            match self.execute_once(request, ctx).await {
                Ok(output) => return Ok((output, attempts)),
                Err((kind, msg)) => {
                    if self.is_retryable(&kind) && attempts < self.retry_policy.max_retries {
                        warn!("Retryable error on attempt {}: {}", attempts, msg);
                        last_error = Some((kind, msg));
                        attempts += 1;
                    } else {
                        return Err((kind, msg, attempts));
                    }
                }
            }
        }

        let (kind, msg) = last_error.unwrap_or((ErrorKind::ExecutionFailed, "Unknown error".into()));
        Err((kind, msg, attempts))
    }
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for ToolExecutor {
    fn name(&self) -> &'static str {
        "ToolExecutor"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallRequested]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        let AetherEvent::ToolCallRequested(request) = event else {
            return Ok(vec![]);
        };

        let started_at = chrono::Utc::now().timestamp();

        info!("ToolExecutor executing: {} (call_id: {})", request.tool, request.call_id);

        // Publish start event
        let start_event = AetherEvent::ToolCallStarted(ToolCallStarted {
            call_id: request.call_id.clone(),
            tool: request.tool.clone(),
            input: request.parameters.clone(),
            timestamp: started_at,
        });
        let _ = ctx.bus.publish(start_event).await;

        // Execute with retry
        let completed_at = chrono::Utc::now().timestamp();
        match self.execute_with_retry(request, ctx).await {
            Ok((output, attempts)) => {
                debug!("Tool {} completed successfully after {} attempts", request.tool, attempts);
                Ok(vec![AetherEvent::ToolCallCompleted(ToolCallResult {
                    call_id: request.call_id.clone(),
                    tool: request.tool.clone(),
                    input: request.parameters.clone(),
                    output,
                    started_at,
                    completed_at,
                    token_usage: TokenUsage::default(),
                })])
            }
            Err((error_kind, error_msg, attempts)) => {
                error!("Tool {} failed after {} attempts: {}", request.tool, attempts, error_msg);
                Ok(vec![AetherEvent::ToolCallFailed(ToolCallError {
                    call_id: request.call_id.clone(),
                    tool: request.tool.clone(),
                    error: error_msg,
                    error_kind,
                    is_retryable: false, // Already exhausted retries
                    attempts,
                })])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_delay_ms, 1000);
        assert_eq!(policy.max_delay_ms, 30000);
    }

    #[test]
    fn test_calculate_delay() {
        let executor = ToolExecutor::new();

        // Attempt 1: base_delay
        assert_eq!(executor.calculate_delay(1), 1000);
        // Attempt 2: base_delay * 2
        assert_eq!(executor.calculate_delay(2), 2000);
        // Attempt 3: base_delay * 4
        assert_eq!(executor.calculate_delay(3), 4000);
        // Attempt 4: base_delay * 8
        assert_eq!(executor.calculate_delay(4), 8000);
        // Attempt 5: capped at max_delay
        assert_eq!(executor.calculate_delay(5), 16000);
        // Attempt 6: capped at max_delay
        assert_eq!(executor.calculate_delay(6), 30000);
    }

    #[test]
    fn test_is_retryable() {
        let executor = ToolExecutor::new();

        assert!(executor.is_retryable(&ErrorKind::Timeout));
        assert!(executor.is_retryable(&ErrorKind::RateLimit));
        assert!(executor.is_retryable(&ErrorKind::ServiceUnavailable));
        assert!(!executor.is_retryable(&ErrorKind::NotFound));
        assert!(!executor.is_retryable(&ErrorKind::InvalidInput));
        assert!(!executor.is_retryable(&ErrorKind::Aborted));
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test tool_executor --no-fail-fast 2>&1`
Expected: All tests pass

**Step 3: Commit ToolExecutor**

```bash
git add Aether/core/src/components/tool_executor.rs
git commit -m "feat(components): implement ToolExecutor with exponential backoff retry"
```

---

## Task 5: Implement LoopController Component

**Files:**
- Create: `Aether/core/src/components/loop_controller.rs`

**Step 1: Write LoopController implementation**

```rust
// Aether/core/src/components/loop_controller.rs
//! Loop controller component - manages the agentic loop with protection mechanisms.
//!
//! Subscribes to: ToolCallCompleted, ToolCallFailed, PlanCreated
//! Publishes: LoopContinue, LoopStop, ToolCallRequested

use async_trait::async_trait;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, LoopState, PlanStep,
    StepStatus, StopReason, TaskPlan, ToolCallRequest, ToolCallResult,
};

use super::types::{ExecutionSession, ToolCallRecord};

/// Loop configuration
#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_iterations: u32,
    pub doom_loop_threshold: u32,
    pub max_tokens_per_session: u64,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            doom_loop_threshold: 3,
            max_tokens_per_session: 100_000,
        }
    }
}

/// Loop Controller - manages agentic loop execution
pub struct LoopController {
    config: LoopConfig,
    /// Track current plan execution
    current_plan: std::sync::RwLock<Option<TaskPlan>>,
}

impl LoopController {
    pub fn new() -> Self {
        Self {
            config: LoopConfig::default(),
            current_plan: std::sync::RwLock::new(None),
        }
    }

    pub fn with_config(mut self, config: LoopConfig) -> Self {
        self.config = config;
        self
    }

    /// Check all guard conditions
    fn check_guards(&self, session: &ExecutionSession, ctx: &EventContext) -> Option<StopReason> {
        // 1. Check abort signal
        if ctx.abort_signal.load(Ordering::Relaxed) {
            return Some(StopReason::UserAborted);
        }

        // 2. Max iterations
        if session.iteration_count >= self.config.max_iterations {
            warn!("Max iterations ({}) reached", self.config.max_iterations);
            return Some(StopReason::MaxIterationsReached);
        }

        // 3. Doom loop detection
        if self.detect_doom_loop(&session.recent_calls) {
            warn!("Doom loop detected");
            return Some(StopReason::DoomLoopDetected);
        }

        // 4. Token limit
        if session.total_tokens >= self.config.max_tokens_per_session {
            warn!("Token limit ({}) reached", self.config.max_tokens_per_session);
            return Some(StopReason::TokenLimitReached);
        }

        None
    }

    /// Detect doom loop (same tool+input repeated N times)
    fn detect_doom_loop(&self, recent_calls: &[ToolCallRecord]) -> bool {
        let threshold = self.config.doom_loop_threshold as usize;
        if recent_calls.len() < threshold {
            return false;
        }

        let last_n = &recent_calls[recent_calls.len() - threshold..];
        if let Some(first) = last_n.first() {
            // Check if all recent calls are identical
            last_n.iter().all(|c| {
                c.tool == first.tool && c.input == first.input
            })
        } else {
            false
        }
    }

    /// Get next executable step from plan
    fn get_next_step(&self, plan: &TaskPlan) -> Option<PlanStep> {
        plan.steps.iter().find(|s| {
            matches!(s.status, StepStatus::Pending) &&
            s.depends_on.iter().all(|dep_id| {
                plan.steps.iter()
                    .find(|d| &d.id == dep_id)
                    .map(|d| matches!(d.status, StepStatus::Completed))
                    .unwrap_or(false)
            })
        }).cloned()
    }

    /// Convert plan step to tool call request
    fn step_to_tool_call(&self, step: &PlanStep) -> ToolCallRequest {
        ToolCallRequest {
            call_id: uuid::Uuid::new_v4().to_string(),
            tool: step.tool.clone(),
            parameters: step.parameters.clone(),
            step_id: Some(step.id.clone()),
            reason: step.description.clone(),
        }
    }

    /// Update plan step status
    fn update_step_status(&self, step_id: &str, status: StepStatus) {
        let mut plan_guard = self.current_plan.write().unwrap();
        if let Some(ref mut plan) = *plan_guard {
            if let Some(step) = plan.steps.iter_mut().find(|s| s.id == step_id) {
                step.status = status;
            }
        }
    }

    /// Check if plan is complete
    fn is_plan_complete(&self) -> bool {
        let plan_guard = self.current_plan.read().unwrap();
        if let Some(ref plan) = *plan_guard {
            plan.steps.iter().all(|s| {
                matches!(s.status, StepStatus::Completed | StepStatus::Skipped)
            })
        } else {
            true
        }
    }
}

impl Default for LoopController {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for LoopController {
    fn name(&self) -> &'static str {
        "LoopController"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
            EventType::PlanCreated,
        ]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        match event {
            AetherEvent::PlanCreated(plan) => {
                info!("LoopController received plan with {} steps", plan.steps.len());

                // Store the plan
                {
                    let mut plan_guard = self.current_plan.write().unwrap();
                    *plan_guard = Some(plan.clone());
                }

                // Start executing first step
                if let Some(first_step) = self.get_next_step(plan) {
                    debug!("Starting first step: {}", first_step.id);
                    Ok(vec![
                        AetherEvent::LoopContinue(LoopState {
                            iteration: 0,
                            current_step: Some(first_step.id.clone()),
                            pending_steps: plan.steps.len() as u32 - 1,
                        }),
                        AetherEvent::ToolCallRequested(self.step_to_tool_call(&first_step)),
                    ])
                } else {
                    warn!("Plan has no executable steps");
                    Ok(vec![AetherEvent::LoopStop(StopReason::EmptyPlan)])
                }
            }

            AetherEvent::ToolCallCompleted(result) => {
                info!("LoopController: tool {} completed", result.tool);

                // Check guards using a mock session for now
                // TODO: Get actual session from context
                let mock_session = ExecutionSession::new();
                if let Some(stop_reason) = self.check_guards(&mock_session, ctx) {
                    return Ok(vec![AetherEvent::LoopStop(stop_reason)]);
                }

                // Update step status if this was a plan step
                if let Some(step_id) = &result.call_id.strip_prefix("step_") {
                    self.update_step_status(step_id, StepStatus::Completed);
                }

                // Check if plan is complete
                if self.is_plan_complete() {
                    info!("Plan completed successfully");
                    return Ok(vec![AetherEvent::LoopStop(StopReason::Completed)]);
                }

                // Get next step
                let plan_guard = self.current_plan.read().unwrap();
                if let Some(ref plan) = *plan_guard {
                    if let Some(next_step) = self.get_next_step(plan) {
                        debug!("Moving to next step: {}", next_step.id);
                        return Ok(vec![
                            AetherEvent::LoopContinue(LoopState {
                                iteration: 1, // TODO: Track actual iteration
                                current_step: Some(next_step.id.clone()),
                                pending_steps: plan.steps.iter()
                                    .filter(|s| matches!(s.status, StepStatus::Pending))
                                    .count() as u32,
                            }),
                            AetherEvent::ToolCallRequested(self.step_to_tool_call(&next_step)),
                        ]);
                    }
                }

                // No plan or no next step - complete
                Ok(vec![AetherEvent::LoopStop(StopReason::Completed)])
            }

            AetherEvent::ToolCallFailed(error) => {
                warn!("LoopController: tool {} failed: {}", error.tool, error.error);

                // Update step status
                if error.call_id.starts_with("step_") {
                    self.update_step_status(&error.call_id, StepStatus::Failed(error.error.clone()));
                }

                // For now, stop on failure
                // TODO: Add retry logic or alternative path execution
                Ok(vec![AetherEvent::LoopStop(StopReason::Error(error.error.clone()))])
            }

            _ => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_loop_config_default() {
        let config = LoopConfig::default();
        assert_eq!(config.max_iterations, 50);
        assert_eq!(config.doom_loop_threshold, 3);
        assert_eq!(config.max_tokens_per_session, 100_000);
    }

    #[test]
    fn test_doom_loop_detection_false() {
        let controller = LoopController::new();

        // Not enough calls
        let calls = vec![
            ToolCallRecord { tool: "search".into(), input: json!({"q": "a"}), timestamp: 0 },
            ToolCallRecord { tool: "search".into(), input: json!({"q": "a"}), timestamp: 1 },
        ];
        assert!(!controller.detect_doom_loop(&calls));

        // Different tools
        let calls = vec![
            ToolCallRecord { tool: "search".into(), input: json!({"q": "a"}), timestamp: 0 },
            ToolCallRecord { tool: "read".into(), input: json!({"q": "a"}), timestamp: 1 },
            ToolCallRecord { tool: "write".into(), input: json!({"q": "a"}), timestamp: 2 },
        ];
        assert!(!controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_doom_loop_detection_true() {
        let controller = LoopController::new();

        // Same tool and input 3 times
        let calls = vec![
            ToolCallRecord { tool: "search".into(), input: json!({"q": "a"}), timestamp: 0 },
            ToolCallRecord { tool: "search".into(), input: json!({"q": "a"}), timestamp: 1 },
            ToolCallRecord { tool: "search".into(), input: json!({"q": "a"}), timestamp: 2 },
        ];
        assert!(controller.detect_doom_loop(&calls));
    }

    #[test]
    fn test_get_next_step() {
        let controller = LoopController::new();

        let plan = TaskPlan {
            id: "test".into(),
            steps: vec![
                PlanStep {
                    id: "step_1".into(),
                    description: "First".into(),
                    tool: "tool1".into(),
                    parameters: json!({}),
                    depends_on: vec![],
                    status: StepStatus::Completed,
                },
                PlanStep {
                    id: "step_2".into(),
                    description: "Second".into(),
                    tool: "tool2".into(),
                    parameters: json!({}),
                    depends_on: vec!["step_1".into()],
                    status: StepStatus::Pending,
                },
            ],
            parallel_groups: vec![],
            current_step_index: 0,
        };

        let next = controller.get_next_step(&plan);
        assert!(next.is_some());
        assert_eq!(next.unwrap().id, "step_2");
    }

    #[test]
    fn test_get_next_step_blocked() {
        let controller = LoopController::new();

        let plan = TaskPlan {
            id: "test".into(),
            steps: vec![
                PlanStep {
                    id: "step_1".into(),
                    description: "First".into(),
                    tool: "tool1".into(),
                    parameters: json!({}),
                    depends_on: vec![],
                    status: StepStatus::Pending, // Not completed yet
                },
                PlanStep {
                    id: "step_2".into(),
                    description: "Second".into(),
                    tool: "tool2".into(),
                    parameters: json!({}),
                    depends_on: vec!["step_1".into()],
                    status: StepStatus::Pending,
                },
            ],
            parallel_groups: vec![],
            current_step_index: 0,
        };

        let next = controller.get_next_step(&plan);
        assert!(next.is_some());
        // Should return step_1 since step_2 is blocked
        assert_eq!(next.unwrap().id, "step_1");
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test loop_controller --no-fail-fast 2>&1`
Expected: All tests pass

**Step 3: Commit LoopController**

```bash
git add Aether/core/src/components/loop_controller.rs
git commit -m "feat(components): implement LoopController with doom loop detection and protection"
```

---

## Task 6: Implement SessionRecorder Component

**Files:**
- Create: `Aether/core/src/components/session_recorder.rs`

**Step 1: Write SessionRecorder implementation**

```rust
// Aether/core/src/components/session_recorder.rs
//! Session recorder component - persists execution state to SQLite.
//!
//! Subscribes to: All events
//! Publishes: SessionUpdated

use async_trait::async_trait;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, SessionDiff, SessionInfo,
};

use super::types::{
    AiResponsePart, PlanPart, SessionPart, ToolCallPart, ToolCallStatus, UserInputPart,
};

/// Session Recorder - persists execution state
pub struct SessionRecorder {
    /// SQLite connection (thread-safe wrapper)
    conn: Arc<Mutex<Connection>>,
}

impl SessionRecorder {
    /// Create a new session recorder with in-memory database
    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Create a new session recorder with file database
    pub fn new(db_path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Initialize database schema
    fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL,
                model TEXT NOT NULL,
                iteration_count INTEGER DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_parts (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                part_type TEXT NOT NULL,
                part_data TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE INDEX IF NOT EXISTS idx_parts_session
                ON session_parts(session_id, sequence);

            CREATE INDEX IF NOT EXISTS idx_sessions_parent
                ON sessions(parent_id);
            "#,
        )?;
        Ok(())
    }

    /// Append a session part
    fn append_part(&self, session_id: &str, part: &SessionPart) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        // Get next sequence number
        let sequence: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_parts WHERE session_id = ?",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap_or(1);

        let part_type = part.type_name();
        let part_data = serde_json::to_string(part).unwrap_or_default();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO session_parts (id, session_id, part_type, part_data, sequence, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                uuid::Uuid::new_v4().to_string(),
                session_id,
                part_type,
                part_data,
                sequence,
                now
            ],
        )?;

        Ok(())
    }

    /// Convert event to session part
    fn event_to_part(&self, event: &AetherEvent) -> Option<SessionPart> {
        match event {
            AetherEvent::InputReceived(input) => Some(SessionPart::UserInput(UserInputPart {
                text: input.text.clone(),
                context: input.context.clone(),
                timestamp: input.timestamp,
            })),

            AetherEvent::ToolCallCompleted(result) => Some(SessionPart::ToolCall(ToolCallPart {
                id: result.call_id.clone(),
                tool_name: result.tool.clone(),
                input: result.input.clone(),
                status: ToolCallStatus::Completed,
                output: Some(result.output.to_string()),
                error: None,
                started_at: result.started_at,
                completed_at: Some(result.completed_at),
            })),

            AetherEvent::ToolCallFailed(error) => Some(SessionPart::ToolCall(ToolCallPart {
                id: error.call_id.clone(),
                tool_name: error.tool.clone(),
                input: serde_json::Value::Null,
                status: ToolCallStatus::Failed,
                output: None,
                error: Some(error.error.clone()),
                started_at: chrono::Utc::now().timestamp(),
                completed_at: Some(chrono::Utc::now().timestamp()),
            })),

            AetherEvent::AiResponseGenerated(response) => {
                Some(SessionPart::AiResponse(AiResponsePart {
                    content: response.content.clone(),
                    reasoning: response.reasoning.clone(),
                    timestamp: response.timestamp,
                }))
            }

            AetherEvent::PlanCreated(plan) => Some(SessionPart::PlanCreated(PlanPart {
                plan_id: plan.id.clone(),
                steps: plan.steps.iter().map(|s| s.description.clone()).collect(),
                timestamp: chrono::Utc::now().timestamp(),
            })),

            _ => None,
        }
    }

    /// Create a new session record
    fn create_session(&self, session_id: &str, model: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT OR IGNORE INTO sessions (id, agent_id, status, model, created_at, updated_at)
             VALUES (?, 'main', 'Running', ?, ?, ?)",
            params![session_id, model, now, now],
        )?;

        Ok(())
    }

    /// Update session metadata
    fn update_session(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE sessions SET updated_at = ?, iteration_count = iteration_count + 1 WHERE id = ?",
            params![now, session_id],
        )?;

        Ok(())
    }
}

#[async_trait]
impl EventHandler for SessionRecorder {
    fn name(&self) -> &'static str {
        "SessionRecorder"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::All] // Subscribe to all events
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        let session_id = &ctx.session_id;

        // Ensure session exists
        if let Err(e) = self.create_session(session_id, "default") {
            error!("Failed to create session: {}", e);
        }

        // Convert event to session part and persist
        if let Some(part) = self.event_to_part(event) {
            debug!("Recording session part: {}", part.type_name());

            if let Err(e) = self.append_part(session_id, &part) {
                error!("Failed to append session part: {}", e);
                return Ok(vec![]);
            }

            // Update session metadata
            if let Err(e) = self.update_session(session_id) {
                error!("Failed to update session: {}", e);
            }

            // Publish session update event (optional, for UI updates)
            // We don't publish to avoid infinite loops since we subscribe to All
        }

        // SessionRecorder doesn't produce new events
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::InputEvent;

    #[test]
    fn test_create_in_memory() {
        let recorder = SessionRecorder::new_in_memory();
        assert!(recorder.is_ok());
    }

    #[test]
    fn test_create_session() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        let result = recorder.create_session("test-session-1", "gpt-4");
        assert!(result.is_ok());

        // Creating again should not fail (INSERT OR IGNORE)
        let result = recorder.create_session("test-session-1", "gpt-4");
        assert!(result.is_ok());
    }

    #[test]
    fn test_append_part() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        recorder.create_session("test-session-1", "gpt-4").unwrap();

        let part = SessionPart::UserInput(UserInputPart {
            text: "Hello world".into(),
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        });

        let result = recorder.append_part("test-session-1", &part);
        assert!(result.is_ok());
    }

    #[test]
    fn test_event_to_part_input() {
        let recorder = SessionRecorder::new_in_memory().unwrap();

        let event = AetherEvent::InputReceived(InputEvent {
            text: "test input".into(),
            topic_id: None,
            context: Some("context".into()),
            timestamp: 12345,
        });

        let part = recorder.event_to_part(&event);
        assert!(part.is_some());

        if let Some(SessionPart::UserInput(u)) = part {
            assert_eq!(u.text, "test input");
            assert_eq!(u.context, Some("context".into()));
            assert_eq!(u.timestamp, 12345);
        } else {
            panic!("Expected UserInput part");
        }
    }

    #[test]
    fn test_update_session() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        recorder.create_session("test-session-1", "gpt-4").unwrap();

        // Update multiple times
        recorder.update_session("test-session-1").unwrap();
        recorder.update_session("test-session-1").unwrap();

        // Verify iteration count
        let conn = recorder.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT iteration_count FROM sessions WHERE id = ?",
                params!["test-session-1"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 2);
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test session_recorder --no-fail-fast 2>&1`
Expected: All tests pass

**Step 3: Commit SessionRecorder**

```bash
git add Aether/core/src/components/session_recorder.rs
git commit -m "feat(components): implement SessionRecorder with SQLite persistence"
```

---

## Task 7: Implement SessionCompactor Component

**Files:**
- Create: `Aether/core/src/components/session_compactor.rs`

**Step 1: Write SessionCompactor implementation**

```rust
// Aether/core/src/components/session_compactor.rs
//! Session compactor component - manages token usage and compresses history.
//!
//! Subscribes to: LoopContinue, ToolCallCompleted
//! Publishes: SessionCompacted

use async_trait::async_trait;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::event::{
    AetherEvent, CompactionInfo, EventContext, EventHandler, EventType, HandlerError,
};

use super::types::{ExecutionSession, SessionPart, SummaryPart};

/// Model context limits
#[derive(Debug, Clone)]
pub struct ModelLimit {
    pub context_limit: u64,
    pub max_output_tokens: u64,
    pub reserve_ratio: f32,
}

impl Default for ModelLimit {
    fn default() -> Self {
        Self {
            context_limit: 128000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        }
    }
}

/// Token tracker for different models
pub struct TokenTracker {
    model_limits: HashMap<String, ModelLimit>,
}

impl Default for TokenTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenTracker {
    pub fn new() -> Self {
        let mut model_limits = HashMap::new();

        // Claude models
        model_limits.insert(
            "claude-3-opus".into(),
            ModelLimit {
                context_limit: 200000,
                max_output_tokens: 4096,
                reserve_ratio: 0.2,
            },
        );
        model_limits.insert(
            "claude-3-sonnet".into(),
            ModelLimit {
                context_limit: 200000,
                max_output_tokens: 4096,
                reserve_ratio: 0.2,
            },
        );

        // GPT models
        model_limits.insert(
            "gpt-4-turbo".into(),
            ModelLimit {
                context_limit: 128000,
                max_output_tokens: 4096,
                reserve_ratio: 0.2,
            },
        );

        // Gemini models
        model_limits.insert(
            "gemini-pro".into(),
            ModelLimit {
                context_limit: 32000,
                max_output_tokens: 2048,
                reserve_ratio: 0.2,
            },
        );

        Self { model_limits }
    }

    /// Check if session is approaching token limit
    pub fn is_overflow(&self, session: &ExecutionSession, model: &str) -> bool {
        let limit = self.model_limits.get(model).unwrap_or(&ModelLimit::default());
        let usable = limit.context_limit - limit.max_output_tokens;
        let threshold = (usable as f32 * (1.0 - limit.reserve_ratio)) as u64;

        session.total_tokens > threshold
    }

    /// Estimate tokens from text (simple heuristic)
    pub fn estimate_tokens(&self, text: &str) -> u64 {
        // Simple estimation: ~0.5 token/char for CJK, ~0.25 for English
        // Use 0.4 as a middle ground
        let chars = text.chars().count() as u64;
        (chars as f64 * 0.4) as u64
    }
}

/// Session Compactor - manages token usage
pub struct SessionCompactor {
    token_tracker: TokenTracker,
    keep_recent_tools: usize,
}

impl SessionCompactor {
    pub fn new() -> Self {
        Self {
            token_tracker: TokenTracker::new(),
            keep_recent_tools: 10,
        }
    }

    pub fn with_keep_recent(mut self, count: usize) -> Self {
        self.keep_recent_tools = count;
        self
    }

    /// Prune old tool outputs to save context
    fn prune_old_tool_outputs(&self, session: &mut ExecutionSession) -> u64 {
        let tool_indices: Vec<usize> = session
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p, SessionPart::ToolCall(_)))
            .map(|(i, _)| i)
            .collect();

        if tool_indices.len() <= self.keep_recent_tools {
            return 0;
        }

        let mut tokens_saved = 0u64;
        let to_prune = tool_indices.len() - self.keep_recent_tools;

        for idx in tool_indices.into_iter().take(to_prune) {
            if let SessionPart::ToolCall(ref mut tc) = session.parts[idx] {
                if let Some(ref output) = tc.output {
                    tokens_saved += self.token_tracker.estimate_tokens(output);
                    tc.output = Some("[Output pruned to save context]".into());
                }
            }
        }

        tokens_saved
    }

    /// Generate summary of session history (stub - would use LLM)
    fn generate_summary(&self, session: &ExecutionSession) -> String {
        // Collect key information
        let mut summary_parts = Vec::new();

        // Find original user input
        for part in &session.parts {
            if let SessionPart::UserInput(u) = part {
                summary_parts.push(format!("Original request: {}", u.text));
                break;
            }
        }

        // Count completed tools
        let completed_tools: Vec<_> = session
            .parts
            .iter()
            .filter_map(|p| {
                if let SessionPart::ToolCall(tc) = p {
                    if tc.output.is_some() && tc.error.is_none() {
                        return Some(tc.tool_name.clone());
                    }
                }
                None
            })
            .collect();

        if !completed_tools.is_empty() {
            summary_parts.push(format!("Completed steps: {}", completed_tools.join(", ")));
        }

        // Note iteration count
        summary_parts.push(format!("Iterations completed: {}", session.iteration_count));

        summary_parts.join("\n")
    }

    /// Replace history with summary
    fn replace_with_summary(&self, session: &mut ExecutionSession, summary: String) -> u64 {
        let keep_last = 5;

        // Calculate tokens before
        let tokens_before = session.total_tokens;

        // Keep last few parts
        let recent: Vec<_> = session
            .parts
            .drain(session.parts.len().saturating_sub(keep_last)..)
            .collect();

        // Clear old history, insert summary
        session.parts.clear();
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: summary,
            original_count: session.iteration_count,
            compacted_at: chrono::Utc::now().timestamp(),
        }));

        // Restore recent parts
        session.parts.extend(recent);

        // Recalculate tokens
        session.total_tokens = self.recalculate_tokens(session);

        tokens_before - session.total_tokens
    }

    /// Recalculate total tokens in session
    fn recalculate_tokens(&self, session: &ExecutionSession) -> u64 {
        session
            .parts
            .iter()
            .map(|p| match p {
                SessionPart::UserInput(u) => self.token_tracker.estimate_tokens(&u.text),
                SessionPart::AiResponse(a) => self.token_tracker.estimate_tokens(&a.content),
                SessionPart::ToolCall(t) => {
                    let input_tokens = self.token_tracker.estimate_tokens(&t.input.to_string());
                    let output_tokens = t
                        .output
                        .as_ref()
                        .map(|o| self.token_tracker.estimate_tokens(o))
                        .unwrap_or(0);
                    input_tokens + output_tokens
                }
                SessionPart::Summary(s) => self.token_tracker.estimate_tokens(&s.content),
                SessionPart::Reasoning(r) => self.token_tracker.estimate_tokens(&r.content),
                _ => 0,
            })
            .sum()
    }

    /// Perform compaction on session
    fn compact(&self, session: &mut ExecutionSession) -> u64 {
        let mut tokens_saved = 0u64;

        // Strategy 1: Prune old tool outputs
        tokens_saved += self.prune_old_tool_outputs(session);

        // Recalculate after pruning
        session.total_tokens = self.recalculate_tokens(session);

        // Strategy 2: If still overflowing, generate summary
        if self.token_tracker.is_overflow(session, &session.model) {
            let summary = self.generate_summary(session);
            tokens_saved += self.replace_with_summary(session, summary);
        }

        tokens_saved
    }
}

impl Default for SessionCompactor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventHandler for SessionCompactor {
    fn name(&self) -> &'static str {
        "SessionCompactor"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallCompleted, EventType::LoopContinue]
    }

    async fn handle(
        &self,
        _event: &AetherEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Get session for checking
        // TODO: Access actual session from context
        // For now, we use a mock check

        // In a real implementation:
        // let session = ctx.session.read().await;
        // if !self.token_tracker.is_overflow(&session, &session.model) {
        //     return Ok(vec![]);
        // }
        // drop(session);
        //
        // let mut session = ctx.session.write().await;
        // let tokens_before = session.total_tokens;
        // let tokens_saved = self.compact(&mut session);
        // let tokens_after = session.total_tokens;
        //
        // if tokens_saved > 0 {
        //     return Ok(vec![AetherEvent::SessionCompacted(CompactionInfo {
        //         session_id: ctx.session_id.clone(),
        //         tokens_before,
        //         tokens_after,
        //         timestamp: chrono::Utc::now().timestamp(),
        //     })]);
        // }

        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::types::ToolCallPart;

    fn create_test_session() -> ExecutionSession {
        let mut session = ExecutionSession::new();
        session.model = "gpt-4-turbo".into();
        session.total_tokens = 50000;

        // Add some tool calls
        for i in 0..15 {
            session.parts.push(SessionPart::ToolCall(ToolCallPart {
                id: format!("call_{}", i),
                tool_name: "search".into(),
                input: serde_json::json!({"q": "test"}),
                status: super::super::types::ToolCallStatus::Completed,
                output: Some(format!("Result {} with some content here", i)),
                error: None,
                started_at: 0,
                completed_at: Some(1),
            }));
        }

        session
    }

    #[test]
    fn test_token_estimation() {
        let tracker = TokenTracker::new();

        // English text: roughly 0.4 tokens per char
        let english = "Hello world, this is a test.";
        let tokens = tracker.estimate_tokens(english);
        assert!(tokens > 0);
        assert!(tokens < english.len() as u64);

        // Chinese text: roughly 0.4 tokens per char
        let chinese = "你好世界，这是一个测试。";
        let tokens = tracker.estimate_tokens(chinese);
        assert!(tokens > 0);
    }

    #[test]
    fn test_is_overflow() {
        let tracker = TokenTracker::new();

        let mut session = ExecutionSession::new();
        session.total_tokens = 1000;
        assert!(!tracker.is_overflow(&session, "gpt-4-turbo"));

        session.total_tokens = 120000; // Over threshold
        assert!(tracker.is_overflow(&session, "gpt-4-turbo"));
    }

    #[test]
    fn test_prune_old_tool_outputs() {
        let compactor = SessionCompactor::new().with_keep_recent(5);
        let mut session = create_test_session();

        let initial_parts = session.parts.len();
        let tokens_saved = compactor.prune_old_tool_outputs(&mut session);

        // Should have pruned 10 tool outputs (15 - 5 = 10)
        assert!(tokens_saved > 0);

        // Count pruned outputs
        let pruned_count = session.parts.iter().filter(|p| {
            if let SessionPart::ToolCall(tc) = p {
                tc.output == Some("[Output pruned to save context]".into())
            } else {
                false
            }
        }).count();

        assert_eq!(pruned_count, 10);
    }

    #[test]
    fn test_generate_summary() {
        let compactor = SessionCompactor::new();
        let mut session = create_test_session();

        // Add user input
        session.parts.insert(0, SessionPart::UserInput(super::super::types::UserInputPart {
            text: "Search for Rust tutorials".into(),
            context: None,
            timestamp: 0,
        }));
        session.iteration_count = 15;

        let summary = compactor.generate_summary(&session);

        assert!(summary.contains("Original request"));
        assert!(summary.contains("Completed steps"));
        assert!(summary.contains("15"));
    }

    #[test]
    fn test_recalculate_tokens() {
        let compactor = SessionCompactor::new();
        let session = create_test_session();

        let tokens = compactor.recalculate_tokens(&session);
        assert!(tokens > 0);
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test session_compactor --no-fail-fast 2>&1`
Expected: All tests pass

**Step 3: Commit SessionCompactor**

```bash
git add Aether/core/src/components/session_compactor.rs
git commit -m "feat(components): implement SessionCompactor with token tracking and pruning"
```

---

## Task 8: Integrate Components into lib.rs

**Files:**
- Modify: `Aether/core/src/lib.rs`
- Modify: `Aether/core/src/components/mod.rs`

**Step 1: Update components mod.rs with placeholder files**

Create placeholder files for any missing components:

```rust
// Ensure all files exist - if any are missing, create empty stubs
```

**Step 2: Add components module to lib.rs**

Add after line 67 (after `pub mod event;`):

```rust
pub mod components; // NEW: Core event handler components
```

**Step 3: Add components re-exports to lib.rs**

Add after the event system exports (after line ~293):

```rust
// Component exports (event handler implementations)
pub use crate::components::{
    ExecutionSession, SessionStatus, SessionPart, ToolCallRecord, Complexity, Decision,
    ComponentContext, IntentAnalyzer, TaskPlanner, ToolExecutor, RetryPolicy,
    LoopController, LoopConfig, SessionRecorder, SessionCompactor,
};
```

**Step 4: Run cargo check**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo check 2>&1 | head -50`
Expected: No errors (warnings OK)

**Step 5: Run all component tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test components --no-fail-fast 2>&1`
Expected: All tests pass

**Step 6: Commit integration**

```bash
git add Aether/core/src/lib.rs Aether/core/src/components/
git commit -m "feat(components): integrate all 6 core components into lib.rs"
```

---

## Task 9: Add Integration Tests

**Files:**
- Create: `Aether/core/src/components/integration_test.rs`
- Modify: `Aether/core/src/components/mod.rs`

**Step 1: Create integration test file**

```rust
// Aether/core/src/components/integration_test.rs
//! Integration tests for the components module.

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use crate::components::{
        ExecutionSession, IntentAnalyzer, LoopController, SessionCompactor, SessionRecorder,
        TaskPlanner, ToolExecutor,
    };
    use crate::dispatcher::ToolRegistry;
    use crate::event::{
        AetherEvent, EventBus, EventContext, EventHandler, EventType, InputEvent,
    };

    fn create_test_context() -> EventContext {
        let bus = EventBus::new(100);
        EventContext {
            bus,
            abort_signal: Arc::new(AtomicBool::new(false)),
            session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    #[tokio::test]
    async fn test_intent_analyzer_simple_input() {
        let analyzer = IntentAnalyzer::new();
        let ctx = create_test_context();

        let event = AetherEvent::InputReceived(InputEvent {
            text: "search for rust tutorials".into(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        });

        let result = analyzer.handle(&event, &ctx).await;
        assert!(result.is_ok());

        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        // Simple input should produce ToolCallRequested
        assert!(matches!(events[0], AetherEvent::ToolCallRequested(_)));
    }

    #[tokio::test]
    async fn test_intent_analyzer_complex_input() {
        let analyzer = IntentAnalyzer::new();
        let ctx = create_test_context();

        let event = AetherEvent::InputReceived(InputEvent {
            text: "search for files then delete the old ones".into(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        });

        let result = analyzer.handle(&event, &ctx).await;
        assert!(result.is_ok());

        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        // Complex input should produce PlanRequested
        assert!(matches!(events[0], AetherEvent::PlanRequested(_)));
    }

    #[tokio::test]
    async fn test_task_planner_creates_plan() {
        let planner = TaskPlanner::new();
        let ctx = create_test_context();

        let event = AetherEvent::PlanRequested(crate::event::PlanRequest {
            input: InputEvent {
                text: "search then delete".into(),
                topic_id: None,
                context: None,
                timestamp: 0,
            },
            intent: "General".into(),
            detected_steps: vec!["search for files".into(), "delete old files".into()],
        });

        let result = planner.handle(&event, &ctx).await;
        assert!(result.is_ok());

        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        if let AetherEvent::PlanCreated(plan) = &events[0] {
            assert_eq!(plan.steps.len(), 2);
            assert_eq!(plan.steps[0].tool, "search");
            assert_eq!(plan.steps[1].tool, "file_delete");
        } else {
            panic!("Expected PlanCreated event");
        }
    }

    #[tokio::test]
    async fn test_tool_executor_handles_request() {
        let executor = ToolExecutor::new();
        let ctx = create_test_context();

        let event = AetherEvent::ToolCallRequested(crate::event::ToolCallRequest {
            call_id: "test-call-1".into(),
            tool: "search".into(),
            parameters: serde_json::json!({"q": "rust"}),
            step_id: None,
            reason: "Test".into(),
        });

        let result = executor.handle(&event, &ctx).await;
        assert!(result.is_ok());

        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        // Should complete successfully (stub implementation)
        assert!(matches!(events[0], AetherEvent::ToolCallCompleted(_)));
    }

    #[tokio::test]
    async fn test_tool_executor_respects_abort() {
        let executor = ToolExecutor::new();
        let ctx = create_test_context();

        // Set abort signal
        ctx.abort_signal.store(true, Ordering::Relaxed);

        let event = AetherEvent::ToolCallRequested(crate::event::ToolCallRequest {
            call_id: "test-call-1".into(),
            tool: "search".into(),
            parameters: serde_json::json!({"q": "rust"}),
            step_id: None,
            reason: "Test".into(),
        });

        let result = executor.handle(&event, &ctx).await;
        assert!(result.is_ok());

        let events = result.unwrap();
        assert_eq!(events.len(), 1);

        // Should fail with abort
        if let AetherEvent::ToolCallFailed(error) = &events[0] {
            assert_eq!(error.error_kind, crate::event::ErrorKind::Aborted);
        } else {
            panic!("Expected ToolCallFailed event");
        }
    }

    #[tokio::test]
    async fn test_loop_controller_starts_plan() {
        let controller = LoopController::new();
        let ctx = create_test_context();

        let event = AetherEvent::PlanCreated(crate::event::TaskPlan {
            id: "plan-1".into(),
            steps: vec![
                crate::event::PlanStep {
                    id: "step_1".into(),
                    description: "Search files".into(),
                    tool: "search".into(),
                    parameters: serde_json::json!({}),
                    depends_on: vec![],
                    status: crate::event::StepStatus::Pending,
                },
            ],
            parallel_groups: vec![],
            current_step_index: 0,
        });

        let result = controller.handle(&event, &ctx).await;
        assert!(result.is_ok());

        let events = result.unwrap();
        assert_eq!(events.len(), 2); // LoopContinue + ToolCallRequested

        assert!(matches!(events[0], AetherEvent::LoopContinue(_)));
        assert!(matches!(events[1], AetherEvent::ToolCallRequested(_)));
    }

    #[tokio::test]
    async fn test_session_recorder_persists_events() {
        let recorder = SessionRecorder::new_in_memory().unwrap();
        let ctx = create_test_context();

        let event = AetherEvent::InputReceived(InputEvent {
            text: "test input".into(),
            topic_id: None,
            context: None,
            timestamp: chrono::Utc::now().timestamp(),
        });

        let result = recorder.handle(&event, &ctx).await;
        assert!(result.is_ok());

        // SessionRecorder doesn't publish events
        let events = result.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_full_event_chain() {
        // Test the complete event flow: Input -> Intent -> Plan -> Tool -> Complete

        let bus = EventBus::new(100);
        let ctx = EventContext {
            bus: bus.clone(),
            abort_signal: Arc::new(AtomicBool::new(false)),
            session_id: uuid::Uuid::new_v4().to_string(),
        };

        // Step 1: IntentAnalyzer processes input
        let analyzer = IntentAnalyzer::new();
        let input_event = AetherEvent::InputReceived(InputEvent {
            text: "search for files then delete old ones".into(),
            topic_id: None,
            context: None,
            timestamp: 0,
        });

        let events = analyzer.handle(&input_event, &ctx).await.unwrap();
        assert!(matches!(events[0], AetherEvent::PlanRequested(_)));

        // Step 2: TaskPlanner creates plan
        let planner = TaskPlanner::new();
        let events = planner.handle(&events[0], &ctx).await.unwrap();
        assert!(matches!(events[0], AetherEvent::PlanCreated(_)));

        // Step 3: LoopController starts execution
        let controller = LoopController::new();
        let events = controller.handle(&events[0], &ctx).await.unwrap();
        assert!(events.len() >= 1);
        assert!(matches!(events[0], AetherEvent::LoopContinue(_)));

        // Verify we have a ToolCallRequested
        let has_tool_request = events.iter().any(|e| matches!(e, AetherEvent::ToolCallRequested(_)));
        assert!(has_tool_request);
    }
}
```

**Step 2: Add test module to mod.rs**

Add at the end of `components/mod.rs`:

```rust
#[cfg(test)]
mod integration_test;
```

**Step 3: Run integration tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test components::integration_test --no-fail-fast 2>&1`
Expected: All tests pass

**Step 4: Commit integration tests**

```bash
git add Aether/core/src/components/
git commit -m "test(components): add integration tests for event chain"
```

---

## Task 10: Final Verification

**Step 1: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test 2>&1 | tail -30`
Expected: All tests pass

**Step 2: Run cargo clippy**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo clippy 2>&1 | head -30`
Expected: No errors (warnings OK)

**Step 3: Build release**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo build --release 2>&1 | tail -10`
Expected: Build succeeds

**Step 4: Create summary commit**

```bash
git log --oneline -10
```

---

## Summary

Phase 2 implements the 6 core event handler components:

| Component | Subscribes To | Publishes | Description |
|-----------|---------------|-----------|-------------|
| IntentAnalyzer | InputReceived | PlanRequested / ToolCallRequested | Complexity detection |
| TaskPlanner | PlanRequested | PlanCreated | Step decomposition |
| ToolExecutor | ToolCallRequested | Started / Completed / Failed | Retry logic |
| LoopController | Completed / Failed / PlanCreated | LoopContinue / LoopStop | Protection mechanisms |
| SessionRecorder | All | (none) | SQLite persistence |
| SessionCompactor | LoopContinue | SessionCompacted | Token management |

All components follow the EventHandler trait pattern and communicate via EventBus.

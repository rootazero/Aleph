# Design: enhance-l3-agent-planning

## Architecture Overview

```
User Input
     │
┌────┴────┐
│   L1    │  Regex Match (<10ms)
└────┬────┘
     │ miss
┌────┴────┐
│   L2    │  Semantic Match (200-500ms)
└────┬────┘
     │ miss or low confidence
┌────┴────────────────────────────────────────┐
│              Enhanced L3 Router             │
│                                             │
│  ┌─────────────────┐                        │
│  │  Quick Heuristic │  <10ms               │
│  │  (multi-verb?)   │                       │
│  └────────┬────────┘                        │
│           │                                 │
│     ┌─────┴─────┐                           │
│     │           │                           │
│   Single     Likely                         │
│   Tool       MultiStep                      │
│     │           │                           │
│  ┌──┴──┐   ┌────┴────┐                      │
│  │ L3  │   │   L3    │  LLM Planning        │
│  │Route│   │ Planner │  (500ms-2s)          │
│  └──┬──┘   └────┬────┘                      │
│     │           │                           │
│     ▼           ▼                           │
│  SingleTool  ExecutionPlan                  │
└─────────────────────────────────────────────┘
           │
     ┌─────┴─────┐
     │           │
 SingleTool   ExecutionPlan
     │           │
     ▼           ▼
  Execute    ┌───────────────┐
  Directly   │ PlanConfirm UI│
             └───────┬───────┘
                     │ confirmed
             ┌───────┴───────┐
             │ PlanExecutor  │
             │ (sequential)  │
             └───────────────┘
```

## Core Data Structures

### Extended IntentAction

```rust
/// Recommended action based on confidence and task complexity
pub enum IntentAction {
    /// Execute single tool directly (confidence >= auto_execute)
    Execute,

    /// Execute multi-step plan (NEW)
    ExecutePlan {
        plan: TaskPlan,
    },

    /// Request user confirmation (medium confidence)
    RequestConfirmation,

    /// Request clarification for missing parameters
    RequestClarification {
        prompt: String,
        suggestions: Vec<String>,
    },

    /// Fall back to general chat (no tool match)
    GeneralChat,
}
```

### TaskPlan Structure

```rust
/// Execution plan for multi-step tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    /// Unique plan identifier
    pub id: Uuid,

    /// Natural language description of the plan
    pub description: String,

    /// Ordered list of execution steps
    pub steps: Vec<PlanStep>,

    /// Overall confidence score (0.0-1.0)
    pub confidence: f32,

    /// Whether plan requires user confirmation
    pub requires_confirmation: bool,

    /// Estimated total duration hint
    pub estimated_duration_hint: Option<String>,

    /// Whether plan contains irreversible operations
    pub has_irreversible_steps: bool,
}

/// Single step in an execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step index (1-based for display)
    pub index: u32,

    /// Tool to execute
    pub tool_name: String,

    /// Tool parameters (may contain $prev reference)
    pub parameters: serde_json::Value,

    /// Human-readable step description
    pub description: String,

    /// Safety level of this step
    pub safety_level: ToolSafetyLevel,

    /// Maximum execution time for this step
    pub timeout_ms: u64,
}

/// Tool safety classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolSafetyLevel {
    /// Read-only operations (search, query, read file)
    ReadOnly,

    /// Can be undone (copy file, create file)
    Reversible,

    /// Cannot be undone but low risk (send notification)
    IrreversibleLowRisk,

    /// Cannot be undone and high risk (delete, execute command)
    IrreversibleHighRisk,
}
```

### Step Result Passing

```rust
/// Result from executing a single step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Step index that produced this result
    pub step_index: u32,

    /// Output data (available to subsequent steps as $prev)
    pub output: serde_json::Value,

    /// Execution duration in milliseconds
    pub duration_ms: u64,

    /// Whether step succeeded
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,
}

/// Execution context for plan executor
pub struct PlanExecutionContext {
    /// Plan being executed
    pub plan: TaskPlan,

    /// Results from completed steps
    pub step_results: Vec<StepResult>,

    /// Current step index
    pub current_step: u32,

    /// Rollback data for reversible steps
    pub rollback_data: Vec<(u32, serde_json::Value)>,
}
```

## schemars Integration for Tool Parameters

### Design Rationale

Instead of hand-writing JSON Schema for tool parameters:

```rust
// BEFORE: Hand-written, error-prone, verbose
UnifiedTool::new(...)
    .with_parameters_schema(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Search query" },
            "max_results": { "type": "integer", "default": 5 }
        },
        "required": ["query"]
    }))
```

Use schemars derive macro for type-safe, auto-documented schemas:

```rust
// AFTER: Type-safe, self-documenting, compiler-checked
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// The search query to execute
    pub query: String,
    /// Maximum number of results to return
    #[serde(default = "default_max_results")]
    pub max_results: Option<u32>,
}

fn default_max_results() -> u32 { 5 }

// Schema is auto-generated from the struct
UnifiedTool::new(...)
    .with_parameters_schema(schemars::schema_for!(SearchParams))
```

### ToolParams Trait

Marker trait for all tool parameter structs:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Marker trait for tool parameter types
///
/// All tool parameters must implement this trait to enable:
/// - Automatic JSON Schema generation via schemars
/// - Type-safe serialization/deserialization via serde
/// - Compile-time parameter validation
pub trait ToolParams: JsonSchema + Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Get the JSON Schema for this parameter type
    fn json_schema() -> serde_json::Value {
        let schema = schemars::schema_for!(Self);
        serde_json::to_value(schema).unwrap_or_default()
    }

    /// Validate parameters against schema (optional override)
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}

// Blanket implementation for common case
impl<T> ToolParams for T
where
    T: JsonSchema + Serialize + for<'de> Deserialize<'de> + Send + Sync
{}
```

### Built-in Tool Parameter Definitions

```rust
// tools/params/search.rs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// The search query string
    pub query: String,

    /// Maximum number of results (1-20)
    #[serde(default = "default_max_results")]
    #[schemars(range(min = 1, max = 20))]
    pub max_results: u32,

    /// Search provider to use (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

fn default_max_results() -> u32 { 5 }

// tools/params/translate.rs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranslateParams {
    /// Text content to translate (use "$prev" for previous step output)
    pub content: String,

    /// Target language code (ISO 639-1, e.g., "en", "zh", "ja")
    pub target_language: String,

    /// Source language (auto-detect if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_language: Option<String>,
}

// tools/params/summarize.rs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizeParams {
    /// Content to summarize (use "$prev" for previous step output)
    pub content: String,

    /// Summary style
    #[serde(default)]
    pub style: SummaryStyle,

    /// Maximum length in words (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_words: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStyle {
    #[default]
    Paragraph,
    BulletPoints,
    KeyTakeaways,
}
```

### ToolHandler Trait with Type-Safe Parameters

```rust
use async_trait::async_trait;

/// Handler for executing a tool with typed parameters
#[async_trait]
pub trait ToolHandler<P: ToolParams>: Send + Sync {
    /// Execute the tool with the given parameters
    async fn execute(&self, params: P) -> Result<ToolOutput>;

    /// Get the tool definition for LLM function calling
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: P::json_schema(),
        }
    }

    /// Tool name
    fn name(&self) -> &str;

    /// Tool description
    fn description(&self) -> &str;

    /// Safety level for confirmation decisions
    fn safety_level(&self) -> ToolSafetyLevel {
        ToolSafetyLevel::ReadOnly
    }
}

/// Tool output after execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Result data (passed to next step as $prev)
    pub result: serde_json::Value,

    /// Human-readable summary for UI display
    pub summary: String,

    /// Rollback data (for reversible operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_data: Option<serde_json::Value>,
}
```

### Tool Registration with Auto-Schema

```rust
impl ToolRegistry {
    /// Register a tool handler with auto-generated schema
    pub fn register_handler<P, H>(&mut self, handler: H)
    where
        P: ToolParams + 'static,
        H: ToolHandler<P> + 'static,
    {
        let definition = handler.definition();
        let tool = UnifiedTool::new(
            format!("native:{}", definition.name),
            &definition.name,
            &definition.description,
            ToolSource::Native,
        )
        .with_parameters_schema(definition.parameters)
        .with_safety_level(handler.safety_level());

        self.tools.insert(tool.name.clone(), tool);
        self.handlers.insert(definition.name, Box::new(handler));
    }
}
```

## Custom Agent Loop with Tool Calling

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Agent Loop (Custom)                       │
│                                                             │
│  ┌─────────┐    ┌──────────┐    ┌───────────┐    ┌───────┐ │
│  │ Build   │───▶│ LLM Call │───▶│ Parse     │───▶│ Route │ │
│  │ Messages│    │ (tools)  │    │ Response  │    │       │ │
│  └─────────┘    └──────────┘    └───────────┘    └───┬───┘ │
│       ▲                                              │      │
│       │                                              ▼      │
│       │         ┌──────────────────────────────────────┐   │
│       │         │           Decision Branch            │   │
│       │         └──────────┬───────────┬───────────────┘   │
│       │                    │           │                    │
│       │              tool_calls    content_only             │
│       │                    │           │                    │
│       │                    ▼           ▼                    │
│       │         ┌──────────────┐  ┌──────────┐             │
│       │         │Execute Tools │  │ Return   │             │
│       │         │& Collect     │  │ Response │             │
│       │         │Results       │  └──────────┘             │
│       │         └──────┬───────┘                           │
│       │                │                                    │
│       └────────────────┘ (loop back with tool results)     │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Agent Loop Implementation

```rust
pub struct AgentLoop {
    provider: Arc<dyn AiProvider>,
    tool_registry: Arc<ToolRegistry>,
    event_handler: Arc<dyn AetherEventHandler>,
    config: AgentConfig,
}

impl AgentLoop {
    /// Execute agent loop with tool calling support
    pub async fn run(&self, input: &str, context: Option<&str>) -> Result<AgentResult> {
        let mut history = ConversationHistory::new();
        let tools = self.build_tool_definitions();
        let mut turn_count = 0;

        // Add initial user message
        history.add_user_message(input);
        if let Some(ctx) = context {
            history.add_context(ctx);
        }

        loop {
            turn_count += 1;

            // Check max turns
            if turn_count > self.config.max_agent_turns {
                return Err(AetherError::MaxTurnsExceeded {
                    max: self.config.max_agent_turns,
                });
            }

            // Build messages for LLM
            let messages = history.to_chat_messages();

            // Call LLM with tools
            let response = self.provider
                .chat_with_tools(&messages, &tools)
                .await?;

            // Check for tool calls
            match response.tool_calls {
                Some(calls) if !calls.is_empty() => {
                    // Notify UI: tool execution starting
                    self.event_handler.on_agent_tools_called(
                        calls.iter().map(|c| c.function.name.clone()).collect()
                    ).await;

                    // Execute each tool call
                    for call in calls {
                        let result = self.execute_tool_call(&call).await;

                        // Add tool result to history
                        history.add_tool_result(
                            &call.id,
                            &call.function.name,
                            &result,
                        );

                        // Notify UI: tool completed
                        self.event_handler.on_agent_tool_completed(
                            &call.function.name,
                            result.is_ok(),
                        ).await;
                    }

                    // Continue loop - LLM will process tool results
                }
                _ => {
                    // No tool calls - this is the final response
                    let content = response.content.unwrap_or_default();

                    return Ok(AgentResult {
                        response: content,
                        tool_calls_made: turn_count - 1,
                        history,
                    });
                }
            }
        }
    }

    /// Execute a single tool call
    async fn execute_tool_call(&self, call: &ToolCall) -> Result<serde_json::Value> {
        let tool_name = &call.function.name;
        let arguments: serde_json::Value = serde_json::from_str(&call.function.arguments)?;

        // Get handler from registry
        let handler = self.tool_registry
            .get_handler(tool_name)
            .ok_or_else(|| AetherError::ToolNotFound(tool_name.clone()))?;

        // Execute with timeout
        let output = tokio::time::timeout(
            Duration::from_millis(self.config.tool_timeout_ms),
            handler.execute_raw(arguments),
        ).await??;

        Ok(output.result)
    }

    /// Build tool definitions for LLM
    fn build_tool_definitions(&self) -> Vec<serde_json::Value> {
        self.tool_registry
            .list_active_tools()
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters_schema
                    }
                })
            })
            .collect()
    }
}

/// Result from agent loop execution
pub struct AgentResult {
    /// Final response text
    pub response: String,

    /// Number of tool calls made during execution
    pub tool_calls_made: u32,

    /// Full conversation history (for debugging/logging)
    pub history: ConversationHistory,
}
```

### Conversation History Management

```rust
pub struct ConversationHistory {
    messages: Vec<ChatMessage>,
}

impl ConversationHistory {
    pub fn new() -> Self {
        Self { messages: Vec::new() }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: Role::User,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    pub fn add_assistant_message(&mut self, content: &str, tool_calls: Option<Vec<ToolCall>>) {
        self.messages.push(ChatMessage {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls,
            tool_call_id: None,
        });
    }

    pub fn add_tool_result(&mut self, call_id: &str, tool_name: &str, result: &Result<serde_json::Value>) {
        let content = match result {
            Ok(value) => serde_json::to_string(value).unwrap_or_default(),
            Err(e) => format!("Error: {}", e),
        };

        self.messages.push(ChatMessage {
            role: Role::Tool,
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
        });
    }

    pub fn turns(&self) -> usize {
        self.messages.iter().filter(|m| m.role == Role::Assistant).count()
    }
}
```

### AiProvider Extension for Tool Calling

```rust
// providers/mod.rs - extend AiProvider trait
#[async_trait]
pub trait AiProvider: Send + Sync {
    // ... existing methods ...

    /// Chat with tool calling support
    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> Result<ChatResponse>;
}

pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

pub struct ToolCall {
    pub id: String,
    pub function: FunctionCall,
}

pub struct FunctionCall {
    pub name: String,
    pub arguments: String, // JSON string
}
```

## Component Designs

### 1. Quick Heuristics Detector

Fast detection without LLM call:

```rust
pub struct QuickHeuristics;

impl QuickHeuristics {
    /// Check if input likely requires multi-step execution
    pub fn is_likely_multi_step(input: &str) -> bool {
        // Chinese action verbs
        const CN_ACTIONS: &[&str] = &[
            "翻译", "总结", "发送", "保存", "搜索",
            "分析", "生成", "创建", "删除", "移动",
            "复制", "格式化", "转换", "提取",
        ];

        // English action verbs
        const EN_ACTIONS: &[&str] = &[
            "translate", "summarize", "send", "save", "search",
            "analyze", "generate", "create", "delete", "move",
            "copy", "format", "convert", "extract",
        ];

        // Connector words indicating sequence
        const CN_CONNECTORS: &[&str] = &[
            "然后", "接着", "之后", "并且", "同时", "再",
        ];
        const EN_CONNECTORS: &[&str] = &[
            "then", "and then", "after that", "also", "next",
        ];

        let input_lower = input.to_lowercase();

        // Count action words
        let action_count = CN_ACTIONS.iter()
            .chain(EN_ACTIONS.iter())
            .filter(|w| input_lower.contains(*w))
            .count();

        // Check for connectors
        let has_connector = CN_CONNECTORS.iter()
            .chain(EN_CONNECTORS.iter())
            .any(|c| input_lower.contains(c));

        // Multi-step if: 2+ actions OR connector present with 1+ action
        action_count >= 2 || (has_connector && action_count >= 1)
    }
}
```

### 2. L3 Task Planner

Single LLM call for both analysis and planning:

```rust
pub struct L3TaskPlanner {
    provider: Arc<dyn AiProvider>,
    timeout: Duration,
}

impl L3TaskPlanner {
    /// Analyze and optionally plan multi-step execution
    pub async fn analyze_and_plan(
        &self,
        input: &str,
        tools: &[UnifiedTool],
        is_likely_multi_step: bool,
    ) -> Result<L3PlanningResult> {
        let prompt = if is_likely_multi_step {
            self.build_planning_prompt(input, tools)
        } else {
            self.build_routing_prompt(input, tools)
        };

        let response = tokio::time::timeout(
            self.timeout,
            self.provider.process(&prompt, None),
        ).await??;

        self.parse_response(&response, tools)
    }

    fn build_planning_prompt(&self, input: &str, tools: &[UnifiedTool]) -> String {
        format!(r#"
## Task
Analyze the user request and determine if it requires single-tool or multi-step execution.

## Available Tools
{tools_list}

## Rules
1. Use MINIMUM steps necessary - prefer single tool when possible
2. Each step must use an existing tool from the list
3. Use "$prev" to reference the previous step's output
4. Output pure JSON, no explanation

## User Request
{input}

## Output Format
Single tool:
{{"type": "single", "tool": "tool_name", "parameters": {{}}, "confidence": 0.9}}

Multi-step:
{{"type": "multi", "description": "Plan description", "confidence": 0.8, "steps": [
  {{"tool": "tool1", "parameters": {{}}, "description": "Step 1"}},
  {{"tool": "tool2", "parameters": {{"input": "$prev"}}, "description": "Step 2"}}
]}}
"#,
            tools_list = self.format_tools(tools),
            input = sanitize_for_prompt(input),
        )
    }
}

pub enum L3PlanningResult {
    SingleTool {
        tool: String,
        parameters: serde_json::Value,
        confidence: f32,
    },
    ExecutionPlan(TaskPlan),
    NeedsClarification {
        question: String,
    },
    FallbackToChat,
}
```

### 3. Plan Executor

Sequential execution with result passing:

```rust
pub struct PlanExecutor {
    tool_registry: Arc<ToolRegistry>,
    event_handler: Arc<dyn AetherEventHandler>,
}

impl PlanExecutor {
    /// Execute a task plan sequentially
    pub async fn execute(&self, plan: TaskPlan) -> Result<PlanExecutionResult> {
        let mut ctx = PlanExecutionContext::new(plan.clone());

        // Notify UI: plan started
        self.event_handler.on_plan_started(plan.to_info()).await;

        for step in &plan.steps {
            // Notify UI: step starting
            self.event_handler.on_plan_progress(PlanProgress {
                plan_id: plan.id.to_string(),
                current_step: step.index,
                total_steps: plan.steps.len() as u32,
                step_description: step.description.clone(),
                status: StepStatus::Running,
            }).await;

            // Resolve $prev references in parameters
            let resolved_params = self.resolve_params(&step.parameters, &ctx)?;

            // Execute step with timeout
            let result = tokio::time::timeout(
                Duration::from_millis(step.timeout_ms),
                self.execute_step(step, resolved_params),
            ).await;

            match result {
                Ok(Ok(step_result)) => {
                    ctx.step_results.push(step_result.clone());
                    ctx.current_step = step.index;

                    // Notify UI: step completed
                    self.event_handler.on_plan_progress(PlanProgress {
                        plan_id: plan.id.to_string(),
                        current_step: step.index,
                        total_steps: plan.steps.len() as u32,
                        step_description: step.description.clone(),
                        status: StepStatus::Completed,
                    }).await;
                }
                Ok(Err(e)) | Err(_) => {
                    // Step failed - attempt rollback if enabled
                    if !ctx.rollback_data.is_empty() {
                        self.attempt_rollback(&ctx).await;
                    }

                    self.event_handler.on_plan_failed(PlanError {
                        plan_id: plan.id.to_string(),
                        failed_step: step.index,
                        error: e.to_string(),
                    }).await;

                    return Err(e);
                }
            }
        }

        // Notify UI: plan completed
        let final_result = ctx.step_results.last()
            .map(|r| r.output.clone())
            .unwrap_or_default();

        self.event_handler.on_plan_completed(PlanResult {
            plan_id: plan.id.to_string(),
            final_output: serde_json::to_string(&final_result)?,
            total_steps: plan.steps.len() as u32,
        }).await;

        Ok(PlanExecutionResult {
            plan_id: plan.id,
            final_output: final_result,
            step_results: ctx.step_results,
        })
    }

    /// Resolve $prev references in parameters
    fn resolve_params(
        &self,
        params: &serde_json::Value,
        ctx: &PlanExecutionContext,
    ) -> Result<serde_json::Value> {
        let params_str = serde_json::to_string(params)?;

        // Replace $prev with previous step's output
        if params_str.contains("$prev") {
            let prev_output = ctx.step_results.last()
                .map(|r| serde_json::to_string(&r.output).unwrap_or_default())
                .unwrap_or_default();

            let resolved = params_str.replace("\"$prev\"", &prev_output);
            Ok(serde_json::from_str(&resolved)?)
        } else {
            Ok(params.clone())
        }
    }
}
```

### 4. UniFFI Event Handler Extensions

```idl
// aether.udl additions

callback interface AetherEventHandler {
    // ... existing methods ...

    /// Plan execution started
    void on_plan_started(PlanInfo plan);

    /// Plan step progress update
    void on_plan_progress(PlanProgress progress);

    /// Plan completed successfully
    void on_plan_completed(PlanResult result);

    /// Plan execution failed
    void on_plan_failed(PlanError error);
};

dictionary PlanInfo {
    string plan_id;
    string description;
    sequence<PlanStepInfo> steps;
    boolean has_irreversible_steps;
};

dictionary PlanStepInfo {
    u32 index;
    string tool_name;
    string description;
    string safety_level;
};

dictionary PlanProgress {
    string plan_id;
    u32 current_step;
    u32 total_steps;
    string step_description;
    string status;  // "pending" | "running" | "completed" | "failed"
};

dictionary PlanResult {
    string plan_id;
    string final_output;
    u32 total_steps;
};

dictionary PlanError {
    string plan_id;
    u32 failed_step;
    string error;
};
```

### 5. Configuration Schema

```toml
[dispatcher.agent]
# Enable agent planning mode
enabled = true

# Maximum steps in a single plan
max_plan_steps = 10

# Auto-execute threshold (plans with confidence >= this don't need confirmation)
auto_execute_threshold = 0.95

# Always confirm plans with irreversible steps
always_confirm_irreversible = true

# Per-step timeout (milliseconds)
step_timeout_ms = 30000

# Overall plan timeout (milliseconds)
plan_timeout_ms = 300000

# Enable quick heuristics pre-check
enable_heuristics = true
```

## Integration Points

### L3Router Integration

The existing `L3Router` will be extended:

```rust
impl L3Router {
    pub async fn route(&self, input: &str, tools: &[UnifiedTool]) -> Result<L3Result> {
        // Quick heuristic check
        let is_likely_multi_step = if self.config.enable_heuristics {
            QuickHeuristics::is_likely_multi_step(input)
        } else {
            false
        };

        // Analyze and optionally plan
        let result = self.planner.analyze_and_plan(input, tools, is_likely_multi_step).await?;

        match result {
            L3PlanningResult::SingleTool { tool, parameters, confidence } => {
                // Existing single-tool flow
                Ok(L3Result::SingleTool { tool, parameters, confidence })
            }
            L3PlanningResult::ExecutionPlan(plan) => {
                // New multi-step flow
                Ok(L3Result::ExecutionPlan { plan })
            }
            L3PlanningResult::NeedsClarification { question } => {
                Ok(L3Result::NeedsClarification { question, options: vec![] })
            }
            L3PlanningResult::FallbackToChat => {
                Ok(L3Result::FallbackToChat)
            }
        }
    }
}
```

### Swift UI Integration

Plan confirmation view in SwiftUI:

```swift
struct PlanConfirmationView: View {
    let plan: PlanInfo
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Header
            HStack {
                Image(systemName: "list.clipboard")
                Text("执行计划")
                    .font(.headline)
                Spacer()
                Text("\(plan.steps.count) 步骤")
                    .foregroundStyle(.secondary)
            }

            // Steps
            ForEach(plan.steps, id: \.index) { step in
                PlanStepRow(step: step)
            }

            // Warning for irreversible
            if plan.hasIrreversibleSteps {
                HStack {
                    Image(systemName: "exclamationmark.triangle")
                    Text("包含不可撤销操作")
                }
                .foregroundStyle(.orange)
                .font(.caption)
            }

            // Actions
            HStack {
                Button("取消", role: .cancel) { onCancel() }
                Spacer()
                Button("执行") { onConfirm() }
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding()
        .background(.ultraThinMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}
```

## Error Handling

### Plan Generation Errors

| Error Type | Handling |
|------------|----------|
| LLM timeout | Fall back to single-tool routing |
| Invalid JSON | Fall back to single-tool routing |
| Unknown tool in plan | Remove step, warn user |
| Empty plan | Fall back to GeneralChat |

### Plan Execution Errors

| Error Type | Handling |
|------------|----------|
| Step timeout | Stop execution, attempt rollback |
| Tool not found | Stop execution, report error |
| Parameter resolution failed | Stop execution, report error |
| Tool execution failed | Stop execution, attempt rollback |

## Performance Considerations

1. **Quick heuristics**: <10ms overhead for simple inputs
2. **Planning LLM call**: 500ms-2s (acceptable for complex tasks)
3. **Step execution**: Depends on individual tool performance
4. **$prev resolution**: O(1) string replacement

## Security Considerations

1. **Prompt injection**: Sanitize user input before including in planning prompt
2. **Tool validation**: Verify all tool names in plan exist in registry
3. **Parameter validation**: Validate resolved parameters against tool schema
4. **Confirmation for high-risk**: Always require confirmation for irreversible operations

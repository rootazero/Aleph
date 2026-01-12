# Spec: L3 Agent Planning

## Overview

This capability extends the L3 Router to support multi-step task planning and execution, enabling users to issue complex requests that require sequential tool invocations.

## ADDED Requirements

### Requirement: Quick Heuristics Detection

The system MUST provide fast heuristic detection (<10ms) to identify inputs that likely require multi-step execution before invoking the LLM.

#### Scenario: Chinese multi-action detection

**Given** user input "把这个文档翻译成英文，然后总结要点"
**When** the quick heuristics detector processes the input
**Then** it should return `true` for `is_likely_multi_step`
**And** processing time should be less than 10ms

#### Scenario: English multi-action detection

**Given** user input "Search for the latest news about AI, then summarize the top 3 articles"
**When** the quick heuristics detector processes the input
**Then** it should return `true` for `is_likely_multi_step`

#### Scenario: Simple single-tool detection

**Given** user input "翻译这段文字"
**When** the quick heuristics detector processes the input
**Then** it should return `false` for `is_likely_multi_step`

---

### Requirement: Task Plan Generation

The L3 Router MUST generate structured execution plans for multi-step tasks when heuristics indicate complex input.

#### Scenario: Generate valid execution plan

**Given** user input "translate this to English then format as markdown"
**And** available tools include "translate" and "format_markdown"
**When** the L3 planner processes the input
**Then** it should return a `TaskPlan` with 2 steps
**And** step 1 should use tool "translate"
**And** step 2 should use tool "format_markdown" with "$prev" parameter reference

#### Scenario: Fall back to single tool when appropriate

**Given** user input "search for weather in Tokyo"
**And** available tools include "search"
**When** the L3 planner processes the input
**Then** it should return a `SingleTool` result (not ExecutionPlan)
**And** the tool should be "search"

#### Scenario: Handle unknown tools gracefully

**Given** user input "use magic_tool then analyze"
**And** "magic_tool" does not exist in the registry
**When** the L3 planner generates a plan
**Then** it should either skip the unknown tool or fall back to GeneralChat

---

### Requirement: Sequential Plan Execution

The Plan Executor MUST execute plan steps sequentially, passing results between steps via the `$prev` reference.

#### Scenario: Execute two-step plan successfully

**Given** a `TaskPlan` with steps:
  1. Tool: "search", params: {"query": "AI news"}
  2. Tool: "summarize", params: {"content": "$prev"}
**When** the executor runs the plan
**Then** step 1 should execute first and produce a result
**And** step 2 should receive step 1's output as the "content" parameter
**And** the final result should be step 2's output

#### Scenario: Stop execution on step failure

**Given** a `TaskPlan` with 3 steps
**And** step 2 fails with an error
**When** the executor runs the plan
**Then** steps 1 and 2 should execute
**And** step 3 should NOT execute
**And** the executor should return an error result

#### Scenario: Respect step timeout

**Given** a `TaskPlan` with step timeout of 5000ms
**And** a step takes longer than 5000ms to execute
**When** the executor runs the plan
**Then** the step should be terminated after 5000ms
**And** the plan should fail with a timeout error

---

### Requirement: Plan Confirmation UI

The system MUST display a confirmation UI before executing plans that contain irreversible operations or have confidence below the auto-execute threshold.

#### Scenario: Show confirmation for irreversible steps

**Given** a `TaskPlan` containing a step with `ToolSafetyLevel::IrreversibleHighRisk`
**When** the plan is ready for execution
**Then** the confirmation UI should appear
**And** it should display a warning about irreversible operations
**And** execution should NOT start until user confirms

#### Scenario: Auto-execute high-confidence safe plans

**Given** a `TaskPlan` with confidence >= 0.95
**And** all steps have `ToolSafetyLevel::ReadOnly` or `Reversible`
**When** the plan is ready for execution
**Then** the plan should execute automatically without confirmation UI

#### Scenario: Show step-by-step progress

**Given** a `TaskPlan` being executed
**When** each step completes
**Then** the progress UI should update to show:
  - Current step number and total
  - Step description
  - Status (pending/running/completed/failed)

---

### Requirement: Tool Safety Classification

Each tool MUST have a safety level classification that determines confirmation requirements and rollback behavior.

#### Scenario: Default safety levels by tool type

**Given** a tool of type "search" (read-only operation)
**When** the safety level is queried
**Then** it should return `ToolSafetyLevel::ReadOnly`

**Given** a tool of type "create_file" (can be undone by deleting)
**When** the safety level is queried
**Then** it should return `ToolSafetyLevel::Reversible`

**Given** a tool of type "send_email" (cannot undo sending)
**When** the safety level is queried
**Then** it should return `ToolSafetyLevel::IrreversibleLowRisk`

**Given** a tool of type "delete_file" (destructive, hard to recover)
**When** the safety level is queried
**Then** it should return `ToolSafetyLevel::IrreversibleHighRisk`

---

### Requirement: Plan Event Notifications

The system MUST notify the UI of plan lifecycle events through the AetherEventHandler interface.

#### Scenario: Notify on plan start

**Given** a confirmed `TaskPlan` starting execution
**When** the executor begins
**Then** `on_plan_started(PlanInfo)` should be called
**And** `PlanInfo` should contain plan ID, description, and step list

#### Scenario: Notify on step progress

**Given** a running `TaskPlan` with step completing
**When** a step finishes (success or failure)
**Then** `on_plan_progress(PlanProgress)` should be called
**And** `PlanProgress` should contain current step, total steps, and status

#### Scenario: Notify on plan completion

**Given** a `TaskPlan` with all steps completed successfully
**When** the executor finishes
**Then** `on_plan_completed(PlanResult)` should be called
**And** `PlanResult` should contain the final output

#### Scenario: Notify on plan failure

**Given** a `TaskPlan` with a failed step
**When** the executor stops due to failure
**Then** `on_plan_failed(PlanError)` should be called
**And** `PlanError` should contain the failed step index and error message

---

### Requirement: schemars-Based Tool Parameter Definitions

Tool parameters MUST be defined using schemars derive macros for automatic JSON Schema generation, type safety, and self-documentation.

#### Scenario: Auto-generate JSON Schema from Rust struct

**Given** a tool parameter struct with `#[derive(JsonSchema)]`
**And** fields with `/// doc comments`
**When** `schemars::schema_for!()` is called
**Then** a valid JSON Schema should be generated
**And** field descriptions should come from doc comments
**And** required/optional fields should be correctly marked

#### Scenario: SearchParams generates correct schema

**Given** the `SearchParams` struct:
```rust
#[derive(JsonSchema)]
pub struct SearchParams {
    /// The search query string
    pub query: String,
    /// Maximum results (1-20)
    #[serde(default)]
    pub max_results: Option<u32>,
}
```
**When** the schema is generated
**Then** it should include `query` as required
**And** `max_results` should be optional with default
**And** descriptions should match doc comments

#### Scenario: ToolParams trait provides schema generation

**Given** a type implementing `ToolParams`
**When** `ToolParams::json_schema()` is called
**Then** it should return a valid `serde_json::Value` containing the JSON Schema
**And** the schema should be suitable for LLM function calling

---

### Requirement: ToolHandler Trait with Type-Safe Execution

Tools MUST implement the `ToolHandler<P>` trait for type-safe parameter handling and execution.

#### Scenario: ToolHandler provides definition

**Given** a handler implementing `ToolHandler<SearchParams>`
**When** `handler.definition()` is called
**Then** it should return a `ToolDefinition` with name, description, and auto-generated schema

#### Scenario: ToolHandler executes with typed params

**Given** a handler implementing `ToolHandler<TranslateParams>`
**And** valid JSON parameters `{"content": "hello", "target_language": "zh"}`
**When** the executor deserializes and calls `handler.execute(params)`
**Then** `params` should be a strongly-typed `TranslateParams` instance
**And** execution should proceed with compile-time type safety

#### Scenario: Invalid parameters rejected

**Given** a handler expecting `SearchParams`
**And** invalid JSON parameters `{"wrong_field": 123}`
**When** deserialization is attempted
**Then** it should fail with a descriptive error
**And** the tool should NOT execute

---

### Requirement: Custom Agent Loop with Tool Calling

The system MUST support a custom agent loop that processes tool calls iteratively until the LLM produces a final response.

#### Scenario: Single-turn tool execution

**Given** user input that triggers one tool call
**When** the agent loop processes the input
**Then** it should send request to LLM with tool definitions
**And** receive and execute the tool call
**And** return the LLM's final response after processing

#### Scenario: Multi-turn tool execution

**Given** user input that triggers multiple sequential tool calls
**When** the agent loop runs
**Then** it should execute tool calls iteratively
**And** feed tool results back to LLM as assistant messages
**And** continue until LLM produces content without tool_calls

#### Scenario: Max turns guard prevents infinite loop

**Given** an agent loop with `max_agent_turns = 5`
**And** LLM keeps requesting tool calls indefinitely
**When** the loop reaches turn 5
**Then** it should terminate with `MaxTurnsExceeded` error
**And** should NOT continue executing

#### Scenario: Conversation history tracks tool results

**Given** an agent loop executing tools
**When** a tool completes (success or failure)
**Then** the result should be added to conversation history
**And** the history should include the tool_call_id for proper message threading

---

### Requirement: AiProvider Tool Calling Extension

The AiProvider trait MUST support chat with tool definitions and tool call responses.

#### Scenario: chat_with_tools sends tool definitions

**Given** an AiProvider implementation (OpenAI, Anthropic)
**And** a list of tool definitions
**When** `chat_with_tools()` is called
**Then** the request should include tools in provider-appropriate format
**And** the provider should return `ChatResponse` with optional tool_calls

#### Scenario: Parse tool calls from response

**Given** an LLM response with tool_calls
**When** the provider parses the response
**Then** each `ToolCall` should have `id`, `function.name`, and `function.arguments`
**And** arguments should be a JSON string

---

### Requirement: Agent Loop Event Notifications

The agent loop MUST notify the UI of tool execution events.

#### Scenario: Notify when tools are called

**Given** LLM response contains tool_calls
**When** the agent loop processes them
**Then** `on_agent_tools_called(tool_names)` should be called
**And** `tool_names` should list all tools being executed

#### Scenario: Notify when tool completes

**Given** a tool execution finishes
**When** the result is available
**Then** `on_agent_tool_completed(tool_name, success)` should be called
**And** `success` should indicate execution outcome

---

## MODIFIED Requirements

### Requirement: IntentAction Enum Extension

The existing `IntentAction` enum MUST be extended with an `ExecutePlan` variant.

#### Scenario: IntentAction includes ExecutePlan

**Given** the `IntentAction` enum
**Then** it should include variant `ExecutePlan { plan: TaskPlan }`
**And** serialization should be compatible with UniFFI

---

### Requirement: L3Router Integration

The L3Router MUST integrate the task planner while maintaining backward compatibility.

#### Scenario: Single-tool routing unchanged

**Given** input that clearly maps to a single tool
**When** L3Router processes the input
**Then** behavior should be identical to pre-enhancement implementation
**And** no planning overhead should be introduced

#### Scenario: Multi-step routing triggers planner

**Given** input that heuristics identify as likely multi-step
**When** L3Router processes the input
**Then** the task planner should be invoked
**And** result should be either `ExecutionPlan` or fallback to single-tool

---

## Configuration Requirements

### Requirement: Agent Configuration Section

The configuration MUST support an `[dispatcher.agent]` section for controlling planning behavior.

#### Scenario: Configuration defaults

**Given** no explicit `[dispatcher.agent]` configuration
**When** the agent subsystem initializes
**Then** default values should apply:
  - `enabled`: true
  - `max_plan_steps`: 10
  - `auto_execute_threshold`: 0.95
  - `always_confirm_irreversible`: true
  - `step_timeout_ms`: 30000
  - `plan_timeout_ms`: 300000
  - `enable_heuristics`: true

#### Scenario: Disable agent planning

**Given** configuration with `[dispatcher.agent] enabled = false`
**When** multi-step input is received
**Then** planning should be skipped
**And** input should route through standard single-tool L3 flow

---

## Cross-References

- **ai-routing**: Extended by this capability for multi-step support
- **event-handler**: Extended with plan lifecycle callbacks and agent tool events
- **uniffi-bridge**: Extended with new callback types
- **tool-registry**: Extended with schemars-based tool registration
- **ai-provider**: Extended with chat_with_tools() method for tool calling

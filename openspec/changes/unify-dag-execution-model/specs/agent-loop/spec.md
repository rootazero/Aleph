# Agent Loop Specification

This specification defines the Agent Loop execution model for Aleph.

## ADDED Requirements

### Requirement: Multi-Tool Decision Support

The Agent Loop SHALL support `Decision::UseTools` variant that enables the LLM to request execution of multiple tools in a single decision.

The UseTools decision MUST include:
- A list of tool calls (each with tool_name and arguments)
- A parallel flag indicating parallel (true) or sequential (false) execution

The system SHALL enforce a maximum of 25 tools per UseTools decision.

#### Scenario: Parallel tool execution
- **WHEN** LLM returns UseTools decision with parallel=true
- **THEN** Agent Loop executes all tools concurrently using tokio::join_all
- **AND** returns MultiToolResults containing all individual tool results

#### Scenario: Sequential tool execution
- **WHEN** LLM returns UseTools decision with parallel=false
- **THEN** Agent Loop executes tools in order
- **AND** each subsequent tool receives prior results in context
- **AND** returns MultiToolResults containing all individual tool results

#### Scenario: Tool limit enforcement
- **WHEN** UseTools contains more than 25 tools
- **THEN** decision validation fails
- **AND** error is returned to LLM

### Requirement: Graph Execution Decision Support

The Agent Loop SHALL support `Decision::ExecuteGraph` variant that enables the LLM to request execution of a task graph with dependencies.

The ExecuteGraph decision MUST include:
- A TaskGraphSpec containing tasks and their dependencies

The system SHALL delegate graph execution to the existing DagScheduler.

#### Scenario: Graph execution delegation
- **WHEN** LLM returns ExecuteGraph decision
- **THEN** Agent Loop creates TaskGraph from TaskGraphSpec
- **AND** calls DagScheduler::execute_graph()
- **AND** returns GraphResult containing completed/failed tasks and outputs

#### Scenario: Graph validation
- **WHEN** TaskGraphSpec contains circular dependencies
- **THEN** validation fails with error
- **AND** error is returned to LLM

### Requirement: batch_execute Tool

The system SHALL provide a `batch_execute` tool that allows the LLM to request batch execution of multiple tools through the standard tool calling interface.

The batch_execute tool MUST accept:
- tools: Array of tool calls (max 25)
- parallel: Boolean flag (default true)

#### Scenario: Batch tool invocation
- **WHEN** LLM calls batch_execute tool with tools array
- **THEN** system internally creates UseTools decision
- **AND** executes tools according to parallel flag
- **AND** returns aggregated results to LLM

#### Scenario: Empty tools array
- **WHEN** batch_execute is called with empty tools array
- **THEN** tool returns error indicating no tools provided

### Requirement: Multi-Tool Action Results

The ActionResult enum SHALL support results from multi-tool and graph executions.

MultiToolResults MUST include:
- Individual results for each tool (success, output, or error)
- Total execution duration

GraphResult MUST include:
- List of completed task IDs
- List of failed task IDs
- Outputs keyed by task ID
- Total execution duration

#### Scenario: All tools succeed
- **WHEN** all tools in UseTools complete successfully
- **THEN** MultiToolResults contains all ToolSuccess results
- **AND** ActionResult is considered successful

#### Scenario: Partial tool failure
- **WHEN** some tools in UseTools fail
- **THEN** MultiToolResults contains mix of success and error results
- **AND** execution continues for all tools (no short-circuit)
- **AND** LLM receives full results to handle failures

### Requirement: Backward Compatible Single Tool Execution

The Agent Loop SHALL maintain full backward compatibility with existing single-tool Decision::UseTool variant.

No changes SHALL be made to the existing UseTool execution path.

#### Scenario: Single tool execution unchanged
- **WHEN** LLM returns Decision::UseTool (existing format)
- **THEN** Agent Loop executes single tool as before
- **AND** returns ToolSuccess or ToolError result

### Requirement: Decision Parser Extension

The Thinker's decision parser SHALL support parsing multi-tool and graph execution decisions from LLM responses.

The parser MUST support these response formats:
```json
{
  "action": {
    "type": "tools",
    "tools": [{"tool_name": "...", "arguments": {...}}, ...],
    "parallel": true
  }
}
```

```json
{
  "action": {
    "type": "execute_graph",
    "graph": {
      "tasks": [{"id": "...", "tool_name": "...", "arguments": {...}}, ...],
      "dependencies": [["task1", "task2"], ...]
    }
  }
}
```

#### Scenario: Parse multi-tool decision
- **WHEN** LLM returns action with type="tools"
- **THEN** parser creates Decision::UseTools with parsed tools and parallel flag

#### Scenario: Parse graph decision
- **WHEN** LLM returns action with type="execute_graph"
- **THEN** parser creates Decision::ExecuteGraph with parsed TaskGraphSpec

#### Scenario: Fallback to single tool
- **WHEN** LLM returns action with type="tool" (existing format)
- **THEN** parser creates Decision::UseTool as before

### Requirement: Guard Integration for Multi-Tool

The LoopGuard SHALL track multi-tool executions for doom loop detection and step counting.

Each tool in a UseTools decision SHALL be counted individually for step limits.

#### Scenario: Step counting for batch
- **WHEN** UseTools executes 5 tools
- **THEN** step count increments by 5

#### Scenario: Doom loop detection for batch
- **WHEN** same UseTools (identical tools and arguments) is repeated 3 times
- **THEN** doom loop detection triggers
- **AND** user is prompted to continue or abort

### Requirement: Compaction Event Integration

The Agent Loop SHALL emit appropriate compaction events for multi-tool and graph executions.

ToolCallCompleted events SHALL be emitted for each individual tool execution.

#### Scenario: Events for parallel execution
- **WHEN** UseTools executes 3 tools in parallel
- **THEN** 3 ToolCallCompleted events are emitted
- **AND** SessionCompactor can track token usage for each

### Requirement: Deprecate FFI DAG Execution

The `ffi/dag_executor.rs::run_dag_execution` function SHALL be deprecated in favor of Agent Loop graph execution.

Existing FFI callers SHALL be migrated to use Agent Loop with ExecuteGraph decision.

#### Scenario: Deprecation warning
- **WHEN** run_dag_execution is called
- **THEN** deprecation warning is logged
- **AND** function continues to work during transition period

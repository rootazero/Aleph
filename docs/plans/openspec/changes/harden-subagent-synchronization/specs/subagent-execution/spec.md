# Capability: Sub-Agent Execution

The Sub-Agent Execution system provides synchronous result collection, progress tracking, and unified aggregation for multi-agent task orchestration.

## ADDED Requirements

### Requirement: Synchronous Result Wait

The ExecutionCoordinator SHALL provide synchronous wait capability for sub-agent results.

#### Scenario: Single sub-agent wait
- **GIVEN** a parent agent dispatches a sub-agent request
- **WHEN** the parent calls `wait_for_result(request_id, timeout)`
- **THEN** the call SHALL block until the sub-agent completes
- **AND** the returned result SHALL contain the full `SubAgentResult`
- **AND** the result SHALL include aggregated `tools_called` summary

#### Scenario: Wait with timeout
- **GIVEN** a sub-agent execution in progress
- **WHEN** the timeout duration elapses before completion
- **THEN** the wait SHALL return `ExecutionError::Timeout`
- **AND** the error SHALL include any partial tool summary available
- **AND** the sub-agent execution SHALL continue in background

#### Scenario: Wait for completed execution
- **GIVEN** a sub-agent that has already completed
- **WHEN** the parent calls `wait_for_result(request_id, _)`
- **THEN** the result SHALL be returned immediately from cache
- **AND** no blocking SHALL occur

### Requirement: Parallel Execution with Ordering

The SubAgentDispatcher SHALL support parallel dispatch with result-request correlation.

#### Scenario: Dispatch multiple requests in parallel
- **GIVEN** three sub-agent requests with IDs ["req-1", "req-2", "req-3"]
- **WHEN** the parent calls `dispatch_parallel_sync(requests, timeout)`
- **THEN** all three sub-agents SHALL execute concurrently
- **AND** the method SHALL block until all complete or timeout

#### Scenario: Result-request correlation
- **GIVEN** parallel execution completes with results in order [B, A, C]
- **WHEN** results are returned to the parent
- **THEN** each result SHALL be paired with its original request_id
- **AND** the return type SHALL be `Vec<(String, Result<SubAgentResult>)>`
- **AND** the parent can correlate by request_id regardless of completion order

#### Scenario: Partial failure in parallel
- **GIVEN** three parallel requests where request "req-2" fails
- **WHEN** all executions complete
- **THEN** results SHALL contain:
  - `("req-1", Ok(result1))`
  - `("req-2", Err(ExecutionError::ExecutionFailed{...}))`
  - `("req-3", Ok(result3))`
- **AND** successful results SHALL NOT be affected by the failure

### Requirement: Tool Call Aggregation

The ResultCollector SHALL aggregate all tool calls made during sub-agent execution.

#### Scenario: Collect tool call start
- **GIVEN** a sub-agent begins executing tool "bash"
- **WHEN** `ToolCallStarted` event is emitted
- **THEN** ResultCollector SHALL record the tool call
- **AND** status SHALL be `Running`
- **AND** started_at timestamp SHALL be captured

#### Scenario: Collect tool call completion
- **GIVEN** a running tool call for "bash"
- **WHEN** `ToolCallCompleted` event is emitted with output
- **THEN** ResultCollector SHALL update the record
- **AND** status SHALL be `Completed`
- **AND** output_preview SHALL contain first 200 chars of output

#### Scenario: Collect tool call failure
- **GIVEN** a running tool call for "bash"
- **WHEN** `ToolCallFailed` event is emitted with error
- **THEN** ResultCollector SHALL update the record
- **AND** status SHALL be `Failed`
- **AND** error message SHALL be captured

#### Scenario: Get execution summary
- **GIVEN** a sub-agent executed 3 tools: [glob (success), read (success), edit (failed)]
- **WHEN** `get_summary(request_id)` is called
- **THEN** the summary SHALL contain all 3 tool calls
- **AND** each entry SHALL have: id, tool name, status, optional title
- **AND** entries SHALL be ordered by execution start time

### Requirement: Artifact Collection

The ResultCollector SHALL aggregate artifacts produced during sub-agent execution.

#### Scenario: Collect file artifact
- **GIVEN** a sub-agent creates a file at "/tmp/output.txt"
- **WHEN** the artifact is recorded
- **THEN** ResultCollector SHALL store the artifact
- **AND** artifact_type SHALL be "file"
- **AND** path SHALL be "/tmp/output.txt"

#### Scenario: Collect multiple artifacts
- **GIVEN** a sub-agent produces 3 files and 2 URLs
- **WHEN** execution completes
- **THEN** `get_artifacts(request_id)` SHALL return all 5 artifacts
- **AND** artifacts SHALL be grouped by type

### Requirement: Context Propagation

The SubAgentRequest SHALL propagate parent execution context to child agents.

#### Scenario: Propagate working directory
- **GIVEN** parent execution has working_directory = "/Users/foo/project"
- **WHEN** a sub-agent request is created via `from_parent_context()`
- **THEN** request.execution_context.working_directory SHALL be "/Users/foo/project"

#### Scenario: Propagate history summary
- **GIVEN** parent has completed 5 tool calls
- **WHEN** a sub-agent request is created
- **THEN** request.execution_context.history_summary SHALL describe previous work
- **AND** recent_steps SHALL contain last 3 step summaries

#### Scenario: Propagate original request
- **GIVEN** user's original prompt was "Analyze and fix the bug"
- **WHEN** a sub-agent is spawned
- **THEN** request.execution_context.original_request SHALL be "Analyze and fix the bug"
- **AND** sub-agent can understand its role in the larger task

### Requirement: Progress Events

The SubAgentHandler SHALL emit progress events for real-time UI updates.

#### Scenario: Subscribe to tool progress
- **GIVEN** a UI component subscribes to `SubAgentProgress` events
- **WHEN** a sub-agent starts executing a tool
- **THEN** an event SHALL be emitted with:
  - request_id
  - tool_name
  - status: "started"
  - timestamp

#### Scenario: Progress event on completion
- **GIVEN** a tool completes within sub-agent execution
- **WHEN** completion is recorded
- **THEN** a progress event SHALL be emitted
- **AND** status SHALL be "completed" or "failed"
- **AND** title SHALL be included if available

### Requirement: Result Lifecycle Management

The ExecutionCoordinator SHALL manage result lifecycle with TTL-based cleanup.

#### Scenario: Result caching
- **GIVEN** a sub-agent completes successfully
- **WHEN** the result is stored
- **THEN** it SHALL remain available for `result_ttl_ms` (default 1 hour)
- **AND** subsequent `wait_for_result()` calls SHALL return immediately

#### Scenario: Result cleanup
- **GIVEN** a completed result older than `result_ttl_ms`
- **WHEN** the cleanup task runs
- **THEN** the result SHALL be removed from cache
- **AND** subsequent `wait_for_result()` calls SHALL return `NotFound`

#### Scenario: Pending execution cleanup
- **GIVEN** a pending execution older than `execution_timeout_ms` without completion
- **WHEN** the cleanup task runs
- **THEN** the pending execution SHALL be marked as timed out
- **AND** any waiting callers SHALL receive `ExecutionError::Timeout`

### Requirement: Concurrency Limits

The ExecutionCoordinator SHALL enforce limits on concurrent sub-agent executions.

#### Scenario: Under limit
- **GIVEN** `max_concurrent = 5` and 3 executions in progress
- **WHEN** a new execution is started
- **THEN** it SHALL proceed immediately

#### Scenario: At limit
- **GIVEN** `max_concurrent = 5` and 5 executions in progress
- **WHEN** a new execution is requested
- **THEN** it SHALL be queued
- **AND** it SHALL start when a slot becomes available

#### Scenario: Queue timeout
- **GIVEN** an execution queued waiting for a slot
- **WHEN** the timeout elapses before a slot is available
- **THEN** the execution SHALL fail with `ExecutionError::QueueTimeout`

## MODIFIED Requirements

### Requirement: SubAgentResult Enhancement

The SubAgentResult (from `agents/sub_agents/traits.rs`) SHALL be enhanced with mandatory tool summary.

#### Scenario: Result includes tool summary
- **GIVEN** a sub-agent execution completes
- **WHEN** the result is returned
- **THEN** `tools_called` SHALL contain ToolCallSummary for each tool executed
- **AND** each summary SHALL have: id, tool name, status, optional title

#### Scenario: Result includes execution metadata
- **GIVEN** a sub-agent execution completes
- **WHEN** the result is returned
- **THEN** it SHALL include:
  - `execution_duration_ms`: Total execution time
  - `waiting_duration_ms`: Time spent waiting (if queued)

### Requirement: SubAgentHandler Event Subscriptions

The SubAgentHandler SHALL subscribe to additional event types.

#### Scenario: Tool call event subscription
- **GIVEN** SubAgentHandler is initialized
- **WHEN** `subscriptions()` is called
- **THEN** it SHALL include:
  - `EventType::SubAgentStarted`
  - `EventType::SubAgentCompleted`
  - `EventType::ToolCallStarted` (NEW)
  - `EventType::ToolCallCompleted` (NEW)
  - `EventType::ToolCallFailed` (NEW)

### Requirement: Dispatcher Sync Methods

The SubAgentDispatcher SHALL provide synchronous dispatch alternatives.

#### Scenario: dispatch_sync method
- **GIVEN** a parent agent needs to wait for sub-agent result
- **WHEN** `dispatch_sync(request, timeout)` is called
- **THEN** the method SHALL return `Result<SubAgentResult>`
- **AND** it SHALL block until completion or timeout

#### Scenario: dispatch_parallel_sync method
- **GIVEN** a parent agent needs to dispatch multiple sub-agents
- **WHEN** `dispatch_parallel_sync(requests, timeout)` is called
- **THEN** it SHALL return `Vec<(String, Result<SubAgentResult>)>`
- **AND** each tuple SHALL correlate request_id with its result

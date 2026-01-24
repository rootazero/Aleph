# Tasks: Harden Sub-Agent Synchronization

## Phase 1: Core Infrastructure (Priority: Critical) ✅ COMPLETED

### 1.1 ExecutionCoordinator Implementation

- [x] **Task 1.1.1**: Create `core/src/agents/sub_agents/coordinator.rs`
  - Define `ExecutionCoordinator` struct with `pending` and `completed` HashMaps
  - Implement `PendingExecution` with oneshot channel for signaling
  - Implement `CompletedExecution` with TTL tracking
  - **Validation**: Unit test for struct creation and basic operations ✅

- [x] **Task 1.1.2**: Implement `start_execution()` method
  - Create `PendingExecution` entry with oneshot channel
  - Return `ExecutionHandle` for tracking
  - **Validation**: Test that pending execution is tracked correctly ✅

- [x] **Task 1.1.3**: Implement `wait_for_result()` method
  - Use oneshot receiver to wait for completion
  - Implement timeout using `tokio::time::timeout`
  - Return partial results on timeout if available
  - **Validation**: Test synchronous wait, test timeout behavior ✅

- [x] **Task 1.1.4**: Implement `wait_for_all()` method
  - Collect multiple oneshot receivers
  - Wait for all with shared timeout
  - Return results paired with request_ids
  - **Validation**: Test parallel wait with 3+ requests, test partial failure ✅

- [x] **Task 1.1.5**: Implement `on_execution_completed()` callback
  - Find pending execution by request_id
  - Send result via oneshot channel
  - Move to completed cache
  - **Validation**: Test completion signaling, test double-completion handling ✅

- [x] **Task 1.1.6**: Implement TTL cleanup task
  - Background task to clean expired completed results
  - Clean timed-out pending executions
  - **Validation**: Test cleanup after TTL expiry ✅

### 1.2 ResultCollector Implementation

- [x] **Task 1.2.1**: Create `core/src/agents/sub_agents/result_collector.rs`
  - Define `ResultCollector` struct with tool_records and artifacts maps
  - Define `ToolCallRecord` struct with status enum
  - Define `ToolCallSummary` for OpenCode-compatible output
  - **Validation**: Unit tests for struct creation ✅

- [x] **Task 1.2.2**: Implement `record_tool_start()`
  - Add new ToolCallRecord with status=Running
  - Capture started_at timestamp
  - **Validation**: Test tool recording ✅

- [x] **Task 1.2.3**: Implement `update_tool_status()`
  - Update existing record by call_id
  - Handle status transitions (Running→Completed, Running→Failed)
  - Capture output_preview (first 200 chars)
  - **Validation**: Test status updates, test output preview truncation ✅

- [x] **Task 1.2.4**: Implement `record_artifact()` and `get_artifacts()`
  - Store artifacts indexed by request_id
  - Group by artifact_type on retrieval
  - **Validation**: Test artifact storage and retrieval ✅

- [x] **Task 1.2.5**: Implement `get_summary()` method
  - Return Vec<ToolCallSummary> sorted by started_at
  - Format matching OpenCode's summary structure
  - **Validation**: Test summary generation with multiple tools ✅

- [x] **Task 1.2.6**: Implement `cleanup()` and `init_request()`
  - Initialize empty collections for new request
  - Clean up collections for completed request
  - **Validation**: Test cleanup removes all data for request ✅

### 1.3 Module Integration

- [x] **Task 1.3.1**: Update `core/src/agents/sub_agents/mod.rs`
  - Export `ExecutionCoordinator` and related types
  - Export `ResultCollector` and related types
  - **Validation**: Compile check ✅

- [x] **Task 1.3.2**: Add configuration types
  - Create `CoordinatorConfig` with timeout and TTL settings
  - Add to main config module
  - **Validation**: Config parsing test ✅

---

## Phase 2: Event Integration (Priority: High) ✅ COMPLETED

### 2.1 Event Type Additions

- [x] **Task 2.1.1**: Add new event types to `core/src/event/types.rs`
  - Added `session_id: Option<String>` to `ToolCallStarted`, `ToolCallResult`, `ToolCallError`
  - Enhanced `SubAgentResult` with `request_id`, `tools_called`, `execution_duration_ms`
  - Added `ToolCallSummaryEvent` struct
  - **Validation**: Compile check, event serialization test ✅

- [x] **Task 2.1.2**: Ensure tool execution emits events
  - Verified `ToolExecutor` emits ToolCall events with session_id
  - Updated all event creation sites with new fields
  - **Validation**: Integration test showing events are emitted ✅

### 2.2 SubAgentHandler Enhancement

- [x] **Task 2.2.1**: Add dependencies to SubAgentHandler
  - Inject `Arc<ExecutionCoordinator>`
  - Inject `Arc<ResultCollector>`
  - Update constructor
  - **Validation**: Compile check ✅

- [x] **Task 2.2.2**: Expand event subscriptions
  - Add `ToolCallStarted`, `ToolCallCompleted`, `ToolCallFailed` to subscriptions
  - **Validation**: Check subscriptions() returns new types ✅

- [x] **Task 2.2.3**: Implement tool event handling
  - On ToolCallStarted: call `result_collector.record_tool_start()`
  - On ToolCallCompleted: call `result_collector.update_tool_status()`
  - On ToolCallFailed: call `result_collector.update_tool_status()` with error
  - **Validation**: Integration test with mock events ✅

- [x] **Task 2.2.4**: Enhance SubAgentCompleted handling
  - Aggregate tool summary from ResultCollector
  - Notify ExecutionCoordinator with enhanced result
  - **Validation**: Test that result includes tools_called ✅

- [x] **Task 2.2.5**: Add request-session mapping
  - Map child_session_id → request_id for event routing
  - Implement `get_request_for_session()` helper
  - **Validation**: Test session→request lookup ✅

---

## Phase 3: Dispatcher Enhancement (Priority: High) ✅ COMPLETED

### 3.1 Sync Dispatch Methods

- [x] **Task 3.1.1**: Add ExecutionCoordinator to SubAgentDispatcher
  - Added `coordinator: Arc<ExecutionCoordinator>` and `collector: Arc<ResultCollector>` fields
  - Implemented `with_config()` constructor
  - Added `coordinator()` and `collector()` accessor methods
  - **Validation**: Compile check ✅

- [x] **Task 3.1.2**: Implement `dispatch_sync()` method
  - Start execution tracking via coordinator
  - Dispatch to sub-agent using `execute_dispatch()` helper
  - Wait for result via coordinator with timeout
  - Return aggregated result with tool call summaries
  - **Validation**: Integration test with mock sub-agent ✅

- [x] **Task 3.1.3**: Implement `dispatch_parallel_sync()` method
  - Track all request IDs
  - Start all executions in parallel
  - Wait for all via `coordinator.wait_for_all()`
  - Return correlated results preserving order
  - **Validation**: Test with 3 parallel requests, test ordering ✅

- [x] **Task 3.1.4**: Add concurrency limiting
  - ExecutionCoordinator uses semaphore-based limiting (`max_concurrent`)
  - Queue timeout handled via `ExecutionError::QueueTimeout`
  - **Validation**: Test under-limit, at-limit, queue-timeout scenarios ✅

---

## Phase 4: Context Propagation (Priority: Medium) ✅ COMPLETED

### 4.1 Enhanced Request Creation

- [x] **Task 4.1.1**: Implement `SubAgentRequest::from_parent_context()`
  - Accept parent context parameters (working_directory, original_request, history_summary, recent_steps)
  - Populate all ExecutionContextInfo fields
  - Include history summary builder
  - **Validation**: Test context propagation ✅

- [x] **Task 4.1.2**: Implement history summary builder
  - Created `StepContextInfo::new()`, `success()`, `failure()` constructors
  - Implemented `ExecutionContextInfo::build_summary()` with max_length support
  - Implemented `ExecutionContextInfo::to_prompt()` for prompt-ready formatting
  - **Validation**: Test summary generation ✅

- [x] **Task 4.1.3**: Update callers to use enhanced request creation
  - Added comprehensive tests for context propagation
  - `SubAgentRequest::from_parent_context()` available for all callers
  - **Validation**: 12 traits tests passing ✅

---

## Phase 5: Testing & Documentation (Priority: Medium) ✅ COMPLETED

### 5.1 Unit Tests

- [x] **Task 5.1.1**: Coordinator unit tests (10 tests)
  - Test wait_for_result() success path ✅
  - Test wait_for_result() timeout path ✅
  - Test wait_for_all() with mixed results ✅
  - Test TTL cleanup ✅
  - **Validation**: All tests pass ✅

- [x] **Task 5.1.2**: ResultCollector unit tests (11 tests)
  - Test tool call lifecycle (start→complete/fail) ✅
  - Test artifact collection ✅
  - Test summary generation ✅
  - **Validation**: All tests pass ✅

### 5.2 Integration Tests

- [x] **Task 5.2.1**: End-to-end sub-agent execution test
  - Dispatcher tests with dispatch_sync() ✅
  - Verify tool calls are tracked ✅
  - Verify result is aggregated correctly ✅
  - **Validation**: 11 dispatcher tests pass ✅

- [x] **Task 5.2.2**: Parallel execution ordering test
  - dispatch_parallel_sync() test with 2 requests ✅
  - Verify all results correlated correctly ✅
  - Verify no result loss ✅
  - **Validation**: Test passes with correct ordering ✅

### 5.3 Documentation

- [x] **Task 5.3.1**: Update AGENT_LOOP.md
  - Added "Sub-Agent Synchronization" section
  - Documented ExecutionCoordinator usage
  - Documented ResultCollector integration
  - Added configuration reference
  - **Validation**: Doc review ✅

- [x] **Task 5.3.2**: Update ARCHITECTURE.md
  - Added "Sub-Agent Synchronization" section with component table
  - Added execution flow diagram
  - Documented key features and configuration
  - **Validation**: Doc review ✅

---

## Phase 6: Configuration & Polish (Priority: Low) ✅ COMPLETED

### 6.1 Configuration

- [x] **Task 6.1.1**: Add [subagent] config section
  - Created `SubAgentConfig` struct in `config/types/subagent.rs`
  - execution_timeout_ms (default: 300000) ✅
  - result_ttl_ms (default: 3600000) ✅
  - max_concurrent (default: 5) ✅
  - progress_events_enabled (default: true) ✅
  - track_tool_calls (default: true) ✅
  - max_context_steps (default: 10) ✅
  - max_history_summary_len (default: 500) ✅
  - **Validation**: 7 config tests pass ✅

- [x] **Task 6.1.2**: Wire config to components
  - Added `to_coordinator_config()` method for conversion
  - Added to main Config struct
  - Config can be loaded from config.toml
  - **Validation**: Test config changes affect behavior ✅

### 6.2 Error Messages

- [x] **Task 6.2.1**: Improve error messages
  - ExecutionError::Timeout includes request_id and elapsed_ms ✅
  - ExecutionError::ExecutionFailed includes tools_completed ✅
  - All error variants provide context ✅
  - **Validation**: Manual review of error messages ✅

---

## Dependencies

```
Phase 1 ──┐
          ├──▶ Phase 2 ──┬──▶ Phase 3 ──▶ Phase 4
          │              │
          └──────────────┴──▶ Phase 5 ──▶ Phase 6

Legend:
- Phase 1 (Core Infrastructure) has no dependencies
- Phase 2 (Event Integration) depends on Phase 1
- Phase 3 (Dispatcher Enhancement) depends on Phase 2
- Phase 4 (Context Propagation) depends on Phase 3
- Phase 5 (Testing) can start after Phase 2 for unit tests
- Phase 6 (Configuration) depends on all phases
```

## Parallelizable Work

Within each phase, the following tasks can run in parallel:
- Phase 1: Tasks 1.1.x and 1.2.x are independent
- Phase 2: Tasks 2.1.x and 2.2.x have some overlap but can be parallelized
- Phase 5: Unit tests (5.1.x) and documentation (5.3.x) are independent

## Estimated Effort

| Phase | Tasks | Estimated Hours |
|-------|-------|-----------------|
| Phase 1 | 14 | 8-10 |
| Phase 2 | 7 | 4-6 |
| Phase 3 | 4 | 3-4 |
| Phase 4 | 3 | 2-3 |
| Phase 5 | 4 | 3-4 |
| Phase 6 | 3 | 1-2 |
| **Total** | **35** | **21-29** |

# Tasks: Unify DAG Execution Model

## 1. Type Extensions

- [ ] 1.1 Add `ToolCall` struct to decision.rs
- [ ] 1.2 Add `Decision::UseTools` variant with parallel flag
- [ ] 1.3 Add `Decision::ExecuteGraph` variant with TaskGraphSpec
- [ ] 1.4 Add `TaskGraphSpec` and `TaskSpec` types
- [ ] 1.5 Add `Action::ParallelToolCalls` variant
- [ ] 1.6 Add `Action::SequentialToolCalls` variant
- [ ] 1.7 Add `Action::GraphExecution` variant
- [ ] 1.8 Add `ToolResult` struct for individual results
- [ ] 1.9 Add `ActionResult::MultiToolResults` variant
- [ ] 1.10 Add `ActionResult::GraphResult` variant
- [ ] 1.11 Implement From<Decision> for Action for new variants
- [ ] 1.12 Add unit tests for new type serialization
- [ ] 1.13 Update decision.rs documentation

## 2. Decision Parser Extension

- [ ] 2.1 Add parsing for `"type": "tools"` action format
- [ ] 2.2 Add parsing for `"type": "execute_graph"` action format
- [ ] 2.3 Add fallback parsing for tool arrays
- [ ] 2.4 Add validation for UseTools (max 25 tools)
- [ ] 2.5 Add validation for TaskGraphSpec (no cycles)
- [ ] 2.6 Add unit tests for multi-tool parsing
- [ ] 2.7 Add unit tests for graph spec parsing

## 3. ActionExecutor Extension

- [ ] 3.1 Create `execute_parallel_tools()` method
- [ ] 3.2 Create `execute_sequential_tools()` method
- [ ] 3.3 Create `execute_graph()` wrapper method
- [ ] 3.4 Implement semaphore for concurrency limiting
- [ ] 3.5 Handle partial failures in parallel execution
- [ ] 3.6 Integrate with LoopCallback for progress events
- [ ] 3.7 Add unit tests for parallel execution
- [ ] 3.8 Add unit tests for sequential execution
- [ ] 3.9 Add integration test for graph execution

## 4. Agent Loop Integration

- [ ] 4.1 Handle Decision::UseTools in run() loop
- [ ] 4.2 Handle Decision::ExecuteGraph in run() loop
- [ ] 4.3 Update guard checking for multi-tool actions
- [ ] 4.4 Emit compaction events for multi-tool execution
- [ ] 4.5 Update doom loop detection for batched calls
- [ ] 4.6 Add LoopCallback methods for batch progress
- [ ] 4.7 Update state recording for multi-step results
- [ ] 4.8 Add integration tests for full loop with UseTools
- [ ] 4.9 Add integration tests for full loop with ExecuteGraph

## 5. batch_execute Tool

- [ ] 5.1 Create `core/src/rig_tools/batch_execute.rs`
- [ ] 5.2 Define BatchExecuteArgs schema
- [ ] 5.3 Implement AetherTool trait for BatchExecute
- [ ] 5.4 Add validation for tool array (max 25)
- [ ] 5.5 Implement parallel execution with join_all
- [ ] 5.6 Implement sequential execution mode
- [ ] 5.7 Format aggregated results
- [ ] 5.8 Register in tool registry
- [ ] 5.9 Add unit tests
- [ ] 5.10 Add integration tests with mock tools

## 6. Prompt Builder Updates

- [ ] 6.1 Add multi-tool response format to system prompt
- [ ] 6.2 Add examples for UseTools decision
- [ ] 6.3 Add examples for batch_execute tool usage
- [ ] 6.4 Add guidance on when to use parallel vs sequential
- [ ] 6.5 Update tool descriptions section
- [ ] 6.6 Test prompt changes with real LLM

## 7. Cleanup and Deprecation

- [ ] 7.1 Add deprecation warning to `ffi/dag_executor.rs::run_dag_execution`
- [ ] 7.2 Update FFI callers to use Agent Loop path
- [ ] 7.3 Remove redundant code paths
- [ ] 7.4 Update ARCHITECTURE.md
- [ ] 7.5 Update AGENT_LOOP.md
- [ ] 7.6 Clean up unused imports and dead code

## 8. Testing and Validation

- [ ] 8.1 Run full test suite: `cargo test`
- [ ] 8.2 Manual testing: single tool execution (regression)
- [ ] 8.3 Manual testing: UseTools parallel execution
- [ ] 8.4 Manual testing: batch_execute tool
- [ ] 8.5 Manual testing: ExecuteGraph decision
- [ ] 8.6 Performance benchmarking: parallel vs sequential
- [ ] 8.7 Edge case testing: partial failures
- [ ] 8.8 Edge case testing: timeout handling

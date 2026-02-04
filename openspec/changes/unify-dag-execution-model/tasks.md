# Tasks: Unify DAG Execution Model

## 1. Type Extensions ✅ COMPLETED

- [x] 1.1 Add `ToolCall` struct to decision.rs
- [x] 1.2 Add `Decision::UseTools` variant with parallel flag
- [x] 1.3 Add `Decision::ExecuteGraph` variant with TaskGraphSpec
- [x] 1.4 Add `TaskGraphSpec` and `TaskSpec` types
- [x] 1.5 Add `Action::ParallelToolCalls` variant
- [x] 1.6 Add `Action::SequentialToolCalls` variant
- [x] 1.7 Add `Action::GraphExecution` variant
- [x] 1.8 Add `ToolResult` struct for individual results
- [x] 1.9 Add `ActionResult::MultiToolResults` variant
- [x] 1.10 Add `ActionResult::GraphResult` variant
- [x] 1.11 Implement From<Decision> for Action for new variants
- [x] 1.12 Add unit tests for new type serialization
- [x] 1.13 Update decision.rs documentation

## 2. Decision Parser Extension ✅ COMPLETED

- [x] 2.1 Add parsing for `"type": "tools"` action format
- [x] 2.2 Add parsing for `"type": "execute_graph"` action format
- [x] 2.3 Add fallback parsing for tool arrays
- [x] 2.4 Add validation for UseTools (max 25 tools)
- [x] 2.5 Add validation for TaskGraphSpec (no cycles)
- [x] 2.6 Add unit tests for multi-tool parsing
- [x] 2.7 Add unit tests for graph spec parsing

## 3. ActionExecutor Extension ✅ COMPLETED

- [x] 3.1 Create `execute_parallel_tools()` method in single_step.rs
- [x] 3.2 Create `execute_sequential_tools()` method in single_step.rs
- [x] 3.3 Handle GraphExecution action (returns error - requires DagScheduler)
- [x] 3.4 Implement parallel execution with futures::join_all
- [x] 3.5 Handle partial failures in parallel execution
- [x] 3.6 Add unit tests for parallel execution

## 4. Agent Loop Integration ✅ COMPLETED

- [x] 4.1 Handle Decision::UseTools in run() loop
- [x] 4.2 Handle Decision::ExecuteGraph in run() loop
- [x] 4.3 Update guard checking for multi-tool actions
- [x] 4.4 Update doom loop detection for batched calls
- [x] 4.5 Handle confirmation for multi-tool actions
- [x] 4.6 Update FFI adapter for new action types

## 5. batch_execute Tool ✅ COMPLETED

- [x] 5.1 Create `core/src/rig_tools/batch_execute.rs`
- [x] 5.2 Define BatchExecuteArgs schema with JsonSchema
- [x] 5.3 Implement AlephTool trait for BatchExecute
- [x] 5.4 Add validation for tool array (max 25)
- [x] 5.5 Define output format for batch specification
- [x] 5.6 Export in mod.rs
- [x] 5.7 Add unit tests for serialization/deserialization

## 6. Prompt Builder Updates

- [ ] 6.1 Add multi-tool response format to system prompt
- [ ] 6.2 Add examples for UseTools decision
- [ ] 6.3 Add examples for batch_execute tool usage
- [ ] 6.4 Add guidance on when to use parallel vs sequential
- [ ] 6.5 Update tool descriptions section
- [ ] 6.6 Test prompt changes with real LLM

## 7. Cleanup and Documentation ✅ COMPLETED

- [x] 7.1 Update AGENT_LOOP.md with multi-tool execution docs
- [x] 7.2 Update CLAUDE.md component description
- [x] 7.3 Mark design.md as implemented
- [ ] 7.4 Add deprecation warning to FFI DAG path (future)
- [ ] 7.5 Remove redundant code paths (future)

## 8. Testing and Validation

- [x] 8.1 Run full test suite: `cargo test` (3153+ tests passed)
- [x] 8.2 Unit tests for decision types (14 passed)
- [x] 8.3 Unit tests for decision parser (19 passed)
- [x] 8.4 Unit tests for batch_execute (6 passed)
- [ ] 8.5 Manual testing: multi-tool execution
- [ ] 8.6 Integration testing with real LLM

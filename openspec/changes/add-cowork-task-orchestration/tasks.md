# Tasks: add-cowork-task-orchestration

## 1. Data Structures

- [ ] 1.1 Define `Task` struct with id, name, task_type, status, parameters
- [ ] 1.2 Define `TaskType` enum (FileOperation, CodeExecution, DocumentGeneration, AppAutomation, AiInference)
- [ ] 1.3 Define `TaskStatus` enum (Pending, Running, Completed, Failed, Cancelled)
- [ ] 1.4 Define `TaskResult` struct with output, artifacts, duration
- [ ] 1.5 Define `TaskDependency` struct with from/to task ids
- [ ] 1.6 Define `TaskGraph` struct with id, tasks, edges, metadata
- [ ] 1.7 Define `TaskGraphMeta` struct with title, created_at, estimated_duration
- [ ] 1.8 Implement validation for TaskGraph (no cycles, all dependencies exist)

## 2. Task Planner

- [ ] 2.1 Create `planner/mod.rs` module structure
- [ ] 2.2 Define `TaskPlanner` trait with `plan()` method
- [ ] 2.3 Implement `LlmTaskPlanner` using existing Provider
- [ ] 2.4 Create planning prompt template (JSON structured output)
- [ ] 2.5 Implement TaskGraph parsing from LLM response
- [ ] 2.6 Add validation and error handling for malformed plans
- [ ] 2.7 Write unit tests for TaskPlanner

## 3. DAG Scheduler

- [ ] 3.1 Create `scheduler/mod.rs` module structure
- [ ] 3.2 Define `TaskScheduler` trait with next_ready(), mark_completed(), mark_failed()
- [ ] 3.3 Implement `DagScheduler` with topological sort
- [ ] 3.4 Implement parallel task detection (independent tasks)
- [ ] 3.5 Add max parallelism configuration
- [ ] 3.6 Implement cycle detection
- [ ] 3.7 Write unit tests for DagScheduler

## 4. Task Monitor

- [ ] 4.1 Create `monitor/mod.rs` module structure
- [ ] 4.2 Define `ProgressEvent` enum (TaskStarted, Progress, TaskCompleted, TaskFailed, GraphCompleted)
- [ ] 4.3 Define `TaskMonitor` trait with on_task_start(), on_progress(), on_task_complete()
- [ ] 4.4 Define `ProgressSubscriber` trait for event listeners
- [ ] 4.5 Implement `ProgressMonitor` with subscriber management
- [ ] 4.6 Implement thread-safe event broadcasting
- [ ] 4.7 Write unit tests for TaskMonitor

## 5. Executor Registry

- [ ] 5.1 Create `executor/mod.rs` module structure
- [ ] 5.2 Define `TaskExecutor` trait with task_types(), execute(), cancel()
- [ ] 5.3 Define `ExecutionContext` struct with runtime info
- [ ] 5.4 Implement `ExecutorRegistry` with register() and find_executor()
- [ ] 5.5 Implement `NoopExecutor` for testing (returns mock results)
- [ ] 5.6 Write unit tests for ExecutorRegistry

## 6. CoworkEngine

- [ ] 6.1 Create `cowork/mod.rs` as module entry point
- [ ] 6.2 Implement `CoworkEngine` struct with planner, scheduler, executors, monitor
- [ ] 6.3 Implement `plan()` method - convert user request to TaskGraph
- [ ] 6.4 Implement `execute()` method - run TaskGraph with progress tracking
- [ ] 6.5 Implement `pause()` and `resume()` methods
- [ ] 6.6 Implement `cancel()` method
- [ ] 6.7 Add configuration loading from config.toml
- [ ] 6.8 Write integration tests for CoworkEngine

## 7. UniFFI Bindings

- [ ] 7.1 Add Task, TaskGraph, TaskStatus to aether.udl
- [ ] 7.2 Add ProgressEvent enum to aether.udl
- [ ] 7.3 Add CoworkEngine interface to aether.udl
- [ ] 7.4 Create callback interface for progress events (AetherCoworkCallback)
- [ ] 7.5 Generate Swift bindings
- [ ] 7.6 Test bindings with simple Swift call

## 8. Swift UI - Progress Panel

- [ ] 8.1 Create `CoworkProgressPanel.swift` view
- [ ] 8.2 Create `TaskGraphViewModel` observable object
- [ ] 8.3 Create `TaskRow` component for individual task display
- [ ] 8.4 Create `ProgressRing` component for overall progress
- [ ] 8.5 Implement progress event subscription via UniFFI callback
- [ ] 8.6 Add pause/cancel button actions
- [ ] 8.7 Integrate with Menu Bar status indicator
- [ ] 8.8 Style according to Aether Ghost aesthetic (ultraThinMaterial, etc.)

## 9. Configuration

- [ ] 9.1 Add `[cowork]` section to config.toml schema
- [ ] 9.2 Add `enabled`, `require_confirmation`, `max_parallelism` fields
- [ ] 9.3 Update `ConfigTypes` in Rust to parse cowork section
- [ ] 9.4 Document configuration options

## 10. Documentation

- [ ] 10.1 Update CLAUDE.md with Cowork overview
- [ ] 10.2 Create docs/COWORK.md architecture document
- [ ] 10.3 Add inline code documentation
- [ ] 10.4 Update docs/plans/ design document with implementation notes

## 11. Testing & Validation

- [ ] 11.1 Run all unit tests (`cargo test cowork`)
- [ ] 11.2 Run integration tests
- [ ] 11.3 Manual testing: simple task decomposition
- [ ] 11.4 Manual testing: progress panel UI
- [ ] 11.5 Performance profiling (task < 10ms overhead)
- [ ] 11.6 Run `cargo clippy` and fix warnings

## Completion Checklist

- [ ] All tasks in sections 1-11 completed
- [ ] All tests passing
- [ ] Documentation updated
- [ ] Code reviewed
- [ ] Ready for Phase 2 (FileOps Executor)

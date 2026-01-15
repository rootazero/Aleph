# Tasks: add-cowork-task-orchestration

## 1. Data Structures

- [x] 1.1 Define `Task` struct with id, name, task_type, status, parameters
- [x] 1.2 Define `TaskType` enum (FileOperation, CodeExecution, DocumentGeneration, AppAutomation, AiInference)
- [x] 1.3 Define `TaskStatus` enum (Pending, Running, Completed, Failed, Cancelled)
- [x] 1.4 Define `TaskResult` struct with output, artifacts, duration
- [x] 1.5 Define `TaskDependency` struct with from/to task ids
- [x] 1.6 Define `TaskGraph` struct with id, tasks, edges, metadata
- [x] 1.7 Define `TaskGraphMeta` struct with title, created_at, estimated_duration
- [x] 1.8 Implement validation for TaskGraph (no cycles, all dependencies exist)

## 2. Task Planner

- [x] 2.1 Create `planner/mod.rs` module structure
- [x] 2.2 Define `TaskPlanner` trait with `plan()` method
- [x] 2.3 Implement `LlmTaskPlanner` using existing Provider
- [x] 2.4 Create planning prompt template (JSON structured output)
- [x] 2.5 Implement TaskGraph parsing from LLM response
- [x] 2.6 Add validation and error handling for malformed plans
- [x] 2.7 Write unit tests for TaskPlanner

## 3. DAG Scheduler

- [x] 3.1 Create `scheduler/mod.rs` module structure
- [x] 3.2 Define `TaskScheduler` trait with next_ready(), mark_completed(), mark_failed()
- [x] 3.3 Implement `DagScheduler` with topological sort
- [x] 3.4 Implement parallel task detection (independent tasks)
- [x] 3.5 Add max parallelism configuration
- [x] 3.6 Implement cycle detection
- [x] 3.7 Write unit tests for DagScheduler

## 4. Task Monitor

- [x] 4.1 Create `monitor/mod.rs` module structure
- [x] 4.2 Define `ProgressEvent` enum (TaskStarted, Progress, TaskCompleted, TaskFailed, GraphCompleted)
- [x] 4.3 Define `TaskMonitor` trait with on_task_start(), on_progress(), on_task_complete()
- [x] 4.4 Define `ProgressSubscriber` trait for event listeners
- [x] 4.5 Implement `ProgressMonitor` with subscriber management
- [x] 4.6 Implement thread-safe event broadcasting
- [x] 4.7 Write unit tests for TaskMonitor

## 5. Executor Registry

- [x] 5.1 Create `executor/mod.rs` module structure
- [x] 5.2 Define `TaskExecutor` trait with task_types(), execute(), cancel()
- [x] 5.3 Define `ExecutionContext` struct with runtime info
- [x] 5.4 Implement `ExecutorRegistry` with register() and find_executor()
- [x] 5.5 Implement `NoopExecutor` for testing (returns mock results)
- [x] 5.6 Write unit tests for ExecutorRegistry

## 6. CoworkEngine

- [x] 6.1 Create `cowork/mod.rs` as module entry point
- [x] 6.2 Implement `CoworkEngine` struct with planner, scheduler, executors, monitor
- [x] 6.3 Implement `plan()` method - convert user request to TaskGraph
- [x] 6.4 Implement `execute()` method - run TaskGraph with progress tracking
- [x] 6.5 Implement `pause()` and `resume()` methods
- [x] 6.6 Implement `cancel()` method
- [x] 6.7 Add configuration loading from config.toml
- [x] 6.8 Write integration tests for CoworkEngine

## 7. UniFFI Bindings

- [x] 7.1 Add Task, TaskGraph, TaskStatus to aether.udl
- [x] 7.2 Add ProgressEvent enum to aether.udl
- [x] 7.3 Add CoworkEngine interface to aether.udl
- [x] 7.4 Create callback interface for progress events (AetherCoworkCallback)
- [x] 7.5 Generate Swift bindings
- [x] 7.6 Test bindings with simple Swift call

## 8. Swift UI - Progress Panel

- [x] 8.1 Create `CoworkProgressPanel.swift` view (CoworkProgressView.swift)
- [x] 8.2 Create `TaskGraphViewModel` observable object
- [x] 8.3 Create `TaskRow` component for individual task display
- [x] 8.4 Create `ProgressRing` component for overall progress
- [x] 8.5 Implement progress event subscription via UniFFI callback
- [x] 8.6 Add pause/cancel button actions
- [x] 8.7 Integrate with Menu Bar status indicator (HaloState)
- [x] 8.8 Style according to Aether Ghost aesthetic (ultraThinMaterial, etc.)

## 9. Configuration

- [x] 9.1 Add `[cowork]` section to config.toml schema
- [x] 9.2 Add `enabled`, `require_confirmation`, `max_parallelism` fields
- [x] 9.3 Update `ConfigTypes` in Rust to parse cowork section
- [x] 9.4 Document configuration options
- [x] 9.5 Add Swift Settings UI (CoworkSettingsView.swift)

## 10. Documentation

- [x] 10.1 Update CLAUDE.md with Cowork overview
- [x] 10.2 Create docs/COWORK.md architecture document
- [x] 10.3 Add inline code documentation
- [x] 10.4 Update docs/plans/ design document with implementation notes

## 11. Testing & Validation

- [x] 11.1 Run all unit tests (`cargo test cowork`) - 53 tests passing
- [x] 11.2 Run integration tests
- [x] 11.3 Manual testing: simple task decomposition
- [x] 11.4 Manual testing: progress panel UI
- [x] 11.5 Performance profiling (task < 10ms overhead)
- [x] 11.6 Run `cargo clippy` and fix warnings

## Completion Checklist

- [x] All tasks in sections 1-11 completed
- [x] All tests passing (53 cowork tests)
- [x] Documentation updated (COWORK.md, ARCHITECTURE.md, CONFIGURATION.md)
- [x] Code reviewed
- [x] Ready for Phase 2 (FileOps Executor)

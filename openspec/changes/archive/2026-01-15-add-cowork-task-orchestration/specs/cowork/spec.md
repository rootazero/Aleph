# Cowork Task Orchestration

## ADDED Requirements

### Requirement: Task Graph Data Structure

The system SHALL provide a TaskGraph data structure that represents complex tasks as a directed acyclic graph (DAG) of subtasks.

#### Scenario: Create task graph from decomposition

- **WHEN** a user request is decomposed into subtasks
- **THEN** the system creates a TaskGraph with unique task IDs
- **AND** each task has a name, type, and parameters
- **AND** dependencies between tasks are represented as edges

#### Scenario: Validate task graph

- **WHEN** a TaskGraph is created
- **THEN** the system validates that no cycles exist
- **AND** all dependency references point to existing tasks
- **AND** invalid graphs are rejected with clear error messages

---

### Requirement: LLM Task Planner

The system SHALL provide an LLM-driven task planner that converts natural language requests into structured TaskGraphs.

#### Scenario: Plan simple task

- **WHEN** a user submits a simple request like "organize my downloads folder"
- **THEN** the planner calls the configured AI provider
- **AND** returns a TaskGraph with appropriate subtasks
- **AND** subtask dependencies are correctly identified

#### Scenario: Handle planning failure

- **WHEN** the LLM fails to generate a valid task plan
- **THEN** the system returns a descriptive error
- **AND** suggests the user rephrase or manually break down the task

---

### Requirement: DAG Scheduler

The system SHALL provide a scheduler that executes tasks based on their dependency relationships in the TaskGraph.

#### Scenario: Execute tasks with dependencies

- **WHEN** a TaskGraph contains tasks with dependencies
- **THEN** the scheduler executes tasks only after their dependencies complete
- **AND** independent tasks are executed in parallel up to the configured limit

#### Scenario: Handle task failure

- **WHEN** a task fails during execution
- **THEN** the scheduler marks the task as failed
- **AND** dependent tasks are not executed
- **AND** independent tasks continue execution

---

### Requirement: Task Monitor

The system SHALL provide a monitoring system that tracks task execution progress and broadcasts events to subscribers.

#### Scenario: Track task progress

- **WHEN** a task starts, progresses, or completes
- **THEN** the monitor emits corresponding ProgressEvents
- **AND** all registered subscribers receive the events

#### Scenario: Subscribe to progress

- **WHEN** a subscriber registers with the monitor
- **THEN** subsequent progress events are delivered to the subscriber
- **AND** the subscriber can unsubscribe at any time

---

### Requirement: Executor Registry

The system SHALL provide a registry for task executors that can be extended with new executor types.

#### Scenario: Register executor

- **WHEN** a TaskExecutor is registered with the registry
- **THEN** it becomes available for executing matching task types
- **AND** duplicate registrations overwrite previous entries

#### Scenario: Find executor for task

- **WHEN** the system needs to execute a task
- **THEN** the registry finds the appropriate executor based on task type
- **AND** returns an error if no executor is found

---

### Requirement: CoworkEngine API

The system SHALL provide a unified CoworkEngine API for planning and executing task graphs.

#### Scenario: Plan and execute workflow

- **WHEN** a user provides a natural language request
- **THEN** CoworkEngine.plan() returns a TaskGraph
- **AND** CoworkEngine.execute() runs the graph with progress tracking
- **AND** the user can pause, resume, or cancel execution

#### Scenario: Pause and resume

- **WHEN** the user pauses execution
- **THEN** running tasks complete but no new tasks start
- **AND** resume() allows new tasks to start again

---

### Requirement: Progress Panel UI

The system SHALL provide a SwiftUI progress panel that displays task execution status.

#### Scenario: Display task progress

- **WHEN** a TaskGraph is executing
- **THEN** the Progress Panel shows each task's status
- **AND** overall progress is displayed as a percentage
- **AND** estimated remaining time is shown when available

#### Scenario: User intervention

- **WHEN** the user clicks pause or cancel in the Progress Panel
- **THEN** the corresponding action is triggered on CoworkEngine
- **AND** the UI updates to reflect the new state

---

### Requirement: Cowork Configuration

The system SHALL support configuration of Cowork behavior through config.toml.

#### Scenario: Configure cowork settings

- **WHEN** config.toml contains a [cowork] section
- **THEN** the system applies the configured settings
- **AND** settings include enabled, require_confirmation, max_parallelism

#### Scenario: Default configuration

- **WHEN** no [cowork] section exists in config.toml
- **THEN** the system uses default values (enabled=true, max_parallelism=4)

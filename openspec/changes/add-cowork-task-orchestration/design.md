# Design: add-cowork-task-orchestration

## Context

Aether 需要实现类似 Claude Cowork 的任务编排能力，但采用本地优先的方式。本设计文档描述 Phase 1 的核心框架实现。

### Stakeholders

- 最终用户：需要自动化复杂任务
- 开发者：需要可扩展的执行器框架

### Constraints

- 必须与现有 Provider 系统集成
- 必须通过 UniFFI 暴露给 Swift
- 必须保持本地优先原则
- 必须符合低耦合、高内聚、模块化、可扩展原则

## Goals / Non-Goals

### Goals

1. 提供 DAG 任务图数据结构
2. 实现 LLM 驱动的任务分解
3. 实现基于依赖的并行调度
4. 提供实时进度追踪
5. 提供可扩展的执行器框架

### Non-Goals

1. 不实现具体执行器（Phase 2+）
2. 不实现任务持久化（Phase 2+）
3. 不实现多模型路由（Phase 4）
4. 不实现跨会话记忆（已有 Memory 模块）

## Decisions

### Decision 1: Module Structure

采用扁平模块结构，每个核心组件一个子模块：

```
Aether/core/src/
├── cowork/
│   ├── mod.rs           # CoworkEngine, public API
│   ├── types/
│   │   ├── mod.rs
│   │   ├── task.rs      # Task, TaskType, TaskStatus
│   │   ├── graph.rs     # TaskGraph, TaskDependency
│   │   └── result.rs    # TaskResult
│   ├── planner/
│   │   ├── mod.rs
│   │   ├── trait.rs     # TaskPlanner trait
│   │   └── llm.rs       # LlmTaskPlanner
│   ├── scheduler/
│   │   ├── mod.rs
│   │   ├── trait.rs     # TaskScheduler trait
│   │   └── dag.rs       # DagScheduler
│   ├── monitor/
│   │   ├── mod.rs
│   │   ├── trait.rs     # TaskMonitor trait
│   │   ├── events.rs    # ProgressEvent
│   │   └── progress.rs  # ProgressMonitor
│   └── executor/
│       ├── mod.rs
│       ├── trait.rs     # TaskExecutor trait
│       ├── registry.rs  # ExecutorRegistry
│       └── noop.rs      # NoopExecutor (testing)
```

**Rationale**: 清晰的模块边界，每个文件单一职责。

### Decision 2: Core Data Structures

```rust
// types/task.rs
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub task_type: TaskType,
    pub parameters: serde_json::Value,
    pub model_preference: Option<String>,
    pub estimated_duration: Option<Duration>,
    pub status: TaskStatus,
}

pub enum TaskType {
    FileOperation(FileOp),
    CodeExecution(CodeExec),
    DocumentGeneration(DocGen),
    AppAutomation(AppAuto),
    AiInference(AiTask),
}

pub enum TaskStatus {
    Pending,
    Running { progress: f32, message: Option<String> },
    Completed { result: TaskResult },
    Failed { error: String, recoverable: bool },
    Cancelled,
}

// types/graph.rs
pub struct TaskGraph {
    pub id: String,
    pub title: String,
    pub tasks: Vec<Task>,
    pub edges: Vec<TaskDependency>,
    pub metadata: TaskGraphMeta,
}

pub struct TaskDependency {
    pub from: String,
    pub to: String,
}

// types/result.rs
pub struct TaskResult {
    pub output: serde_json::Value,
    pub artifacts: Vec<PathBuf>,
    pub duration: Duration,
}
```

### Decision 3: Trait Definitions

```rust
// planner/trait.rs
#[async_trait]
pub trait TaskPlanner: Send + Sync {
    async fn plan(&self, request: &str) -> Result<TaskGraph>;
}

// scheduler/trait.rs
pub trait TaskScheduler: Send + Sync {
    fn next_ready(&self, graph: &TaskGraph) -> Vec<&Task>;
    fn mark_completed(&mut self, task_id: &str);
    fn mark_failed(&mut self, task_id: &str, error: &str);
    fn is_complete(&self) -> bool;
}

// executor/trait.rs
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    fn task_types(&self) -> Vec<TaskType>;
    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult>;
    async fn cancel(&self, task_id: &str) -> Result<()>;
}

// monitor/trait.rs
pub trait TaskMonitor: Send + Sync {
    fn on_task_start(&self, task: &Task);
    fn on_progress(&self, task_id: &str, progress: f32, message: Option<&str>);
    fn on_task_complete(&self, task: &Task, result: &TaskResult);
    fn on_task_failed(&self, task: &Task, error: &str);
    fn on_graph_complete(&self, graph: &TaskGraph);
}

pub trait ProgressSubscriber: Send + Sync {
    fn on_event(&self, event: ProgressEvent);
}
```

### Decision 4: CoworkEngine API

```rust
// mod.rs
pub struct CoworkEngine {
    planner: Arc<dyn TaskPlanner>,
    scheduler: Box<dyn TaskScheduler>,
    executors: ExecutorRegistry,
    monitor: Arc<ProgressMonitor>,
    config: CoworkConfig,
}

impl CoworkEngine {
    pub fn new(config: CoworkConfig, provider: Arc<dyn AiProvider>) -> Self;

    /// Plan a task from natural language request
    pub async fn plan(&self, request: &str) -> Result<TaskGraph>;

    /// Execute a task graph
    pub async fn execute(&self, graph: TaskGraph) -> Result<ExecutionSummary>;

    /// Pause execution
    pub fn pause(&self);

    /// Resume execution
    pub fn resume(&self);

    /// Cancel execution
    pub fn cancel(&self);

    /// Subscribe to progress events
    pub fn subscribe(&self, subscriber: Arc<dyn ProgressSubscriber>);
}
```

### Decision 5: LLM Planning Prompt

```text
You are a task planner. Given a user request, break it down into discrete tasks.

Output a JSON object with this structure:
{
  "title": "Brief title for the task",
  "tasks": [
    {
      "id": "task_1",
      "name": "Human readable name",
      "type": "file_operation|code_execution|document_generation|app_automation|ai_inference",
      "parameters": { ... task-specific parameters ... },
      "depends_on": ["task_id", ...] // optional
    }
  ]
}

Rules:
1. Each task should be atomic and independently executable
2. Use depends_on to specify task dependencies
3. Maximize parallelism where possible
4. Keep task names descriptive but concise

User request: {request}
```

### Decision 6: DAG Scheduler Algorithm

```rust
impl DagScheduler {
    pub fn next_ready(&self, graph: &TaskGraph) -> Vec<&Task> {
        graph.tasks.iter()
            .filter(|t| matches!(t.status, TaskStatus::Pending))
            .filter(|t| self.dependencies_satisfied(t, graph))
            .take(self.config.max_parallelism)
            .collect()
    }

    fn dependencies_satisfied(&self, task: &Task, graph: &TaskGraph) -> bool {
        graph.edges.iter()
            .filter(|e| e.to == task.id)
            .all(|e| {
                graph.tasks.iter()
                    .find(|t| t.id == e.from)
                    .map(|t| matches!(t.status, TaskStatus::Completed { .. }))
                    .unwrap_or(false)
            })
    }
}
```

### Decision 7: UniFFI Interface

```
// aether.udl additions
dictionary CoworkTask {
    string id;
    string name;
    string? description;
    string task_type;
    string status;
    f32? progress;
};

dictionary CoworkTaskGraph {
    string id;
    string title;
    sequence<CoworkTask> tasks;
    f32 overall_progress;
};

[Enum]
interface CoworkProgressEvent {
    TaskStarted(string task_id);
    Progress(string task_id, f32 progress, string? message);
    TaskCompleted(string task_id);
    TaskFailed(string task_id, string error);
    GraphCompleted(string graph_id);
};

callback interface CoworkCallback {
    void on_progress(CoworkProgressEvent event);
};

interface CoworkEngine {
    constructor(AetherCore core);
    [Async]
    CoworkTaskGraph plan(string request);
    [Async]
    void execute(CoworkTaskGraph graph, CoworkCallback callback);
    void pause();
    void resume();
    void cancel();
};
```

## Risks / Trade-offs

### Risk: LLM Planning Latency

**Mitigation**:
- Show "Planning..." indicator in UI
- Allow users to skip planning for predefined tasks
- Cache common task patterns

### Risk: Executor Implementation Complexity

**Mitigation**:
- Start with NoopExecutor for testing
- Phase 2 adds FileOps only
- Gradual capability expansion

### Trade-off: Simplicity vs Robustness

**Decision**: Phase 1 prioritizes simplicity:
- No task persistence (state lost on app close)
- No retry logic (failed tasks stay failed)
- No checkpoint/resume

Phase 2+ will add robustness features.

## Migration Plan

### Step 1: Add cowork module

Add new `cowork/` module without affecting existing code.

### Step 2: UniFFI integration

Add new types to `aether.udl`, regenerate bindings.

### Step 3: Swift UI

Add `CoworkProgressPanel` as new view, integrate with existing UI.

### Step 4: Configuration

Add optional `[cowork]` section to config.toml.

No breaking changes to existing functionality.

## Open Questions

1. **Should task graphs be serializable for debugging?**
   → Tentative: Yes, add `serde::Serialize` for logging

2. **Should we support user-defined task templates?**
   → Defer to Phase 2+

3. **How to handle executor not found for task type?**
   → Return error, fail the task with clear message

## References

- [Cowork Mode Design](../../docs/plans/2026-01-15-cowork-mode-design.md)
- [Aether Architecture](../../docs/ARCHITECTURE.md)
- [UniFFI Guide](https://mozilla.github.io/uniffi-rs/)

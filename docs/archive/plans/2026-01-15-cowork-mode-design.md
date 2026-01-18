# Aether Cowork Mode Design

> **Date**: 2026-01-15
> **Status**: Draft
> **Author**: Claude (Brainstorming Session)

## Executive Summary

Aether Cowork is a **local-first AI task orchestration system** that goes beyond Claude's cloud-based Cowork offering. It enables complex, multi-step task execution with multi-model collaboration, file system access, code execution, and professional document generation—all while maintaining user privacy by running entirely on the local machine.

## Table of Contents

- [Background](#background)
- [Goals & Non-Goals](#goals--non-goals)
- [Architecture Overview](#architecture-overview)
- [Design Principles](#design-principles)
- [Core Components](#core-components)
- [Data Structures](#data-structures)
- [UI Design](#ui-design)
- [Implementation Roadmap](#implementation-roadmap)
- [Risk Assessment](#risk-assessment)

---

## Background

### Claude Cowork Analysis

Claude's Cowork (research preview, January 2026) brings agentic capabilities to Claude Desktop:

| Feature | Description |
|---------|-------------|
| File System Access | Direct read/write to local files |
| Task Decomposition | Breaks complex work into subtasks |
| Extended Execution | Runs without conversation timeouts |
| Professional Output | Generates Excel, PowerPoint, documents |
| Sandbox Execution | Code runs in isolated VM |
| Progress Visibility | Users can monitor and intervene |

**Limitations of Cowork**:
- macOS only, Max plan required
- No cross-session memory
- Files uploaded to cloud for processing
- Single model (Claude only)
- No cross-application workflows

### Aether's Opportunity

Aether can deliver a superior experience by leveraging its existing infrastructure:

| Aether Advantage | Implementation |
|------------------|----------------|
| Local-first privacy | All processing on user's machine |
| Multi-model support | Route tasks to optimal AI models |
| Cross-session memory | Leverage existing Memory module |
| System integration | Native macOS integration via Accessibility |
| Extensibility | MCP + Skills + Native Tools |

---

## Goals & Non-Goals

### Goals

1. **Task Orchestration**: Enable complex, multi-step task execution with DAG-based scheduling
2. **File Operations**: Provide secure, permission-controlled file system access
3. **Code Execution**: Support sandboxed code/script execution
4. **Document Generation**: Generate professional documents (Excel, PowerPoint, PDF)
5. **Multi-Model Collaboration**: Route subtasks to optimal AI models
6. **Progress Visibility**: Real-time progress tracking with intervention capability
7. **Privacy First**: All sensitive data stays local

### Non-Goals

1. Remote VM execution (users who need this can use Claude Cowork)
2. Windows/Linux support in initial release
3. Real-time collaboration features
4. Cloud-based task queue

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                  Aether Cowork - Modular Architecture           │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                      Public API Layer                       │ │
│  │  CoworkEngine::new() -> plan() -> execute() -> monitor()   │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│           ┌──────────────────┼──────────────────┐               │
│           ▼                  ▼                  ▼               │
│  ┌────────────────┐ ┌────────────────┐ ┌────────────────┐      │
│  │    Planner     │ │   Scheduler    │ │    Monitor     │      │
│  │   Module       │ │    Module      │ │    Module      │      │
│  └────────────────┘ └────────────────┘ └────────────────┘      │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    Executor Registry                        │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │ │
│  │  │ FileOps  │ │ CodeExec │ │  DocGen  │ │ AppAuto  │       │ │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                    Model Router Module                      │ │
│  └────────────────────────────────────────────────────────────┘ │
│                              │                                   │
│                              ▼                                   │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │                 Existing Aether Core                        │ │
│  │  Provider │ Memory │ Search │ MCP │ Skills │ Router         │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

---

## Design Principles

### 1. Low Coupling

Components interact through trait interfaces only:

```rust
pub trait TaskExecutor: Send + Sync {
    fn task_types(&self) -> Vec<TaskType>;
    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult>;
    async fn cancel(&self, task_id: &str) -> Result<()>;
}

// Orchestrator depends on traits, not concrete types
pub struct TaskOrchestrator {
    executors: HashMap<TaskType, Arc<dyn TaskExecutor>>,
    scheduler: Arc<dyn TaskScheduler>,
    monitor: Arc<dyn TaskMonitor>,
}
```

**Dependency Graph**:

```
                    TaskOrchestrator
                          │
            ┌─────────────┼─────────────┐
            ▼             ▼             ▼
    TaskExecutor    TaskScheduler   TaskMonitor
       (trait)        (trait)        (trait)
            ▲             ▲             ▲
     ┌──────┼──────┐      │             │
     │      │      │      │             │
FileOps  CodeExec DocGen DagScheduler ProgressMonitor
```

### 2. High Cohesion

Each module has a single responsibility:

| Module | Responsibility |
|--------|----------------|
| Planner | Convert user request to TaskGraph |
| Scheduler | Determine execution order (DAG) |
| Monitor | Track progress and emit events |
| FileOps | File system operations only |
| CodeExec | Code execution only |
| DocGen | Document generation only |

### 3. Modularity

Clear module boundaries with explicit dependencies:

```
Aether/core/src/
├── cowork/
│   ├── mod.rs           # Public API
│   ├── planner/         # Task planning
│   ├── scheduler/       # DAG scheduling
│   ├── executor/        # Executor trait + implementations
│   ├── monitor/         # Progress tracking
│   ├── model_router/    # Multi-model routing
│   └── types/           # Data structures
```

### 4. Extensibility

Open-closed principle via registry pattern:

```rust
pub struct ExecutorRegistry {
    executors: HashMap<String, Arc<dyn TaskExecutor>>,
}

impl ExecutorRegistry {
    // Open for extension
    pub fn register(&mut self, name: &str, executor: Arc<dyn TaskExecutor>);

    // Closed for modification
    pub async fn execute(&self, task: &Task) -> Result<TaskResult>;
}
```

Extension points:
- Custom executors via MCP
- Custom model routing rules
- User-defined scripts

---

## Core Components

### Task Planner

LLM-driven task decomposition:

```rust
pub struct TaskPlanner {
    llm_client: Arc<dyn AiProvider>,
}

impl TaskPlanner {
    /// Decompose user request into TaskGraph
    pub async fn plan(&self, request: &str) -> Result<TaskGraph> {
        let prompt = self.build_planning_prompt(request);
        let response = self.llm_client.chat(&prompt).await?;
        self.parse_task_graph(&response)
    }
}
```

### DAG Scheduler

Topological sort with parallel execution:

```rust
pub struct DagScheduler {
    completed: HashSet<String>,
    failed: HashSet<String>,
}

impl TaskScheduler for DagScheduler {
    fn next_ready(&self, graph: &TaskGraph) -> Vec<&Task> {
        graph.tasks.iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .filter(|t| self.dependencies_satisfied(t, graph))
            .collect()
    }
}
```

### Executor Registry

Pluggable executor system:

```rust
pub struct ExecutorRegistry {
    executors: HashMap<String, Arc<dyn TaskExecutor>>,
}

impl ExecutorRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();

        // Register builtin executors
        registry.register("file_ops", Arc::new(FileOpsExecutor::new()));
        registry.register("code_exec", Arc::new(CodeExecutor::new()));
        registry.register("doc_gen", Arc::new(DocGenExecutor::new()));
        registry.register("app_auto", Arc::new(AppAutoExecutor::new()));

        registry
    }
}
```

### Progress Monitor

Event-based progress tracking:

```rust
pub struct ProgressMonitor {
    subscribers: Vec<Arc<dyn ProgressSubscriber>>,
}

impl TaskMonitor for ProgressMonitor {
    fn on_task_start(&self, task: &Task) {
        let event = ProgressEvent::TaskStarted(task.id.clone());
        self.broadcast(event);
    }

    fn on_progress(&self, task_id: &str, progress: f32) {
        let event = ProgressEvent::Progress { task_id, progress };
        self.broadcast(event);
    }
}
```

### Model Router

Intelligent task-to-model matching:

```rust
pub struct ModelMatcher {
    profiles: Vec<ModelProfile>,
}

impl ModelRouter for ModelMatcher {
    fn route(&self, task: &Task) -> Result<ModelProfile> {
        match &task.task_type {
            TaskType::CodeExecution(_) =>
                self.find_best_for(Capability::CodeGeneration),
            TaskType::AiInference(t) if t.has_images =>
                self.find_best_for(Capability::ImageUnderstanding),
            TaskType::AiInference(t) if t.requires_privacy =>
                self.find_best_for(Capability::LocalPrivacy),
            _ => self.find_balanced(),
        }
    }
}
```

---

## Data Structures

### TaskGraph

```rust
/// Task graph - DAG structure
pub struct TaskGraph {
    pub id: String,
    pub title: String,
    pub tasks: Vec<Task>,
    pub edges: Vec<TaskDependency>,
    pub metadata: TaskGraphMeta,
}

/// Single task
pub struct Task {
    pub id: String,
    pub name: String,
    pub task_type: TaskType,
    pub model_preference: Option<String>,
    pub parameters: serde_json::Value,
    pub estimated_duration: Option<Duration>,
    pub status: TaskStatus,
}

/// Task dependency
pub struct TaskDependency {
    pub from: String,  // predecessor task id
    pub to: String,    // successor task id
}
```

### TaskType

```rust
pub enum TaskType {
    FileOperation(FileOp),
    CodeExecution(CodeExec),
    DocumentGeneration(DocGen),
    AppAutomation(AppAuto),
    AiInference(AiTask),
}

pub enum FileOp {
    Read { path: PathBuf },
    Write { path: PathBuf, content: Vec<u8> },
    Move { from: PathBuf, to: PathBuf },
    Search { pattern: String, dir: PathBuf },
    BatchMove { operations: Vec<(PathBuf, PathBuf)> },
}

pub enum CodeExec {
    Script { code: String, language: Language },
    File { path: PathBuf },
    Command { cmd: String, args: Vec<String> },
}

pub enum DocGen {
    Excel { template: Option<PathBuf>, data: Value },
    PowerPoint { template: Option<PathBuf>, slides: Vec<Slide> },
    Pdf { style: PdfStyle, content: String },
}
```

### TaskStatus

```rust
pub enum TaskStatus {
    Pending,
    Running { progress: f32, message: Option<String> },
    Completed { result: TaskResult },
    Failed { error: String, recoverable: bool },
    Cancelled,
}

pub struct TaskResult {
    pub output: Value,
    pub artifacts: Vec<PathBuf>,
    pub duration: Duration,
}
```

### Model Profiles

```rust
pub struct ModelProfile {
    pub name: String,
    pub provider: String,
    pub strengths: Vec<Capability>,
    pub cost_tier: CostTier,
    pub latency_tier: LatencyTier,
}

pub enum Capability {
    CodeGeneration,
    CodeReview,
    TextAnalysis,
    ImageUnderstanding,
    LongContext,
    Reasoning,
    LocalPrivacy,
    FastResponse,
}
```

---

## UI Design

### Interaction Flow

```
1. User input complex task
   ┌──────────────────────────────────────┐
   │  Organize my Downloads folder by     │
   │  type and generate a report          │
   └──────────────────────────────────────┘
                    │
                    ▼
2. Task plan confirmation (optional)
   ┌──────────────────────────────────────┐
   │  Task Plan:                          │
   │  • Scan ~/Downloads (1,234 files)    │
   │  • Categorize by file type           │
   │  • Generate Excel report             │
   │                                      │
   │  [Execute] [Modify] [Cancel]         │
   └──────────────────────────────────────┘
                    │
                    ▼
3. Background execution with Menu Bar indicator
   ┌───┐
   │ ◐ │ ← Animated progress indicator
   └───┘
                    │
                    ▼
4. Progress panel (click to expand)
   ┌──────────────────────────────────────┐
   │  Organizing Downloads                │
   │  ├─ ✅ Scan directory (1,234 files)  │
   │  ├─ ⏳ Categorizing files (45%)      │
   │  │   └─ Moving: photo_001.jpg        │
   │  ├─ ⏸ Generate report                │
   │  └─ Est. remaining: 2 min            │
   │                                      │
   │  [Pause] [Cancel] [Details]          │
   └──────────────────────────────────────┘
                    │
                    ▼
5. Completion notification
   ┌──────────────────────────────────────┐
   │  ✅ Task Complete                     │
   │  • Organized 1,234 files             │
   │  • Report: ~/Downloads/report.xlsx   │
   │                                      │
   │  [Open Report] [Open Folder] [Close] │
   └──────────────────────────────────────┘
```

### SwiftUI Components

```swift
// CoworkProgressPanel.swift
struct CoworkProgressPanel: View {
    @ObservedObject var taskGraph: TaskGraphViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Header
            HStack {
                Text(taskGraph.title)
                    .font(.headline)
                Spacer()
                ProgressRing(progress: taskGraph.overallProgress)
            }

            // Task list
            ForEach(taskGraph.tasks) { task in
                TaskRow(task: task)
            }

            // Controls
            HStack {
                Button("Pause") { taskGraph.pause() }
                Button("Cancel") { taskGraph.cancel() }
                Spacer()
                Text("Est: \(taskGraph.estimatedTimeRemaining)")
                    .foregroundColor(.secondary)
            }
        }
        .padding()
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }
}
```

---

## Implementation Roadmap

### Phase 1: Foundation

| Task | Description | Dependencies |
|------|-------------|--------------|
| Define TaskGraph structures | Rust structs + UniFFI export | None |
| Implement Task Planner | LLM-driven decomposition | Existing Provider |
| Implement DAG Scheduler | Topological sort + parallel scheduling | TaskGraph |
| Implement Task Monitor | Status tracking + callbacks | UniFFI |
| Progress Panel UI | SwiftUI progress panel | Monitor |

### Phase 2: File Operations

| Task | Description | Dependencies |
|------|-------------|--------------|
| FileOps Executor | Read/write/move/search | Phase 1 |
| Permission control | Path whitelist + confirmation | FileOps |
| Batch optimization | Parallel IO + progress | FileOps |
| Native Tool registration | Integrate with ToolRegistry | Existing tools |

### Phase 3: Advanced Execution

| Task | Description | Dependencies |
|------|-------------|--------------|
| Code Executor (Direct) | Direct Python/Shell execution | Phase 2 |
| Code Executor (Sandbox) | Docker/WASM isolation | Direct Executor |
| Document Generator | MCP integration | Existing MCP |
| App Automation | AppleScript + Accessibility | Phase 1 |

### Phase 4: Multi-Model

| Task | Description | Dependencies |
|------|-------------|--------------|
| Model Profiles config | Capability tagging system | Existing Provider |
| Model Matcher | Task-model matching algorithm | Profiles |
| Pipeline Executor | Multi-model chained execution | Matcher |
| Memory integration | Cross-task context passing | Existing Memory |

---

## Risk Assessment

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Sandbox execution complexity | Medium | High | Implement Direct mode first, sandbox as optional |
| Document generation dependencies | Low | Medium | Use MCP servers (Context7, etc.) |
| Multi-model cost | Medium | Medium | Cost-aware routing, user-controlled |
| Long-running task stability | Medium | High | Implement checkpoint/resume mechanism |
| File operation security | Low | High | Strict permission model, confirmation dialogs |

---

## Configuration

```toml
# config.toml
[cowork]
enabled = true
require_confirmation = true  # Confirm before executing

[cowork.file_ops]
allowed_paths = ["~/Downloads", "~/Documents"]
denied_paths = ["~/.ssh", "~/.gnupg"]
max_file_size = "100MB"

[cowork.code_exec]
sandbox_strategy = "direct"  # direct | docker | wasm
allowed_languages = ["python", "shell", "javascript"]

[cowork.model_routing]
code_generation = "claude-sonnet"
code_review = "claude-sonnet"
image_analysis = "gpt-4o"
video_understanding = "gemini-pro"
long_document = "gemini-pro"
quick_tasks = "claude-haiku"
privacy_sensitive = "ollama/llama3"
reasoning = "claude-opus"

[cowork.model_routing.overrides]
# User-specific overrides
# code_generation = "gpt-4-turbo"
```

---

## Comparison: Claude Cowork vs Aether Cowork

| Aspect | Claude Cowork | Aether Cowork |
|--------|--------------|---------------|
| Execution | Remote VM sandbox | Local execution + optional sandbox |
| AI Models | Claude only | Multi-model orchestration |
| Privacy | Files uploaded to cloud | Local processing, privacy-first |
| Memory | No cross-session | Persistent Memory integration |
| Integration | Standalone app | macOS system-level integration |
| Cost | Max plan required | Pay-per-use API |
| Extensibility | Limited | MCP + Skills + Native Tools |

---

## References

- [Claude Cowork Documentation](https://support.claude.com/en/articles/13345190-getting-started-with-cowork)
- [Aether Architecture](./ARCHITECTURE.md)
- [Aether MCP Integration](./MCP_INTEGRATION.md)
- [Aether Skills System](./SKILLS.md)

---

**Last Updated**: 2026-01-15
**Next Steps**: Create OpenSpec change proposal for Phase 1 implementation

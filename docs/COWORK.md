# Cowork: Task Orchestration System

Cowork is Aether's multi-task orchestration system that decomposes complex user requests into DAG-structured task graphs and executes them with parallel scheduling.

## Overview

When users submit complex requests like "Help me write a report about climate change with charts and export to PDF", Cowork:

1. **Plans** - Uses LLM to decompose the request into discrete tasks
2. **Validates** - Shows the task graph for user confirmation
3. **Executes** - Runs tasks in parallel where dependencies allow
4. **Reports** - Provides real-time progress and final summary

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         CoworkEngine                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │  TaskPlanner │  │ DAGScheduler │  │  ExecutorRegistry    │   │
│  │  (LLM-based) │  │  (topo sort) │  │  (extensible)        │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ TaskMonitor  │  │  TaskGraph   │  │  CoworkConfig        │   │
│  │  (progress)  │  │  (DAG model) │  │  (settings)          │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Swift UI Layer                             │
│  ┌────────────────────┐  ┌────────────────────────────────────┐ │
│  │ CoworkConfirmation │  │ CoworkProgressView                 │ │
│  │ View (DAG preview) │  │ (real-time task status)            │ │
│  └────────────────────┘  └────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. Task Data Structures (`cowork/types/`)

**Task** - A single unit of work:
```rust
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub progress: f32,
    pub dependencies: Vec<String>,
}
```

**TaskType** - Categories of tasks:
- `FileOperation` - Read, write, copy, delete files
- `CodeExecution` - Run scripts or code
- `DocumentGeneration` - Create documents, reports
- `AppAutomation` - Control applications via AppleScript
- `AiInference` - Call AI models for generation

**TaskGraph** - DAG structure containing tasks and edges:
```rust
pub struct TaskGraph {
    pub id: String,
    pub title: String,
    pub original_request: Option<String>,
    pub tasks: Vec<Task>,
    pub edges: Vec<TaskDependency>,
}
```

### 2. Task Planner (`cowork/planner/`)

Uses LLM to decompose complex requests into task graphs.

**Input**: Natural language request
**Output**: Structured `TaskGraph` with dependencies

Example decomposition:
```
User: "Create a summary of sales.csv and email it to the team"

Tasks:
  1. read_file (FileOperation) - Read sales.csv
  2. analyze_data (AiInference) - Analyze the data [depends: 1]
  3. generate_summary (DocumentGeneration) - Create summary [depends: 2]
  4. send_email (AppAutomation) - Send via Mail.app [depends: 3]
```

### 3. DAG Scheduler (`cowork/scheduler/`)

Executes tasks respecting dependencies with configurable parallelism.

**Algorithm**:
1. Compute in-degree for all tasks
2. Queue tasks with zero in-degree
3. Execute up to `max_parallelism` tasks concurrently
4. When task completes, decrement dependents' in-degree
5. Queue newly ready tasks
6. Repeat until all complete or failure

**Features**:
- Topological sort validation
- Cycle detection
- Parallel execution with semaphore
- Failure propagation to dependents

### 4. Task Monitor (`cowork/monitor/`)

Real-time progress tracking with subscription support.

**Events**:
```rust
pub enum CoworkProgressEventType {
    GraphCreated,
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    GraphCompleted,
    GraphFailed,
}
```

**Subscription API**:
```rust
// Rust
engine.subscribe(|event| {
    println!("Task {} progress: {}%", event.task_id, event.progress * 100.0);
});

// Swift (via UniFFI)
core.coworkSubscribe(handler: MyProgressHandler())
```

### 5. Executor Registry (`cowork/executor/`)

Extensible system for task execution.

**Executor Trait**:
```rust
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    fn can_execute(&self, task_type: &TaskType) -> bool;
    async fn execute(&self, task: &Task, context: &ExecutionContext) -> TaskResult;
}
```

**Built-in Executors**:
- `NoopExecutor` - For testing, simulates work with configurable delay

**Adding Custom Executors**:
```rust
let registry = ExecutorRegistry::new();
registry.register(Arc::new(MyCustomExecutor::new()));
```

### 6. CoworkEngine (`cowork/engine.rs`)

Unified API for the entire system.

```rust
impl CoworkEngine {
    // Planning
    pub async fn plan(&self, request: &str) -> Result<TaskGraph>;

    // Execution
    pub async fn execute(&self, graph: &TaskGraph) -> Result<ExecutionSummary>;

    // Control
    pub fn pause(&self);
    pub fn resume(&self);
    pub fn cancel(&self);

    // Monitoring
    pub fn subscribe<F>(&self, callback: F);
    pub fn get_state(&self) -> CoworkExecutionState;
}
```

## Configuration

### TOML Configuration (`config.toml`)

```toml
[cowork]
# Enable/disable Cowork
enabled = true

# Require user confirmation before executing task graphs
require_confirmation = true

# Maximum parallel tasks (1-32)
max_parallelism = 4

# Plan but don't execute (for testing)
dry_run = false

# AI provider for task planning (optional, uses default if not set)
planner_provider = "claude"

# Confidence threshold for auto-execution (0.0-1.0)
auto_execute_threshold = 0.95

# Maximum tasks allowed in a single graph
max_tasks_per_graph = 20

# Task execution timeout in seconds (0 = no timeout)
task_timeout_seconds = 300

# Enable sandboxed execution for code tasks
sandbox_enabled = true

# Allowed task categories (empty = all allowed)
allowed_categories = []

# Blocked task categories (takes precedence over allowed)
blocked_categories = []
```

### Valid Categories

- `file_operation` - File system operations
- `code_execution` - Running code/scripts
- `document_generation` - Creating documents
- `app_automation` - Controlling applications
- `ai_inference` - AI model calls

### File Operations Configuration

```toml
[cowork.file_ops]
# Enable file operations executor
enabled = true

# Paths allowed for file operations (glob patterns)
# Empty = all paths allowed (except denied)
allowed_paths = ["~/Downloads/**", "~/Documents/**"]

# Paths denied for file operations (takes precedence)
# Default denied paths (~/.ssh, ~/.gnupg, etc.) are always applied
denied_paths = []

# Maximum file size for read operations (human-readable)
max_file_size = "100MB"

# Require confirmation before write operations
require_confirmation_for_write = true

# Require confirmation before delete operations
require_confirmation_for_delete = true
```

### Default Denied Paths

For security, these paths are always denied regardless of configuration:

- `~/.ssh/**` - SSH keys
- `~/.gnupg/**` - GPG keys
- `~/.config/aether/**` - Aether config
- `~/.aws/**` - AWS credentials
- `~/.kube/**` - Kubernetes config
- `/etc/passwd` - System password file
- `/etc/shadow` - System shadow file
- `/etc/sudoers` - Sudo configuration

### Code Execution Configuration

```toml
[cowork.code_exec]
# Enable code execution (DISABLED by default for security)
enabled = false

# Default runtime for code execution
default_runtime = "shell"

# Execution timeout in seconds
timeout_seconds = 60

# Enable sandboxed execution (macOS sandbox-exec)
sandbox_enabled = true

# Allowed runtimes (empty = all)
allowed_runtimes = ["shell", "python"]

# Allow network access in sandbox
allow_network = false

# Working directory for executions
working_directory = "~/Downloads"

# Environment variables to pass to executed code
pass_env = ["PATH", "HOME", "USER"]

# Blocked command patterns (regex)
blocked_commands = ["rm\\s+-rf\\s+/", "sudo\\s+"]
```

### Supported Runtimes

| Runtime | Command | Use Case |
|---------|---------|----------|
| Shell | `bash`, `zsh` | System commands, file processing |
| Python | `python3` | Data analysis, scripts |
| Node.js | `node` | Web-related, JSON processing |
| Ruby | `ruby` | Scripting |

### Default Blocked Commands

For security, these command patterns are always blocked:

- `rm -rf /` - Recursive delete root
- `rm -rf ~` - Recursive delete home
- `sudo *` - Privilege escalation
- `chmod 777 /` - Dangerous permissions
- Fork bomb patterns
- Disk overwrite commands (`dd`, `mkfs`)

### Settings UI

Access via: **Settings → Cowork**

| Setting | Description |
|---------|-------------|
| Enable Cowork | Master switch for task orchestration |
| Require Confirmation | Show task preview before execution |
| Max Parallelism | Concurrent task limit (1-16 in UI) |
| Dry Run Mode | Plan without executing |

## Swift Integration

### UniFFI Bindings

```swift
// Get current config
let config = core.coworkGetConfig()

// Update config
let newConfig = CoworkConfigFfi(
    enabled: true,
    requireConfirmation: true,
    maxParallelism: 4,
    dryRun: false
)
try core.coworkUpdateConfig(config: newConfig)

// Plan a request
let graph = try core.coworkPlan(request: "Complex task...")

// Execute with progress monitoring
core.coworkSubscribe(handler: progressHandler)
let summary = try await core.coworkExecute(graph: graph)

// Control execution
core.coworkPause()
core.coworkResume()
core.coworkCancel()
```

### UI Components

**CoworkConfirmationView** - Shows task graph for approval:
- DAG visualization with task nodes
- Task details (name, type, dependencies)
- Execute/Cancel buttons
- Safety level indicator

**CoworkProgressView** - Real-time execution status:
- Overall progress bar
- Per-task status indicators
- Error display
- Pause/Resume/Cancel controls

### HaloState Integration

```swift
// Show confirmation
HaloState.coworkConfirmation(graph: taskGraph, onExecute: {...}, onCancel: {...})

// Show progress
HaloState.coworkProgress(graph: taskGraph, currentTask: "task_1", progress: 0.5)
```

## Example Flow

1. **User Input**: "Create a presentation about Q4 sales with charts"

2. **Planning** (LLM decomposition):
   ```
   TaskGraph {
     tasks: [
       Task { id: "1", name: "Read Q4 data", type: FileOperation },
       Task { id: "2", name: "Analyze trends", type: AiInference, deps: ["1"] },
       Task { id: "3", name: "Generate charts", type: DocumentGeneration, deps: ["2"] },
       Task { id: "4", name: "Create slides", type: DocumentGeneration, deps: ["2", "3"] },
       Task { id: "5", name: "Export PDF", type: FileOperation, deps: ["4"] }
     ]
   }
   ```

3. **Confirmation**: User reviews and approves the task graph

4. **Execution**:
   - Task 1 runs immediately (no dependencies)
   - Task 2 starts after Task 1 completes
   - Tasks 3 and 4 can run in parallel after Task 2
   - Task 5 runs after Task 4

5. **Result**: PDF presentation delivered to user

## Testing

Run all Cowork tests:
```bash
cd Aether/core
cargo test cowork
```

Test categories:
- `cowork::types` - Data structure tests
- `cowork::planner` - Planning/decomposition tests
- `cowork::scheduler` - DAG scheduling tests
- `cowork::executor` - Executor registry tests
- `cowork::monitor` - Progress monitoring tests
- `cowork::engine` - Integration tests
- `cowork_ffi` - UniFFI binding tests
- `config::types::cowork` - Configuration tests

## Multi-Model Router (Phase 4)

Cowork includes a sophisticated Multi-Model Router that intelligently routes tasks to the most appropriate AI model based on task type, required capabilities, and cost optimization strategy.

### Overview

The Model Router enables:
- **Task-Specific Routing**: Route code generation to Opus, quick tasks to Haiku
- **Capability Matching**: Find models with specific capabilities (vision, long context, local privacy)
- **Cost Optimization**: Balance between quality and cost with configurable strategies
- **Multi-Model Pipelines**: Execute complex workflows across multiple models

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Model Router                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ ModelMatcher │  │ PipelineExec │  │  ContextManager      │   │
│  │ (routing)    │  │ (orchestrate)│  │  (cross-model)       │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ ModelProfile │  │ RoutingRules │  │  CostStrategy        │   │
│  │ (definition) │  │ (mappings)   │  │  (optimization)      │   │
│  └──────────────┘  └──────────────┘  └──────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

#### 1. Model Profiles (`model_router/profiles.rs`)

Define AI model characteristics:

```rust
pub struct ModelProfile {
    pub id: String,                    // Unique identifier
    pub provider: String,              // anthropic, openai, google, ollama
    pub model: String,                 // Model name for API
    pub capabilities: Vec<Capability>, // What the model can do
    pub cost_tier: CostTier,           // free, low, medium, high
    pub latency_tier: LatencyTier,     // fast, medium, slow
    pub max_context: Option<u32>,      // Max context window
    pub local: bool,                   // Local model (no API calls)
}
```

**Capabilities**:
- `CodeGeneration` - Code writing and generation
- `CodeReview` - Code analysis and review
- `TextAnalysis` - Text understanding
- `ImageUnderstanding` - Vision/image analysis
- `VideoUnderstanding` - Video analysis
- `LongContext` - Large context windows (100K+)
- `Reasoning` - Complex reasoning tasks
- `LocalPrivacy` - Local/private processing
- `FastResponse` - Quick response times
- `SimpleTask` - Simple/lightweight tasks
- `LongDocument` - Document processing

**Cost Tiers**: `Free` (0.0x), `Low` (0.5x), `Medium` (1.0x), `High` (2.0x)

**Latency Tiers**: `Fast` (<1s), `Medium` (1-5s), `Slow` (>5s)

#### 2. Model Matcher (`model_router/matcher.rs`)

Intelligent model selection:

```rust
impl ModelMatcher {
    // Find best model for a task type
    pub fn route_by_task_type(&self, task_type: &str) -> Option<&ModelProfile>;

    // Find models with specific capability
    pub fn find_by_capability(&self, cap: Capability) -> Vec<&ModelProfile>;

    // Cost-optimized selection
    pub fn find_cheapest_with_capability(&self, cap: Capability) -> Option<&ModelProfile>;
    pub fn find_best_for_capability(&self, cap: Capability) -> Option<&ModelProfile>;

    // Full routing with strategy
    pub fn route(&self, requirements: &TaskRequirements) -> Option<&ModelProfile>;
}
```

**Routing Priority**:
1. Explicit task type mapping (code_generation → claude-opus)
2. Required capability matching
3. Cost strategy application (cheapest/balanced/best_quality)
4. Default model fallback

#### 3. Cost Strategy (`model_router/rules.rs`)

Three optimization strategies:

| Strategy | Description | Use Case |
|----------|-------------|----------|
| `Cheapest` | Minimize cost | Development, high volume |
| `Balanced` | Cost/quality balance | Production default |
| `BestQuality` | Maximize quality | Critical tasks |

#### 4. Pipeline Executor (`model_router/pipeline.rs`)

Execute multi-stage workflows:

```rust
pub struct PipelineStage {
    pub id: String,
    pub name: String,
    pub model_requirement: ModelRequirement,
    pub dependencies: Vec<String>,
    pub priority: u32,
}

pub struct PipelineExecutor {
    // Execute stages respecting dependencies
    pub async fn execute(&self, pipeline: Pipeline) -> PipelineResult;
}
```

**Features**:
- DAG-based stage execution
- Parallel execution where dependencies allow
- Cross-stage context passing
- Progress events and monitoring

#### 5. Context Manager (`model_router/context.rs`)

Manages context across pipeline stages:

```rust
impl ContextManager {
    // Store result from completed stage
    pub fn store_result(&self, stage_id: &str, result: StageResult);

    // Get context for dependent stage
    pub fn get_context(&self, stage_id: &str, dependencies: &[String]) -> TaskContext;

    // Build combined context from multiple results
    pub fn build_combined_context(&self, stage_ids: &[String]) -> String;
}
```

### Configuration

#### Model Profiles

```toml
[cowork.model_profiles.claude-opus]
provider = "anthropic"
model = "claude-opus-4"
capabilities = ["reasoning", "code_generation", "long_context"]
cost_tier = "high"
latency_tier = "slow"
max_context = 200000

[cowork.model_profiles.claude-sonnet]
provider = "anthropic"
model = "claude-sonnet-4"
capabilities = ["code_generation", "code_review", "text_analysis"]
cost_tier = "medium"
latency_tier = "medium"
max_context = 200000

[cowork.model_profiles.claude-haiku]
provider = "anthropic"
model = "claude-haiku-3.5"
capabilities = ["fast_response", "simple_task"]
cost_tier = "low"
latency_tier = "fast"
max_context = 200000

[cowork.model_profiles.gpt-4o]
provider = "openai"
model = "gpt-4o"
capabilities = ["image_understanding", "code_generation"]
cost_tier = "medium"
latency_tier = "medium"

[cowork.model_profiles.ollama-llama]
provider = "ollama"
model = "llama3.2"
capabilities = ["local_privacy", "fast_response"]
cost_tier = "free"
latency_tier = "fast"
local = true
```

#### Routing Rules

```toml
[cowork.model_routing]
# Task type to model mappings
code_generation = "claude-opus"
code_review = "claude-sonnet"
image_analysis = "gpt-4o"
video_understanding = "gemini-pro"
long_document = "gemini-pro"
quick_tasks = "claude-haiku"
privacy_sensitive = "ollama-llama"
reasoning = "claude-opus"

# Cost optimization strategy: cheapest | balanced | best_quality
cost_strategy = "balanced"

# Enable multi-model pipeline execution
enable_pipelines = true

# Default model when no specific rule matches
default_model = "claude-sonnet"
```

### Swift Integration

#### UniFFI API

```swift
// Get all model profiles
let profiles = core.coworkGetModelProfiles()

// Get routing rules
let rules = core.coworkGetRoutingRules()

// Update model profile
let profile = ModelProfileFfi(
    id: "claude-opus",
    provider: "anthropic",
    model: "claude-opus-4",
    capabilities: [.codeGeneration, .reasoning, .longContext],
    costTier: .high,
    latencyTier: .slow,
    maxContext: 200000,
    local: false
)
try core.coworkUpdateModelProfile(profile: profile)

// Delete model profile
try core.coworkDeleteModelProfile(profileId: "old-profile")

// Update routing rule
try core.coworkUpdateRoutingRule(taskType: "code_generation", modelId: "claude-opus")

// Delete routing rule
try core.coworkDeleteRoutingRule(taskType: "code_generation")

// Update cost strategy
try core.coworkUpdateCostStrategy(strategy: .balanced)

// Update default model
try core.coworkUpdateDefaultModel(modelId: "claude-sonnet")
```

#### Settings UI

Access via: **Settings → Cowork → Model Routing**

**Model Profiles View** (`ModelProfilesSettingsView.swift`):
- List all configured model profiles
- View capabilities, cost/latency tiers
- Add, edit, and delete profiles

**Routing Settings View** (`ModelRoutingSettingsView.swift`):
- Configure cost strategy (Cheapest/Balanced/Best Quality)
- Set default model
- Enable/disable pipelines
- Map task types to specific models

### Example Workflow

```
User: "Review this PR and write unit tests for the changes"

1. Task Decomposition:
   - Task 1: Read PR files (FileOperation)
   - Task 2: Review code (AiInference, type: code_review)
   - Task 3: Generate tests (AiInference, type: code_generation)

2. Model Routing:
   - Task 2 → claude-sonnet (code_review mapping)
   - Task 3 → claude-opus (code_generation mapping)

3. Pipeline Execution:
   - Stage 1: claude-sonnet reviews code, outputs findings
   - Stage 2: claude-opus generates tests using review context

4. Context Flow:
   - Review results passed to test generation stage
   - Context manager tracks cross-stage dependencies
```

### Testing

```bash
# Run all model router tests
cargo test model_router

# Test categories (71 tests total):
# - profiles: ModelProfile, Capability, CostTier tests
# - rules: RoutingRules, CostStrategy tests
# - matcher: ModelMatcher routing tests
# - pipeline: PipelineExecutor, PipelineStage tests
# - context: ContextManager, TaskContext tests
```

---

## Future Enhancements

### Phase 2 - File Operations (Complete)

- ✅ FileOpsExecutor implementation (Read, Write, Move, Copy, Delete, Search, List)
- ✅ Permission system with allowed/denied paths
- ✅ Glob pattern support for path matching
- ✅ Path canonicalization (resolve symlinks, ~, ..)
- ✅ Default denied paths for security
- ✅ Configuration types (FileOpsConfigToml)
- ✅ Integration with CoworkEngine

### Phase 3 - Code Execution (Complete)

- ✅ CodeExecutor implementation (Shell, Python, Node.js)
- ✅ RuntimeInfo for detecting available runtimes
- ✅ CommandChecker for blocking dangerous commands
- ✅ SandboxConfig for macOS sandbox-exec profiles
- ✅ Timeout handling with process cleanup
- ✅ Output capture with size limits (10MB stdout, 1MB stderr)
- ✅ Configuration types (CodeExecConfigToml)
- ✅ Integration with CoworkEngine
- ✅ Swift UI settings for code_exec
- ✅ 73 tests passing

### Phase 4 - Multi-Model Router (Complete)

- ✅ ModelProfile data structures with capabilities
- ✅ ModelMatcher for intelligent routing
- ✅ CostStrategy optimization (cheapest/balanced/best_quality)
- ✅ PipelineExecutor for multi-model workflows
- ✅ ContextManager for cross-stage context
- ✅ Configuration types (ModelProfileConfigToml, ModelRoutingConfigToml)
- ✅ UniFFI exports for Swift integration
- ✅ Settings UI (ModelProfilesSettingsView, ModelRoutingSettingsView)
- ✅ 71 tests passing

### Phase 5 (Planned)
- Visual DAG editor in UI
- Custom task type definitions
- Workflow templates
- Multi-graph orchestration
- Checkpoint/resume for long-running graphs
- Task result caching

## Files Reference

| Path | Description |
|------|-------------|
| `core/src/cowork/mod.rs` | Module exports |
| `core/src/cowork/types/` | Task, TaskGraph, TaskResult |
| `core/src/cowork/planner/` | LLM-based task planning |
| `core/src/cowork/scheduler/` | DAG scheduler |
| `core/src/cowork/executor/` | Executor trait and registry |
| `core/src/cowork/executor/file_ops.rs` | File operations executor |
| `core/src/cowork/executor/code_exec.rs` | Code execution executor |
| `core/src/cowork/executor/permission.rs` | Path permission checking |
| `core/src/cowork/monitor/` | Progress events and tracking |
| `core/src/cowork/engine.rs` | CoworkEngine unified API |
| `core/src/cowork/model_router/` | Multi-model router module |
| `core/src/cowork/model_router/profiles.rs` | ModelProfile, Capability, CostTier |
| `core/src/cowork/model_router/rules.rs` | RoutingRules, CostStrategy |
| `core/src/cowork/model_router/matcher.rs` | ModelMatcher routing logic |
| `core/src/cowork/model_router/pipeline.rs` | PipelineExecutor, PipelineStage |
| `core/src/cowork/model_router/context.rs` | ContextManager, TaskContext |
| `core/src/cowork_ffi.rs` | UniFFI type conversions |
| `core/src/config/types/cowork.rs` | Configuration types |
| `Sources/CoworkSettingsView.swift` | Settings UI |
| `Sources/ModelProfilesSettingsView.swift` | Model profiles settings |
| `Sources/ModelProfileEditSheet.swift` | Model profile edit form |
| `Sources/ModelRoutingSettingsView.swift` | Routing rules settings |
| `Sources/Components/CoworkConfirmationView.swift` | Confirmation UI |
| `Sources/Components/CoworkProgressView.swift` | Progress UI |

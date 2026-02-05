# Agent System

> Core agent loop, thinker, and dispatcher architecture

---

## Overview

The Agent System implements the **Observe-Think-Act-Feedback (OTAF)** loop, the heart of Aleph's intelligence.

```
┌─────────────────────────────────────────────────────────────────┐
│                        Agent Loop                                │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌──────────┐     ┌──────────┐     ┌──────────┐               │
│   │ OBSERVE  │ ──▶ │  THINK   │ ──▶ │   ACT    │               │
│   │          │     │          │     │          │               │
│   │ • Input  │     │ • LLM    │     │ • Tools  │               │
│   │ • Memory │     │ • Decide │     │ • Execute│               │
│   │ • Context│     │ • Plan   │     │ • Output │               │
│   └──────────┘     └──────────┘     └──────────┘               │
│        ▲                                  │                     │
│        │           ┌──────────┐           │                     │
│        └────────── │ FEEDBACK │ ◀─────────┘                     │
│                    │          │                                  │
│                    │ • Eval   │                                  │
│                    │ • Learn  │                                  │
│                    │ • Compress│                                 │
│                    └──────────┘                                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Agent Loop

**Location**: `core/src/agent_loop/`

### Core Structure

```rust
pub struct AgentLoop<T, E, C> {
    thinker: Arc<T>,              // LLM decision maker
    executor: Arc<E>,              // Tool executor
    compressor: Arc<C>,            // Context compressor
    pub config: LoopConfig,        // Loop configuration
    overflow_detector: Option<Arc<OverflowDetector>>,
}
```

### Key Components

| Component | File | Purpose |
|-----------|------|---------|
| `AgentLoop` | `agent_loop.rs` | Main loop controller |
| `LoopConfig` | `config.rs` | Configuration options |
| `LoopState` | `state.rs` | State machine |
| `LoopGuards` | `guards.rs` | Pre-execution safety checks |
| `OverflowDetector` | `overflow.rs` | Context window overflow detection |
| `LoopCallback` | `callback.rs` | Event callbacks |
| `MessageBuilder` | `message_builder/` | Prompt construction |
| `SessionSync` | `session_sync.rs` | Session persistence |

### State Machine

```
┌─────────┐
│  IDLE   │
└────┬────┘
     │ start()
     ▼
┌─────────┐     ┌─────────┐     ┌─────────┐
│OBSERVING│ ──▶ │THINKING │ ──▶ │ ACTING  │
└─────────┘     └────┬────┘     └────┬────┘
                     │               │
                     │ no_action     │ tool_result
                     ▼               ▼
               ┌─────────┐     ┌─────────┐
               │RESPONDING│◀───│EVALUATING│
               └────┬────┘     └─────────┘
                    │
                    ▼
               ┌─────────┐
               │COMPRESSING│
               └────┬────┘
                    │
                    ▼
               ┌─────────┐
               │COMPLETED │
               └─────────┘
```

### Loop Events

```rust
pub enum LoopEvent {
    Started { run_id: String },
    ThinkingStarted,
    ThinkingComplete { decision: Decision },
    ToolExecutionStarted { tool_name: String },
    ToolExecutionComplete { result: ToolResult },
    StreamChunk { content: String },
    OverflowDetected { tokens: usize },
    CompressionStarted,
    CompressionComplete,
    Completed { response: String },
    Error { error: String },
}
```

---

## Thinker

**Location**: `core/src/thinker/`

The Thinker is responsible for LLM interactions and decision making.

### Components

| Component | File | Purpose |
|-----------|------|---------|
| `Thinker` | `mod.rs` | Main thinker interface |
| `PromptBuilder` | `prompt_builder.rs` | Construct prompts from context |
| `DecisionParser` | `decision_parser.rs` | Parse LLM responses |
| `ModelRouter` | `model_router.rs` | Select optimal model |
| `ToolFilter` | `tool_filter.rs` | Filter available tools |
| `StreamingHandler` | `streaming/` | Handle streaming responses |
| `InteractionManifest` | `interaction.rs` | Channel capability awareness |
| `SecurityContext` | `security_context.rs` | Policy-driven permissions |
| `ContextAggregator` | `context.rs` | Reconcile interaction and security |
| `SoulManifest` | `soul.rs` | Identity/personality definition |
| `IdentityResolver` | `identity.rs` | Layered identity resolution |

### Thinking Levels

```rust
pub enum ThinkingLevel {
    Off,        // No extended thinking
    Minimal,    // budget_tokens: 1024
    Low,        // budget_tokens: 2048
    Medium,     // budget_tokens: 4096 (default)
    High,       // budget_tokens: 8192
    XHigh,      // budget_tokens: 16384
}
```

### Provider Fallback

When a provider doesn't support extended thinking, Aleph falls back gracefully:

```
User requests: thinking = High
    │
    ├─▶ Claude Opus → ✓ Native extended thinking
    │
    ├─▶ GPT-4o → ✗ No support → Fallback to o1
    │
    └─▶ Gemini → ✗ No support → Use thinkingPreface prompt
```

### Streaming Architecture

```
LLM Response Stream
    │
    ▼
┌─────────────────────────────────────────┐
│ BlockStateManager                        │
│   • Track current block type             │
│   • Detect block boundaries              │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ BlockReplyChunker                        │
│   • Split into semantic chunks           │
│   • Handle code blocks, lists, etc.      │
└─────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────┐
│ BlockCoalescer                           │
│   • Merge small chunks                   │
│   • Emit complete blocks                 │
└─────────────────────────────────────────┘
    │
    ▼
Event: StreamChunk { content, block_type }
```

---

## Channel Capability Awareness

**Location**: `core/src/thinker/` (interaction.rs, security_context.rs, context.rs)

Aleph's Thinker uses a two-layer context system to adapt AI behavior based on the current environment.

### InteractionManifest

Describes what the channel can technically do:

```rust
InteractionManifest {
    paradigm: InteractionParadigm::WebRich,
    capabilities: [MultiGroupUI, Streaming, MermaidCharts, Canvas],
    constraints: { max_output_chars: None, supports_streaming: true }
}
```

**Paradigms**:

| Paradigm | Description | Default Capabilities |
|----------|-------------|---------------------|
| `CLI` | Terminal interface | RichText, CodeHighlight, Streaming |
| `WebRich` | Full web interface | All capabilities including Canvas |
| `Messaging` | Chat platforms | RichText, ImageInline |
| `Background` | Scheduled tasks | SilentReply |
| `Embedded` | Constrained env | None |

**Capabilities**: RichText, InlineButtons, MultiGroupUI, Streaming, ImageInline, MermaidCharts, CodeHighlight, FileUpload, Canvas, SilentReply

### SecurityContext

Orthogonal layer defining what policy allows:

```rust
SecurityContext {
    sandbox_level: SandboxLevel::Standard,
    filesystem_scope: Some("/workspace"),
    elevated_policy: ElevatedPolicy::Ask,
}
```

**Sandbox Levels**:

| Level | Description | Tool Impact |
|-------|-------------|-------------|
| `None` | Full access | All tools allowed |
| `Standard` | Limited filesystem/network | exec requires approval |
| `Strict` | Read-only operations | file_ops/exec blocked |
| `Untrusted` | Full isolation | Most tools blocked |

### ContextAggregator

Reconciles the two layers with a two-phase filtering approach:

```
Phase 1: Interaction Filter (Silent)
    └── Removes tools unsupported by channel
        └── AI doesn't know these tools exist

Phase 2: Security Filter (Transparent)
    └── Blocks/marks tools per policy
        └── AI knows "this tool requires approval" or "blocked by policy"
```

```rust
let resolved = ContextAggregator::resolve(&interaction, &security, &tools);
// resolved.available_tools    - tools ready to use
// resolved.disabled_tools     - tools with reasons (BlockedByPolicy, RequiresApproval)
// resolved.environment_contract - for system prompt generation
```

### Environment Contract in System Prompt

The resolved context feeds into PromptBuilder, generating an "Environment Contract" section:

```markdown
## Environment Contract

**Paradigm**: CLI (text-only terminal)

**Active Capabilities**:
- `rich_text`: You can use markdown formatting
- `code_highlight`: Code blocks will have syntax highlighting
- `streaming`: Responses will stream in real-time

**Constraints**:
- No multi-group UI available

## Security Notes

- Standard Sandbox Mode
- Filesystem scope: /workspace
- Shell execution requires user approval
```

### Terminal Decision Types

For background/scheduled tasks, two additional decision types:

| Decision | Use Case |
|----------|----------|
| `Silent` | Background task with nothing to report |
| `HeartbeatOk` | Confirmation that scheduled task is alive |

---

## Embodiment Engine

**Location**: `core/src/thinker/soul.rs`, `core/src/thinker/identity.rs`

The Embodiment Engine gives the AI a consistent identity and personality through layered soul definitions.

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    IdentityResolver                              │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │ Priority Stack                                           │    │
│  │   ┌─────────────┐                                        │    │
│  │   │  Session    │ ← Runtime override (highest)           │    │
│  │   ├─────────────┤                                        │    │
│  │   │  Project    │ ← .soul/identity.md                    │    │
│  │   ├─────────────┤                                        │    │
│  │   │  Global     │ ← ~/.aleph/soul.md                     │    │
│  │   ├─────────────┤                                        │    │
│  │   │  Default    │ ← Empty manifest (lowest)              │    │
│  │   └─────────────┘                                        │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### SoulManifest

```rust
pub struct SoulManifest {
    pub identity: String,           // Core identity statement
    pub voice: SoulVoice,           // Communication style
    pub directives: Vec<String>,    // Behavioral guidelines
    pub anti_patterns: Vec<String>, // What the AI should never do
    pub relationship: RelationshipMode, // User relationship type
    pub expertise: Vec<String>,     // Areas of expertise
    pub addendum: Option<String>,   // Custom additions
}

pub struct SoulVoice {
    pub tone: String,               // e.g., "friendly", "professional"
    pub verbosity: Verbosity,       // Minimal, Concise, Balanced, Verbose
    pub formatting_style: FormattingStyle, // Compact, Standard, Rich
    pub language_notes: Option<String>,
}
```

### Soul File Format (Markdown)

```markdown
---
relationship: mentor
expertise:
  - Rust
  - System design
---

# Identity

I am Aleph, your AI programming partner.

## Directives

- Be helpful and encouraging
- Explain concepts clearly
- Suggest best practices

## Anti-Patterns

- Never be condescending
- Never make up information
```

### RPC Methods

| Method | Description |
|--------|-------------|
| `identity.get` | Returns effective SoulManifest |
| `identity.set` | Sets session-level override |
| `identity.clear` | Clears session override |
| `identity.list` | Lists available identity sources |

---

## Chain-of-Thought Transparency

**Location**: `core/src/agent_loop/thinking.rs`

CoT Transparency parses LLM reasoning into structured, understandable steps.

### StructuredThinking

```rust
pub struct StructuredThinking {
    pub reasoning: String,          // Original raw reasoning
    pub steps: Vec<ReasoningStep>,  // Parsed semantic steps
    pub confidence: ConfidenceLevel,// Overall confidence
    pub alternatives: Vec<String>,  // Considered alternatives
    pub uncertainties: Vec<String>, // Expressed uncertainties
}

pub struct ReasoningStep {
    pub content: String,
    pub step_type: ReasoningStepType,
    pub confidence: Option<ConfidenceLevel>,
}
```

### Reasoning Step Types

| Type | Description | Indicator |
|------|-------------|-----------|
| `Observation` | Observing current state | "Looking at", "I see", "Based on" |
| `Analysis` | Analyzing options | "Considering", "Comparing", "Trade-off" |
| `Planning` | Planning approach | "I'll start by", "First...then" |
| `Decision` | Stating conclusion | "Therefore", "I will", "So I've decided" |
| `Reflection` | Self-review | "Wait", "Let me reconsider" |
| `RiskAssessment` | Identifying risks | "Risk", "Might fail", "Careful" |

### Confidence Levels

| Level | Indicators |
|-------|------------|
| `High` | "Confident", "Clearly", "Definitely" |
| `Medium` | "I think", "Should work", "Likely" |
| `Low` | "Not sure", "Might", "Possibly" |
| `Exploratory` | "Let's try", "Experiment", "Worth testing" |

### ThinkingParser

The `ThinkingParser` automatically extracts structured thinking from LLM reasoning:

```rust
// Automatically called by DecisionParser
let thinking = parser.parse(response)?;

// Access structured reasoning
if let Some(structured) = &thinking.structured {
    for step in &structured.steps {
        println!("{:?}: {}", step.step_type, step.content);
    }
}
```

### Stream Events

For real-time CoT visibility, the Gateway emits:

| Event | Description |
|-------|-------------|
| `ReasoningBlock` | Individual reasoning step |
| `UncertaintySignal` | Detected uncertainty with suggested action |

---

## Dispatcher

**Location**: `core/src/dispatcher/`

The Dispatcher orchestrates complex multi-step tasks using DAG-based scheduling.

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Dispatcher                               │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Analyzer   │ ──▶ │   Planner    │ ──▶ │  Scheduler   │    │
│  │              │     │              │     │              │    │
│  │ • Intent     │     │ • TaskGraph  │     │ • DAG exec   │    │
│  │ • Risk       │     │ • Dependencies│    │ • Parallel   │    │
│  │ • Category   │     │ • Priority   │     │ • Monitor    │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │  ToolFilter  │     │ Confirmation │     │   Executor   │    │
│  │              │     │              │     │              │    │
│  │ • Whitelist  │     │ • User ask   │     │ • Run tool   │    │
│  │ • Blacklist  │     │ • Auto-approve│    │ • Capture    │    │
│  │ • Smart      │     │ • Deny       │     │ • Timeout    │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Task Graph

```rust
pub struct TaskGraph {
    pub nodes: HashMap<TaskId, TaskNode>,
    pub edges: Vec<(TaskId, TaskId)>,  // dependency edges
}

pub struct TaskNode {
    pub id: TaskId,
    pub tool: String,
    pub args: Value,
    pub status: TaskStatus,
    pub dependencies: Vec<TaskId>,
}

pub enum TaskStatus {
    Pending,
    Running,
    Completed(Value),
    Failed(String),
    Cancelled,
}
```

### Execution Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| **Single-Step** | One tool call, immediate result | Simple queries |
| **Multi-Step** | Sequential tool chain | Complex tasks |
| **Parallel** | DAG with concurrent execution | Independent subtasks |
| **Sub-Agent** | Delegate to specialized agent | Domain expertise |

### Smart Filtering

```rust
pub struct SmartFilter {
    /// Tools always available
    pub always_allow: Vec<String>,

    /// Tools requiring confirmation
    pub require_confirmation: Vec<String>,

    /// Tools never available
    pub never_allow: Vec<String>,

    /// Context-based filtering
    pub context_rules: Vec<ContextRule>,
}
```

---

## Guards

**Location**: `core/src/agent_loop/guards.rs`

Safety checks before each loop iteration.

| Guard | Purpose |
|-------|---------|
| `MaxIterationsGuard` | Prevent infinite loops |
| `TokenBudgetGuard` | Enforce token limits |
| `TimeoutGuard` | Enforce time limits |
| `ToolRateLimitGuard` | Prevent tool spam |
| `ErrorAccumulatorGuard` | Stop on repeated errors |

```rust
pub trait LoopGuard: Send + Sync {
    fn check(&self, state: &LoopState) -> GuardResult;
    fn name(&self) -> &str;
}

pub enum GuardResult {
    Continue,
    Warn(String),
    Stop(String),
}
```

---

## Callback System

**Location**: `core/src/agent_loop/callback.rs`

```rust
#[async_trait]
pub trait LoopCallback: Send + Sync {
    async fn on_event(&self, event: LoopEvent);

    async fn on_user_question(
        &self,
        question: &UserQuestion,
    ) -> Option<String>;

    async fn on_confirmation(
        &self,
        request: &ConfirmationRequest,
    ) -> bool;
}
```

### CLI Callback

```rust
pub struct CliCallback {
    // Uses `inquire` crate for interactive prompts
}

impl LoopCallback for CliCallback {
    async fn on_user_question(&self, q: &UserQuestion) -> Option<String> {
        // Display question with inquire::Text or inquire::Select
    }
}
```

---

## Sub-Agent Delegation

**Location**: `core/src/agents/sub_agents/`

Main agent can spawn sub-agents for specialized tasks:

```
Main Agent (claude-opus-4)
    │
    ├─── Translator Sub-Agent (claude-haiku)
    │       Session: subagent:agent:main:translator
    │
    ├─── Code Reviewer Sub-Agent (claude-sonnet)
    │       Session: subagent:agent:main:code-reviewer
    │
    └─── Research Sub-Agent (gpt-4o)
            Session: subagent:agent:main:researcher
```

### Session Key Nesting

```rust
SessionKey::Subagent {
    parent: Box::new(SessionKey::Main { agent_id }),
    subagent_id: "translator".into(),
}
// Serializes to: "subagent:agent:main:translator"
```

---

## Configuration

```rust
pub struct LoopConfig {
    /// Maximum iterations per run
    pub max_iterations: usize,

    /// Token budget for context
    pub token_budget: usize,

    /// Timeout per iteration
    pub iteration_timeout: Duration,

    /// Enable context compression
    pub enable_compression: bool,

    /// Compression threshold (tokens)
    pub compression_threshold: usize,

    /// Model routing strategy
    pub model_routing: ModelRoutingConfig,

    /// Thinking level
    pub thinking_level: ThinkingLevel,
}
```

---

## Multi-Agent Resilience

**Location**: `core/src/memory/database/resilience/`

The Multi-Agent Resilience architecture provides robust task recovery, event persistence, session management, and resource governance for long-running multi-agent workflows.

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    Multi-Agent Resilience                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Recovery   │     │  Perception  │     │ Collaboration│    │
│  │              │     │              │     │              │    │
│  │ • Shadow     │     │ • Classifier │     │ • Handles    │    │
│  │   Replay     │     │ • Emitter    │     │ • Swapping   │    │
│  │ • Graceful   │     │ • Observer   │     │ • Coordinator│    │
│  │   Shutdown   │     │ • Gap-Fill   │     │              │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │  Governance  │     │    Types     │     │   Database   │    │
│  │              │     │              │     │              │    │
│  │ • Governor   │     │ • AgentTask  │     │ • Tasks CRUD │    │
│  │ • Sentry     │     │ • TaskTrace  │     │ • Traces     │    │
│  │ • Quotas     │     │ • AgentEvent │     │ • Events     │    │
│  │              │     │ • Session    │     │ • Sessions   │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Recovery Layer

**Location**: `core/src/memory/database/resilience/recovery/`

Handles task recovery after system restarts or crashes.

#### Shadow Replay Engine

Deterministic task recovery without LLM token consumption:

```rust
pub struct ShadowReplayEngine {
    db: Arc<VectorDatabase>,
}

impl ShadowReplayEngine {
    /// Replay all traces for a task
    pub async fn replay_task(&self, task_id: &str) -> Result<ReplayResult, AlephError>;

    /// Replay until a specific step
    pub async fn replay_until_step(&self, task_id: &str, step: u32) -> Result<ReplayResult, AlephError>;

    /// Check for divergence from recorded trace
    pub async fn check_divergence(&self, task_id: &str, step: u32, actual: Option<&str>) -> Result<DivergenceStatus, AlephError>;
}
```

#### Graceful Shutdown

Handles SIGTERM/SIGINT for clean task checkpointing:

```rust
pub struct GracefulShutdown {
    db: Arc<VectorDatabase>,
    shutdown_tx: broadcast::Sender<ShutdownSignal>,
}

pub enum ShutdownSignal {
    Term,       // SIGTERM
    Interrupt,  // SIGINT (Ctrl+C)
    Requested,  // Programmatic
}
```

#### Recovery Manager

Risk-aware recovery decisions on startup:

```rust
pub enum RecoveryDecision {
    /// Low risk: auto-resume with Shadow Replay
    AutoResume { task: AgentTask, replay_engine: Arc<ShadowReplayEngine> },

    /// High risk: needs user confirmation
    PendingConfirmation { task: AgentTask },

    /// Cannot recover
    Skip { task_id: String, reason: String },
}
```

### Perception Layer

**Location**: `core/src/memory/database/resilience/perception/`

Event classification and observation with gap-fill support.

#### Skeleton & Pulse Model

Events are classified into tiers for efficient persistence:

| Tier | Description | Persistence | Examples |
|------|-------------|-------------|----------|
| **Skeleton** | Structural events | Immediate | task_started, tool_call_completed |
| **Pulse** | Streaming events | Batched | ai_streaming, progress_update |
| **Volatile** | Ephemeral events | Memory only | heartbeat, metrics_snapshot |

```rust
pub enum EventTier {
    Skeleton,  // Immediate persistence
    Pulse,     // Batched persistence
    Volatile,  // No persistence
}

pub struct EventClassifier;

impl EventClassifier {
    pub fn classify(event_type: &EventType) -> EventTier;
}
```

#### Gap-Fill Protocol

Self-healing event observation with database backfill:

```rust
pub struct TaskObserver {
    db: Arc<VectorDatabase>,
    last_seen_seq: AtomicU64,
}

impl TaskObserver {
    /// Check for and fill gaps in event sequence
    pub async fn gap_fill(&self, task_id: &str) -> Result<GapFillResult, AlephError>;
}
```

### Collaboration Layer

**Location**: `core/src/memory/database/resilience/collaboration/`

Session-as-a-Service for persistent subagent sessions.

#### Session Handle

Reusable handle for subagent sessions:

```rust
pub struct SessionHandle {
    session_id: String,
    agent_type: String,
    db: Arc<VectorDatabase>,
}

impl SessionHandle {
    pub fn session_id(&self) -> &str;
    pub async fn is_idle(&self) -> bool;
    pub async fn record_usage(&self, tokens: u64, tool_calls: u64) -> Result<(), AlephError>;
}
```

#### Session Coordinator

Manages session lifecycle and reuse:

```rust
pub struct SessionCoordinator {
    db: Arc<VectorDatabase>,
    config: CoordinatorConfig,
}

impl SessionCoordinator {
    /// Create a new session
    pub async fn create_session(&self, agent_type: &str, parent_id: &str) -> Result<SessionHandle, AlephError>;

    /// Acquire an existing idle session or create new
    pub async fn acquire_session(&self, agent_type: &str, parent_id: &str) -> Result<SessionHandle, AlephError>;

    /// Release session back to idle pool
    pub async fn release_session(&self, session_id: &str) -> Result<(), AlephError>;
}
```

#### Agent Swapping

Serialize idle agents to disk for memory optimization:

```rust
pub struct SwapManager {
    db: Arc<VectorDatabase>,
    swap_dir: PathBuf,
    config: SwapConfig,
}

impl SwapManager {
    /// Swap out an idle session to disk
    pub async fn swap_out(&self, session_id: &str, context: &SwappedContext) -> Result<SwapResult, AlephError>;

    /// Swap in a session from disk
    pub async fn swap_in(&self, session_id: &str) -> Result<SwappedContext, AlephError>;
}
```

### Governance Layer

**Location**: `core/src/memory/database/resilience/governance/`

Resource governance and recursion limiting.

#### Resource Governor

Lane-based priority isolation:

```rust
pub struct ResourceGovernor {
    db: Arc<VectorDatabase>,
    config: GovernorConfig,
    main_lane: LaneResources,      // High priority (20%)
    subagent_lane: LaneResources,  // Normal priority (80%)
}

pub enum Lane {
    Main,      // User interactions, abort commands
    Subagent,  // Background work
}

impl ResourceGovernor {
    /// Acquire resources for a task
    pub async fn acquire(&self, lane: Lane) -> Result<ResourcePermit, AlephError>;

    /// Check if lane has capacity
    pub fn has_capacity(&self, lane: Lane) -> bool;

    /// Track token usage
    pub async fn record_tokens(&self, session_id: &str, tokens: u64) -> Result<bool, AlephError>;
}
```

#### Recursive Sentry

Prevents infinite task spawning:

```rust
pub struct RecursiveSentry {
    max_depth: u32,
}

impl RecursiveSentry {
    /// Check if spawning is allowed at current depth
    pub fn check_spawn(&self, current_depth: u32) -> Result<(), RecursionLimitExceeded>;
}
```

#### Quota Manager

Concurrency and resource limits:

```rust
pub struct QuotaManager {
    db: Arc<VectorDatabase>,
    config: QuotaConfig,
}

pub struct QuotaConfig {
    pub max_running: usize,           // Max concurrent subagents
    pub max_idle: usize,              // Max idle in memory
    pub max_depth: u32,               // Max recursion depth
    pub token_budget: u64,            // Per-session token budget
    pub max_total: usize,             // Max total subagents
    pub max_tool_calls_per_task: u64, // Tool call limit
}

impl QuotaManager {
    /// Check if action is within quota
    pub async fn check(&self) -> Result<QuotaCheckResult, AlephError>;
}
```

### Database Schema

The resilience module uses four main tables:

```sql
-- Agent tasks with recovery support
CREATE TABLE agent_tasks (
    id TEXT PRIMARY KEY,
    parent_session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    task_prompt TEXT NOT NULL,
    status TEXT NOT NULL,           -- pending, running, completed, failed, interrupted
    risk_level TEXT NOT NULL,       -- low, high
    lane TEXT NOT NULL,             -- main, subagent
    checkpoint_snapshot_path TEXT,
    last_tool_call_id TEXT,
    recursion_depth INTEGER DEFAULT 0,
    parent_task_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    completed_at INTEGER,
    metadata_json TEXT
);

-- Execution traces for Shadow Replay
CREATE TABLE task_traces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    role TEXT NOT NULL,             -- assistant, tool
    content_json TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    UNIQUE(task_id, step_index)
);

-- Tiered event persistence
CREATE TABLE agent_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    is_structural INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    UNIQUE(task_id, seq)
);

-- Subagent sessions
CREATE TABLE subagent_sessions (
    id TEXT PRIMARY KEY,
    agent_type TEXT NOT NULL,
    status TEXT NOT NULL,           -- active, idle, swapped
    context_path TEXT,
    parent_session_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_active_at INTEGER NOT NULL,
    total_tokens_used INTEGER DEFAULT 0,
    total_tool_calls INTEGER DEFAULT 0
);
```

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - Tool development
- [Gateway](GATEWAY.md) - RPC interface
- [Agent Design Philosophy](AGENT_DESIGN_PHILOSOPHY.md) - POE architecture
- [Memory System](MEMORY_SYSTEM.md) - Facts DB and vector search

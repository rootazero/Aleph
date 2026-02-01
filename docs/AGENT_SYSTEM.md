# Agent System

> Core agent loop, thinker, and dispatcher architecture

---

## Overview

The Agent System implements the **Observe-Think-Act-Feedback (OTAF)** loop, the heart of Aether's intelligence.

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

When a provider doesn't support extended thinking, Aether falls back gracefully:

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

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - Tool development
- [Gateway](GATEWAY.md) - RPC interface
- [Agent Design Philosophy](AGENT_DESIGN_PHILOSOPHY.md) - POE architecture

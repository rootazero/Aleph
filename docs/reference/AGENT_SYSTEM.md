# Agent System

> Core agent loop, thinker, and dispatcher architecture

---

## Overview

The Agent System implements the **Think → Act** loop, the heart of Aleph's intelligence. The LLM handles all reasoning (intent, planning, tool selection) in a single inference call, keeping the system minimal.

The runtime topology for Gateway chat is:

```
Gateway (chat ingress, protocol adapters)
   │ FlowRequest { agent_id, input, tool_service, trace_sink, identity, ... }
   ▼
Orchestrator (resolves AgentDef + FlowSpec, builds HarnessDeps, dispatches)
   │ HarnessRunner::run
   ▼
AgentHarness (Think → Act loop, stop-hooks, context budget, compaction)
   │ uses
   ├── SessionService  (append-only history)
   ├── ToolService     (tool catalog + execution)
   ├── Sandbox         (exec environment, capability ledger)
   └── AiProvider      (LLM)
   │
   ▼
FlowOutcome → Gateway renders response
```

> **Note**: SubagentTool (in-tool agent spawning) was migrated to Harness in
> Phase 7 (2026-04-21). The legacy `AgentLoop` path in `src/agent_loop/` has
> been deleted. All agent execution now routes through Orchestrator → Harness.

The inner loop inside `AgentHarness`:

```
┌─────────────────────────────────────────────────────────────────┐
│                        AgentHarness                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│   │ PREPARE  │ ──▶ │  THINK   │ ──▶ │ RESOLVE  │ ──▶ │   ACT    │
│   │          │     │          │     │          │     │          │
│   │• Budget  │     │ • LLM    │     │• Parse   │     │• Execute │
│   │• Context │     │ • Decide │     │• Decision│     │• Tools   │
│   │• Preflight│    │ • Plan   │     │          │     │          │
│   └──────────┘     └──────────┘     └──────────┘     └──────────┘
│                                                              │
│                        ┌──────────┐                           │
│                        │ FINALIZE │ ◀──────────────────────────┘
│                        │          │
│                        │• Eval    │
│                        │• Compress│
│                        │• Decision│
│                        └──────────┘
└─────────────────────────────────────────────────────────────────┘
```

---

## AgentHarness (Gateway chat path)

**Location**: `src/harness/`

The `AgentHarness` is the runtime that drives the Think → Act loop for all
Gateway chat requests. It is constructed by the `Orchestrator` and receives
pre-resolved deps (`SessionService`, `ToolService`, `Sandbox`, `AiProvider`).

### Core Structure

```rust
pub struct AgentHarness {
    provider: Arc<dyn AiProvider>,           // LLM provider
    tool_service: Arc<dyn ToolService>,      // Tool catalog + execution
    sandbox: Arc<dyn Sandbox>,              // Exec environment
    session: Arc<dyn SessionService>,        // Append-only history
    stop_hook: Arc<StopHookHandler>,         // Stop hooks
    compaction_pipeline: CompactionPipeline, // Emergency compaction
    preflight_pipeline: PreflightPipeline,  // Pre-flight context prep
    config: HarnessConfig,                   // Loop configuration
}
```

### Key Components

| Component | Location | Purpose |
|-----------|----------|---------|
| `AgentHarness` | `src/harness/` | Think→Act loop controller |
| `HarnessConfig` | `src/harness/` | Configuration options |
| `TurnState` | `src/harness/` | State machine enum |
| `ToolPipeline` | `src/harness/tool_pipeline.rs` | 7-stage tool execution pipeline |
| `ToolOrchestrator` | `src/harness/tool_orchestrator.rs` | Tool batch orchestration |
| `ToolExecutionContext` | `src/harness/tool_execution_context.rs` | Per-tool cancel/progress context |
| `ContextBudget` | `src/harness/context_budget/` | Pressure sensing + directives |
| `PreflightPipeline` | `src/harness/context_budget/preflight.rs` | Pre-flight async context prep |
| `CompactionPipeline` | `src/harness/context_budget/pipeline.rs` | Emergency compaction stages |
| `StreamingBridge` | `src/harness/streaming_bridge.rs` | Streaming + delta management |
| `SafetyGuard` | `src/harness/safety.rs` | Permission enforcement |
| `StopHookHandler` | `src/harness/stop_hooks.rs` | Stop hook execution |
| `TruncationRecovery` | `src/harness/truncation_recovery.rs` | MaxTokens escalation recovery |
| `Orchestrator` | `src/orchestrator/` | AgentDef resolution + Harness construction |
| `AgentRuntime` | `src/agents/runtime.rs` | Runtime context + model resolution |
| `ToolService` | `src/tools/service.rs` | Tool definitions registry + execution |

> **Migration complete**: The legacy `src/agent_loop/` directory was deleted in
> Phase 7 (2026-04-21). All agent execution — including SubagentTool — now routes
> through Orchestrator → Harness.

### State Machine

```
TurnState enum (5 states):

┌─────────┐
│ PREPARE │
└────┬────┘
     │ budget computed
     ▼
┌─────────┐     ┌─────────┐     ┌─────────┐
│  THINK  │ ──▶ │ RESOLVE │ ──▶ │   ACT   │
└─────────┘     └────┬────┘     └────┬────┘
                     │               │
                     │ EndTurn       │ tools complete
                     │ no_action     │
                     ▼               ▼
               ┌─────────┐     ┌─────────┐
               │FINALIZE │     │FINALIZE │
               └────┬────┘     └────┬────┘
                    │               │
                    ▼               ▼
              ContinueLoop      ExitLoop

TurnAdvance enum:
  • Next(TurnState)    — advance to next state
  • ContinueLoop       — restart with new turn
  • ExitLoop(decision) — terminate loop
  • Cancelled          — cancelled by token
```

### Turn Execution Flow

```
TurnExecution enum captures each state result:

  Prepared(BudgetState)  ← TurnState::Prepare
  Thought(ThinkTurnResult) ← TurnState::Think
  Resolved(TurnResolve)    ← TurnState::Resolve
    • Restart    — restart loop (error/nothing to do)
    • Act(turn)  — proceed to tool execution
    • Finalize(turn) — skip to finalize
  Acted(TurnArtifacts) ← TurnState::Act
  Finalized(LoopDecision) ← TurnState::Finalize
    • Continue — loop again
    • Exit(LoopExitDecision) — done
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

### Busy-Input Modes (message arrives mid-run)

When a message reaches a session whose Think→Act loop is **already running**, the
gateway's busy branch (`execution_engine/execute.rs`) chooses one of two policies,
selected **explicitly per channel** (R7 — never inferred from message content) via
`ChannelPolicyConfig.busy_input_mode`, stamped into run metadata
(`BUSY_INPUT_MODE_KEY`). Absent on Panel/CLI paths → `Steer`.

| Mode | Behaviour | Mechanism (all reused — R10, no new dispatch) |
|------|-----------|-----------------------------------------------|
| `Steer` (**default**) | Inject the message into the live event log; the running loop consumes it at its next turn boundary and course-corrects without losing progress. | `steering::try_inject_steering` + watermark final-turn catch + `MAX_PENDING_STEERING=16` backpressure + reconcile-preamble coalescing. |
| `Interrupt` | Cancel the running sibling on this session; the message then restarts as a **fresh run** (via the inbound router's existing `AgentBusy` busy/retry back-off) that reads the interrupted task's full context from the session log plus the new instruction. | `find_steering_target_id` → `ExecutionEngine::cancel` (`cancel_tx` → `CancellationToken` → `ExecutionError::Cancelled`) → existing retry loop in `inbound_router/executor.rs`. |

Reference parity: hermes `HERMES_GATEWAY_BUSY_INPUT_MODE`, openclaw `QueueMode`,
Pi `streamingBehavior` — all expose this as explicit policy; Aleph previously
hardcoded `Steer`.

Opt a channel into interrupt in its config block:

```toml
[channels.ops-bot]
busy_input_mode = "interrupt"   # default is "steer"
```

**Deferred (YAGNI / honest scoping):**
- *Subagent-aware demotion* (hermes #30170): interrupt currently cancels in-flight
  work including any subagents the run spawned. Demoting interrupt→steer when the
  run has active subagents needs per-session subagent detection — not wired.
  Operators enabling interrupt on a channel accept this today.
- *Follow-up lane* (defer-until-stop, Pi/openclaw `followUp`): `Steer` already lets
  the model decide when to address an interjection; a separate defer-until-stop
  queue would add loop-touching drain logic for marginal gain.

---

## Thinker

**Location**: `src/thinker/`

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
| `SoulManifest` | `soul.rs` | SOUL.md structured parser (for `identity.get` preview) |
| `AgentIdentityProfile` | `identity_profile.rs` | IDENTITY.md structured parser (name/role/vibe/emoji/language) |
| identity files | `identity_files.rs` | Single file-based identity source (SOUL.md), read/written by `self_config` + `identity.*` |

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

**Location**: `src/thinker/` (interaction.rs, security_context.rs, context.rs)

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

**Location**: `src/thinker/identity_files.rs` (file I/O — the single source of truth),
`src/thinker/layers/{soul,profile}.rs` (prompt injection), `src/thinker/soul.rs`
(`SoulManifest` structured parser, used only for the `identity.get` preview),
`src/thinker/identity_profile.rs` (`AgentIdentityProfile` — the single structured
parser for `IDENTITY.md`'s rich fields).

The Embodiment Engine gives the AI a consistent identity and personality from a
**single file-based source of truth** — the per-agent identity files under
`~/.aleph/agents/{id}/` (`SOUL.md` = persona, `AGENTS.md` = project context). Both
the `self_config` LLM tool and the `identity.*` RPC/CLI write these files, and the
prompt layers inject them into the system prompt each turn, so an edit takes effect
on the next turn.

> **History**: identity was once resolved by a session/project/global priority stack
> (`IdentityResolver` → `SoulManifest`), but that resolver was never wired into prompt
> assembly, so its overrides silently no-op'd. It was dissolved (2026-07) in favor of
> the single file-based source below.

### Architecture

```
Write path (any of):                Inject path (every turn):
┌──────────────────────┐            ┌──────────────────────────────────┐
│ self_config LLM tool │──┐         │ IdentityFiles::load              │
│ identity.* RPC / CLI │──┤         │   ├─ SOUL.md  → SoulLayer        │
└──────────────────────┘  │         │   └─ AGENTS.md → ProfileLayer    │
                          ▼         │ (both run on the Cached path →   │
        ~/.aleph/agents/{id}/       │  land in the cacheable prefix    │
          ├─ SOUL.md   (persona) ──▶│  of the system prompt)           │
          └─ AGENTS.md (project) ──▶└──────────────────────────────────┘
```

### SoulManifest

Structured view of a `SOUL.md` file, parsed by `SoulManifest::from_file`. It is a
**read-only preview** surfaced by `identity.get` — the raw `SOUL.md` markdown (not this
struct) is what gets injected into the prompt.

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

### AgentIdentityProfile

Structured view of an agent's `IDENTITY.md` rich fields, parsed by
`AgentIdentityProfile::{from_markdown, from_agent_dir}`. This is the **single**
`IDENTITY.md` parser — do not hand-roll `strip_prefix("**Name:**")` scans, which is
what `agent_manager::crud` used to do before delegating here.

```rust
pub struct AgentIdentityProfile {
    pub name: Option<String>,     // **Name:**
    pub role: Option<String>,     // **Role:**     — archetype-seeded at creation
    pub vibe: Option<String>,     // **Vibe:**     — archetype-seeded at creation
    pub emoji: Option<String>,    // **Emoji:**    — archetype-seeded at creation
    pub language: Option<String>, // **Language:** — unseeded
}
```

Every field is `Option` so callers can distinguish *unset* from *set to something*.
Three parsing rules matter, because the creation template walks straight into all
three:

1. **Decoration is stripped** — `**`, `_`, `` ` ``, and a fully-parenthesized value.
2. **Dashes fold to ASCII** — the template writes a typographic em dash
   (`your signature — swap if you like`), which would otherwise never match an
   ASCII-authored placeholder constant.
3. **Placeholders and trailing asides read as `None`** — `- **Role:** systems thinker
   _(edit to taste)_` yields `Some("systems thinker")`, while the shipped
   `- **Language:** _(preferred language for conversation)_` yields `None` rather
   than a language literally named "preferred language for conversation".

Parsing is deliberately **infallible** (empty profile, never `Result`): identity is
decorative metadata, and no caller has a better answer to a malformed file than
"treat the agent as unnamed", so an error type would only push `unwrap_or_default()`
to every call site.

> The `round_trips_every_archetype_seeded_template` test asserts every archetype's
> generated template parses back to the exact `role_hint()` / `vibe_hint()` /
> `emoji_hint()` it was built from, so the template format and the parser cannot
> drift apart.

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

All operate on `~/.aleph/agents/{agent_id}/` (`agent_id` optional, defaults to the
daemon's boot agent) — the same files `self_config` writes.

| Method | Description |
|--------|-------------|
| `identity.get` | Live `SOUL.md` (raw markdown + parsed `SoulManifest` preview), parsed `IDENTITY.md` rich fields (`identity`), + identity-file status |
| `identity.set` | Write an identity file (`SOUL.md` by default), snapshotting the prior version |
| `identity.clear` | Snapshot and remove `SOUL.md` (revert to the default persona) |
| `identity.list` | List identity files (exists / size / path) |

---

## Chain-of-Thought Transparency

**Location**: `src/thinker/thinking.rs`

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

**Location**: `src/dispatcher/`

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

**Location**: `src/harness/guards.rs`

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

**Location**: `src/harness/callback.rs`

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

**Location**: `src/agents/sub_agents/` (tool layer) + `src/agents/runtime.rs` (runtime)

The main agent can spawn sub-agents for specialized tasks via the `SubagentTool`.
Sub-agent spawning was migrated to Harness in Phase 7 (2026-04-21). The legacy
`src/agent_loop/` directory has been deleted. All agent execution now routes
through Orchestrator → AgentHarness.

```
Main Agent (claude-opus-4)  — runs via AgentHarness (Gateway chat)
    │
    ├─── Translator Sub-Agent (claude-haiku)
    │       Session: subagent:agent:main:translator
    │       Runtime: AgentHarness (via Orchestrator dispatch)
    │
    ├─── Code Reviewer Sub-Agent (claude-sonnet)
    │       Session: subagent:agent:main:code-reviewer
    │       Runtime: AgentHarness (via Orchestrator dispatch)
    │
    └─── Research Sub-Agent (gpt-4o)
            Session: subagent:agent:main:researcher
            Runtime: AgentHarness (via Orchestrator dispatch)
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

### Sleep inhibitor

Each Think→Act turn acquires an `IOPMAssertion` of type
`PreventUserIdleSystemSleep` with the reason string `"Aleph agent loop"`. The
assertion is held by an RAII `InhibitorGuard` whose `Drop` implementation
releases it the moment the turn returns. Long-running agent turns can no longer
be silently cut short by macOS putting the host to sleep mid-flight.

The guard is acquired at the start of `run_turn` in
`src/harness/agent.rs` and released automatically when the function returns,
whether it succeeds, returns an error, or is cancelled. No explicit cleanup
code is needed.

To verify that an assertion is active while a turn is in flight:

```bash
pmset -g assertions | grep "Aleph agent loop"
```

The assertion disappears from the list the moment the turn completes.

Implementation files:
- `PowerCapability` trait: `desktop/shared/src/traits/power.rs`
- macOS IOPMAssertion FFI: `desktop/macos/src/sleep_inhibitor.rs`
- Agent loop wiring: `src/harness/agent.rs::run_turn`

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - Tool development
- [Gateway](GATEWAY.md) - RPC interface
- [Agent Design Philosophy](AGENT_DESIGN_PHILOSOPHY.md) - Design principles
- [Memory System](MEMORY_SYSTEM.md) - Facts DB and vector search
- [Desktop Bridge](DESKTOP_BRIDGE.md) - Swift helper process and JSON-RPC protocol

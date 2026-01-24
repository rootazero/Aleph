# Agent Loop Architecture

This document describes Aether's Agent Loop implementation, including safety mechanisms, retry logic, and tool execution patterns.

## Overview

The Agent Loop implements an **observe-think-act-feedback** cycle that enables autonomous task execution while maintaining safety and reliability.

```
┌──────────────────────────────────────────────────────────────┐
│                        Agent Loop                             │
├──────────────────────────────────────────────────────────────┤
│                                                               │
│   ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌──────────┐  │
│   │ Observe │───▶│  Think  │───▶│   Act   │───▶│ Feedback │  │
│   └─────────┘    └─────────┘    └─────────┘    └──────────┘  │
│        │              │              │              │         │
│        │              │              │              │         │
│   ┌────▼──────────────▼──────────────▼──────────────▼────┐   │
│   │                    Guards                             │   │
│   │  • Doom Loop Detection                               │   │
│   │  • Iteration Limits                                  │   │
│   │  • Token Budget                                      │   │
│   │  • Error Tracking                                    │   │
│   └──────────────────────────────────────────────────────┘   │
│                                                               │
└──────────────────────────────────────────────────────────────┘
```

---

## Core Components

### 1. Loop Guards (`agent_loop/guards.rs`)

Guards prevent runaway execution and detect problematic patterns.

| Guard | Description | Default |
|-------|-------------|---------|
| **Iteration Guard** | Maximum loop iterations | 50 |
| **Token Guard** | Maximum tokens consumed | Configurable |
| **Error Guard** | Consecutive error limit | 3 |
| **Doom Loop Guard** | Identical tool call detection | 3 repeats |

#### Doom Loop Detection

Inspired by OpenCode's pattern, detects when the agent repeatedly calls the same tool with identical arguments:

```rust
struct ToolCallRecord {
    tool_name: String,
    arguments_hash: u64,  // FxHash of JSON arguments
    arguments: Value,
}

// Triggered when 3 consecutive calls have same name + hash
if last_3_calls.all_identical() {
    return GuardViolation::DoomLoop { ... };
}
```

**Key behavior**:
- Uses argument **hashing** for efficient comparison
- Compares both tool name AND arguments
- Different arguments = no doom loop (allows legitimate retries with modified params)

### 2. Retry Mechanism (`providers/retry.rs`)

Automatic retry with exponential backoff for transient failures.

```rust
pub struct ThinkRetryConfig {
    pub max_retries: u32,          // default: 3
    pub initial_backoff_ms: u64,   // default: 2000
    pub backoff_multiplier: f64,   // default: 2.0
    pub max_backoff_ms: u64,       // default: 30000
    pub respect_retry_after: bool, // default: true
}
```

**Retry-After Header Support**:
- Parses both numeric seconds and HTTP date formats
- Respects provider rate limit guidance
- Falls back to exponential backoff if no header

**Retryable errors**:
- Network timeouts
- Rate limits (429)
- Server errors (5xx)

**Non-retryable errors**:
- Authentication failures (401)
- Invalid requests (400)
- Context overflow

### 3. Callbacks (`agent_loop/callback.rs`)

Extension points for UI integration and monitoring:

```rust
pub trait AgentLoopCallback: Send + Sync {
    // Existing callbacks
    async fn on_iteration_start(&self, iteration: u32);
    async fn on_tool_call(&self, name: &str, args: &Value);
    async fn on_tool_result(&self, name: &str, result: &str, success: bool);
    async fn on_thinking(&self, thought: &str);

    // Safety callbacks (new)
    async fn on_doom_loop_detected(
        &self,
        tool_name: &str,
        arguments: &Value,
        repeat_count: usize
    ) -> bool;  // Returns true to continue anyway

    async fn on_retry_scheduled(
        &self,
        attempt: u32,
        max_retries: u32,
        delay_ms: u64,
        error: &str,
    );

    async fn on_retries_exhausted(&self, attempts: u32, error: &str);
}
```

### 4. Part Event Publishing (`ffi/agent_loop_adapter.rs`)

The `FfiLoopCallback` publishes Part events for real-time UI rendering:

```rust
pub struct FfiLoopCallback {
    handler: Arc<dyn AetherEventHandler>,
    bus: Arc<EventBus>,
    session_id: RwLock<String>,
    active_tool_calls: RwLock<HashMap<String, ToolCallPart>>,
}
```

**Part Event Flow**:

```
on_action_start()
    │
    ├─ Create ToolCallPart { id, tool_name, status: Running }
    ├─ Store in active_tool_calls map
    └─ Publish PartAdded event via EventBus
           │
           ▼
on_action_done()
    │
    ├─ Update ToolCallPart { status: Completed/Failed, output }
    ├─ Remove from active_tool_calls map
    └─ Publish PartUpdated event via EventBus
           │
           ▼
CallbackBridge.handle()
    │
    ├─ Convert to PartUpdateEventFfi
    └─ Call handler.on_part_update(event)
```

**Event Types**:
| Event | Trigger | Part Content |
|-------|---------|--------------|
| `PartAdded` | Tool call starts | ToolCallPart with `Running` status |
| `PartUpdated` | Tool call completes | ToolCallPart with `Completed/Failed` status |
| `PartUpdated` | Streaming delta | Delta text content |
| `PartRemoved` | Part cleanup | Part ID only |

See [ARCHITECTURE.md](./ARCHITECTURE.md#message-flow-system) for complete message flow documentation.

---

## Tool Execution

### Multi-Tool Execution (NEW)

The Agent Loop now supports executing multiple tools in a single decision:

```
Decision Types:
├─ UseTool       → Single tool execution
├─ UseTools      → Multiple tools (parallel or sequential)
└─ ExecuteGraph  → Task graph with dependencies (DAG)
```

**Parallel Execution** (`UseTools { parallel: true }`):
- All tools execute concurrently using `tokio::join_all`
- Results aggregated in `MultiToolResults`
- Maximum 25 tools per batch (`MAX_BATCH_TOOLS`)

**Sequential Execution** (`UseTools { parallel: false }`):
- Tools execute one after another
- Output from previous tool available to subsequent
- Continues even if individual tools fail

**Graph Execution** (`ExecuteGraph`):
- Tasks with explicit dependencies
- DAG-based scheduling
- Cycle detection and validation

**LLM Response Format**:
```json
{
  "reasoning": "I need to read multiple files",
  "action": {
    "type": "tools",
    "tools": [
      {"tool_name": "file_ops", "arguments": {"path": "/a.txt"}},
      {"tool_name": "file_ops", "arguments": {"path": "/b.txt"}}
    ],
    "parallel": true
  }
}
```

**Alternative: batch_execute Tool**:
For LLMs that only support single-tool responses, the `batch_execute` tool provides equivalent functionality.

### Tool Call Repair (`tools/server.rs`)

Automatic correction of invalid tool calls:

```
Tool Call: "WebSearch" (invalid)
     │
     ▼
┌────────────────────────────────┐
│   1. Exact Match               │ ─── Not found
└────────────────────────────────┘
     │
     ▼
┌────────────────────────────────┐
│   2. Case-Insensitive Match    │ ─── Found: "web_search"
└────────────────────────────────┘
     │
     ▼
Tool Executed: "web_search" ✓
```

**Repair strategies**:
1. Case-insensitive matching (`WebSearch` → `web_search`)
2. Snake case conversion (`webSearch` → `web_search`)
3. Invalid tool fallback (returns suggestions)

### Invalid Tool Handler (`rig_tools/invalid.rs`)

When no match is found, provides helpful feedback:

```rust
InvalidToolOutput {
    success: false,
    message: "Tool 'unknown_tool' not found",
    suggestion: "Available tools: search, web_fetch, youtube, ... (and 15 more)"
}
```

### Output Truncation (`tool_output/`)

Prevents large outputs from overflowing context:

| Limit | Value | Direction |
|-------|-------|-----------|
| Max Lines | 2000 | Head (keep first) |
| Max Bytes | 50 KB | Head (keep first) |

**Truncation behavior**:
1. Content exceeding limits is saved to file
2. Truncated preview + file path returned to agent
3. Agent can use `Read` tool with offset/limit for full content

**Cleanup scheduler**:
- Retention: 7 days
- Cleanup interval: 1 hour
- Location: `~/.config/aether/tool_output/`

---

## Skills System

### Multi-Location Discovery

Skills are discovered from multiple locations in priority order:

```
Priority Order:
1. .aether/skills/     (project level, traverse up to git root)
2. .claude/skills/     (project level, Claude Code compatible)
3. ~/.config/aether/skills (global)
4. ~/.claude/skills    (global, Claude Code compatible)
```

**First occurrence wins** - if same skill ID exists in multiple locations, the first (highest priority) is used.

### Progressive Disclosure

Three levels of skill information:

| Level | Content | When Loaded |
|-------|---------|-------------|
| **L1 Metadata** | name, description, location | System prompt (always) |
| **L2 Instructions** | Full SKILL.md content | On `read_skill` call |
| **L3 Resources** | Additional files (REFERENCE.md, etc.) | On `read_skill` with file_name |

```rust
// Level 1: Available in prompt
pub struct SkillMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub location: PathBuf,
    pub source: SkillSource,  // Project or Global
}

// Level 2-3: Loaded on demand via read_skill tool
```

---

## Configuration

### Agent Loop Config (`agent_loop/config.rs`)

```toml
[agent_loop]
max_iterations = 50
doom_loop_threshold = 3

[agent_loop.retry]
max_retries = 3
initial_backoff_ms = 2000
backoff_multiplier = 2.0
max_backoff_ms = 30000
respect_retry_after = true

[agent_loop.truncation]
max_lines = 2000
max_bytes = 51200  # 50 KB
direction = "head"
retention_days = 7
```

---

## Sub-Agent Synchronization

The Agent Loop supports delegating tasks to specialized sub-agents with synchronous wait capability. This enables complex multi-step workflows where the parent agent can wait for sub-agent completion and collect results.

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Main Agent Loop                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   ┌──────────────────────────────────────────────────────┐  │
│   │              SubAgentDispatcher                       │  │
│   │                                                       │  │
│   │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │  │
│   │  │  McpAgent   │  │ SkillAgent  │  │ CustomAgent │   │  │
│   │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘   │  │
│   │         │                │                │          │  │
│   │         └────────────────┼────────────────┘          │  │
│   │                          ▼                           │  │
│   │               ExecutionCoordinator                   │  │
│   │                          │                           │  │
│   │                  ┌───────┴───────┐                   │  │
│   │                  ▼               ▼                   │  │
│   │            wait_for_result  wait_for_all             │  │
│   │                  │               │                   │  │
│   │                  └───────┬───────┘                   │  │
│   │                          ▼                           │  │
│   │               ResultCollector                        │  │
│   │              (tools_called, artifacts)               │  │
│   └──────────────────────────────────────────────────────┘  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Components

#### ExecutionCoordinator (`sub_agents/coordinator.rs`)

Manages synchronous wait for sub-agent completion using oneshot channels:

```rust
// Single request - blocks until completion or timeout
let result = dispatcher.dispatch_sync(request, Duration::from_secs(60)).await?;

// Multiple parallel requests - waits for all
let results = dispatcher.dispatch_parallel_sync(
    vec![(request1, None), (request2, Some("mcp_agent"))],
    Duration::from_secs(120)
).await;
```

**Key features**:
- Timeout handling with partial result recovery
- Concurrency limiting via semaphore
- TTL-based cleanup of completed results
- Real-time progress tracking

#### ResultCollector (`sub_agents/result_collector.rs`)

Aggregates tool calls and artifacts during sub-agent execution:

```rust
// Automatically collects during execution
let summary = collector.get_summary(&request_id).await;
// Returns Vec<ToolCallSummary> with:
// - tool name
// - status (pending/running/completed/error)
// - title (completion message)
```

#### Context Propagation

Sub-agents receive context from the parent agent:

```rust
let request = SubAgentRequest::from_parent_context(
    "Search for Rust files",
    parent_session_id,
    Some(working_directory),
    Some(original_request),
    Some(history_summary),
    recent_steps,
);
```

**Context includes**:
- Working directory
- Original user request
- History summary (what's been done)
- Recent steps (with success/failure status)

### Configuration

```toml
[subagent]
execution_timeout_ms = 300000  # 5 minutes
result_ttl_ms = 3600000        # 1 hour
max_concurrent = 5
progress_events_enabled = true
```

### Error Handling

```rust
pub enum ExecutionError {
    Timeout { request_id, elapsed_ms, partial_summary },
    ExecutionFailed { request_id, error, tools_completed },
    NotFound { request_id },
    QueueTimeout { request_id },
    Internal(String),
}
```

On timeout, partial results (completed tool calls) are still available via `partial_summary`.

---

## Overflow Detection

**Status**: Implemented (2026-01-24)

**Location**: `core/src/agent_loop/overflow.rs`

The OverflowDetector provides real-time context overflow detection for proactive compaction triggers before context window limits are exceeded.

### OverflowDetector

```rust
use aether_core::agent_loop::overflow::{OverflowDetector, OverflowConfig};

let detector = OverflowDetector::new(OverflowConfig::default());

// Check if overflow threshold exceeded
if detector.is_overflow(&session) {
    // Trigger compaction
}

// Get usage percentage (0.0 - 1.0)
let usage = detector.usage_percent(&session);
```

### Model Limits

The detector uses predefined model limits for accurate overflow detection:

| Model | Context | Max Output | Usable Tokens |
|-------|---------|------------|---------------|
| claude-3.5-sonnet | 200K | 8K | ~173K |
| claude-3-opus | 200K | 4K | ~177K |
| gpt-4o | 128K | 16K | ~101K |
| gpt-4-turbo | 128K | 4K | ~112K |
| gemini-1.5-pro | 2M | 8K | ~1.8M |

### Integration with Agent Loop

The OverflowDetector is integrated at the start of each agent loop iteration:

1. **Before Think Phase**: Check if current token usage exceeds threshold
2. **Threshold Action**: If overflow detected, inject warning into system prompt
3. **Compaction Trigger**: Signal to MessageBuilder to trigger compaction

```rust
// In AgentLoop iteration
let overflow_result = self.overflow_detector.check(&session);
if overflow_result.is_overflow {
    message_builder.inject_token_limit_warning(overflow_result.usage_percent);
}
```

---

## Session Compaction

The Agent Loop integrates with the SessionCompactor to manage token usage and prevent context overflow.

### Compaction Integration Points

The agent loop integrates with compaction at three points:

1. **Before each iteration** (`LoopContinue` event): Check if tokens exceed threshold
2. **After tool execution** (`ToolCallCompleted` event): Trigger pruning check
3. **At session end**: Final pruning pass

```
┌────────────────────────────────────────────────────────────────┐
│                    Agent Loop Iteration                         │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│   ┌─────────────────────────────────────────────────────────┐  │
│   │  1. Check Token Overflow                                 │  │
│   │     └─ If overflow: trigger SessionCompactor             │  │
│   └─────────────────────────────────────────────────────────┘  │
│                           │                                     │
│                           ▼                                     │
│   ┌─────────────────────────────────────────────────────────┐  │
│   │  2. Observe → Think → Act                               │  │
│   │     └─ Execute tool calls                               │  │
│   └─────────────────────────────────────────────────────────┘  │
│                           │                                     │
│                           ▼                                     │
│   ┌─────────────────────────────────────────────────────────┐  │
│   │  3. After Tool Execution                                │  │
│   │     └─ Check pruning if enabled                         │  │
│   └─────────────────────────────────────────────────────────┘  │
│                           │                                     │
│                           ▼                                     │
│   ┌─────────────────────────────────────────────────────────┐  │
│   │  4. Feedback                                            │  │
│   │     └─ Publish SessionCompacted if compaction occurred  │  │
│   └─────────────────────────────────────────────────────────┘  │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

### Compaction Trigger Logic

```rust
// Before each iteration
let limit = token_tracker.get_model_limit(&session.model);
if session.total_tokens >= limit.compaction_threshold() {
    // Trigger compaction
    if let Some(info) = compactor.check_and_compact(&mut session).await {
        event_bus.publish(AetherEvent::SessionCompacted(info)).await;
    }
}
```

### Protected Context

During compaction, certain content is protected:

- **Recent tool outputs**: Last 2 user turns are never pruned
- **Protected tools**: Tools in `protected_tools` list (e.g., "skill") are never pruned
- **Token threshold**: Only prune if savings exceed `prune_minimum` (default 20K tokens)

### Events

| Event | When Published | Data |
|-------|----------------|------|
| `SessionCompacted` | After successful compaction | `CompactionInfo { session_id, tokens_before, tokens_after, timestamp }` |

### Configuration

```toml
[compaction]
auto_compact = true          # Enable automatic compaction
prune_enabled = true         # Enable tool output pruning
prune_minimum = 20000        # Minimum tokens before pruning
prune_protect = 40000        # Protect recent N tokens
protected_tools = ["skill"]  # Tools that are never pruned
```

See [SESSION_COMPACTION.md](./SESSION_COMPACTION.md) for complete compaction documentation.

---

## Smart Compaction

**Status**: Implemented (2026-01-24)

**Location**: `core/src/compressor/`

Smart Compaction provides intelligent context management using a unified compactor that combines strategy, truncation, and turn protection for fine-grained control over what gets compacted.

### Components

| Component | File | Purpose |
|-----------|------|---------|
| **SmartCompactor** | `smart_compactor.rs` | Unified compaction component combining all sub-components |
| **SmartCompactionStrategy** | `smart_strategy.rs` | Decision-making for which parts to compact |
| **ToolTruncator** | `tool_truncator.rs` | Truncates large tool outputs with summary generation |
| **TurnProtector** | `turn_protector.rs` | Protects recent conversation turns from compaction |

### SmartCompactor

The main entry point for intelligent compaction:

```rust
use aether_core::compressor::{SmartCompactor, SmartCompactionStrategy};

// Create with default settings
let compactor = SmartCompactor::new();

// Or with custom strategy
let strategy = SmartCompactionStrategy::new()
    .with_compaction_threshold(0.90)   // Trigger at 90% usage
    .with_protected_turns(3)            // Protect last 3 turns
    .with_tool_output_max_chars(2000)   // Max chars per tool output
    .add_protected_tool("skill");       // Never compact skill output

let compactor = SmartCompactor::with_strategy(strategy);

// Compact session parts
let result = compactor.compact(&session_parts, 0.92);
if result.marker.is_some() {
    println!("Compacted {} parts, freed ~{} tokens",
        result.parts_compacted, result.tokens_freed_estimate);
}
```

### Compaction Actions

The strategy evaluates each session part and returns an action:

```rust
pub enum CompactionAction {
    Keep,           // Leave unchanged
    Truncate {      // Truncate with summary
        max_chars: usize,
        summary: String,
    },
    RemoveOutput,   // Remove output entirely (keep call record)
    Summarize {     // Merge multiple parts (future)
        original_count: usize,
    },
}
```

### Turn Protection

The TurnProtector ensures recent conversation context is preserved:

```rust
use aether_core::compressor::TurnProtector;

let protector = TurnProtector::new(2); // Protect last 2 turns

// A "turn" = one UserInput + all subsequent parts until next UserInput
// Example with 5 turns and protected_turns=2:
// - Turns 0, 1, 2: NOT protected (can be compacted)
// - Turns 3, 4: PROTECTED (never compacted)

// Check if a specific turn is protected
if protector.is_protected(4, 5) {
    println!("Turn 4 is in the protected window");
}

// Get all protected part indices
let protected_indices = protector.protected_part_indices(&session.parts);
```

### Tool Output Truncation

The ToolTruncator handles large outputs with summary generation:

```rust
use aether_core::compressor::ToolTruncator;

let truncator = ToolTruncator::new(2000)
    .with_summary_template("[Truncated {tool_name}: {original_len} -> {truncated_len}] {preview}...");

let output = truncator.truncate(&large_output, "read_file");
if output.was_truncated {
    println!("Original: {} chars, Truncated: {} chars",
        output.original_len, output.content.len());
    println!("Summary: {}", output.summary);
}
```

### Protected Tools

Certain tools are never compacted regardless of size:

```rust
let strategy = SmartCompactionStrategy::new()
    .add_protected_tool("skill")    // Skill execution output
    .add_protected_tool("plan");    // Planning output

// Default protected tools: ["skill", "plan"]
```

### Step and Compaction Markers

Smart compaction works with session part types for step tracking:

```rust
/// Marks the start of an agent loop step
pub struct StepStartPart {
    pub step_id: usize,
    pub timestamp: i64,
    pub snapshot_id: Option<String>,  // For file revert capability
}

/// Marks the finish of an agent loop step
pub struct StepFinishPart {
    pub step_id: usize,
    pub reason: StepFinishReason,     // Completed, Failed, UserAborted, etc.
    pub tokens: Option<StepTokenUsage>,
    pub duration_ms: u64,
}

/// Marker indicating compaction occurred
pub struct CompactionMarker {
    pub timestamp: i64,
    pub auto: bool,           // Auto-triggered vs manual
    pub marker_id: Option<String>,
    pub parts_compacted: Option<usize>,
    pub tokens_freed: Option<u64>,
}
```

These parts enable:
- **Audit Trail**: Track when compaction occurred
- **Restoration**: Identify compaction points for potential rollback
- **Analytics**: Measure compaction effectiveness over time

### Configuration

```toml
[smart_compaction]
enabled = true
compaction_threshold = 0.85     # Trigger at 85% context usage
protected_turns = 2             # Protect last 2 turns
tool_output_max_chars = 2000    # Max chars per tool output
protected_tools = ["skill", "plan"]
```

### Integration with Agent Loop

Smart compaction integrates with the agent loop via MessageBuilder:

```rust
// In agent loop iteration
let message_builder = MessageBuilder::new()
    .with_smart_compaction(compactor.clone());

// During message construction, compaction is applied
// based on current context usage
let messages = message_builder.build(&session_parts, token_usage);
```

---

## Error Handling

### Guard Violations

```rust
pub enum GuardViolation {
    MaxIterations { limit: u32, current: u32 },
    TokenBudget { limit: usize, used: usize },
    ConsecutiveErrors { limit: u32, count: u32, last_error: String },
    DoomLoop { tool_name: String, repeat_count: usize, arguments_preview: String },
}
```

### Recovery Strategies

| Violation | Default Action | Override |
|-----------|----------------|----------|
| Max Iterations | Stop loop | Increase limit |
| Token Budget | Stop loop | Increase budget |
| Consecutive Errors | Stop loop | Fix underlying issue |
| Doom Loop | Stop + notify user | Callback can continue |

---

## Testing

```bash
# Run agent loop tests
cargo test agent_loop

# Run specific guard tests
cargo test guards

# Run tool repair tests
cargo test call_with_repair

# Run skill discovery tests
cargo test skill_reader
cargo test get_all_skills_dirs
```

---

## References

- [OpenCode](https://github.com/opencode-ai/opencode) - Inspiration for doom loop detection and retry patterns
- [ARCHITECTURE.md](./ARCHITECTURE.md) - Overall system architecture
- [DISPATCHER.md](./DISPATCHER.md) - Tool routing and execution

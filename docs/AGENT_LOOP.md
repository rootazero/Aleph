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

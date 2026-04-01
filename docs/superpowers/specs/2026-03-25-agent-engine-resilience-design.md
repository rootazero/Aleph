# Agent Engine Resilience: Timeout Unification, Compression Isolation & Pair-Aware Truncation

**Date**: 2026-03-25
**Status**: Approved
**Motivation**: Inspired by OpenClaw's agent engine optimizations, but designed from Aleph's Rust-native architecture principles — prevention over repair, type safety over runtime patching, precise control over blunt timeouts.

---

## Problem Statement

Aleph's agent execution engine has four resilience gaps that limit long-running tasks and risk conversation corruption:

1. **Timeout too short & fragmented** — Three independent timeout sources (LoopConfig 5min, Engine 15min, Orchestrator 10min) with no coordination. Effective timeout is `min(5min, 15min)` = 5 minutes.
2. **Compression counted against timeout** — `prepare_history` (LanceDB queries + LLM summarization) runs inside the engine's `tokio::select!` timeout, stealing agent work time.
3. **Truncation breaks ToolCall/ToolResult pairs** — `enforce_context_limit` removes messages individually, potentially orphaning ToolResults whose ToolCalls were deleted (or vice versa), causing provider API errors.
4. **Unnecessary I/O on short sessions** — `prepare_history` queries LanceDB even when session has fewer messages than `fresh_tail_count`.

## Non-Goals

- Replacing the existing 3-tier compression architecture (it's already superior to OpenClaw's)
- Adding session file corruption repair (Aleph uses SQLite, not JSON lines — corruption is handled at the DB level)
- Copying OpenClaw's aggregate timeout for empty session loops (Aleph's `partition_fresh_tail` already prevents this; we just add an early return in `prepare_history`)

---

## Design

### 1. Timeout Unification

**Principle**: Single source of timeout truth at the Engine layer. The Loop doesn't time itself.

#### Changes

**Delete `LoopConfig.timeout_secs`**

The field is removed from `LoopConfig`. The loop's `while` condition becomes iteration-count-only:

```rust
// loop_core.rs — LoopConfig
pub struct LoopConfig {
    pub max_iterations: usize,   // Default: 200 (was 25)
    pub token_budget: usize,     // Default: 100_000 (unchanged)
    // timeout_secs: REMOVED
}
```

The timeout check inside the loop (`if start.elapsed().as_secs() >= self.config.timeout_secs`) is deleted. Timeout is enforced exclusively by the Engine's `tokio::select!`.

**Update defaults**

| Config | Old Default | New Default |
|--------|-------------|-------------|
| `ExecutionEngineConfig.default_timeout_secs` | 900 (15min) | 172_800 (48h) |
| `LoopConfig.max_iterations` | 25 | 200 |
| `OrchestratorGuards.timeout_seconds` | 600 (10min) | 172_800 (48h) |

**Agent-level timeout override**

Add `timeout_secs: Option<u64>` to `AgentInstanceConfig` (existing struct in `agent_instance.rs`).

**Cascade resolution** (in `engine.rs`):

```rust
let timeout_secs = request.timeout_secs          // per-request (API callers)
    .or(agent.config().timeout_secs)              // per-agent (config/UI)
    .unwrap_or(self.config.default_timeout_secs); // global default (48h)
```

**Config persistence**

New section in `config.toml`:

```toml
[execution]
default_timeout_secs = 172800
max_iterations = 200
```

Struct:

```rust
// config/types/execution.rs (NEW FILE)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionConfig {
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,

    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_timeout_secs() -> u64 { 172_800 }
fn default_max_iterations() -> usize { 200 }
```

`ExecutionEngineConfig` reads from `Config.execution` at startup.

### 2. Compression Isolation (Resettable Deadline)

**Principle**: Compression is system housekeeping, not agent work. It should not consume the agent's time budget.

#### Mechanism

Replace the fixed `tokio::time::sleep` in `engine.rs` with a resettable deadline:

```rust
// engine.rs — execute()
let deadline = Arc::new(tokio::sync::Mutex::new(
    tokio::time::Instant::now() + Duration::from_secs(timeout_secs)
));

let result = tokio::select! {
    result = self.run_agent_loop(&run_id, &request, agent.clone(), emitter.clone(), deadline.clone()) => result,
    _ = cancel_rx.recv() => Err(ExecutionError::Cancelled),
    _ = wait_for_deadline(deadline.clone()) => {
        warn!("Run {} timed out after {}s effective time", run_id, timeout_secs);
        Err(ExecutionError::Timeout)
    }
};
```

**`wait_for_deadline` helper:**

```rust
async fn wait_for_deadline(deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>) {
    loop {
        let dl = *deadline.lock().await;
        tokio::time::sleep_until(dl).await;
        // Re-check: deadline may have been extended while we slept.
        // The mutex is only ever contended briefly during compression
        // start/end, so lock overhead is negligible.
        if tokio::time::Instant::now() >= *deadline.lock().await {
            break;
        }
        // Guard against theoretical busy-spin if deadline is in the past
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
```

**In `run_agent_loop`:**

The deadline `Arc` is passed through. Compression calls are wrapped:

```rust
// Before prepare_history
let before_compress = tokio::time::Instant::now();
let history = session_compactor.prepare_history(...).await;
let compress_elapsed = before_compress.elapsed();
*deadline.lock().await += compress_elapsed;
```

Same pattern for `compact_if_needed` in the loop (though it's synchronous and fast, wrapping it is cheap insurance).

**Type signature change for `run_agent_loop`:**

```rust
async fn run_agent_loop(
    &self,
    run_id: &str,
    request: &RunRequest,
    agent: Arc<AgentInstance>,
    emitter: Arc<E>,
    deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,  // NEW
) -> Result<String, ExecutionError>
```

### 3. Pair-Aware Truncation

**Principle**: Prevention over repair. Truncation boundaries fall on complete "conversation rounds", never splitting ToolCall/ToolResult pairs.

#### Definition: Conversation Round

A conversation round is one of:
- A single `User` message
- A single `Assistant` message (no tool calls)
- An `Assistant` message (with tool calls) + all corresponding `ToolResult` messages

#### Modified `enforce_context_limit`

```rust
fn enforce_context_limit(
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    tool_defs: &[ToolDefinition],
    token_budget: usize,
    fresh_tail_count: usize,
    ratio: f64,
) {
    // ... estimate overhead, check budget (unchanged) ...

    if msg_tokens <= msg_budget { return; }

    // Phase 1: Find safe cut point at round boundary
    let tail_start = messages.len().saturating_sub(fresh_tail_count);
    let cut = find_safe_cut_point(messages, tail_start);

    if cut > 0 {
        messages.drain(0..cut);
        messages.insert(0, UnifiedMessage::user(TRUNCATION_NOTICE));
    }

    // Phase 2: If still over budget, remove oldest complete rounds one by one
    while messages.len() > 2 && estimate_total_tokens(messages, ratio) > msg_budget {
        remove_oldest_complete_round(messages);
    }
}
```

**`find_safe_cut_point`:**

Walk backwards from `tail_start` to find a position where:
- The message at `cut` is NOT a `ToolResult`
- The message at `cut` is NOT an `Assistant` with tool calls whose `ToolResult`s would be orphaned

```rust
fn find_safe_cut_point(messages: &[UnifiedMessage], initial_cut: usize) -> usize {
    let mut cut = initial_cut;

    // Walk backwards until we land on a clean boundary
    while cut > 0 {
        match &messages[cut] {
            m if m.is_tool_result() => {
                // ToolResult without its ToolCall — walk back further
                cut -= 1;
            }
            m if m.is_assistant() && m.has_tool_calls() => {
                // This Assistant's ToolResults are after `cut`.
                // `drain(0..cut)` excludes index `cut`, so this
                // Assistant and its ToolResults are preserved.
                break;
            }
            _ => break, // User or plain Assistant — clean boundary
        }
    }

    cut
}
```

**`remove_oldest_complete_round`:**

Starting from index 1 (after truncation notice):
1. If `messages[1]` is `User` or plain `Assistant` → remove just that message
2. If `messages[1]` is `Assistant` with tool calls → count how many `ToolResult`s follow it, remove the group

```rust
fn remove_oldest_complete_round(messages: &mut Vec<UnifiedMessage>) {
    if messages.len() <= 2 { return; }

    let first = &messages[1];
    if first.is_assistant() && first.has_tool_calls() {
        // Find end of this round: count consecutive ToolResults after it
        let mut end = 2;
        while end < messages.len() && messages[end].is_tool_result() {
            end += 1;
        }
        messages.drain(1..end);
    } else {
        messages.remove(1);
    }
}
```

**Invariant**: After `enforce_context_limit`, every `ToolResult` in `messages` has a corresponding `ToolCall` in a preceding `Assistant` message, and vice versa.

### 4. Empty Session Early Return

**One-line change** in `SessionCompactor::prepare_history`:

```rust
pub async fn prepare_history(...) -> Vec<UnifiedMessage> {
    self.metrics.prepare_history_calls.fetch_add(1, Ordering::Relaxed);

    if !self.config.enabled {
        let raw = agent.get_history(session_key, None).await;
        return raw.into_iter().map(|m| session_message_to_unified(&m)).collect();
    }

    let raw_messages = agent.get_history(session_key, None).await;

    // NEW: Short session — no compression needed, skip LanceDB query
    if raw_messages.len() <= self.config.fresh_tail_count {
        return raw_messages.iter().map(session_message_to_unified).collect();
    }

    // ... existing LanceDB query + assembly logic (unchanged) ...
}
```

This eliminates unnecessary LanceDB queries for new or short sessions.

### 5. Panel Execution Settings

**Backend: New handler `execution_config.rs`**

Following the established pattern (`general_config.rs`, `browser_config.rs`):

- `execution_config.get` → reads `Config.execution`, returns `ExecutionConfig`
- `execution_config.update` → validates, writes, calls `save_incremental(&["execution"])`, broadcasts `ConfigChanged`

Validation rules:
- `default_timeout_secs`: min 60 (1 minute), max 604_800 (7 days)
- `max_iterations`: min 5, max 10_000

**Frontend: New `execution.rs` settings page**

Two fields:
1. **Default Timeout** — numeric input (seconds) + human-readable label (e.g., "48 hours")
2. **Max Iterations** — numeric input

Follows `section_save!` macro + `RwSignal` pattern from existing pages.

**Agent-level override** (in existing Agent edit UI):
- Optional "Timeout Override" field (seconds). Empty = use global default.
- Display as human-readable (e.g., "2 hours") with raw seconds in tooltip.

---

## Files Changed

### Modified

| File | Change |
|------|--------|
| `src/agent_loop/loop_core.rs` | Remove `timeout_secs` from `LoopConfig`, delete timeout check in loop, rewrite `enforce_context_limit` for pair-awareness |
| `src/gateway/execution_engine/mod.rs` | Update `ExecutionEngineConfig` default to 48h |
| `src/gateway/execution_engine/engine.rs` | Replace fixed sleep with resettable deadline, pass deadline to `run_agent_loop`, add cascade timeout resolution |
| `src/gateway/execution_engine/run_loop.rs` | Accept deadline param, wrap compression in deadline extension, **delete duplicate timeout resolution at lines 151-153** |
| `src/config/types/orchestrator.rs` | Update default timeout to 48h (vestigial — `timeout()` method is defined but never called in execution path; changed for consistency only) |
| `src/memory/session_compactor/mod.rs` | Add early return in `prepare_history` for short sessions |
| `src/gateway/agent_instance.rs` | Add `timeout_secs: Option<u64>` to `AgentInstanceConfig` |
| `src/gateway/handlers/mod.rs` | Register `execution_config` handler |
| `src/config/types/mod.rs` | Add `execution: ExecutionConfig` field to `Config` |
| `interfaces/webchat/src/views/settings/mod.rs` | Add Execution tab |

### New Files

| File | Purpose |
|------|---------|
| `src/config/types/execution.rs` | `ExecutionConfig` struct with serde + JsonSchema |
| `src/gateway/handlers/execution_config.rs` | RPC handler for `execution_config.get` / `execution_config.update` |
| `interfaces/webchat/src/views/settings/execution.rs` | Panel Execution settings page |

### Deleted Code

| Location | What |
|----------|------|
| `LoopConfig.timeout_secs` | Field and its default |
| `loop_core.rs:339-342` | `if start.elapsed().as_secs() >= self.config.timeout_secs` block |

---

## Testing Strategy

1. **Unit tests for pair-aware truncation**: Verify that `enforce_context_limit` never produces orphaned ToolResults or ToolCalls. Test with various message patterns (nested tool calls, consecutive tool calls, edge cases).
2. **Unit tests for `find_safe_cut_point`**: Verify boundary detection across all message type combinations.
3. **Unit test for `remove_oldest_complete_round`**: Verify group removal for tool-call rounds. Add `debug_assert!(messages[0].is_user())` precondition check — this function assumes index 0 is the truncation notice.
4. **Unit test for cascade timeout resolution**: Verify `request > agent > global` priority.
5. **Integration test for deadline extension**: Mock a slow `prepare_history`, verify the deadline is extended and the run doesn't timeout prematurely.
6. **Integration test for short session early return**: Verify no LanceDB query when session length <= fresh_tail_count.
7. **Update existing tests**: ~14 test cases in `loop_core.rs` construct `LoopConfig` with `timeout_secs` field — all must be updated to remove this field.
8. **Reconcile `AgentInstanceConfig.max_loops`** (currently defaults to 100) with the new `LoopConfig.max_iterations` default of 200. The agent-level `max_loops` takes precedence at runtime, so existing agents are unaffected. New agents without explicit config get 200.

---

## Design Philosophy: Aleph vs OpenClaw

| Aspect | OpenClaw | Aleph |
|--------|----------|-------|
| Orphan repair | Runtime detection + repair after the fact | Prevention by construction — truncation never creates orphans |
| Compression timeout | Fixed +15min grace period | Precise deadline extension — compression time excluded exactly |
| Empty session | Aggregate timeout + idle detection loop | Early return — don't enter compression at all |
| Timeout architecture | Single configurable default | Three-layer cascade (request > agent > global) with Panel UI |
| Language advantage | JS timer limits (max ~24.8 days) | Rust `Duration` — no artificial ceiling |

# Lifecycle Observability Logging — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add ~15 new lifecycle log points to 5 priority subsystems (Agent Loop, Memory, Resilience, Dispatcher, Thinker) so post-run log analysis can verify which functional chains were actually activated.

**Architecture:** Direct `tracing::info!` / `tracing::debug!` insertion at lifecycle points. "First activation" logs use `AtomicBool` to fire only once. No new abstractions, traits, or dependencies.

**Tech Stack:** `tracing` crate (already in workspace), `std::sync::atomic::AtomicBool`

**Design doc:** `docs/plans/2026-03-04-lifecycle-observability-logging-design.md`

**Note on existing coverage:** Several subsystems already have partial logging. This plan only adds NEW log points — it does NOT modify existing logs. Subsystems already covered: compression service (start+end), meta-cognition (trigger+result), recovery manager (scan+resume), governor (init+acquire), thinker (model override, raw response, parse failure).

---

### Task 1: Agent Loop — Lifecycle Logs

**Files:**
- Modify: `core/src/agent_loop/agent_loop.rs`

**Step 1: Add tracing import and AtomicBool**

At the top of `agent_loop.rs` (after line 4), add:

```rust
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
```

And add a module-level static (after the imports, before `fn extract_affected_files`):

```rust
static FIRST_CYCLE_LOGGED: AtomicBool = AtomicBool::new(false);
```

**Step 2: Add "initialized" log**

After line 357 (`callback.on_loop_start(&state).await;`), insert:

```rust
        tracing::info!(
            subsystem = "agent_loop",
            event = "initialized",
            session_id = %state.session_id,
            "agent loop initialized"
        );
```

**Step 3: Add "first_cycle_started" log**

At line 366, right before `loop {`, insert:

```rust
        // Reset first-cycle flag for this process lifetime
        // (fires once per process, proving the agent loop path is exercised)
```

Then inside the loop, after the abort check block (after line 377), insert:

```rust
            if !FIRST_CYCLE_LOGGED.swap(true, AtomicOrdering::Relaxed) {
                tracing::info!(
                    subsystem = "agent_loop",
                    event = "first_cycle_started",
                    session_id = %state.session_id,
                    "agent loop entered first execution cycle"
                );
            }
```

**Step 4: Add "session_completed" logs at all exit paths**

There are 8 return paths. Add a `tracing::info!` before each `return LoopResult::*`:

Before line 375 (`return LoopResult::UserAborted`):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "user_aborted",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
```

Before line 436 (`return LoopResult::GuardTriggered(violation)`):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "guard_triggered",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
```

Before line 498 (`return LoopResult::Failed { ... }`):
```rust
                        tracing::info!(
                            subsystem = "agent_loop",
                            event = "session_completed",
                            session_id = %state.session_id,
                            result = "thinking_failed",
                            steps = state.step_count,
                            "agent loop session ended"
                        );
```

Before line 527 (`return LoopResult::Completed { ... }` — Decision::Complete):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "completed",
                        steps = state.step_count,
                        total_tokens = state.total_tokens,
                        "agent loop session ended"
                    );
```

Before line 539 (`return LoopResult::Failed { ... }` — Decision::Fail):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "decision_failed",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
```

Before line 651 (`return LoopResult::GuardTriggered(final_violation)` — doom loop):
```rust
                            tracing::info!(
                                subsystem = "agent_loop",
                                event = "session_completed",
                                session_id = %state.session_id,
                                result = "doom_loop",
                                steps = state.step_count,
                                "agent loop session ended"
                            );
```

Before line 707 (`return LoopResult::Completed { ... }` — Decision::Silent):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "silent",
                        steps = state.step_count,
                        total_tokens = state.total_tokens,
                        "agent loop session ended"
                    );
```

Before line 719 (`return LoopResult::Completed { ... }` — Decision::HeartbeatOk):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "heartbeat_ok",
                        steps = state.step_count,
                        total_tokens = state.total_tokens,
                        "agent loop session ended"
                    );
```

Before line 819 (`return LoopResult::GuardTriggered(violation)` — PoeStrategySwitch):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "poe_strategy_switch",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
```

Before line 826 (`return LoopResult::PoeAborted { reason }`):
```rust
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "poe_aborted",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
```

**Step 5: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 6: Commit**

```bash
git add core/src/agent_loop/agent_loop.rs
git commit -m "agent_loop: add lifecycle observability logs"
```

---

### Task 2: Memory System — Lifecycle Logs

**Files:**
- Modify: `core/src/memory/store/lance/mod.rs`
- Modify: `core/src/memory/store/lance/facts.rs`

**Step 1: Add store_initialized log to LanceMemoryBackend**

In `core/src/memory/store/lance/mod.rs`, before the `Ok(Self { ... })` return at line 64, insert:

```rust
        tracing::info!(
            subsystem = "memory",
            event = "store_initialized",
            backend = "lancedb",
            db_path = %db_path.display(),
            "LanceDB memory store initialized"
        );
```

**Step 2: Add first_write and first_read logs to facts.rs**

In `core/src/memory/store/lance/facts.rs`, add at the top (after existing imports):

```rust
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

static FIRST_WRITE_LOGGED: AtomicBool = AtomicBool::new(false);
static FIRST_READ_LOGGED: AtomicBool = AtomicBool::new(false);
```

In `insert_fact()` (line 149), at the start of the function body, insert:

```rust
        if !FIRST_WRITE_LOGGED.swap(true, AtomicOrdering::Relaxed) {
            tracing::info!(
                subsystem = "memory",
                event = "first_write",
                table = "facts",
                fact_id = %fact.id,
                "memory store received first fact write"
            );
        }
```

In `vector_search()` (line 191), at the start of the function body (after line 197), insert:

```rust
        if !FIRST_READ_LOGGED.swap(true, AtomicOrdering::Relaxed) {
            tracing::info!(
                subsystem = "memory",
                event = "first_read",
                table = "facts",
                dim = dim_hint,
                limit = limit,
                "memory store received first vector search"
            );
        }
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/mod.rs core/src/memory/store/lance/facts.rs
git commit -m "memory: add lifecycle observability logs for store init and first read/write"
```

---

### Task 3: Resilience / StateDatabase — Lifecycle Logs

**Files:**
- Modify: `core/src/resilience/database/state_database.rs`
- Modify: `core/src/resilience/database/events.rs`

**Step 1: Add database_initialized log**

In `core/src/resilience/database/state_database.rs`, before the `Ok(Self { ... })` return at line 584, insert:

```rust
        tracing::info!(
            subsystem = "resilience",
            event = "database_initialized",
            db_path = %db_path.display(),
            embedding_dim = DEFAULT_EMBEDDING_DIM,
            "StateDatabase initialized"
        );
```

**Step 2: Add first_event_recorded log**

In `core/src/resilience/database/events.rs`, add at the top (after line 9):

```rust
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

static FIRST_EVENT_LOGGED: AtomicBool = AtomicBool::new(false);
```

In `insert_event()` (line 18), after the successful `conn.execute(...)` call (before `Ok(conn.last_insert_rowid())`), insert:

```rust
        if !FIRST_EVENT_LOGGED.swap(true, AtomicOrdering::Relaxed) {
            tracing::info!(
                subsystem = "resilience",
                event = "first_event_recorded",
                event_type = %event.event_type,
                task_id = %event.task_id,
                "StateDatabase recorded first event"
            );
        }
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add core/src/resilience/database/state_database.rs core/src/resilience/database/events.rs
git commit -m "resilience: add lifecycle observability logs for database init and first event"
```

---

### Task 4: Dispatcher — Lifecycle Logs

**Files:**
- Modify: `core/src/dispatcher/engine/core.rs`

**Step 1: Add dag_constructed log**

In `core/src/dispatcher/engine/core.rs`, the `execute()` function at line 283 already logs `"Executing task graph: ..."`. We need a more structured lifecycle log. After line 295 (after the existing `info!` call), insert:

```rust
        info!(
            subsystem = "dispatcher",
            event = "dag_constructed",
            graph_id = %graph.id,
            node_count = graph.tasks.len(),
            edge_count = graph.edges.len(),
            "dispatcher task graph ready for execution"
        );
```

**Step 2: Add task_execution_started log**

In the task execution loop, after line 372 (`self.monitor.on_task_start(&task);`), insert:

```rust
                info!(
                    subsystem = "dispatcher",
                    event = "task_execution_started",
                    task_id = %task_id,
                    task_name = %task.name,
                    "dispatcher starting task execution"
                );
```

**Step 3: Add task_execution_completed log**

In the results processing loop, after line 387 (`debug!("Task {} completed successfully", task_id);`), insert:

```rust
                        info!(
                            subsystem = "dispatcher",
                            event = "task_execution_completed",
                            task_id = %task_id,
                            status = "success",
                            "dispatcher task completed"
                        );
```

After line 401 (`error!("Task {} failed: {}", task_id, e);`), insert:

```rust
                        info!(
                            subsystem = "dispatcher",
                            event = "task_execution_completed",
                            task_id = %task_id,
                            status = "failed",
                            "dispatcher task completed"
                        );
```

**Step 4: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 5: Commit**

```bash
git add core/src/dispatcher/engine/core.rs
git commit -m "dispatcher: add lifecycle observability logs for DAG and task execution"
```

---

### Task 5: Thinker — Lifecycle Logs

**Files:**
- Modify: `core/src/thinker/mod.rs`

**Step 1: Add provider_selected info-level log**

The thinker already logs model selection at `debug!` level (line 221). We need an `info!`-level log that confirms the provider is actually being used. In the `ThinkerTrait::think_with_level()` implementation (line 408), after the provider is obtained (after line 439), insert:

```rust
        tracing::info!(
            subsystem = "thinker",
            event = "provider_selected",
            model = %model_id.as_str(),
            provider = %provider.name(),
            think_level = %level,
            tool_count = filtered_tools.len(),
            "thinker selected provider for LLM call"
        );
```

**Step 2: Add response_completed info-level log**

After the token estimation (after line 474 `thinking.tokens_used = Some(estimated_tokens);`), insert:

```rust
        tracing::info!(
            subsystem = "thinker",
            event = "response_completed",
            model = %model_id.as_str(),
            estimated_tokens = estimated_tokens,
            response_len = response.len(),
            decision_type = %thinking.decision.variant_name(),
            "thinker LLM response completed"
        );
```

**Note:** If `Decision` doesn't have a `variant_name()` method, use a simpler approach:

```rust
        let decision_type = match &thinking.decision {
            crate::agent_loop::decision::Decision::Complete { .. } => "complete",
            crate::agent_loop::decision::Decision::Fail { .. } => "fail",
            crate::agent_loop::decision::Decision::ToolCall { .. } => "tool_call",
            crate::agent_loop::decision::Decision::AskUser { .. } => "ask_user",
            crate::agent_loop::decision::Decision::Silent => "silent",
            crate::agent_loop::decision::Decision::HeartbeatOk => "heartbeat_ok",
            _ => "other",
        };
        tracing::info!(
            subsystem = "thinker",
            event = "response_completed",
            model = %model_id.as_str(),
            estimated_tokens = estimated_tokens,
            response_len = response.len(),
            decision_type = decision_type,
            "thinker LLM response completed"
        );
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add core/src/thinker/mod.rs
git commit -m "thinker: add lifecycle observability logs for provider selection and response"
```

---

### Task 6: Integration Verification

**Step 1: Full build**

Run: `cargo build -p alephcore`
Expected: Compiles with no errors and no warnings about unused imports

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests still pass (pre-existing failures in `markdown_skill::loader::tests` are known and expected)

**Step 3: Final commit (if any fixups needed)**

```bash
git add -A
git commit -m "observability: fix any compilation issues from lifecycle logs"
```

---

## Summary of Changes

| File | New Log Points | Type |
|------|---------------|------|
| `core/src/agent_loop/agent_loop.rs` | 12 | initialized, first_cycle, 10 exit paths |
| `core/src/memory/store/lance/mod.rs` | 1 | store_initialized |
| `core/src/memory/store/lance/facts.rs` | 2 | first_write, first_read |
| `core/src/resilience/database/state_database.rs` | 1 | database_initialized |
| `core/src/resilience/database/events.rs` | 1 | first_event_recorded |
| `core/src/dispatcher/engine/core.rs` | 4 | dag_constructed, task_start, task_complete×2 |
| `core/src/thinker/mod.rs` | 2 | provider_selected, response_completed |
| **Total** | **23** | |

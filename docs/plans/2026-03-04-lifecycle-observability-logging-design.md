# Lifecycle Observability Logging Design

> Date: 2026-03-04
> Status: Approved
> Scope: Core subsystem lifecycle logging for functional chain verification

## Problem

Aleph Core has ~710 log statements across 191 files, but coverage is extremely uneven. Critical subsystems (Agent Loop, Memory, Resilience) operate in complete darkness — there's no way to verify at runtime whether designed functional logic chains are actually being exercised, or just sitting there unused.

## Goal

Add **lifecycle-level confirmation logs** to 5 priority subsystems so that post-run log analysis can answer: *"Was this functional chain actually activated during this session?"*

**Non-goals**: Request-level span tracing, `#[instrument]` macros, structured span hierarchies, health-check APIs, startup report dashboards.

## Approach

**Direct tracing macros** (方案 A) — insert `info!` / `debug!` calls at lifecycle points in existing code. Zero new dependencies, zero new abstractions.

## Log Strategy & Conventions

### Level Rules

| Event Type | Level | Example |
|------------|-------|---------|
| Subsystem initialized | `info!` | `agent_loop: initialized` |
| First activation (proves it's actually used) | `info!` | `memory: first_write` |
| Key state transitions | `debug!` | `agent_loop: state_transition` |
| Session/task completion stats | `info!` | `agent_loop: session_completed` |

### Format Convention

```rust
info!(
    subsystem = "agent_loop",
    event = "initialized",
    session_id = %session_id,
    "agent loop initialized"
);
```

- `subsystem` field identifies the subsystem (greppable)
- `event` field identifies the lifecycle event type
- Message text is short, greppable, English
- No user data logged (PII filter layer exists as safety net)

### "First Activation" Pattern

```rust
use std::sync::atomic::{AtomicBool, Ordering};

static FIRST_WRITE_LOGGED: AtomicBool = AtomicBool::new(false);

if !FIRST_WRITE_LOGGED.swap(true, Ordering::Relaxed) {
    info!(subsystem = "memory", event = "first_write", table = "facts",
          "memory store received first write");
}
```

## Log Points by Subsystem

### 1. Agent Loop (DARK → Observable) — 5 points

| Location | Event | Level | Fields |
|----------|-------|-------|--------|
| `agent_loop.rs` — loop start | `initialized` | info | session_id |
| `agent_loop.rs` — first cycle entry | `first_cycle_started` | info | — |
| `agent_loop.rs` — each state transition | `state_transition` | debug | from, to |
| `agent_loop.rs` — loop end | `session_completed` | info | total_cycles, duration |
| `meta_cognition.rs` — reflection triggered | `meta_cognition_triggered` | info | reason |

### 2. Memory System (DARK → Observable) — 5 points

| Location | Event | Level | Fields |
|----------|-------|-------|--------|
| Store initialization | `store_initialized` | info | backend |
| First write | `first_write` | info | table |
| First read | `first_read` | info | query_type |
| Compression pipeline start | `compression_started` | info | strategy |
| Compression complete | `compression_completed` | info | facts_before, facts_after |

### 3. Resilience / StateDatabase (DARK → Observable) — 5 points

| Location | Event | Level | Fields |
|----------|-------|-------|--------|
| StateDatabase created | `database_initialized` | info | path |
| First event written | `first_event_recorded` | info | event_type |
| Recovery flow started | `recovery_started` | info | — |
| Recovery completed | `recovery_completed` | info | recovered_tasks_count |
| Governor decision | `governor_decision` | debug | action, reason |

### 4. Dispatcher (PARTIAL → Complete) — 3 points

| Location | Event | Level | Fields |
|----------|-------|-------|--------|
| DAG constructed | `dag_constructed` | info | node_count, edge_count |
| Task execution started | `task_execution_started` | info | task_id |
| Task completed | `task_execution_completed` | info | task_id, status |

### 5. Thinker (PARTIAL → Complete) — 3 points

| Location | Event | Level | Fields |
|----------|-------|-------|--------|
| Provider selected | `provider_selected` | info | provider, model |
| Stream started | `stream_started` | debug | — |
| Response completed | `response_completed` | info | tokens_used, duration |

**Total: ~23 log points across 5 subsystems.**

## Implementation Constraints

1. **Zero new dependencies** — only use existing `tracing` crate
2. **Zero new abstractions** — no new traits/structs, direct macro insertion
3. **"First activation" pattern** — use `AtomicBool` for once-only logs
4. **Additive only** — existing `warn!`/`error!` logs untouched
5. **Negligible performance impact** — info-level logs fire 3-5 times per subsystem lifecycle

## Expected Output

After a normal session with one complete conversation:

```
[INFO] agent_loop: initialized (session_id=abc123)
[INFO] memory: store_initialized (backend=lancedb)
[INFO] resilience: database_initialized (path=~/.aleph/state.db)
[INFO] dispatcher: dag_constructed (nodes=3, edges=2)
[INFO] thinker: provider_selected (provider=claude, model=opus-4)
[INFO] agent_loop: first_cycle_started
[INFO] memory: first_write (table=facts)
[INFO] memory: first_read (query_type=hybrid)
[INFO] thinker: response_completed (tokens=1234, duration_ms=2300)
[INFO] agent_loop: session_completed (cycles=12, duration_ms=45000)
```

**Diagnostic rule**: If a subsystem's logs **never appear**, that functional chain was not activated at runtime — which is exactly what we want to detect.

## Future Extensions (Not In Scope)

- Request-level `#[instrument]` span tracing
- Startup summary report (`system.status` RPC)
- Metrics collection alongside traces
- Correlation IDs for end-to-end request tracking

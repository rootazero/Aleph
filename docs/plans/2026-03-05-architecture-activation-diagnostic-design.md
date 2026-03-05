# Architecture Activation Diagnostic Design

> Date: 2026-03-05
> Status: Approved
> Scope: One-time runtime diagnostic to verify whether Aleph's designed architecture (POE, multi-agent, DAG scheduling) is actually exercised during task execution

## Problem

Aleph has ~60,000+ LOC implementing POE, Swarm, DAG Scheduler, Resilience, and other advanced architectural systems. However, **code existence does not equal runtime activation**. There is no evidence that complex tasks actually exercise these systems rather than falling through to a simple Agent Loop -> Thinker -> stream response path.

## Goal

Run three tasks of increasing complexity through Telegram Bot, collect structured logs, and produce a diagnostic report answering: *"Is Aleph actually using its designed architecture, or just doing simple stream processing?"*

## Approach

**Phase 1**: Supplement lifecycle observability logging (existing design + 7 new POE/Swarm/GroupChat probes)
**Phase 2**: Run three diagnostic tasks via Telegram Bot
**Phase 3**: Analyze logs and produce report with evidence + improvement suggestions

## Phase 1: Lifecycle Probes

### Existing probes (from lifecycle-observability-logging-design.md) — 23 points

| Subsystem | Points | Key Events |
|-----------|--------|------------|
| Agent Loop | 12 | initialized, first_cycle_started, session_completed (10 exit paths) |
| Memory | 3 | store_initialized, first_write, first_read |
| Resilience | 2 | database_initialized, first_event_recorded |
| Dispatcher | 4 | dag_constructed, task_execution_started, task_execution_completed |
| Thinker | 2 | provider_selected, response_completed |

### New probes (architecture-specific) — 7 points

| Subsystem | Event | Level | Fields | Location |
|-----------|-------|-------|--------|----------|
| POE Lazy | `validation_triggered` | info | session_id, summary_len | `poe/lazy_evaluator.rs` — when validation is called |
| POE Lazy | `validation_result` | info | session_id, passed (bool), hint | `poe/lazy_evaluator.rs` — after validation decision |
| POE Full | `manifest_created` | info | task_id, hard_constraints, soft_metrics | `poe/manager.rs` — when P->O->E loop starts |
| POE Full | `poe_loop_started` | info | task_id, max_attempts | `poe/manager.rs` — entering the execute loop |
| Swarm | `event_published` | info | event_type, agent_id | `agents/swarm/coordinator.rs` — when swarm event is published |
| Swarm | `context_injected` | info | agent_id, context_len | `agents/swarm/coordinator.rs` — when collective context is injected |
| Group Chat | `session_created` | info | session_id, persona_count | `group_chat/orchestrator.rs` — when group chat session starts |

**Total: 30 probes across 9 subsystems.**

### "First Activation" Pattern

```rust
use std::sync::atomic::{AtomicBool, Ordering};

static FIRST_WRITE_LOGGED: AtomicBool = AtomicBool::new(false);

if !FIRST_WRITE_LOGGED.swap(true, Ordering::Relaxed) {
    tracing::info!(subsystem = "memory", event = "first_write", ...);
}
```

Used for events that may fire many times but we only need to confirm activation once.

## Phase 2: Diagnostic Tasks

Three tasks via Telegram Bot, executed sequentially (wait for completion between each):

### Task A: Simple Q&A (baseline)

```
"What is the weather like today?"
```

**Expected activation**: Agent Loop, Thinker
**Expected silent**: Memory, Dispatcher DAG, POE, Swarm, Group Chat

### Task B: Tool-calling

```
"Help me check who I've chatted with recently and summarize it"
```

**Expected activation**: Agent Loop, Thinker, Memory (read), tool execution
**Possibly activated**: POE Lazy Evaluator
**Expected silent**: Dispatcher DAG, Swarm, Full POE

### Task C: Complex multi-step

```
"Analyze the topic distribution of all my conversations this past week, generate a markdown report with conversation counts and key summaries for each topic"
```

**Expected activation**: Agent Loop (multi-round), Thinker, Memory (multi-read/write), Dispatcher DAG (multi-step decomposition), POE Lazy Evaluator
**Possibly activated**: Full POE, Swarm
**Expected silent**: Group Chat (no multi-persona scenario)

## Phase 3: Analysis & Report

### Analysis Method

Extract logs from `~/.aleph/logs/aleph-server.log.{date}`, aggregate by `subsystem=` and `event=` fields.

### Activation Matrix

| Subsystem | grep pattern | Pass rule |
|-----------|-------------|-----------|
| Agent Loop | `subsystem="agent_loop"` | initialized + session_completed present |
| Memory | `subsystem="memory"` | first_write or first_read present |
| Resilience | `subsystem="resilience"` | database_initialized present |
| Dispatcher DAG | `subsystem="dispatcher" event="dag_constructed"` | present with node_count > 1 |
| Thinker | `subsystem="thinker"` | provider_selected present |
| POE Lazy | `subsystem="poe_lazy"` | validation_triggered present |
| POE Full | `subsystem="poe"` | manifest_created present |
| Swarm | `subsystem="swarm"` | event_published present |
| Group Chat | `subsystem="group_chat"` | session_created present |

### Report Format

```markdown
# Aleph Architecture Activation Diagnostic Report
> Date: YYYY-MM-DD

## Activation Matrix

| Subsystem | Task A (Q&A) | Task B (Tool) | Task C (Complex) | Status |
|-----------|:---:|:---:|:---:|--------|
| Agent Loop | ? | ? | ? | |
| Thinker | ? | ? | ? | |
| Memory | ? | ? | ? | |
| Dispatcher DAG | ? | ? | ? | |
| POE Lazy | ? | ? | ? | |
| POE Full | ? | ? | ? | |
| Swarm | ? | ? | ? | |
| Group Chat | ? | ? | ? | |
| Resilience | ? | ? | ? | |

## Evidence (key log lines per cell)

## Diagnostic Conclusion
- Fully utilized / Partially utilized / Simple stream processing

## Improvement Suggestions
- Per-subsystem analysis of why not activated + code/config fix proposals
```

### Judgment Criteria

| Verdict | Condition |
|---------|-----------|
| **Fully utilized** | Task C triggers DAG + POE + Memory multi-round |
| **Partially utilized** | Basic pipeline works but POE/DAG/Swarm never trigger |
| **Simple stream processing** | All tasks only trigger Agent Loop -> Thinker -> return |

## Implementation Constraints

1. Zero new dependencies — only `tracing` crate + `std::sync::atomic`
2. Zero new abstractions — direct macro insertion
3. Additive only — existing logs untouched
4. Temporary debug level — restore to info after diagnostic
5. Sequential task execution — avoid log interleaving

## Dependencies

- Existing design: `docs/plans/2026-03-04-lifecycle-observability-logging-design.md`
- Existing impl plan: `docs/plans/2026-03-04-lifecycle-observability-logging-impl.md`
- This design extends the above with 7 additional POE/Swarm/GroupChat probes

# Architecture Activation Diagnostic — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add 7 new lifecycle probes to POE/Swarm/GroupChat subsystems, run 3 diagnostic tasks via Telegram, analyze logs, and produce an architecture activation report.

**Architecture:** Direct `tracing::info!` insertion at lifecycle points. Existing 23 probes already implemented. Only new POE/Swarm/GroupChat probes needed.

**Tech Stack:** `tracing` crate (already in workspace), `std::sync::atomic::AtomicBool`

**Design doc:** `docs/plans/2026-03-05-architecture-activation-diagnostic-design.md`

---

### Task 1: POE Lazy Evaluator — 2 Probes

**Files:**
- Modify: `core/src/poe/lazy_evaluator.rs`

**Step 1: Add validation_triggered probe to validate_completion**

At line 327, the `validate_completion` method starts. Insert a tracing log right after the `is_active()` check (after line 332), before the 3 checks begin:

```rust
// After line 332 (after the early return for !is_active())
// Before line 334 (// Check 1: No tools invoked)

        tracing::info!(
            subsystem = "poe_lazy",
            event = "validation_triggered",
            tools_invoked = manifest.tools_invoked.len(),
            retries_remaining = manifest.retries_remaining(),
            "POE lazy evaluator validation triggered at completion"
        );
```

**Step 2: Add validation_result probe at end of validate_completion**

Replace the final `None` at line 357 with a logged version:

```rust
// Replace line 357:  None
// With:
        tracing::info!(
            subsystem = "poe_lazy",
            event = "validation_result",
            passed = true,
            "POE lazy evaluator validation passed"
        );
        None
```

Also add a log before each `return Some(...)` at lines 336, 345-346, and 352-353. Since there are 3 failure paths, add a single log right before each return. The simplest approach: wrap the entire function exit to log failures. Instead, add one probe at each `return Some`:

Before line 336 (`return Some(...)`):
```rust
            tracing::info!(
                subsystem = "poe_lazy",
                event = "validation_result",
                passed = false,
                reason = "no_tools_invoked",
                "POE lazy evaluator validation failed"
            );
```

Before line 345-346 (`return Some(hint)`):
```rust
                tracing::info!(
                    subsystem = "poe_lazy",
                    event = "validation_result",
                    passed = false,
                    reason = "hallucination_detected",
                    "POE lazy evaluator validation failed"
                );
```

Before line 352-353 (`return Some(hint)`):
```rust
                tracing::info!(
                    subsystem = "poe_lazy",
                    event = "validation_result",
                    passed = false,
                    reason = "low_query_relevance",
                    "POE lazy evaluator validation failed"
                );
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add core/src/poe/lazy_evaluator.rs
git commit -m "poe: add lifecycle observability probes to lazy evaluator"
```

---

### Task 2: POE Full Manager — 2 Probes

**Files:**
- Modify: `core/src/poe/manager.rs`

**Step 1: Add manifest_created probe**

At line 296 (after the existing `emit_event` for ManifestCreated), insert:

```rust
// After line 296 (after the closing `});` of emit_event)

        tracing::info!(
            subsystem = "poe",
            event = "manifest_created",
            task_id = %task.manifest.task_id,
            objective = %task.manifest.objective,
            hard_constraints = task.manifest.hard_constraints.len(),
            soft_metrics = task.manifest.soft_metrics.len(),
            max_attempts = task.manifest.max_attempts,
            "POE full manager created success manifest"
        );
```

**Step 2: Add poe_loop_started probe**

At line 328 (before `while !budget.exhausted()`), insert:

```rust
// Before line 328 (before `// Main P->O->E loop`)

        tracing::info!(
            subsystem = "poe",
            event = "poe_loop_started",
            task_id = %task.manifest.task_id,
            max_attempts = task.manifest.max_attempts,
            max_tokens = self.config.max_tokens,
            "POE full manager entering P->O->E execution loop"
        );
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add core/src/poe/manager.rs
git commit -m "poe: add lifecycle observability probes to full manager"
```

---

### Task 3: Swarm Coordinator — 2 Probes

**Files:**
- Modify: `core/src/agents/swarm/coordinator.rs`
- Modify: `core/src/agents/swarm/context_injector.rs`

**Step 1: Add event_published probe**

In `coordinator.rs`, at line 200-203 (the publish to bus block), replace with:

```rust
        // Publish to bus
        let event_type = match &swarm_event {
            AgentEvent::Critical(_) => "critical",
            AgentEvent::Important(_) => "important",
            AgentEvent::Info(_) => "info",
        };
        tracing::info!(
            subsystem = "swarm",
            event = "event_published",
            event_tier = event_type,
            "swarm coordinator published event to bus"
        );
        if let Err(e) = self.bus.publish(swarm_event).await {
            tracing::warn!("Failed to publish swarm event: {}", e);
        }
```

Note: Check that `AgentEvent` has `Critical`, `Important`, `Info` variants. The code at lines 149-197 confirms these are the three variants.

**Step 2: Add context_injected probe**

In `context_injector.rs`, in the `inject_swarm_state` method (line 135), add a log after collecting recent_updates (after line 137, before the empty check):

```rust
    pub async fn inject_swarm_state(&self, _agent_id: &str) -> String {
        let window = self.context_window.read().await;
        let recent_updates = window.get_recent(DEFAULT_CONTEXT_WINDOW_SIZE);

        // Add this probe:
        if !recent_updates.is_empty() {
            tracing::info!(
                subsystem = "swarm",
                event = "context_injected",
                entries = recent_updates.len(),
                "swarm context injector providing team awareness to agent"
            );
        }

        if recent_updates.is_empty() {
            return String::new();
        }
        // ... rest unchanged
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 4: Commit**

```bash
git add core/src/agents/swarm/coordinator.rs core/src/agents/swarm/context_injector.rs
git commit -m "swarm: add lifecycle observability probes for event publishing and context injection"
```

---

### Task 4: Group Chat Orchestrator — 1 Probe

**Files:**
- Modify: `core/src/group_chat/orchestrator.rs`

**Step 1: Add session_created probe**

After line 94 (`self.sessions.insert(...)`), insert:

```rust
        tracing::info!(
            subsystem = "group_chat",
            event = "session_created",
            session_id = %session_id,
            persona_count = participants.len(),
            "group chat session created"
        );
```

Note: `participants` is defined at line 80 and still in scope. Verify the field name — it's `participants` from `self.persona_registry.resolve(&sources)?`.

Wait — `participants` is consumed by `GroupChatSession::new` at line 86-92. We need to capture the count before that. Add the count capture before the session creation:

At line 80, after `let participants = ...`:
```rust
        let participants = self.persona_registry.resolve(&sources)?;
        let participant_count = participants.len();  // capture before move
```

Then at line 94, after `self.sessions.insert(...)`:
```rust
        tracing::info!(
            subsystem = "group_chat",
            event = "session_created",
            session_id = %session_id,
            persona_count = participant_count,
            "group chat session created"
        );
```

**Step 2: Build and verify**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 3: Commit**

```bash
git add core/src/group_chat/orchestrator.rs
git commit -m "group_chat: add lifecycle observability probe for session creation"
```

---

### Task 5: Full Build + Test

**Step 1: Full build**

Run: `cargo build -p alephcore`
Expected: Compiles with no errors and no warnings about unused imports

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests still pass (pre-existing failures in `markdown_skill::loader::tests` are known)

**Step 3: Fix any issues**

If compilation errors, fix them and commit:
```bash
git add -A
git commit -m "observability: fix compilation issues from new probes"
```

---

### Task 6: Set Log Level to Debug + Restart Server

**Step 1: Set RUST_LOG environment variable**

The server needs to be restarted with debug-level logging to capture all probes. The user runs their server externally, so provide instructions:

```bash
# Stop current server (Ctrl+C or signal)
# Restart with debug logging:
RUST_LOG=debug cargo run --bin aleph
```

Or if using a release binary:
```bash
RUST_LOG=debug ./aleph
```

**Step 2: Verify server started and logs are flowing**

```bash
tail -f ~/.aleph/logs/aleph-server.log.$(date +%Y-%m-%d) | head -20
```

Expected: See initialization logs like `subsystem="memory" event="store_initialized"` and `subsystem="resilience" event="database_initialized"`

---

### Task 7: Run Diagnostic Tasks via Telegram

**Important**: Run tasks sequentially. Wait for each to complete before sending the next.

**Step 1: Send Task A (simple Q&A)**

Via Telegram Bot, send:
```
今天天气怎么样？
```

Wait for response to complete. Note the approximate timestamp.

**Step 2: Send Task B (tool-calling)**

Via Telegram Bot, send:
```
帮我查一下我最近和谁聊过天，总结一下
```

Wait for response to complete. Note the approximate timestamp.

**Step 3: Send Task C (complex multi-step)**

Via Telegram Bot, send:
```
分析我过去一周所有对话的主题分布，生成一份 markdown 报告，包含每个主题的对话次数和关键摘要
```

Wait for response to complete. Note the approximate timestamp.

---

### Task 8: Collect and Analyze Logs

**Step 1: Copy today's log for analysis**

```bash
cp ~/.aleph/logs/aleph-server.log.$(date +%Y-%m-%d) /tmp/aleph-diagnostic.log
```

**Step 2: Extract activation signals per subsystem**

```bash
# Agent Loop
grep 'subsystem="agent_loop"' /tmp/aleph-diagnostic.log

# Thinker
grep 'subsystem="thinker"' /tmp/aleph-diagnostic.log

# Memory
grep 'subsystem="memory"' /tmp/aleph-diagnostic.log

# Dispatcher
grep 'subsystem="dispatcher"' /tmp/aleph-diagnostic.log

# Resilience
grep 'subsystem="resilience"' /tmp/aleph-diagnostic.log

# POE Lazy
grep 'subsystem="poe_lazy"' /tmp/aleph-diagnostic.log

# POE Full
grep 'subsystem="poe"' /tmp/aleph-diagnostic.log

# Swarm
grep 'subsystem="swarm"' /tmp/aleph-diagnostic.log

# Group Chat
grep 'subsystem="group_chat"' /tmp/aleph-diagnostic.log
```

**Step 3: Correlate logs to tasks by timestamp**

Use the timestamps noted in Task 7 to segment the log into three task windows. For each window, check which subsystem probes fired.

---

### Task 9: Produce Diagnostic Report

**Files:**
- Create: `docs/reports/2026-03-05-architecture-activation-diagnostic-report.md`

**Step 1: Build the activation matrix**

Based on log analysis, fill in the matrix:

```markdown
# Aleph Architecture Activation Diagnostic Report
> Date: 2026-03-05

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

## Evidence
(Key log lines for each cell)

## Diagnostic Conclusion
- Fully utilized / Partially utilized / Simple stream processing

## Improvement Suggestions
(Per-subsystem analysis + concrete code/config proposals)
```

**Step 2: Write conclusion and improvement suggestions**

For each subsystem that shows as inactive in Task C:
1. Identify WHY it wasn't triggered (missing call site? configuration gap? no integration point?)
2. Propose a concrete fix (specific file + what to change)

**Step 3: Commit the report**

```bash
mkdir -p docs/reports
git add docs/reports/2026-03-05-architecture-activation-diagnostic-report.md
git commit -m "docs: add architecture activation diagnostic report"
```

---

## Summary of Changes

| Task | Files Modified | Description |
|------|---------------|-------------|
| 1 | `core/src/poe/lazy_evaluator.rs` | 2 probes: validation_triggered, validation_result |
| 2 | `core/src/poe/manager.rs` | 2 probes: manifest_created, poe_loop_started |
| 3 | `core/src/agents/swarm/coordinator.rs`, `context_injector.rs` | 2 probes: event_published, context_injected |
| 4 | `core/src/group_chat/orchestrator.rs` | 1 probe: session_created |
| 5 | (build + test verification) | — |
| 6 | (server restart) | — |
| 7 | (Telegram tasks) | — |
| 8 | (log analysis) | — |
| 9 | `docs/reports/...` | Diagnostic report |

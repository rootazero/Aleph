# Phase 6a Implementation Plan — Runtime Flip

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `AgentLoop::new(...)` at `src/gateway/execution_engine/run_loop.rs:628`
with `Orchestrator::dispatch(...)`. AgentHarness gains delta streaming, context budget,
compaction, stop-hooks, history, iteration/limit reporting, multimodal input, and
cancel-token parity with the retiring loop_core path — all reusing loop_core's
existing helper modules via `agent_loop::` imports (they get relocated in 6b).

**Architecture:** Additive. `HarnessDeps` grows optional fields. `AgentHarness::run`
gains an optional `&mut dyn HarnessCallback` and a `&CancellationToken`. A new
`FlowInput::History` variant carries multi-turn history. `FlowOutcome` gains parity
fields (`iterations`, `tool_calls_made`, `total_tokens`, `hit_limit`). `run_loop.rs`
constructs a `FlowRequest`, calls `Orchestrator::dispatch`, drains events into its
existing `StreamCallback`, and awaits completion.

**Tech Stack:** Rust, tokio, tokio-util (CancellationToken), broadcast channels,
existing `session/service.rs`, `tools/service.rs`, `sandbox/`, `agent_loop/` helpers
(kept in place for 6a; moved in 6b).

---

## Budget & Invariants

- `cargo test -p alephcore --lib` stays ≥ 9076 passing
- `tests/harness_run_e2e.rs` 2/2 passing
- `scripts/check-phase5-exit.sh` green
- New files added in 6a each < 400 LOC
- No new clippy warnings at `-D warnings`
- **No release until user approves** — 6a ships as merge-to-main only

---

### Task 1: Define `HarnessCallback` trait + plumb into `AgentHarness`

**Files:**
- Create: `src/harness/callback.rs`
- Modify: `src/harness/agent.rs` — accept optional `&mut dyn HarnessCallback` on `run`
- Modify: `src/harness/trait_def.rs` — extend signature
- Modify: `src/harness/mod.rs` — pub use new module
- Test: `src/harness/callback.rs` inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

```rust
// src/harness/callback.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CapturingCallback {
        deltas: Vec<String>,
        tools: Vec<String>,
        completed: bool,
    }

    impl HarnessCallback for CapturingCallback {
        fn on_delta(&mut self, text: &str) { self.deltas.push(text.to_string()); }
        fn on_tool_call(&mut self, name: &str) { self.tools.push(name.to_string()); }
        fn on_complete(&mut self) { self.completed = true; }
    }

    #[test]
    fn trait_captures_lifecycle() {
        let mut cb = CapturingCallback::default();
        cb.on_delta("hello ");
        cb.on_delta("world");
        cb.on_tool_call("read_file");
        cb.on_complete();
        assert_eq!(cb.deltas, vec!["hello ", "world"]);
        assert_eq!(cb.tools, vec!["read_file"]);
        assert!(cb.completed);
    }
}
```

- [ ] **Step 2: Run test — should fail (trait not defined)**

Run: `cargo test -p alephcore --lib harness::callback`
Expected: FAIL — trait `HarnessCallback` unresolved.

- [ ] **Step 3: Define the trait**

```rust
// src/harness/callback.rs
//! Lifecycle callback for AgentHarness turn execution. Mirrors LoopCallback
//! from the retiring agent_loop/loop_core.rs on a narrower surface.

pub trait HarnessCallback: Send {
    fn on_delta(&mut self, text: &str);
    fn on_tool_call(&mut self, name: &str);
    fn on_complete(&mut self);
}

/// No-op implementation for contexts that don't stream.
pub struct NoopHarnessCallback;
impl HarnessCallback for NoopHarnessCallback {
    fn on_delta(&mut self, _: &str) {}
    fn on_tool_call(&mut self, _: &str) {}
    fn on_complete(&mut self) {}
}
```

- [ ] **Step 4: Run test — should pass**

Run: `cargo test -p alephcore --lib harness::callback`
Expected: PASS.

- [ ] **Step 5: Thread into `Harness::run` + `AgentHarness::run`**

Extend `trait Harness::run` signature to accept `&mut dyn HarnessCallback`. Default to
`NoopHarnessCallback` via a wrapper method for backward-compat callers.

- [ ] **Step 6: Run full lib tests — should still pass**

Run: `cargo test -p alephcore --lib`
Expected: ≥ 9076 passing.

- [ ] **Step 7: Commit**

```bash
git add src/harness/{callback.rs,agent.rs,trait_def.rs,mod.rs}
git commit -m "harness: add HarnessCallback trait (phase 6a task 1)"
```

---

### Task 2: Wire delta callback through `AgentHarnessRunner` → `FlowStreamEvent`

**Files:**
- Modify: `src/orchestrator/harness_bridge.rs` — build a `HarnessCallback` that fans
  `on_delta` → `FlowStreamEvent::Delta`, `on_tool_call` → `FlowStreamEvent::ToolCall`
- Test: `src/orchestrator/tests/dispatch_streaming.rs`

- [ ] **Step 1: Write the failing test**

```rust
// src/orchestrator/tests/dispatch_streaming.rs
#[tokio::test]
async fn delta_events_flow_end_to_end() {
    // Build an orchestrator with a fake HarnessRunner that invokes
    // events.send(FlowStreamEvent::Delta("hi")) twice, then Complete.
    // Assert: handle.events receives both deltas + Complete.
    // ...
}
```

- [ ] **Step 2: Run — expected FAIL (new test does not exist yet)**

- [ ] **Step 3: Implement bridge adapter**

Inside `AgentHarnessRunner::run`, construct a `BroadcastCallback {
  tx: broadcast::Sender<FlowStreamEvent> }` and pass to `harness.run(&session_id, &mut cb, &cancel)`.

- [ ] **Step 4: Run — should pass**

- [ ] **Step 5: Commit**

```bash
git commit -m "orchestrator: bridge HarnessCallback → FlowStreamEvent (phase 6a task 2)"
```

---

### Task 3: Extend `FlowOutcome` + thread iteration/tool-call/token counts

**Files:**
- Modify: `src/orchestrator/dispatch.rs` — extend `FlowOutcome` struct
- Modify: `src/orchestrator/harness_bridge.rs` — populate new fields
- Modify: `src/harness/trait_def.rs` — return `HarnessReport` struct
- Modify: `src/harness/agent.rs` — track iterations + tool calls + tokens
- Test: assert counts end-to-end

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn outcome_reports_iterations_and_tool_calls() {
    // Dispatch; assert outcome.iterations >= 1, tool_calls_made matches emitted events.
}
```

- [ ] **Step 2: FAIL (new fields absent)**

- [ ] **Step 3: Add fields**

```rust
pub struct FlowOutcome {
    pub final_text: String,
    pub iterations: u32,
    pub tool_calls_made: u32,
    pub total_tokens: u32,
    pub hit_limit: bool,
}
```

- [ ] **Step 4: PASS**

- [ ] **Step 5: Commit**

---

### Task 4: Add `FlowInput::History` + `FlowInput::Multimodal`

**Files:**
- Modify: `src/orchestrator/flow_spec.rs` — extend `FlowInput` enum
- Modify: `src/orchestrator/harness_bridge.rs` — seed per-variant
- Test: unit tests for each variant

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn history_variant_seeds_prior_turns() {
    let input = FlowInput::History {
        history: vec![...],
        prompt: "next".to_string(),
    };
    // dispatch; assert session_service.get_events shows full history before the new prompt.
}
```

- [ ] **Step 2: FAIL (variant missing)**

- [ ] **Step 3: Add variants + seeding logic**

```rust
pub enum FlowInput {
    Prompt(String),
    Messages(Vec<MessageContent>),
    History { history: Vec<HistoryEntry>, prompt: String },
    Multimodal(Vec<crate::providers::MultimodalMessage>),
}
```

- [ ] **Step 4: PASS**

- [ ] **Step 5: Commit**

---

### Task 5: Thread `CancellationToken` end-to-end

**Files:**
- Modify: `src/harness/trait_def.rs` — accept `&CancellationToken`
- Modify: `src/harness/agent.rs` — check `cancel.is_cancelled()` per iteration
- Modify: `src/orchestrator/harness_bridge.rs` — pass `cancel` into `harness.run`
- Test: a mid-run cancel maps to `FlowError::Cancelled`

- [ ] **Step 1: Write the failing test (cancel mid-iteration)**

- [ ] **Step 2: FAIL**

- [ ] **Step 3: Wire cancel checks**

- [ ] **Step 4: PASS**

- [ ] **Step 5: Commit**

---

### Task 6: ~~Plumb `ContextBudget` + `ContextCompactor` + `StopHooks` into `HarnessDeps`~~ — **DEFERRED to Phase 6b**

**Status: DEFERRED.** Moved to Phase 6b alongside the helper relocations.
See `docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md`
§6 "Inherited from Phase 6a — Task 6 deferred" for the full scope that
Phase 6b MUST implement.

**Why defer:** These three helpers (`ContextBudget`, `ContextCompactor`,
`StopHooks`) live in `src/agent_loop/` today. Wiring them into
`HarnessDeps` during 6a would introduce a reverse `harness → agent_loop`
dependency that 6b's relocations (§6 Moves table) dissolve in a single
atomic change. Cleaner to relocate + wire together in 6b.

**Regression window:** Between Task 7's gateway flip (landed in 6a) and
6b's helper-relocation-plus-wiring, the orchestrator path does NOT
enforce budget / compactor / stop-hook checks. This matches the
Phase 4 `ALEPH_HARNESS_V2` opt-in path's existing envelope.

**Tracking markers left in the tree:**
- `PHASE-6b-WIRING` comment in `src/harness/deps.rs` (added below)
- Explicit §6b spec entry with exact field signatures + test scope
- `- [x]` here does NOT mean "done", it means "intentionally deferred"

- [x] **DEFERRED to Phase 6b** — do not implement in 6a.

---

### Task 7: Flip `run_loop.rs` from `AgentLoop::new` to `Orchestrator::dispatch`

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs` — replace lines 625–682 with
  orchestrator dispatch; drain `handle.events` into existing `StreamCallback`; await
  `handle.completion` and convert `FlowOutcome` into the old `LoopRunResult`-shaped
  response block (lines 684–731).
- Modify: `src/gateway/execution_engine/engine.rs` — inject `Arc<Orchestrator>` into
  `ExecutionEngine` (already wired in Phase 5 — just activate use)

- [ ] **Step 1: Write the failing integration test**

```rust
// tests/gateway_chat_through_orchestrator.rs
#[tokio::test]
async fn gateway_chat_uses_orchestrator_path() {
    // Boot a test gateway, send a simple `agent.run`, assert:
    //   - broadcast events reach the ws sink
    //   - final text returned
    //   - `iterations` recorded in metrics event
    //   - no AgentLoop::new on the stack (verified by feature-gated metric)
}
```

- [ ] **Step 2: FAIL (run_loop still calls AgentLoop)**

- [ ] **Step 3: Implement flip**

Construct `FlowRequest`:
```rust
let req = FlowRequest {
    flow_id: None,   // agent→flow resolution via default_routing
    agent_id: agent.id().to_string(),
    input: if let Some(msgs) = multimodal_messages {
        FlowInput::Multimodal(msgs)
    } else if history.is_empty() {
        FlowInput::Prompt(request.input.clone())
    } else {
        FlowInput::History { history: history.clone(), prompt: request.input.clone() }
    },
    channel: request.metadata.get("platform").cloned(),
    session_hint: Some(request.session_key.to_key_string()),
    parent_session: None,
    depth: 0,
};
let handle = self.orchestrator
    .as_ref()
    .ok_or_else(|| anyhow!("orchestrator not provisioned"))?
    .dispatch(req).await?;
// Drain events on a tokio task forwarding to StreamCallback ...
// Await completion:
let outcome = handle.completion.await??;
```

- [ ] **Step 4: PASS all integration + lib tests**

- [ ] **Step 5: Remove stale `use crate::agent_loop::{AgentLoop, LoopConfig, ...}` imports**

- [ ] **Step 6: Commit**

---

### Task 8: Verify grep guarantees + smoke test

- [ ] **Step 1: `grep -rn 'AgentLoop::new' src/ | grep -v agent_loop/ | grep -v '//'`**

Expected: empty.

- [ ] **Step 2: `grep -rn 'use crate::agent_loop' src/gateway/ src/bin/`**

Expected: empty (all helper imports gone or moved to harness side).

- [ ] **Step 3: `cargo test -p alephcore --lib` — ≥ 9076 passing**

- [ ] **Step 4: `tests/harness_run_e2e.rs` — 2/2 passing**

- [ ] **Step 5: `scripts/check-phase5-exit.sh` — green**

- [ ] **Step 6: Manual smoke — boot aleph-server, send a chat via `/v1/chat/completions`,
  verify: streamed deltas, multi-turn history, iteration-limit messaging path,
  mid-response cancel**

- [ ] **Step 7: Commit + update `scripts/check-phase6a-exit.sh`** (new gate script
  mirroring phase5 but also asserting zero AgentLoop::new in run_loop.rs)

---

## Self-Review Checklist

After all 8 tasks:

- [ ] Every `with_*` builder from `AgentLoop` that was active at `run_loop.rs:628`
  has a corresponding `HarnessDeps` field, `FlowInput` variant, or documented drop
  (for `hook_executor`/`shared_snapshot`/etc. explicitly deferred to 6d).
- [ ] `FlowOutcome` covers the 5 fields that `run_loop.rs:684-731` reads from
  `LoopRunResult`.
- [ ] StreamCallback sees the same delta cadence as before the flip.
- [ ] Multi-turn history is preserved across calls.
- [ ] Cancellation propagates in < 1s.
- [ ] No new files exceed 400 LOC.
- [ ] No clippy warnings at `-D warnings`.

---

## Exit Handoff

On green exit criteria, hand off to Phase 6b (helper relocation) per
`docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md` §6.

**Ask user before `just release`.** Phase 6a merge only; release waits for 6d.

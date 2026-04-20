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

### Task 7: ~~Flip `run_loop.rs` from `AgentLoop::new` to `Orchestrator::dispatch`~~ — **DEFERRED to Phase 6b**

**Status: DEFERRED.** The original Task 7 (hard flip of
`src/gateway/execution_engine/run_loop.rs:628`) was scoped assuming only
three behaviours would drop (`context_budget`, `context_compactor`,
`stop_hooks` — the Task 6 triad). Re-auditing the live `AgentLoop::new`
builder chain at line 628 during 6a execution surfaced **ten** wired
behaviours, seven of which are not on any 6a/6b/6c roadmap:

| Builder method | Original plan assumption |
|----------------|--------------------------|
| `with_context_budget` | Covered by Task 6 (now deferred) |
| `with_context_compactor` | Covered by Task 6 (now deferred) |
| `with_stop_hooks` | Covered by Task 6 (now deferred) |
| `with_chain` | **Not planned** — runtime chain context |
| `with_shared_snapshot` | **Not planned** — context snapshot sharing |
| `with_provider_name` / `with_platform_name` / `with_session_id` | **Not planned** — observability / routing tags |
| `with_skill_prefetcher` | **Not planned** — skill system warming |
| `with_hook_executor` | **Not planned** — user-config hook integration |
| `with_tool_refresh` | **Not planned** — dynamic tool discovery |

A hard flip as originally drafted would drop all ten behaviours at once,
which is a far larger regression window than 6a's "runtime flip"
decomposition intended. Phase 6b is the correct home because it already
relocates the Task-6 helpers, and the other seven behaviours can be
catalogued + addressed alongside (either wired on the harness side or
explicitly documented as out-of-scope for the compat flip).

See cleanup-design spec §6 "Inherited from Phase 6a — Task 6 deferred"
and §6.1 "Inherited from Phase 6a — Task 7 deferred" for the full
scope + testing requirements 6b MUST complete.

- [x] **DEFERRED to Phase 6b** — do not implement in 6a.

---

### Task 8: ~~Verify grep guarantees + smoke test~~ — **DEFERRED to Phase 6b**

**Status: DEFERRED.** The grep exit criterion (`grep -rn 'AgentLoop::new'
src/ | grep -v agent_loop/` returning empty) presumes Task 7 has flipped
`run_loop.rs`. With Task 7 deferred to 6b, this verification gate moves
with it. The Phase 6b exit criteria must include the full Task 8 grep +
smoke-test checklist.

- [x] **DEFERRED to Phase 6b** — do not implement in 6a.

---

## Self-Review Checklist

**What Phase 6a as-executed delivers (Tasks 1–5 landed; 6/7/8 deferred):**

- [x] `HarnessCallback` trait + `NoopHarnessCallback` (Task 1)
- [x] `BroadcastCallback` fan-out from `HarnessCallback` → `FlowStreamEvent`
  broadcast (Task 2)
- [x] `FlowOutcome` parity fields: `iterations`, `tool_calls_made`,
  `total_tokens`, `hit_limit` populated (Task 3)
- [x] `FlowInput::History` + `FlowInput::Multimodal` variants with session
  seeding (Task 4)
- [x] `CancellationToken` threaded end-to-end through `Harness::run` with
  `HarnessError::Cancelled → FlowError::Cancelled` mapping (Task 5)
- [x] Task 6 deferral documented in three places (plan, spec §6, code
  marker in `src/harness/deps.rs`)
- [x] Task 7+8 deferral documented in plan + spec §6.1

**What 6a does NOT deliver (intentionally, per the Option-3 scope call):**

- No production runtime flip — `AgentLoop::new` still called from
  `src/gateway/execution_engine/run_loop.rs`.
- No behaviour wiring for `context_budget`, `context_compactor`,
  `stop_hooks`, `chain`, `shared_snapshot`, `provider_name`,
  `platform_name`, `skill_prefetcher`, `hook_executor`, `tool_refresh`.

The Phase 6a exit line is therefore: **additive plumbing merged, rails
laid for 6b to relocate helpers + flip + wire behaviours in one
coherent change.**

---

## Exit Handoff

On green checklist above, hand off to Phase 6b (helper relocation +
inherited Task 6/7/8 work) per
`docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md`
§6 + §6.1.

**Ask user before `just release`.** Phase 6a merge only; release waits
for 6d.

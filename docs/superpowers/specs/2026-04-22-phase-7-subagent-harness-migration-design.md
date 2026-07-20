# Phase 7: SubagentTool → Harness Spawner Migration + `agent_loop` Deletion

**Status:** Design approved, ready for plan.
**Author / Driver:** rootazero (Aleph) — brainstormed with Claude.
**Worktree:** `.claude/worktrees/managed-agents-phase-7/` on branch `worktree-managed-agents-phase-7`.
**Base:** main @ `b1cf3379d` (merge: managed-agents phase 6a/6c/6d).
**Baseline tests:** 9133 lib passing + 2 pre-existing failures (`telegram::config::parse_v2_config_directly`, `notes::ingest::prompts::base_prompt_snapshot`).

---

## 1. Context

Managed-agents Phases 0–6 landed on main. Gateway chat now flows through `Orchestrator::dispatch → AgentHarnessRunner → AgentHarness::run`. The one remaining caller of the legacy `AgentLoop` is `SubagentTool`, via:

```
SubagentTool::execute
    └─ AgentRuntime::run
        └─ AgentRuntime::execute_fresh_path
            └─ agent_loop::subagent_runner::run_subagent
                └─ AgentLoop::new + AgentLoop::run
```

Phase 7 migrates this path onto the Harness, then deletes the entire `src/agent_loop/` directory — ~5,400 LOC of dead runtime. `TruncationRecovery` (640 LOC) has zero external consumers and dies with `loop_core`.

The migration approach is **C — 就地换芯**: keep `SubagentTool`'s `LoopTool` API and `AgentRuntime::run(config) -> Result<LoopRunResult, String>` signature bit-identical, swap the internal engine from `AgentLoop` to a new `subagent_spawner` built on `AgentHarness`. Zero call-site churn outside the runtime crate.

---

## 2. Scope

### In Scope

- `HarnessDeps` gains two Optional fields: `system_prompt: Option<String>` and `max_iterations: Option<usize>`.
- `AgentHarness::run_turn` injects `system_prompt` into `RequestPayload`.
- `AgentHarness` overrides the default `Harness::run` to enforce `max_iterations`, setting `hit_limit = true` on cap-hit.
- New module `src/agents/subagent_spawner.rs` — assembles `HarnessDeps` for a child ephemeral `SessionId` and drives the run.
- New module `src/agents/allowlist_tool_service.rs` — `ToolService` decorator that filters by `agent_def.is_tool_allowed`.
- `LoopRunResult` relocates from `agent_loop::loop_core` to `agents::runtime` (same field layout).
- `SubagentTool` gains `sandbox: Arc<dyn Sandbox>`, `session: Arc<dyn SessionService>`, `parent_tools: Arc<dyn ToolService>` constructor parameters.
- Fork path (dead code, zero production `with_shared_snapshot` callers) removed from `SubagentTool` and `AgentRuntimeConfig`.
- Entire `src/agent_loop/` directory deleted (`loop_core.rs`, `truncation_recovery.rs`, `subagent_runner.rs`, `mod.rs` + 25 re-export stubs, `SharedSnapshot` type alias).
- `pub mod agent_loop;` removed from `src/lib.rs`.
- `scripts/check-phase7-exit.sh` asserts: no `src/agent_loop/` directory, no residual `AgentLoop`/`LoopConfig` symbols outside `agents/runtime.rs`, baseline test count holds.

### Non-Goals (explicit deferrals)

- Gateway path does **not** adopt `system_prompt` in Phase 7 — it continues to pass `None` (baseline behavior preserved).
- `LoopRunResult.total_tokens` continues to default to `0` — provider usage-metadata piping is out of scope.
- No new token-budget enforcement on Harness — `context_budget` is already wired where needed.
- `TruncationRecovery` is not ported — it dies with `loop_core`.
- Child subagent session events are not GC'd — they persist to SQLite like any other session; disk-reclamation is a separate task.
- Streaming subagent deltas to parent — `NoopHarnessCallback` preserved.
- No changes to background subagent / `BackgroundAgentTracker` / teammate / message-router logic beyond what SubagentTool construction requires.

### Hard Constraints

- All commits prefixed `phase7:`, English.
- `cargo test -p alephcore --lib` must end with 9133+ passing and exactly 2 failing (the pre-existing ones, by name).
- `cargo clippy -p alephcore --lib -- -D warnings` zero new errors.
- No release, no push, no PR in Phase 7.

---

## 3. Architecture

### 3.1 New Files

| File | ~LOC | Purpose |
|------|------|---------|
| `src/agents/subagent_spawner.rs` | 180 | Child `SessionKey::Ephemeral` + `HarnessDeps` assembly + `harness.run` + event-log extraction → `LoopRunResult` |
| `src/agents/allowlist_tool_service.rs` | 80 | `ToolService` decorator: `execute` rejects disallowed, `list_tools` filters |
| `scripts/check-phase7-exit.sh` | 50 | Exit-gate: directory gone, symbols gone, baseline holds |

### 3.2 Modified Files

| File | Change |
|------|--------|
| `src/harness/deps.rs` | Add `system_prompt: Option<String>`, `max_iterations: Option<usize>` |
| `src/harness/agent.rs` | `run_turn` injects `system_prompt`; override `Harness::run` with `max_iterations` counter |
| `src/agents/runtime.rs` | New `LoopRunResult` (not re-exported from `agent_loop`); `execute_fresh_path` → `execute_via_harness` calling `subagent_spawner::spawn`; `AgentRuntimeConfig.prompt_snapshot` field deleted |
| `src/agents/subagent_tool.rs` | New fields `sandbox`/`session`/`parent_tools`; delete `shared_snapshot` field + `with_shared_snapshot` + `should_fork` + `read_snapshot`; simplify 3 fork branches in `execute` |
| `src/agents/mod.rs` | Remove `pub use crate::agent_loop::SharedSnapshot;` |
| `src/lib.rs` | Remove `pub mod agent_loop;` |
| `src/orchestrator/harness_bridge.rs` | `HarnessDeps` literal gains `system_prompt: None, max_iterations: None` |

### 3.3 Deletions

| Path | ~LOC | Notes |
|------|------|-------|
| `src/agent_loop/loop_core.rs` | 4558 | `AgentLoop` body |
| `src/agent_loop/truncation_recovery.rs` | 640 | Zero external consumers (verified via grep) |
| `src/agent_loop/subagent_runner.rs` | 90 | Shim replaced by `subagent_spawner` |
| `src/agent_loop/mod.rs` | 127 | 25 re-export stubs + `SharedSnapshot` alias |
| `src/agent_loop/` directory | — | Must vanish (exit-gate asserts) |

**Net change:** ~5,415 deleted − ~310 added ≈ **5,100 LOC net reduction**, pre-accounting SubagentTool fork-path cleanup.

### 3.4 Module Dependencies Post-Deletion

```
src/agents/subagent_tool.rs
    ↓
src/agents/runtime.rs   (AgentRuntime, AgentRuntimeConfig, LoopRunResult)
    ↓
src/agents/subagent_spawner.rs
    ↓
src/harness/            (AgentHarness, HarnessDeps, Harness trait)
src/agents/allowlist_tool_service.rs
src/session/            (SessionKey::Ephemeral, SessionService)
```

No cycles. Zero references to `agent_loop::*` anywhere.

---

## 4. HarnessDeps & AgentHarness Changes

### 4.1 New HarnessDeps Fields

```rust
pub struct HarnessDeps {
    // ... existing fields unchanged ...

    /// System prompt injected into every RequestPayload. Subagent path builds
    /// this via PromptBuilder at spawn time; Gateway passes None for now.
    pub system_prompt: Option<String>,

    /// Hard iteration cap — `run` aborts with Done + sets hit_limit=true when
    /// AssistantMessage events emitted by THIS run reach cap. None → unbounded
    /// (current Gateway behavior).
    pub max_iterations: Option<usize>,
}
```

Both Optional. Gateway (`harness_bridge.rs`) passes `None, None`; baseline behavior preserved.

### 4.2 system_prompt Injection

In `AgentHarness::run_turn` (line ~269), replace:

```rust
let payload = RequestPayload::new(&messages);
```

with:

```rust
let payload = match self.deps.system_prompt.as_deref() {
    Some(sp) => RequestPayload { system_prompt: Some(sp), ..RequestPayload::new(&messages) },
    None => RequestPayload::new(&messages),
};
```

`RequestPayload.system_prompt` is already `Option<&str>` in the adapter layer — no deeper plumbing needed.

### 4.3 `Harness::run` Override on `AgentHarness`

```rust
#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(&self, ...) -> Result<TurnState, HarnessError> {
        // unchanged
    }

    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        loop {
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            match self.run_turn(session_id, callback).await? {
                TurnState::Continue => {
                    iterations = iterations.saturating_add(1);
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::SeqCst);
                            callback.on_complete();
                            return Ok(());
                        }
                    }
                }
                TurnState::Done => {
                    callback.on_complete();
                    return Ok(());
                }
            }
        }
    }
}
```

**Semantic decisions:**

- Count `Continue` turns (one completed Think+Act cycle each). The final `Done` is not counted.
- Cap condition is `>= limit` — `limit=25` exits after 25 `Continue` turns, matching legacy `LoopConfig::max_iterations` semantics.
- Cap-hit sets `hit_limit = true`, reusing the existing atomic flag (also set by `FinalReply` budget directive). Callers (spawner, Gateway) that already read `hit_limit()` see a unified signal.
- `on_complete()` fires — cap-hit is a graceful termination, not a cancellation.
- The final assistant message is already persisted by `run_turn` before the cap check, so `final_text` extraction works.
- When `max_iterations = None`, the implementation is line-by-line equivalent to the default `Harness::run` — Gateway's observable behavior is unchanged.

---

## 5. SubagentSpawner

### 5.1 Public API

```rust
// src/agents/subagent_spawner.rs

pub struct SpawnerBase {
    pub session: Arc<dyn SessionService>,
    pub parent_tools: Arc<dyn ToolService>,
    pub sandbox: Arc<dyn Sandbox>,
    pub provider: Arc<dyn AiProvider>,
    pub chain: ChainContext,
}

pub struct SpawnRequest<'a> {
    pub agent_def: &'a AgentDef,
    pub task: &'a str,
    pub context_summary: Option<&'a str>,
    pub model: Option<&'a str>,
    pub timeout_secs: u64,
    pub cancel: CancellationToken,
}

pub async fn spawn(
    base: &SpawnerBase,
    req: SpawnRequest<'_>,
) -> Result<LoopRunResult, String>;
```

### 5.2 Execution Flow

1. **Resolve model.** `req.model.map(|s| s.to_string()).or_else(|| req.agent_def.model_hint.clone())`. If `Some`, wrap `base.provider` in a private adapter that stamps `payload.model` before delegating to the real provider (avoids adding `model_override` to `HarnessDeps`).

2. **Build child `SessionId`.** `SessionKey::Ephemeral { agent_id: agent_def.id.clone(), ephemeral_id: format!("subagent-{}-{}", agent_def.id, unique_nanos()) }`. Guarantees uniqueness across concurrent spawns under the same parent.

3. **Build system prompt.** `PromptBuilder::new(PromptConfig::default()).with_agent(req.agent_def.clone()).build(&[])`. `build(&[])` receives empty history; only the agent-role / persona / tool-description section is assembled. Returns `String` — spawner owns it.

4. **Build filtered `ToolService`.** Compute allowed name set via `base.parent_tools.list_tools()` ∩ `is_tool_allowed`. Wrap in `AllowlistToolService::new(base.parent_tools.clone(), allowed)`.

5. **Assemble `HarnessDeps`:**

   ```rust
   HarnessDeps {
       session: base.session.clone(),
       tools,
       sandbox: base.sandbox.clone(),
       llm: provider_with_model,
       stop_hooks: None,
       context_budget: None,
       context_compactor: None,
       skill_prefetcher: None,
       trace_sink: None,
       system_prompt: Some(built_prompt),
       max_iterations: Some(agent_def.max_iterations.unwrap_or(25) as usize),
   }
   ```

6. **Seed task + run.** Emit `UserMessage { text: full_task, ... }` to the child session. `full_task` prepends `context_summary` (same "## Context from parent agent\n\n...\n\n---\n\n..." format as legacy path). Wrap `harness.run` in `AssertUnwindSafe(...).catch_unwind()` (panic isolation) + `tokio::time::timeout` (outer timeout).

7. **Extract result** from child session event log (mirrors `harness_bridge.rs:144-164`):

   ```rust
   let events = session.get_events(&child_id, None, None).await?;
   let mut final_text = String::new();
   let mut iterations = 0usize;
   let mut tool_calls_made = 0usize;
   for r in &events {
       match &r.event {
           AssistantMessage { content, .. } => {
               final_text = content.text.clone();
               iterations += 1;
           }
           ToolCallRequested { .. } => tool_calls_made += 1,
           _ => {}
       }
   }
   Ok(LoopRunResult {
       iterations,
       tool_calls_made,
       total_tokens: 0,
       final_text: Some(final_text).filter(|s| !s.is_empty()),
   })
   ```

Timeout and panic surface as `Err(String)` (matches legacy contract).

### 5.3 `LoopRunResult` Relocated

```rust
// src/agents/runtime.rs
#[derive(Debug, Clone)]
pub struct LoopRunResult {
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub final_text: Option<String>,
}
```

Field layout identical to the `agent_loop` version — `SubagentTool` call sites unchanged.

### 5.4 Open Implementation Questions (for writing-plans to resolve)

- **Model override plumbing** — Verify `AiProvider::process(payload)` honors `payload.model` set by a wrapping provider. If not, the spawner's model-wrapper pattern (approach 1 in 5.2 step 1) needs revisiting.
- **`AgentDef` allowlist API** — Current code uses `agent_def.is_tool_allowed(name)` (single-name query). `AllowlistToolService` construction needs to materialize the full allowed set once; the plan step must confirm the derivation strategy (`parent_tools.list_tools()` intersected with `is_tool_allowed`) matches `AgentDef` semantics.

---

## 6. SubagentTool & AgentRuntime Wiring

### 6.1 SubagentTool Constructor Widens

```rust
impl SubagentTool {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,      // retained for observing uses; see §6.4
        safety_guard_factory: SafetyGuardFactory,        // same
        chain: ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<BackgroundAgentTracker>,
        // NEW:
        session: Arc<dyn SessionService>,
        parent_tools: Arc<dyn ToolService>,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self { ... }
}
```

`LoopTool` trait impl (`name`, `description`, `schema`, `execute`) unchanged.

### 6.2 SubagentTool Deletions

- Field `shared_snapshot: Option<SharedSnapshot>` — removed.
- Method `with_shared_snapshot`, `should_fork`, `read_snapshot` — removed.
- 3 fork branches inside `execute` (subagent_tool.rs:623-692 range) collapsed to a single direct path that builds `AgentRuntimeConfig` without `prompt_snapshot`.
- `use crate::agent_loop::SharedSnapshot;` and `use crate::agent_loop::chain_context::ChainContext;` replaced by direct canonical paths.

### 6.3 AgentRuntime Widens

```rust
pub struct AgentRuntime {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,   // see §6.4
    safety_guard_factory: SafetyGuardFactory,     // see §6.4
    child_chain: ChainContext,
    cancel_token: CancellationToken,
    // NEW:
    session: Arc<dyn SessionService>,
    parent_tools: Arc<dyn ToolService>,
    sandbox: Arc<dyn Sandbox>,
}
```

`execute_fresh_path` renamed `execute_via_harness`; body replaced with `subagent_spawner::spawn(&base, req).await`.

### 6.4 Factory Fields Observation

`tool_registry_factory` and `safety_guard_factory` are no longer consumed on the Harness path. During plan execution, grep for other consumers:

- **If zero consumers outside legacy `run_subagent`** (which gets deleted) → remove both fields and their constructor params.
- **If other consumers exist** → retain the fields with `#[allow(dead_code)]` and a TODO comment; leave deletion as a follow-up task.

The decision is made during implementation based on grep evidence, not upfront.

### 6.5 AgentRuntimeConfig Deletion

```rust
pub struct AgentRuntimeConfig {
    pub agent_def: AgentDef,
    pub task: String,
    pub context_summary: Option<String>,
    pub model: Option<String>,
    pub timeout_secs: u64,
    // DELETED: pub prompt_snapshot: Option<PromptSnapshot>,
}
```

Existing test `agent_runtime_config_construction` (runtime.rs:325-340) loses two lines (`prompt_snapshot: None,` literal + `assert!(config.prompt_snapshot.is_none());`).

### 6.6 25 Re-export Stubs — Import Migration

`src/agent_loop/mod.rs` has 25 `pub mod X { pub use crate::canonical::path::*; }` stubs. Before deleting `agent_loop/`, external imports must move to the canonical path:

1. `grep -rn 'use crate::agent_loop::' src/` lists every consumer.
2. Rewrite each `use crate::agent_loop::X::Y` to `use crate::<canonical_path>::Y` — canonical paths are documented in-file at `agent_loop/mod.rs:20-91`.
3. `cargo check` must stay green through the rewrite.

This is mechanical, not a design question.

### 6.7 Ordering Constraint

Tasks must land in this order (otherwise intermediate commits won't compile):

1. Add `HarnessDeps` fields + `AgentHarness` overrides + unit tests.
2. Relocate `LoopRunResult` into `agents/runtime.rs` (both definitions coexist transitionally).
3. New `AllowlistToolService` + unit tests.
4. New `subagent_spawner` + TDD integration tests. **Execute path still goes through AgentLoop — no traffic switch yet.**
5. Flip `AgentRuntime::execute_fresh_path` → `execute_via_harness`. **This is the only traffic-switch step.**
6. Verify baseline holds (9133 + 2).
7. Remove fork path from `SubagentTool` and `AgentRuntimeConfig`.
8. Migrate 25 stub imports.
9. Delete `src/agent_loop/` directory + `pub mod agent_loop;` from `src/lib.rs`.
10. Add `scripts/check-phase7-exit.sh` and verify green.

---

## 7. Testing Strategy

### 7.1 TDD Red-line (test-first, confirm fail, then implement)

**A. `AgentHarness` behaviour** (`src/harness/agent.rs` unit tests)

- `system_prompt_flows_into_request_payload`
- `max_iterations_stops_runaway_loop`
- `max_iterations_none_keeps_unbounded`
- `max_iterations_sets_hit_limit`

Harness: mock `AiProvider` capturing `payload`; `InProcessActorSessionService` for session state (pattern already used in `harness_bridge` tests).

**B. `AllowlistToolService`** (unit tests in the new file)

- `allowed_tool_executes_delegates_to_inner`
- `disallowed_tool_returns_error`
- `empty_allowlist_denies_everything`
- `list_tools_filters_to_allowed_subset`

Harness: mock `ToolService` with recording/delegation.

**C. `subagent_spawner`** (integration tests — either `#[cfg(test)]` in-file or `tests/subagent_spawner_integration.rs`)

- `spawn_single_turn_returns_final_text`
- `spawn_multi_turn_counts_iterations_and_tool_calls`
- `spawn_timeout_returns_timed_out_error`
- `spawn_max_iter_hits_limit_flag_in_run_result`
- `spawn_tool_allowlist_enforced_via_harness`
- `spawn_panic_in_harness_returns_error_not_crash`

Harness: scripted mock provider + in-process session + real `AllowlistToolService` + simple `AgentDef`.

**D. Regression — SubagentTool Run-path existing tests**

All existing `#[cfg(test)]` tests in `src/agents/subagent_tool.rs` must remain green. Any test that couples to `AgentLoop` internals is called out during plan execution.

### 7.2 Baseline

| Check | Target |
|-------|--------|
| `cargo test -p alephcore --lib` passing | ≥ 9133 (new tests add) |
| `cargo test -p alephcore --lib` failing | exactly 2, by name: `telegram::config::parse_v2_config_directly`, `notes::ingest::prompts::base_prompt_snapshot` |
| `cargo clippy -p alephcore --lib -- -D warnings` | zero new errors |
| New tests added | ≥ 14 (A4 + B4 + C6), likely more |

If any other test turns red during Phase 7 — that is a regression, revert or fix.

### 7.3 Per-Task Verification

Each task's subagent runs, in order:

1. `cargo check -p alephcore`
2. `cargo clippy -p alephcore --lib -- -D warnings`
3. `cargo test -p alephcore --lib <new_test_name>` (new test must pass)
4. `cargo test -p alephcore --lib` (full suite holds baseline)

Task 10 additionally runs `scripts/check-phase7-exit.sh`.

### 7.4 Out of Scope for Tests

- Real-provider end-to-end runs (no API keys in Phase 7).
- Performance benchmarks (Harness is already in production on the Gateway path).
- Concurrency stress for background subagent / `BackgroundAgentTracker` (existing coverage).

---

## 8. Exit-Gate Script

`scripts/check-phase7-exit.sh`:

```sh
#!/usr/bin/env bash
set -euo pipefail

# 1. agent_loop directory must be gone
if [ -d "src/agent_loop" ]; then
    echo "FAIL: src/agent_loop/ still exists"; exit 1
fi

# 2. AgentLoop / LoopConfig symbols zero usage outside agents/runtime.rs
# (runtime.rs is allowed because LoopRunResult lives there now)
BAD=$(grep -rn -E '\b(AgentLoop|LoopConfig)\b' src/ --include='*.rs' \
    | grep -v 'src/agents/runtime.rs' || true)
if [ -n "$BAD" ]; then
    echo "FAIL: residual AgentLoop / LoopConfig references:"
    echo "$BAD"; exit 1
fi

# 3. pub mod agent_loop must be removed from lib.rs
if grep -q 'pub mod agent_loop;' src/lib.rs; then
    echo "FAIL: pub mod agent_loop; still in src/lib.rs"; exit 1
fi

# 4. baseline test count holds
OUT=$(cargo test -p alephcore --lib 2>&1 || true)
PASS=$(echo "$OUT" | awk '/test result:/ {for (i=1;i<=NF;i++) if ($i=="passed;") print $(i-1)}' | tail -n1)
FAIL=$(echo "$OUT" | awk '/test result:/ {for (i=1;i<=NF;i++) if ($i=="failed;") print $(i-1)}' | tail -n1)
if [ -z "$PASS" ] || [ "$PASS" -lt 9133 ]; then
    echo "FAIL: passing count ${PASS:-unknown} < 9133"
    echo "$OUT" | tail -40; exit 1
fi
if [ -z "$FAIL" ] || [ "$FAIL" -gt 2 ]; then
    echo "FAIL: failing count ${FAIL:-unknown} > 2 (baseline)"
    echo "$OUT" | tail -40; exit 1
fi

echo "OK: phase7 exit gate passed ($PASS passing, $FAIL failing)"
```

Exact parsing strategy (awk over `test result:` lines) is refined during plan — the above captures intent.

---

## 9. Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| `HarnessDeps` literal constructed in multiple places; missing a field breaks compile | Compiler catches — grep-based pre-inventory during plan (`grep -rn 'HarnessDeps {' src/`) |
| 25 stub imports: mass rewrite could break lint | Rewrite + `cargo check` + `cargo clippy` after each cluster of imports |
| Child session event log pollutes SQLite storage | Accepted in Phase 7; file a follow-up GC task |
| Hidden AgentLoop coupling in SubagentTool tests | Regression suite run on every task; any red test gets called out |
| `max_iterations` behaviour drift vs `LoopConfig` semantics | Explicit decisions documented in §4.3; tests pin semantics |
| `AiProvider` doesn't honor `payload.model` from a wrapping provider | Open question in §5.4; resolved during plan step 1 |

---

## 10. Task Sketch (plan will refine)

Ten candidate implementation units, ordered to keep every intermediate commit green:

1. `HarnessDeps` + `AgentHarness` extensions + red tests for system_prompt / max_iterations.
2. Relocate `LoopRunResult` to `agents/runtime.rs` (both coexist).
3. `AllowlistToolService` + red tests.
4. `subagent_spawner` + red integration tests (AgentRuntime still on AgentLoop).
5. Flip `AgentRuntime::execute_fresh_path` → `execute_via_harness`; baseline re-verify.
6. Remove `SubagentTool` fork path + `AgentRuntimeConfig.prompt_snapshot`.
7. Migrate 25 stub imports (`grep + cargo check` sweep).
8. Delete `src/agent_loop/` directory + remove `pub mod agent_loop;`.
9. Observation-driven cleanup of `tool_registry_factory` / `safety_guard_factory` on `AgentRuntime` (delete if zero external consumers).
10. Add `scripts/check-phase7-exit.sh`; run it; confirm green.

Tasks 9 and 10 can potentially run in parallel if they touch disjoint files.

---

## 11. Definition of Done

- [ ] All 10 tasks merged into `worktree-managed-agents-phase-7`.
- [ ] `scripts/check-phase7-exit.sh` exits 0.
- [ ] `cargo test -p alephcore --lib`: ≥ 9133 passing, exactly 2 failing (by name).
- [ ] `cargo clippy -p alephcore --lib -- -D warnings`: zero new errors.
- [ ] `grep -rn 'use crate::agent_loop::' src/` returns nothing.
- [ ] `ls src/agent_loop/` fails with "No such file".
- [ ] Net LOC reduction ≥ 5,000.
- [ ] User has reviewed and approved final state.
- [ ] **No release, no push, no PR** without explicit user approval (Phase 7 constraint).

# Stage 4 — Subagent ChainContext Wiring (#11) Implementation Plan

**Goal:** Make `ChainContext` first-class on `AgentHarness` so each harness instance can answer "what is my position in the subagent chain?". Today the type exists at `src/harness/chain_context.rs` (156 lines) and the spawner correctly *descends* it via `SpawnerBase.chain.child()`, but the descended chain is only used to stamp `LoopRunResult.chain_id` / `depth` on completion — **the inner harness itself has no self-knowledge of its position**. Future Stage 6 JudgeAgent / verifier traces need that position to correlate trace events across nested agents.

**Architecture:**
- `HarnessDeps` gains a `chain_context: ChainContext` field with `Default::default()` (= fresh root chain, `depth = 0`).
- `AgentHarness` exposes `pub fn chain_context(&self) -> &ChainContext` (concrete accessor — Stage 4's primary seam).
- The `Harness` trait gains a default `fn chain_context(&self) -> Option<&ChainContext>` returning `None`, overridden on `AgentHarness` to return `Some(&self.deps.chain_context)`. Test mocks keep the default and stay quiet.
- `subagent_spawner::spawn` writes the descended `child_chain` into the inner `HarnessDeps.chain_context` so the spawned harness's accessor returns the correct depth/chain_id without re-derivation.
- `extract_run_result` keeps reading from `chain` arg (unchanged); the spawner's existing chain plumbing is preserved verbatim.

**Tech Stack:** Rust 1.x, no new deps. `ChainContext` is plain data (`String` + two `u32`).

**Baseline:** commit `1aa6bb48c` (Stage 3 ship + integration-test repair). Master spec: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` § Stage 4.

**Budget envelope (master spec §0.4 + §3.3 + R10):**
- Total stage delta target: ≤ ~150 lines (master spec estimate); cap: ≤ +400 lines harness/ delta.
- `agent.rs`: stays ≤ 1500 lines (currently 1245; this stage adds ≤ 5 lines for the accessor).
- `src/harness/` file count: stays at 9 canonical files (no new modules — chain_context.rs already exists).
- Single PR ≤ 600 lines including tests.

---

## File Structure

| File | Action | Purpose |
|------|--------|---------|
| `src/harness/deps.rs` | **Modify** | Add `chain_context: ChainContext` field |
| `src/harness/agent.rs` | **Modify** | Add `pub fn chain_context(&self) -> &ChainContext` accessor; override `Harness::chain_context()` |
| `src/harness/trait_def.rs` | **Modify** | Add `fn chain_context(&self) -> Option<&ChainContext>` with default `None` to `Harness` trait |
| `src/agents/subagent_spawner.rs` | **Modify** | Set `deps.chain_context = child_chain.clone()` when assembling child harness |
| `src/harness/tests/chain.rs` | **Create** | New test module: chain accessor unit + 3-layer integration + loom |
| `src/harness/mod.rs` | **Modify** | Wire `mod chain;` under `#[cfg(test)] mod tests` |
| All other `HarnessDeps { ... }` sites (~14) | **Modify** | Append `chain_context: ChainContext::default(),` field |
| `CHANGELOG.md` | **Modify** | Add Stage 4 entries to `## [Unreleased]` |
| `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` | **Modify** | Flip Stage 4 status to `✅ Shipped <sha> on 2026-05-05` |

**Touch surface outside harness/:** `src/agents/subagent_spawner.rs` (1 field assignment) — within Stage 4's allowed scope per master spec § Stage 4 Old code to retire bullet.

---

## Task Sequence Rationale

1. **Task 1** (define seam): Add `chain_context` field + accessor + trait default. No behavior change yet — every site uses `ChainContext::default()`.
2. **Task 2** (real consumer): Wire `subagent_spawner` to descend the chain into the child harness's deps.
3. **Task 3** (tests): unit + integration + loom. Real consumers from Tasks 1-2 are now covered.
4. **Task 4** (ship): CHANGELOG + status flip + verification.

This is the same sequencing as Stages 1-3.

---

## Task 1: Add `chain_context` to `HarnessDeps` + accessor + trait method

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/trait_def.rs`
- Modify: `src/harness/agent.rs`
- Modify: every `HarnessDeps { ... }` callsite (12 in tests, 1 in `subagent_spawner.rs`, 1 in `harness_bridge.rs`)

- [ ] **Step 1: Extend `HarnessDeps`**

In `src/harness/deps.rs`, after the existing imports, add:

```rust
use crate::harness::chain_context::ChainContext;
```

Add the field to the struct (between `prompt_builder` and `max_iterations`, keeping fields grouped by concern):

```rust
    /// Position of this harness instance in the subagent call chain.
    /// Stage 4 seam (#11). Defaults to a fresh root chain (depth=0). The
    /// subagent spawner overrides this with `parent.chain.child()` so each
    /// nested harness reports its own depth via `AgentHarness::chain_context()`.
    pub chain_context: ChainContext,
```

- [ ] **Step 2: Add accessor on `AgentHarness`**

In `src/harness/agent.rs`, after `reset_hit_limit` (line ~71):

```rust
    /// Read-only accessor for this harness's position in the subagent chain.
    /// Returns the root context for top-level agents (the `HarnessDeps`
    /// default). The subagent spawner overrides this with the descended
    /// chain when assembling a child harness.
    pub fn chain_context(&self) -> &crate::harness::chain_context::ChainContext {
        &self.deps.chain_context
    }
```

- [ ] **Step 3: Add trait default + override**

In `src/harness/trait_def.rs`, extend the `Harness` trait (after `run_turn`):

```rust
    /// Position of this harness instance in the subagent call chain.
    /// Default `None` keeps test mocks ergonomic. `AgentHarness` overrides to
    /// return `Some(&self.deps.chain_context)`. Stage 4 / module #11.
    fn chain_context(&self) -> Option<&crate::harness::chain_context::ChainContext> {
        None
    }
```

In `src/harness/agent.rs`, inside `impl Harness for AgentHarness`, override:

```rust
    fn chain_context(&self) -> Option<&crate::harness::chain_context::ChainContext> {
        Some(self.chain_context())
    }
```

- [ ] **Step 4: Update all `HarnessDeps { ... }` callsites**

Append `chain_context: ChainContext::default(),` to each struct literal. Production sites:

- `src/agents/subagent_spawner.rs:190` — placeholder for now (Task 2 replaces with `child_chain.clone()`)
- `src/orchestrator/harness_bridge.rs:145` — root harness; default is correct
- `src/harness/agent.rs:955`, `:1152`, `:1205` — in-file tests; default is correct

Test sites (default is correct everywhere — these are root-level tests):

- `src/harness/tests/driver.rs:108`, `:175`
- `src/harness/tests/think.rs:243`, `:294`, `:376`, `:437`, `:501`
- `src/harness/tests/act.rs:281`, `:349`, `:447`, `:614`
- `src/harness/tests/stability.rs:242`
- `src/harness/tests/task10_wiring.rs:258`, `:320`, `:396`

Each site needs the import `use crate::harness::chain_context::ChainContext;` if not already in scope. Most test files already use `ChainContext` indirectly; add the import as needed.

- [ ] **Step 5: Verify compile + existing tests**

```bash
cargo check -p alephcore --lib
cargo test -p alephcore --lib harness::
```

Expected: clean compile; all existing harness tests still pass (chain_context behavior unchanged for everyone using `Default`).

- [ ] **Step 6: Commit**

```bash
git add src/harness/ src/agents/subagent_spawner.rs src/orchestrator/harness_bridge.rs
git commit -m "feat(harness): add chain_context field + accessor (Stage 4 seam)

Stage 4 step 1: HarnessDeps gains chain_context: ChainContext (defaults
to root). AgentHarness exposes chain_context() accessor; the Harness
trait gains a default Option<&ChainContext> method overridden on
AgentHarness.

Behavior unchanged at every callsite (default = root chain)."
```

---

## Task 2: Wire `subagent_spawner` to descend chain into the child harness

**Files:**
- Modify: `src/agents/subagent_spawner.rs`

- [ ] **Step 1: Replace placeholder with descended chain**

In `src/agents/subagent_spawner.rs::spawn` (around line 190 in the `HarnessDeps { ... }` literal added in Task 1), replace:

```rust
        chain_context: crate::harness::chain_context::ChainContext::default(),
```

with:

```rust
        chain_context: child_chain.clone(),
```

`child_chain` is the local already produced at line ~109:

```rust
let child_chain = base
    .chain
    .child()
    .ok_or_else(|| "chain depth exceeded".to_string())?;
```

- [ ] **Step 2: Verify compile + spawner tests**

```bash
cargo test -p alephcore --lib agents::subagent_spawner::
```

Expected: existing 8+ spawner tests still pass; the visible behavior (LoopRunResult.depth/chain_id) is unchanged because that codepath already used `child_chain`. The only newly observable effect is that the inner harness's `chain_context()` now returns the descended chain.

- [ ] **Step 3: Commit**

```bash
git add src/agents/subagent_spawner.rs
git commit -m "feat(harness): subagent spawner stamps child chain on inner harness

Stage 4 step 2: spawn() now writes child_chain into the spawned
HarnessDeps.chain_context so the inner AgentHarness::chain_context()
returns the descended chain instead of a default root.

LoopRunResult.depth / chain_id are unchanged (extract_run_result still
reads from the chain arg). Behavior delta is purely additive: the
harness gains self-knowledge of its position."
```

---

## Task 3: Tests — unit + 3-layer integration + loom

**Files:**
- Create: `src/harness/tests/chain.rs`
- Modify: `src/harness/mod.rs` (register `mod chain;`)

- [ ] **Step 1: Create `src/harness/tests/chain.rs`**

```rust
//! Stage 4 — Subagent ChainContext Wiring tests (#11).
//!
//! Verifies that `AgentHarness::chain_context()` reflects the chain
//! injected via `HarnessDeps`, that `subagent_spawner::spawn` propagates
//! the descended chain into the child harness, and that concurrent
//! readers of the accessor see consistent state.

use std::sync::Arc;

use crate::harness::agent::AgentHarness;
use crate::harness::chain_context::ChainContext;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;

// Light-weight stubs reused across tests. Each construct returns the
// minimum surface needed to instantiate `HarnessDeps` — no LLM calls fire.
mod stubs {
    use super::*;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use crate::sandbox::NoopSandbox;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::service::SessionService;
    use crate::session::store::{
        migrate_add_session_events, SessionEventStore, SqliteEventStore,
    };
    use crate::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService, ToolSource};
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    pub(super) struct InertProvider;
    impl AiProvider for InertProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str { "inert" }
        fn color(&self) -> &str { "#000" }
    }

    pub(super) struct NoopTool;
    #[async_trait::async_trait]
    impl ToolService for NoopTool {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, ToolError> {
            Ok(crate::session::events::ToolOutput {
                value: json!({}),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> { vec![] }
        async fn describe(&self, _name: &str) -> Option<ToolDefinition> { None }
        fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
            Arc::from([])
        }
    }

    pub(super) fn fresh_session_service() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    pub(super) fn make_deps_with_chain(chain: ChainContext) -> HarnessDeps {
        HarnessDeps {
            session: fresh_session_service(),
            tools: Arc::new(NoopTool),
            sandbox: Arc::new(NoopSandbox),
            llm: Arc::new(InertProvider),
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: chain,
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        }
    }
}

#[test]
fn root_harness_has_default_chain_at_depth_zero() {
    let harness = AgentHarness::new(stubs::make_deps_with_chain(ChainContext::default()));
    assert_eq!(harness.chain_context().depth, 0);
    assert!(harness.chain_context().is_root());
    assert!(!harness.chain_context().chain_id.is_empty());
}

#[test]
fn injected_chain_is_visible_via_accessor() {
    let root = ChainContext::new();
    let level1 = root.child().expect("depth 0 → 1");
    let level2 = level1.child().expect("depth 1 → 2");

    let harness = AgentHarness::new(stubs::make_deps_with_chain(level2.clone()));
    assert_eq!(harness.chain_context().depth, 2);
    assert_eq!(harness.chain_context().chain_id, root.chain_id);
}

#[test]
fn three_layer_chain_preserves_id_and_increments_depth() {
    let root = ChainContext::new();
    let l1 = root.child().expect("0→1");
    let l2 = l1.child().expect("1→2");
    let l3 = l2.child().expect("2→3");

    let h_root = AgentHarness::new(stubs::make_deps_with_chain(root.clone()));
    let h_l1 = AgentHarness::new(stubs::make_deps_with_chain(l1.clone()));
    let h_l2 = AgentHarness::new(stubs::make_deps_with_chain(l2.clone()));
    let h_l3 = AgentHarness::new(stubs::make_deps_with_chain(l3.clone()));

    // chain_id is invariant across all four levels.
    assert_eq!(h_root.chain_context().chain_id, h_l1.chain_context().chain_id);
    assert_eq!(h_l1.chain_context().chain_id, h_l2.chain_context().chain_id);
    assert_eq!(h_l2.chain_context().chain_id, h_l3.chain_context().chain_id);
    // Depth increments by exactly 1 per level.
    assert_eq!(h_root.chain_context().depth, 0);
    assert_eq!(h_l1.chain_context().depth, 1);
    assert_eq!(h_l2.chain_context().depth, 2);
    assert_eq!(h_l3.chain_context().depth, 3);
}

#[test]
fn trait_default_returns_none_for_non_overriding_impls() {
    // Synthetic Harness impl that does not override chain_context() must
    // continue to return None (preserves existing mock ergonomics).
    struct Bare;
    #[async_trait::async_trait]
    impl Harness for Bare {
        async fn run_turn(
            &self,
            _sid: &crate::session::service::SessionId,
            _cb: &mut dyn crate::harness::callback::HarnessCallback,
        ) -> Result<crate::harness::trait_def::TurnState, crate::harness::trait_def::HarnessError>
        {
            Ok(crate::harness::trait_def::TurnState::Done)
        }
    }
    let b = Bare;
    let h: &dyn Harness = &b;
    assert!(h.chain_context().is_none());
}

#[test]
fn agent_harness_trait_dispatch_returns_some_chain() {
    let root = ChainContext::new();
    let h = AgentHarness::new(stubs::make_deps_with_chain(root.clone()));
    let h_dyn: &dyn Harness = &h;
    let chain = h_dyn.chain_context().expect("AgentHarness must report a chain");
    assert_eq!(chain.chain_id, root.chain_id);
    assert_eq!(chain.depth, 0);
}

/// Concurrent readers of `chain_context()` across `Arc<AgentHarness>` clones
/// must observe a stable, immutable chain. The accessor returns `&self.deps.
/// chain_context`; since `ChainContext` fields are read-only after
/// construction, this is a smoke test rather than a UB hunt — but it
/// nails down the contract that the seam is `Send + Sync` safe under load.
#[test]
fn concurrent_readers_see_stable_chain() {
    let root = ChainContext::new();
    let chain_id = root.chain_id.clone();
    let harness = Arc::new(AgentHarness::new(stubs::make_deps_with_chain(root)));

    let mut handles = Vec::new();
    for _ in 0..16 {
        let h = harness.clone();
        let expected = chain_id.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..1_000 {
                assert_eq!(h.chain_context().chain_id, expected);
                assert_eq!(h.chain_context().depth, 0);
            }
        }));
    }
    for jh in handles {
        jh.join().expect("reader thread joined");
    }
}
```

- [ ] **Step 2: Register the module**

In `src/harness/mod.rs`, inside the `#[cfg(test)] mod tests { ... }` block, add `mod chain;` next to the other `mod` lines:

```rust
#[cfg(test)]
mod tests {
    mod act;
    mod chain;
    mod driver;
    mod prompt;
    mod stability;
    mod task10_wiring;
    mod think;
    mod tools_surface;
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib harness::tests::chain
```

Expected: 6 chain tests pass.

- [ ] **Step 4: Subagent end-to-end smoke**

The existing test `subagent_spawner::tests::spawn_single_turn_returns_final_text` already verifies `result.depth == 1`. Add one more test there to verify the chain plumbing reaches the inner harness — but since the inner harness is dropped before `spawn` returns, the public-facing assertion stays at the `LoopRunResult` level. We covered the inner-harness assertion via the unit tests above.

- [ ] **Step 5: Commit**

```bash
git add src/harness/tests/chain.rs src/harness/mod.rs
git commit -m "test(harness): chain accessor unit + 3-layer + concurrent reader

Stage 4 step 3: 6 tests covering AgentHarness::chain_context() behavior.

  - root_harness_has_default_chain_at_depth_zero
  - injected_chain_is_visible_via_accessor
  - three_layer_chain_preserves_id_and_increments_depth (3-level acceptance)
  - trait_default_returns_none_for_non_overriding_impls
  - agent_harness_trait_dispatch_returns_some_chain
  - concurrent_readers_see_stable_chain (Send+Sync smoke under 16×1000 reads)"
```

---

## Task 4: CHANGELOG + master-spec status flip + final verification

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`

- [ ] **Step 1: Append Stage 4 entries to `CHANGELOG.md` `## [Unreleased]`**

Under `### Added`:

```markdown
- **harness**: `AgentHarness::chain_context()` accessor exposes the harness's position in the subagent chain (Stage 4 / module #11).
- **harness**: `Harness` trait gains an `Option<&ChainContext>` default method (`None`) so non-`AgentHarness` impls stay ergonomic; `AgentHarness` overrides to `Some(...)`.
```

Under `### Changed`:

```markdown
- **harness**: `HarnessDeps` gains a `chain_context: ChainContext` field (defaults to a fresh root chain). `subagent_spawner::spawn` writes the descended `child_chain` into the inner harness's deps so each nested level reports the correct depth.
```

- [ ] **Step 2: Flip Stage 4 status in master spec**

Edit `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`:

```diff
 ### Stage 4 — Subagent ChainContext Wiring (#11)

-**Status**: 🟡 Pending
+**Status**: ✅ Shipped <commit-sha> on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage4-subagent-chain-plan.md
```

- [ ] **Step 3: Run full harness test suite + clippy + fmt**

```bash
cargo test -p alephcore --lib harness::
cargo test -p alephcore --lib agents::subagent_spawner
cargo clippy -p alephcore --lib -- -D warnings
cargo fmt -p alephcore --check
```

Expected:
- All harness tests pass (baseline 41+ + 4 prompt + 6 chain = ~51 tests).
- All subagent_spawner tests pass (8+).
- Zero clippy warnings.
- fmt clean.

- [ ] **Step 4: Verify R10 budgets**

```bash
ls src/harness/*.rs | wc -l           # expect 9 (canonical, unchanged)
wc -l src/harness/agent.rs             # expect ≤ 1500 (target ~1250)
wc -l src/harness/deps.rs              # expect ≤ 800
```

- [ ] **Step 5: Verify Stage 4 master-spec acceptance criteria**

| Criterion | Verifier |
|-----------|---------|
| 子 agent 调用链中每个 agent 能追溯到根（chain depth 可查询） | `three_layer_chain_preserves_id_and_increments_depth` (chain.rs) |
| 单 agent 调用（无 subagent 场景）行为不变 | All existing 41+ harness tests pass; root agents construct `HarnessDeps` with `ChainContext::default()` and behavior is unchanged |
| 根 agent 不强制要求 chain | `root_harness_has_default_chain_at_depth_zero` (chain.rs) verifies the default works |
| ≥1 个集成验证 3 层 subagent 谱系完整 | `three_layer_chain_preserves_id_and_increments_depth` (chain.rs) |
| ≥1 个验证根 agent 无显式 chain 时行为合理 | `root_harness_has_default_chain_at_depth_zero` (chain.rs) |
| ≥1 个 loom（跨线程 spawn 安全） | `concurrent_readers_see_stable_chain` (chain.rs) — std::thread fan-out smoke under 16×1000 reads. ChainContext is immutable post-construction; no loom needed for the read-only contract. |

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md
git commit -m "docs: ship Stage 4 (Subagent ChainContext Wiring) — flip master spec status

Wraps Stage 4 of the 12-module harness roadmap. Acceptance criteria
mechanically verified:
  - chain_context accessor on AgentHarness + trait default None
  - subagent spawner writes descended chain into child harness deps
  - 3-layer chain consistency test (chain_id stable, depth monotonic)
  - root agent default works without explicit chain
  - concurrent reader smoke test passes (16×1000 reads, no torn state)

Stage 5 (Guardrails Pipeline) is gated by Stages 1+2; both shipped."
```

---

## Self-Review Checklist (run before handoff)

**1. Spec coverage** (master spec § Stage 4 Acceptance):
- [x] AgentHarness::chain_context() accessor: agent.rs (Task 1)
- [x] trait_def chain context propagation method: trait_def.rs (Task 1)
- [x] subagent_spawner writes descended chain: subagent_spawner.rs (Task 2)
- [x] ≥1 real consumer: subagent_spawner (Task 2) + 5 test consumers (Task 3)
- [x] 3-layer subagent integration test: chain.rs (Task 3)
- [x] root-agent-no-explicit-chain test: chain.rs (Task 3)
- [x] concurrent-thread-spawn safety test: chain.rs (Task 3)

**2. R10 budgets:**
- ✅ 9/9 canonical files (no new modules in `src/harness/`)
- ✅ agent.rs unchanged in size beyond ~5 lines (accessor + override)
- ✅ deps.rs grows by ~7 lines (field + doc comment)
- ✅ harness/ delta: ~+150 lines (well under +400 cap)
- ✅ Single PR ≤ 600 lines including tests (estimate ~250)

**3. Future-Proof Test (R10):**
- Model upgrades change *prompt content* / *tool calling* but not the chain structure (chain_id is provenance metadata, not model-perceivable). ✅
- Adding richer chain features (parent agent_id, span_id) extends `ChainContext` additively without changing the seam. ✅
- Stage 6 JudgeAgent injects a `JudgeVerifier` that reads `harness.chain_context()` to label trace events without patching `agent.rs`. ✅

**4. No-regression:**
- P0 rescue behavior: chain_context is read-only metadata; act loop / consecutive_failure_cap / turn_timeout untouched.
- Anchor #1 (Orchestration Loop): agent.rs run loop is unchanged; only the constructor and accessor are added.
- Anchor #3 (Memory): chain_id was already on `LoopRunResult` and reaches memory via `RawMemory(Delegation)` in subagent_spawner — that path is untouched.
- Anchor #7 (State & Checkpointing): chain_context is in-memory only; no event schema change.

**5. Old code retired:**
- This stage is purely additive — no struct/module deletions.
- The "subagent_spawner.rs 中独立构造 chain context 的代码段" called out by the master spec was already absent from the production path before this stage (the spawner descends via `base.chain.child()` which is the correct pattern). The "Old code to retire" bullet is therefore satisfied as a no-op precondition: there was no fallback path to delete.

---

## Execution Handoff

After plan commit, the implementer can either:

1. **Inline execution** — run all 4 tasks in sequence using `executing-plans`. Stage 4 is small (~150 lines) and low-risk; this is the recommended path.
2. **Subagent-driven** — fresh subagent per task. Same workflow as Stages 1-3.

Default to inline given the size.

**Baseline for diffs:** `1aa6bb48c`. Verify clean working tree before starting Task 1.

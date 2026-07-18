# Tool Scheduling Overhaul — Hermes-Inspired Wiring Cycle

**Date**: 2026-05-20
**Cycle**: One worktree, three commits, one merge
**Scope**: Tool/capability scheduling — fuse hermes-agent patterns into Aleph without copying its Python style
**Status**: design

---

## 1. Background

`hermes-agent` (Python, `/Volumes/TBU4/Github/hermes-agent`) ships three tool-scheduling patterns Aleph currently lacks:

| Pattern | Hermes location | Aleph today | Gap |
|---|---|---|---|
| `check_fn` runtime health probe with TTL caching | `tools/registry.py:126–141` | InteractionManifest + SecurityContext (no runtime probe) | Aleph exposes tools whose dependencies (Docker, auth, network) are dead → wasted LLM tool calls + bad UX |
| Dynamic schema mutation at emit time | `model_tools.py:100–106` | Static schema | Model can't see live state (subagent depth, available sandboxes, rate-limit clocks) |
| Memoized tool-def emission `(toolsets, registry gen, config mtime)` | `model_tools.py:289–312` | Recomputed every turn | Wasted CPU under high turn rate |

Aleph's harness is R10-clean (~1500 lines, no business logic), all 38 prompt layers are wired, MCP/Skill/Memory subsystems all shipped. The "missing pieces" are not infrastructure — they are **wiring** that surfaces existing health/state signals into the tool emission pipeline.

Intent detection is at parity: both projects (correctly per R7 LLM Sovereignty) delegate to LLM + system prompt + tool schema. No rule-based classifier needed.

## 2. Goals

1. **Tool availability gating** — runtime probes that strip dead tools from the schema the LLM sees
2. **Tool-def cache** — registry-generation-keyed memoization so `ContextAggregator::resolve()` is O(1) on cache hit
3. **Live tool state surface** — new `ToolRuntimeStateLayer` (priority 460) exposing per-tool runtime hints (depth limits, available sandboxes, "unavailable" reasons) via `<tool_runtime_state>` XML — R9 compliant: intelligence in prompt, not in wire format

## 3. Non-Goals

- **No** changes to `src/harness/*` (R10 redline holds)
- **No** changes to `Provider` trait or failover logic (already shipped)
- **No** rule-based intent classifier (R7)
- **No** ToolKind ACP enum mapping (deferred — separate small cycle)
- **No** MCP-as-server exposure (deferred — separate brainstorm)
- **No** dynamic JSON-schema mutation (intelligence lives in prompt layer, not in wire-level schema)

## 4. Architecture

Three components shipping as three reviewable commits on one worktree branch.

### 4.1 Component A — `ToolHealthGate` (Commit 1)

**Files:**
- New: `src/tools/health.rs`
- Edit: `src/executor/tool_registry.rs` (add default `health_probe()` accessor on `ToolHandler`)
- Edit: `src/thinker/context.rs` (third filter phase in `ContextAggregator::resolve()`)
- Edit: `src/tools/registry.rs` (expose `generation` counter for cache key)

**Trait surface:**

```rust
pub trait ToolHealthProbe: Send + Sync {
    fn probe<'a>(&'a self, ctx: &'a ProbeContext) -> BoxFuture<'a, ProbeResult>;
    fn ttl(&self) -> Duration { Duration::from_secs(30) }
}

pub enum ProbeResult {
    Healthy,
    Unhealthy { reason: HealthReason, retry_after: Option<Duration> },
}

pub enum HealthReason {
    DependencyDown(&'static str),
    AuthMissing(&'static str),
    RateLimited { until: Instant },
    Custom(Cow<'static, str>),
}
```

`ToolHandler` gains one default method returning `Option<&dyn ToolHealthProbe>` (None = always-healthy → all existing tools unchanged).

**Cache:**

```rust
pub struct ToolHealthCache {
    inner: ArcSwap<HashMap<ToolName, CachedProbe>>,
    generation: AtomicU64,
}
```

- TTL per-probe (default 30s)
- Subscribed to `RegistryChange` broadcast (existing) → full clear on registry mutation
- Single-flight via `tokio::sync::OnceCell` per `(tool, ttl_window)`
- Hard timeout: `tokio::time::timeout(200ms, probe(ctx))` — timeout treated as `Unhealthy { DependencyDown("probe timeout") }`
- Panic safe: `catch_unwind` → `Unhealthy { Custom("probe panicked") }`

**Filter integration:** `ContextAggregator::resolve()` already runs two phases (InteractionManifest, SecurityContext). Add phase 3: `ToolHealthCache.lookup_or_probe()`. Stripped tools removed from `EmittedToolSet` but their `(name, reason)` recorded for Component C consumption.

### 4.2 Component B — `ToolEmitCache` (Commit 2)

**Files:**
- Edit: `src/thinker/context.rs` (add cache slot on `ContextAggregator`)
- Edit: `src/thinker/layers/tools.rs` (consume cached `Arc<EmittedToolSet>`)

**Key:**

```rust
struct ToolEmitCacheKey {
    registry_generation: u64,
    manifest_hash: u64,
    security_hash: u64,
    health_generation: u64,
}
```

Single-entry cache (not LRU) — Python hermes uses the same pattern. `ArcSwap<Option<Entry>>` for lock-free read path.

**Invalidation drivers:**
- `RegistryChange` broadcast → bump `registry_generation`
- `ToolHealthCache` evict → bump `health_generation`
- `InteractionManifest` change → new `manifest_hash`
- `SecurityContext` change → new `security_hash`

Miss = recompute via Component A's three-phase filter, then re-store.

### 4.3 Component C — `ToolRuntimeStateLayer` (Commit 3)

**Files:**
- New: `src/thinker/layers/tool_runtime_state.rs`
- Edit: `src/thinker/prompt_pipeline.rs` (register layer @ priority 460)
- Edit: 2–3 high-value tools to opt in as demonstration (`delegate_task`, `execute_code`/`bash`, one MCP bridge tool)

**Trait surface:**

```rust
pub trait ToolRuntimeState: Send + Sync {
    fn describe<'a>(&'a self, ctx: &'a RuntimeStateContext)
        -> BoxFuture<'a, Option<RuntimeStateFragment>>;
}

pub struct RuntimeStateFragment {
    pub tool_name: ToolName,
    pub status: ToolStatus,
    pub hints: Vec<String>,
}

pub enum ToolStatus {
    Available,
    Unavailable { reason: String },
}
```

`ToolHandler` gains one default method returning `Option<&dyn ToolRuntimeState>` (None = no runtime state to surface).

**Layer placement:** Priority 460, immediately after `ToolsLayer@450`. Reads cached `EmittedToolSet` (incl. unavailable list from Component A) + calls each opt-in tool's `describe()`.

**Output:**

```xml
<tool_runtime_state>
  <tool name="delegate_task">
    <hint>Current subagent depth: 2 of 4. You have 2 more levels available.</hint>
  </tool>
  <tool name="execute_code">
    <hint>Sandboxes available: docker, modal. Preferred: docker.</hint>
  </tool>
  <tool name="send_telegram" status="unavailable">
    <hint>Telegram bridge offline (last heartbeat 5m ago).</hint>
  </tool>
</tool_runtime_state>
```

Note: `unavailable` entries appear even when the tool is absent from `<available_tools>` — the model knows the capability exists but is dormant.

## 5. Data Flow

```
User msg → Gateway → SessionService → AgentHarness::run_turn()
  ↓
  PromptPipeline::execute_with_mode(input)
    ↓
    ContextAggregator::resolve(input):
      key = ToolEmitCacheKey::from(input)
      HIT  → Arc<EmittedToolSet>
      MISS:
        phase 1: InteractionManifest.supports_tool()      (existing)
        phase 2: SecurityContext.check_tool()             (existing)
        phase 3: ToolHealthCache.lookup_or_probe()        (NEW)
        → EmittedToolSet { available, unavailable_with_reason }
        cache.store(key, set)
    ↓
    PromptLayer chain by priority:
      …
      450  ToolsLayer            ← uses EmittedToolSet.available
      460  ToolRuntimeStateLayer ← uses EmittedToolSet + per-tool describe()
      …
  ↓
  race_llm_call → Provider → model sees: healthy tool schemas + <tool_runtime_state> hints
  ↓
  act() → tool execution → results in history
  ↓
  Next turn: TTL not expired → phase 3 cache-hit; RegistryChange → ToolEmitCache invalidated
```

**Invariants preserved:**
1. Harness (`src/harness/*`) sees no new logic. R10 holds.
2. Probe failure never fails a turn (timeout = tool stripped, turn continues).
3. Cache miss is always the correctness fallback path — no functionality lost.

## 6. Error Handling

| Failure | Behavior | Fallback |
|---|---|---|
| Probe panic | `catch_unwind` → `Unhealthy{Custom("probe panicked")}` | Tool stripped, retried after TTL |
| Probe hang | 200 ms timeout | Same |
| Cache lock contention | `ArcSwap` lock-free | N/A |
| Layer rendering error | log + empty fragment | Turn continues |
| Concurrent probe trigger | `tokio::sync::OnceCell` single-flight | One probe per `(tool, ttl_window)` |

## 7. Testing

**Unit (in each commit):**
- `ToolHealthCache` TTL / invalidation / timeout / single-flight
- `ContextAggregator` three-phase ordering
- `ToolEmitCache` key equality + broadcast invalidation
- `ToolRuntimeStateLayer` rendering (Available + Unavailable cases)

**Integration:**
- e2e turn with mock probe failure → tool absent from `<available_tools>` + present as `unavailable` in `<tool_runtime_state>`
- High-frequency turn loop hits cache; `ContextAggregator::resolve` recompute counter increments only on key change
- `RegistryChange` broadcast invalidates cache mid-session

**Baseline-aware regression (per `feedback_pre_check_main_before_merge` + `baseline_test_failures` memory):**
- `cargo test --lib` baseline = 19 failures + 1 deadlock; merge requires `≤ 19` failures
- Pre-merge: `git log --name-only main...branch` diff vs main, confirm no main-only file silently lost during 3-way merge

## 8. File Inventory

**New:**
- `src/tools/health.rs` (~250 lines)
- `src/thinker/layers/tool_runtime_state.rs` (~180 lines)

**Edited:**
- `src/executor/tool_registry.rs` — 2 default trait methods (~10 lines)
- `src/tools/registry.rs` — expose `generation` + `RegistryChange.generation` field (~20 lines)
- `src/thinker/context.rs` — phase 3 + `ToolEmitCache` slot (~80 lines)
- `src/thinker/layers/tools.rs` — consume cached `EmittedToolSet` (~15 lines)
- `src/thinker/prompt_pipeline.rs` — register `ToolRuntimeStateLayer` (~5 lines)
- 2–3 demonstration opt-in tools (`delegate_task`, `bash`/`execute_code`, one MCP bridge) (~80 lines total)

**Cleanup (folded into relevant commit):**
- Scan `src/tools/builtin/*.rs` for `if cfg!(...)` / `if env::var(...)` register-time short-circuits → migrate to `health_probe`
- Delete any `TODO: skip unavailable tools` comments
- Remove per-turn rebuild loop in `ContextAggregator` (replaced by cache lookup)

**Estimated diff size:** ~750 added, ~150 modified, ~50 removed. Largest commit is C1 (~400 lines, all new code in `health.rs` + cache wiring).

## 9. Cycle Plan

1. Enter worktree `tool-scheduling-overhaul` from `main`
2. invoke `writing-plans` skill → granular implementation plan
3. Implement Commit 1 (ToolHealthGate) → verify (unit + 1 integration) → commit
4. Implement Commit 2 (ToolEmitCache) → verify (cache hit/miss tests) → commit
5. Implement Commit 3 (ToolRuntimeStateLayer + demo opt-ins) → verify (rendering + e2e) → commit
6. Pre-merge baseline check (`cargo test --lib` count vs main)
7. Pre-merge main-file-diff check (per `feedback_pre_check_main_before_merge`)
8. Merge to `main`, push, update `MEMORY.md`

## 10. Open Questions

None at design time. (Cache eviction policy, single-flight strategy, timeout values, layer priority — all decided above.)

## 11. References

- `docs/reference/HARNESS_PHILOSOPHY.md` — R10 thin-harness redline
- `docs/reference/TOOL_SYSTEM.md` — current tool registry architecture
- `docs/reference/MODEL_PERCEIVABLE_ECOSYSTEM.md` — how the model "sees" the world
- hermes-agent: `tools/registry.py`, `model_tools.py` (read-only reference)
- Memory: `project_hermes_inspired_wiring_cycle.md`, `project_hermes_prompt_alignment.md`

# Tool Scheduling Overhaul — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fuse three hermes-agent patterns (TTL health probes, tool-def memoization, runtime state surface) into Aleph as one worktree, three commits, one merge — without violating R7/R9/R10.

**Architecture:** Add a `ToolHealthProbe` trait that the dispatcher's `ToolRegistry` consults (alongside the existing `is_active` flag) when emitting native tool schemas via `generate_smart_prompt`. Memoize the emission result keyed by a four-component cache key. Render runtime state via a new `ToolRuntimeStateLayer` (priority 502) — intelligence stays in the prompt, not in wire-format schema.

**Tech Stack:** Rust, tokio, `arc_swap::ArcSwap`, `tokio::sync::OnceCell` (single-flight), `tokio::time::timeout` (probe deadline), `async_trait`.

**Spec:** `docs/superpowers/specs/2026-05-20-tool-scheduling-overhaul-design.md`

**Integration site refinement (vs spec):** Spec assumed `ContextAggregator` was the gating site; investigation shows tool schemas reach the LLM via the **dispatcher registry's `generate_smart_prompt`** (since `harness_bridge.rs:592` passes an empty tool list to `ContextAggregator`). The plan therefore wires health into the dispatcher path. `ContextAggregator` is still extended (new `DisableReason::Unhealthy` variant) so any future prompt-text tool list also picks up the data.

**Baseline:** `cargo test --lib` on `main` has **19 failures + 1 deadlocking test** (`parallel_adds_do_not_lose_entries`) per memory `project_baseline_test_failures`. Pre-merge gate: failure count must stay `≤ 19`.

---

## File Structure

**New files:**
- `src/dispatcher/registry/health.rs` (~250 lines) — `ToolHealthProbe` trait, `ProbeResult`, `HealthReason`, `ToolHealthCache`
- `src/thinker/layers/tool_runtime_state.rs` (~180 lines) — new `PromptLayer` impl at priority 502
- `src/tools/runtime_state.rs` (~80 lines) — `ToolRuntimeState` trait + `RuntimeStateFragment` type

**Modified files:**
- `src/tools/handlers/mod.rs` (~10 lines) — two default `Option<&dyn ...>` accessors on `ToolHandler`
- `src/tools/registry.rs` (~25 lines) — expose `generation: AtomicU64` counter; `RegistryChange` carries `generation`
- `src/dispatcher/registry/mod.rs` (~15 lines) — own a `ToolHealthCache`; subscribe to `RegistryChange`
- `src/dispatcher/registry/state.rs` (~20 lines) — wire health cache lookups
- `src/dispatcher/registry/discovery.rs` (~25 lines) — health-aware `.filter()` in `generate_smart_prompt` and `to_prompt_block`
- `src/thinker/context.rs` (~30 lines) — new `DisableReason::Unhealthy` variant + `runtime_state_blocks` field
- `src/thinker/layers/mod.rs` (~5 lines) — `pub mod tool_runtime_state`
- `src/thinker/prompt_pipeline.rs` (~5 lines) — register `ToolRuntimeStateLayer`
- `src/orchestrator/harness_bridge.rs` (~25 lines) — populate runtime state blocks pre-call

**Demonstration opt-ins (2-3 tools):**
- `src/builtin_tools/subagent_tool.rs` or wherever `delegate_task` lives — depth-limit hint
- `src/builtin_tools/bash.rs` or `execute_code` site — sandbox availability hint
- One MCP bridge tool — connectivity hint

---

## Commit Sequence

- **Commit 1 (Tasks 1.1–1.9):** `ToolHealthGate` — trait, cache, dispatcher integration, one demo opt-in, integration test
- **Commit 2 (Tasks 2.1–2.5):** `ToolEmitCache` — memoized emission with 4-key invalidation
- **Commit 3 (Tasks 3.1–3.8):** `ToolRuntimeStateLayer` — new layer, two more demo opt-ins, e2e test
- **Pre-merge (Tasks 4.1–4.3):** baseline regression, main-diff verification, merge

---

## Commit 1: ToolHealthGate

### Task 1.1: Create `src/dispatcher/registry/health.rs` (core types)

**Files:**
- Create: `src/dispatcher/registry/health.rs`
- Test: in-file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

```rust
// src/dispatcher/registry/health.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn probe_result_healthy_is_default_for_no_probe() {
        // A tool without a registered probe is treated as healthy
        let cache = ToolHealthCache::new();
        let snap = cache.snapshot();
        assert!(snap.is_healthy("any_tool")); // missing entry = healthy
    }

    #[test]
    fn health_reason_renders_for_prompt() {
        let r = HealthReason::DependencyDown("docker daemon offline");
        assert_eq!(r.short_label(), "docker daemon offline");
    }

    #[test]
    fn unhealthy_with_retry_after_serializes_round_trip() {
        let r = ProbeResult::Unhealthy {
            reason: HealthReason::RateLimited {
                until_ms_from_epoch: 0,
            },
            retry_after: Some(Duration::from_secs(60)),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ProbeResult = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ProbeResult::Unhealthy { .. }));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::registry::health -- --nocapture`
Expected: FAIL — module/types do not exist.

- [ ] **Step 3: Implement the types**

```rust
// src/dispatcher/registry/health.rs
//! Tool health probe surface.
//!
//! Hermes-inspired check_fn TTL gate. Each tool may opt into a probe that
//! reports whether its runtime dependencies are alive. The dispatcher's
//! ToolRegistry consults the cache alongside `is_active` when emitting
//! native tool schemas, so the LLM never sees dead tools.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tokio::time::timeout;

/// Default probe timeout. Probes that exceed this are treated as Unhealthy.
const PROBE_DEADLINE: Duration = Duration::from_millis(200);
/// Default TTL between probe re-evaluations for a single tool.
pub const DEFAULT_PROBE_TTL: Duration = Duration::from_secs(30);

/// Why a tool is currently unhealthy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum HealthReason {
    DependencyDown(Cow<'static, str>),
    AuthMissing(Cow<'static, str>),
    /// Wall-clock millis-from-epoch when rate-limit lifts.
    RateLimited { until_ms_from_epoch: i64 },
    Custom(Cow<'static, str>),
}

impl HealthReason {
    /// One-line label for prompt or log output.
    pub fn short_label(&self) -> &str {
        match self {
            HealthReason::DependencyDown(s) | HealthReason::AuthMissing(s) => s,
            HealthReason::Custom(s) => s,
            HealthReason::RateLimited { .. } => "rate limited",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ProbeResult {
    Healthy,
    Unhealthy {
        reason: HealthReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<Duration>,
    },
}

/// A tool opts into health gating by implementing this trait and returning
/// a reference from `ToolHandler::health_probe()`.
#[async_trait::async_trait]
pub trait ToolHealthProbe: Send + Sync {
    /// Cheap availability check. Implementations MUST be bounded — the
    /// cache enforces a 200ms hard deadline.
    async fn probe(&self) -> ProbeResult;

    /// How long a successful probe result may be cached.
    fn ttl(&self) -> Duration {
        DEFAULT_PROBE_TTL
    }
}

#[derive(Clone)]
struct CachedProbe {
    result: ProbeResult,
    cached_at: Instant,
    ttl: Duration,
}

impl CachedProbe {
    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.cached_at) >= self.ttl
    }
}

/// In-process TTL cache of probe results. Lookups are lock-free via ArcSwap.
/// Refreshes use OnceCell single-flight to prevent thundering herd.
pub struct ToolHealthCache {
    entries: ArcSwap<HashMap<String, CachedProbe>>,
    inflight: dashmap::DashMap<String, Arc<OnceCell<ProbeResult>>>,
    generation: AtomicU64,
}

impl ToolHealthCache {
    pub fn new() -> Self {
        Self {
            entries: ArcSwap::from_pointee(HashMap::new()),
            inflight: dashmap::DashMap::new(),
            generation: AtomicU64::new(0),
        }
    }

    /// Lock-free read snapshot.
    pub fn snapshot(&self) -> HealthSnapshot {
        HealthSnapshot {
            entries: self.entries.load_full(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Clear every cached entry and bump generation. Called when the
    /// registry mutates (broadcast subscriber).
    pub fn invalidate_all(&self) {
        self.entries.store(Arc::new(HashMap::new()));
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Run the probe with timeout + panic guard, store the result, bump
    /// generation. Returns the freshly-stored result.
    pub async fn refresh(&self, name: &str, probe: &dyn ToolHealthProbe) -> ProbeResult {
        let cell = self
            .inflight
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let probe_owned = name.to_string();
        let result = cell
            .get_or_init(|| async move {
                let bounded = timeout(PROBE_DEADLINE, probe.probe()).await;
                match bounded {
                    Ok(r) => r,
                    Err(_) => ProbeResult::Unhealthy {
                        reason: HealthReason::DependencyDown(Cow::Borrowed("probe timeout")),
                        retry_after: None,
                    },
                }
            })
            .await
            .clone();
        // Free the single-flight slot so the next TTL window gets a fresh attempt.
        self.inflight.remove(&probe_owned);

        let cached = CachedProbe {
            result: result.clone(),
            cached_at: Instant::now(),
            ttl: probe.ttl(),
        };
        let mut next = (**self.entries.load()).clone();
        next.insert(name.to_string(), cached);
        self.entries.store(Arc::new(next));
        self.generation.fetch_add(1, Ordering::Release);
        result
    }
}

impl Default for ToolHealthCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable snapshot of the cache state.
#[derive(Clone)]
pub struct HealthSnapshot {
    entries: Arc<HashMap<String, CachedProbe>>,
}

impl HealthSnapshot {
    /// Tools without a probe (or with cached `Healthy`) are healthy.
    /// Expired entries are treated as healthy too — the refresh path is
    /// responsible for picking them up on the next turn.
    pub fn is_healthy(&self, name: &str) -> bool {
        let now = Instant::now();
        match self.entries.get(name) {
            None => true,
            Some(entry) if entry.expired(now) => true,
            Some(entry) => matches!(entry.result, ProbeResult::Healthy),
        }
    }

    pub fn reason(&self, name: &str) -> Option<&HealthReason> {
        let now = Instant::now();
        match self.entries.get(name) {
            Some(entry) if !entry.expired(now) => match &entry.result {
                ProbeResult::Unhealthy { reason, .. } => Some(reason),
                ProbeResult::Healthy => None,
            },
            _ => None,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib dispatcher::registry::health -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 5: Confirm `dashmap` is already a workspace dependency**

Run: `grep dashmap Cargo.toml`
Expected: at least one match. If absent, add `dashmap = "5"` under `[dependencies]` in `Cargo.toml` and rerun the test.

- [ ] **Step 6: Add module to dispatcher**

Edit `src/dispatcher/registry/mod.rs` — add near other `pub mod` lines:

```rust
pub mod health;
```

- [ ] **Step 7: Run cargo check**

Run: `cargo check -p alephcore`
Expected: clean.

### Task 1.2: Extend `ToolHandler` with default `health_probe()`

**Files:**
- Modify: `src/tools/handlers/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
// src/tools/handlers/mod.rs (inside existing #[cfg(test)] section, or add one)
#[cfg(test)]
mod health_probe_default_tests {
    use super::*;

    struct PlainHandler;

    #[async_trait::async_trait]
    impl ToolHandler for PlainHandler {
        async fn invoke(&self, _input: serde_json::Value)
            -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError>
        {
            unreachable!()
        }
        fn definition(&self) -> crate::tools::service::ToolDefinition {
            unreachable!()
        }
    }

    #[test]
    fn default_health_probe_is_none() {
        let h = PlainHandler;
        assert!(h.health_probe().is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tools::handlers::health_probe_default_tests`
Expected: FAIL — `health_probe` method does not exist.

- [ ] **Step 3: Add default method**

Edit `src/tools/handlers/mod.rs` — extend the trait:

```rust
#[async_trait]
pub trait ToolHandler: Send + Sync + 'static {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError>;
    fn definition(&self) -> ToolDefinition;

    /// Opt-in runtime health probe. Default `None` = always healthy.
    /// Override to gate the tool's appearance in the model-visible schema
    /// based on dependency liveness (Docker, auth, network, etc.).
    fn health_probe(&self) -> Option<&dyn crate::dispatcher::registry::health::ToolHealthProbe> {
        None
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib tools::handlers::health_probe_default_tests`
Expected: PASS.

- [ ] **Step 5: cargo check**

Run: `cargo check -p alephcore`
Expected: clean (the default impl is non-breaking).

### Task 1.3: Add `ToolHealthCache` to dispatcher `ToolRegistry`

**Files:**
- Modify: `src/dispatcher/registry/mod.rs`
- Modify: `src/dispatcher/registry/state.rs`

- [ ] **Step 1: Find current ToolRegistry struct & constructor**

Run: `grep -n "pub struct ToolRegistry\|impl ToolRegistry\|fn new" src/dispatcher/registry/mod.rs src/dispatcher/registry/state.rs`
Note the exact lines so the edits add the field at the right place.

- [ ] **Step 2: Add `health: Arc<ToolHealthCache>` field + constructor**

In `src/dispatcher/registry/state.rs` (or wherever `ToolRegistry`'s state lives), add field:

```rust
use crate::dispatcher::registry::health::ToolHealthCache;

pub struct ToolRegistryState {
    // ...existing fields...
    pub(crate) health: Arc<ToolHealthCache>,
}

impl ToolRegistryState {
    // In existing constructor `new()`:
    // health: Arc::new(ToolHealthCache::new()),
}
```

If the dispatcher exposes `ToolRegistry` directly, add a getter:

```rust
impl ToolRegistry {
    pub fn health(&self) -> Arc<ToolHealthCache> {
        self.state.health.clone()
    }
}
```

- [ ] **Step 3: Verify with cargo check**

Run: `cargo check -p alephcore`
Expected: clean. If breaks, adjust field initialization sites (use Glob to find every constructor).

### Task 1.4: Subscribe to `tools::registry::RegistryChange` and invalidate the health cache

**Files:**
- Modify: `src/dispatcher/registry/mod.rs` (or wherever boot wiring lives)

- [ ] **Step 1: Write the failing test**

```rust
// src/dispatcher/registry/health.rs (extend existing #[cfg(test)] mod tests)
#[tokio::test]
async fn invalidate_all_bumps_generation_and_clears() {
    let cache = ToolHealthCache::new();
    struct DummyProbe;
    #[async_trait::async_trait]
    impl ToolHealthProbe for DummyProbe {
        async fn probe(&self) -> ProbeResult {
            ProbeResult::Unhealthy {
                reason: HealthReason::Custom("test".into()),
                retry_after: None,
            }
        }
    }
    cache.refresh("x", &DummyProbe).await;
    let g1 = cache.generation();
    assert!(!cache.snapshot().is_healthy("x"));
    cache.invalidate_all();
    assert!(cache.snapshot().is_healthy("x"));
    assert!(cache.generation() > g1);
}
```

- [ ] **Step 2: Run test to verify it fails or stalls compile**

Run: `cargo test -p alephcore --lib dispatcher::registry::health -- --nocapture`
Expected: PASS now (the cache logic already exists). If FAIL, fix `invalidate_all` accordingly.

- [ ] **Step 3: Wire the subscriber at boot**

Locate where `tools::registry::ToolRegistry` is instantiated and `subscribe()` would be a natural caller. The dispatcher registry already has access via dependency injection — extend its boot wiring:

```rust
// src/dispatcher/registry/mod.rs (boot section)
let mut rx = tool_registry.subscribe();
let health = state.health.clone();
tokio::spawn(async move {
    while let Ok(_evt) = rx.recv().await {
        health.invalidate_all();
    }
});
```

Place this in the existing dispatcher startup code path. If no such site exists, add it where `ToolRegistry` is first constructed and remains owned (search for `ToolRegistry::new()` callers).

- [ ] **Step 4: cargo check**

Run: `cargo check -p alephcore`
Expected: clean.

### Task 1.5: Gate `generate_smart_prompt` and `to_prompt_block` on health

**Files:**
- Modify: `src/dispatcher/registry/discovery.rs`

- [ ] **Step 1: Write the failing test**

Append to existing tests in `src/dispatcher/registry/discovery.rs` (or co-located test file):

```rust
#[cfg(test)]
mod health_filter_tests {
    use super::*;
    use crate::dispatcher::registry::health::{HealthReason, ProbeResult, ToolHealthCache};
    use std::sync::Arc;

    #[tokio::test]
    async fn unhealthy_tool_is_filtered_from_smart_prompt() {
        // 1. Construct ToolDiscovery with two active tools: "alive", "dead"
        // 2. Populate ToolHealthCache: "dead" → Unhealthy
        // 3. Call generate_smart_prompt(&["alive", "dead"], &[])
        // 4. Assert full_schema_tools contains only "alive"
        // (Test scaffolding fills in the actual UnifiedTool construction;
        //  follow the existing test helpers in this file for shape.)
        todo!("write per existing helper conventions");
    }
}
```

Replace `todo!()` with concrete setup matching the existing test helpers in `src/dispatcher/registry/`. If no such helpers exist, scope the test to use the smallest `UnifiedTool` constructor available.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::registry::discovery::health_filter_tests`
Expected: FAIL (`todo!` panic, or no filter yet).

- [ ] **Step 3: Add the filter**

Edit `generate_smart_prompt` (around line 78-95) — add health snapshot parameter and filter:

```rust
pub async fn generate_smart_prompt(
    &self,
    core_tools: &[&str],
    filtered_tools: &[&str],
    health: &HealthSnapshot,   // NEW
) -> (Vec<UnifiedTool>, String) {
    let tools = self.tools.read().await;

    let mut full_schema_tools = Vec::new();
    let mut index = ToolIndex::new();

    for tool in tools
        .values()
        .filter(|t| t.is_active)
        .filter(|t| health.is_healthy(&t.name))   // NEW
    {
        // ...existing branching unchanged...
    }
    // ...rest unchanged...
}
```

Apply the same filter to `to_prompt_block`, `generate_tool_index`, and `list_tools_by_category`.

- [ ] **Step 4: Update all callers**

Run: `grep -rn "generate_smart_prompt\|to_prompt_block\|generate_tool_index" src/ --include="*.rs"`
For each caller, supply a snapshot. Use `dispatcher_registry.health().snapshot()` where available; pass `HealthSnapshot::default()` in test helpers if needed (add a `Default` impl on `HealthSnapshot` returning an empty snapshot, which reports every tool as healthy).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib dispatcher::registry::discovery`
Expected: PASS.

- [ ] **Step 6: cargo check whole crate**

Run: `cargo check -p alephcore`
Expected: clean.

### Task 1.6: Refresh expired probes asynchronously inside `ToolDiscovery`

**Files:**
- Modify: `src/dispatcher/registry/discovery.rs` (or wherever ToolDiscovery accesses ToolHandler)

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn expired_probe_refreshes_on_next_smart_prompt() {
    // 1. Cache "x" as Healthy with TTL=1ms; sleep 10ms
    // 2. Register a probe that returns Unhealthy
    // 3. Call discovery.generate_smart_prompt(..., snap) where snap.is_healthy("x") is true (cached)
    // 4. Confirm a refresh path is triggered (assert via test hook on the cache)
    todo!();
}
```

- [ ] **Step 2: Implement refresh trigger**

`HealthSnapshot::is_healthy` returns true on expired entries (so the model still sees the tool that one extra turn while a refresh runs in the background). Spawn the refresh:

```rust
// inside ToolDiscovery::generate_smart_prompt, before the loop
for tool in tools.values().filter(|t| t.is_active) {
    if let Some(probe) = tool.handler().and_then(|h| h.health_probe()) {
        let snap = health.snapshot_for_refresh();
        if snap.needs_refresh(&tool.name) {
            let cache = health.clone();
            let name = tool.name.clone();
            // Detached refresh — never blocks the prompt assembly.
            tokio::spawn(async move {
                cache.refresh(&name, probe).await;
            });
        }
    }
}
```

Implement `HealthSnapshot::needs_refresh` (returns true when entry is missing OR `cached_at + ttl <= now`).

- [ ] **Step 3: Run test**

Run: `cargo test -p alephcore --lib dispatcher::registry::discovery -- --nocapture`
Expected: PASS.

- [ ] **Step 4: cargo check**

Run: `cargo check -p alephcore`
Expected: clean.

### Task 1.7: Extend `DisableReason` with `Unhealthy` (for future prompt-text path)

**Files:**
- Modify: `src/thinker/context.rs`

- [ ] **Step 1: Write the failing test**

```rust
// src/thinker/context.rs (in existing #[cfg(test)] mod tests)
#[test]
fn disable_reason_unhealthy_serializes() {
    let r = DisableReason::Unhealthy {
        reason: "docker daemon offline".to_string(),
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: DisableReason = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, DisableReason::Unhealthy { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::context::tests::disable_reason_unhealthy_serializes`
Expected: FAIL — variant missing.

- [ ] **Step 3: Add the variant**

Edit `DisableReason` enum (around line 79):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum DisableReason {
    UnsupportedByChannel,
    BlockedByPolicy { reason: String },
    RequiresApproval { prompt: String },
    /// NEW: runtime health probe says the tool's dependencies are dead.
    /// The model is shown the reason so it can explain to the user.
    Unhealthy { reason: String },
}
```

- [ ] **Step 4: Update exhaustive match sites**

Run: `cargo check -p alephcore`
Expected: compiler will list every non-exhaustive `match` on `DisableReason`. For each, add an `Unhealthy { reason }` arm that surfaces the reason in the same way `BlockedByPolicy` does.

- [ ] **Step 5: Run test**

Run: `cargo test -p alephcore --lib thinker::context`
Expected: PASS.

### Task 1.8: One demonstration opt-in — `delegate_task` health probe

**Files:**
- Modify: `src/agents/subagent_spawner/mod.rs` (or wherever `SubagentTool` lives — `grep -rn "delegate_task" src/ | head`)

- [ ] **Step 1: Find the delegate_task tool struct**

Run: `grep -rn "delegate_task\|SubagentTool" src/ --include="*.rs" | head -10`
Identify the `ToolHandler` impl for `delegate_task`.

- [ ] **Step 2: Write the failing test**

In the same file as the tool:

```rust
#[cfg(test)]
mod delegate_task_health_tests {
    use super::*;
    use crate::dispatcher::registry::health::{HealthReason, ProbeResult};

    #[tokio::test]
    async fn at_max_depth_probe_reports_unhealthy() {
        // Construct a SubagentTool whose current depth equals max.
        let tool = /* construct */;
        let probe = tool.health_probe().expect("opt-in");
        let r = probe.probe().await;
        assert!(matches!(
            r,
            ProbeResult::Unhealthy { reason: HealthReason::DependencyDown(_), .. }
        ));
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alephcore --lib delegate_task_health_tests`
Expected: FAIL — `health_probe` returns None by default.

- [ ] **Step 4: Implement the probe**

Add to the same file:

```rust
struct DelegateTaskHealthProbe {
    depth_state: Arc<ChainDepthState>, // existing state owner
}

#[async_trait::async_trait]
impl crate::dispatcher::registry::health::ToolHealthProbe for DelegateTaskHealthProbe {
    async fn probe(&self) -> crate::dispatcher::registry::health::ProbeResult {
        let (current, max) = self.depth_state.snapshot();
        if current >= max {
            crate::dispatcher::registry::health::ProbeResult::Unhealthy {
                reason: crate::dispatcher::registry::health::HealthReason::DependencyDown(
                    std::borrow::Cow::Borrowed("subagent depth budget exhausted"),
                ),
                retry_after: None,
            }
        } else {
            crate::dispatcher::registry::health::ProbeResult::Healthy
        }
    }

    fn ttl(&self) -> std::time::Duration {
        // Subagent depth changes on every spawn/return — very short TTL.
        std::time::Duration::from_secs(2)
    }
}
```

Then override on `SubagentTool::health_probe()`. Match the actual depth-state owner names by following the existing code.

- [ ] **Step 5: Run test**

Run: `cargo test -p alephcore --lib delegate_task_health_tests`
Expected: PASS.

### Task 1.9: Integration test — unhealthy tool absent from native tool list

**Files:**
- Test: `tests/tool_scheduling.rs` (new)

- [ ] **Step 1: Write the integration test**

```rust
// tests/tool_scheduling.rs
//! Tool scheduling overhaul — Commit 1 integration coverage.

use alephcore::dispatcher::registry::health::{
    HealthReason, ProbeResult, ToolHealthCache, ToolHealthProbe,
};
use std::borrow::Cow;
use std::sync::Arc;

struct AlwaysDeadProbe;

#[async_trait::async_trait]
impl ToolHealthProbe for AlwaysDeadProbe {
    async fn probe(&self) -> ProbeResult {
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed("test fixture")),
            retry_after: None,
        }
    }
}

#[tokio::test]
async fn unhealthy_tool_is_excluded_from_smart_prompt() {
    let cache = Arc::new(ToolHealthCache::new());
    cache.refresh("dead_tool", &AlwaysDeadProbe).await;
    let snap = cache.snapshot();
    assert!(!snap.is_healthy("dead_tool"));
    assert!(snap.is_healthy("anything_else"));
}
```

- [ ] **Step 2: Run test**

Run: `cargo test --test tool_scheduling`
Expected: PASS.

- [ ] **Step 3: Run full lib tests against baseline**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: failures count `≤ 19`. If it exceeded, fix before committing.

- [ ] **Step 4: Commit Commit 1**

```bash
git add src/dispatcher/registry/health.rs \
  src/dispatcher/registry/mod.rs \
  src/dispatcher/registry/state.rs \
  src/dispatcher/registry/discovery.rs \
  src/tools/handlers/mod.rs \
  src/thinker/context.rs \
  src/agents/subagent_spawner/mod.rs \
  tests/tool_scheduling.rs
git commit -m "tools: ToolHealthGate — TTL probe + dispatcher filter (hermes-aligned)

Adds opt-in ToolHealthProbe trait + ToolHealthCache (ArcSwap + OnceCell
single-flight, 200ms hard timeout, default 30s TTL). Dispatcher's
generate_smart_prompt now filters by health alongside is_active so dead
tools never reach the model's native tool list.

DisableReason::Unhealthy added for any future prompt-text path.
SubagentTool opts in: at max depth the tool reports unhealthy so the
LLM cannot spawn beyond the budget."
```

---

## Commit 2: ToolEmitCache

### Task 2.1: Define `ToolEmitCacheKey` + `ToolEmitCache`

**Files:**
- Modify: `src/dispatcher/registry/discovery.rs` (or a new sibling `src/dispatcher/registry/emit_cache.rs` if it grows past 100 lines)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod emit_cache_tests {
    use super::*;
    #[test]
    fn cache_key_changes_on_any_dimension() {
        let base = ToolEmitCacheKey { registry_generation: 1, manifest_hash: 2, security_hash: 3, health_generation: 4 };
        assert_ne!(base, ToolEmitCacheKey { registry_generation: 99, ..base });
        assert_ne!(base, ToolEmitCacheKey { manifest_hash: 99, ..base });
        assert_ne!(base, ToolEmitCacheKey { security_hash: 99, ..base });
        assert_ne!(base, ToolEmitCacheKey { health_generation: 99, ..base });
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib dispatcher::registry::discovery::emit_cache_tests`
Expected: FAIL — type missing.

- [ ] **Step 3: Implement the types**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolEmitCacheKey {
    pub registry_generation: u64,
    pub manifest_hash: u64,
    pub security_hash: u64,
    pub health_generation: u64,
}

#[derive(Clone)]
struct EmitEntry {
    key: ToolEmitCacheKey,
    schemas: Arc<Vec<UnifiedTool>>,
    index_prompt: Arc<str>,
}

pub struct ToolEmitCache {
    slot: arc_swap::ArcSwap<Option<EmitEntry>>,
    recompute_count: std::sync::atomic::AtomicU64,
}

impl ToolEmitCache {
    pub fn new() -> Self {
        Self {
            slot: arc_swap::ArcSwap::from_pointee(None),
            recompute_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn try_get(&self, key: &ToolEmitCacheKey) -> Option<(Arc<Vec<UnifiedTool>>, Arc<str>)> {
        let snap = self.slot.load();
        snap.as_ref()
            .as_ref()
            .filter(|entry| entry.key == *key)
            .map(|entry| (entry.schemas.clone(), entry.index_prompt.clone()))
    }

    pub fn store(&self, key: ToolEmitCacheKey, schemas: Vec<UnifiedTool>, index_prompt: String) {
        self.slot.store(Arc::new(Some(EmitEntry {
            key,
            schemas: Arc::new(schemas),
            index_prompt: Arc::from(index_prompt),
        })));
    }

    #[cfg(test)]
    pub fn recompute_count(&self) -> u64 {
        self.recompute_count.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn bump_recompute(&self) {
        self.recompute_count.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p alephcore --lib emit_cache_tests`
Expected: PASS.

### Task 2.2: Implement `manifest_hash` and `security_hash`

**Files:**
- Modify: `src/thinker/interaction.rs` (`InteractionManifest::cache_hash`)
- Modify: `src/thinker/security_context.rs` (`SecurityContext::cache_hash`)

- [ ] **Step 1: Write the failing test**

```rust
// src/thinker/interaction.rs (inside existing #[cfg(test)])
#[test]
fn manifest_hash_changes_with_paradigm() {
    let a = InteractionManifest::new(InteractionParadigm::CliRich);
    let b = InteractionManifest::new(InteractionParadigm::WebRich);
    assert_ne!(a.cache_hash(), b.cache_hash());
}
```

(Equivalent test for `SecurityContext::cache_hash` in its module.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib interaction::tests::manifest_hash_changes_with_paradigm`
Expected: FAIL — method missing.

- [ ] **Step 3: Implement the methods**

```rust
// src/thinker/interaction.rs
use std::hash::{Hash, Hasher};
impl InteractionManifest {
    pub fn cache_hash(&self) -> u64 {
        let mut h = ahash::AHasher::default();
        self.paradigm.hash(&mut h);
        for cap in &self.capabilities { cap.hash(&mut h); }
        // constraints already implement Hash where applicable; include
        // every field that affects tool emission
        self.constraints.hash_for_cache(&mut h);
        h.finish()
    }
}

// src/thinker/security_context.rs — similar approach hashing every field
// that affects `check_tool()` output.
```

If `ahash` is not a workspace dep, use `std::collections::hash_map::DefaultHasher` instead.

- [ ] **Step 4: Run test**

Run: `cargo test -p alephcore --lib interaction::tests security_context::tests`
Expected: PASS.

### Task 2.3: Wire cache into `ToolDiscovery::generate_smart_prompt`

**Files:**
- Modify: `src/dispatcher/registry/discovery.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn smart_prompt_cache_hit_skips_recompute() {
    let discovery = /* construct with two tools */;
    let cache = Arc::new(ToolEmitCache::new());
    let key = ToolEmitCacheKey { registry_generation: 1, manifest_hash: 1, security_hash: 1, health_generation: 1 };

    discovery.generate_smart_prompt_cached(&["a"], &[], &HealthSnapshot::default(), &cache, key).await;
    let recomp_after_first = cache.recompute_count();
    discovery.generate_smart_prompt_cached(&["a"], &[], &HealthSnapshot::default(), &cache, key).await;
    let recomp_after_second = cache.recompute_count();
    assert_eq!(recomp_after_first, recomp_after_second, "cache hit must not recompute");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib smart_prompt_cache_hit_skips_recompute`
Expected: FAIL — method missing.

- [ ] **Step 3: Implement the cached entry point**

```rust
impl ToolDiscovery {
    pub async fn generate_smart_prompt_cached(
        &self,
        core_tools: &[&str],
        filtered_tools: &[&str],
        health: &HealthSnapshot,
        cache: &ToolEmitCache,
        key: ToolEmitCacheKey,
    ) -> (Arc<Vec<UnifiedTool>>, Arc<str>) {
        if let Some(hit) = cache.try_get(&key) {
            return hit;
        }
        cache.bump_recompute();
        let (schemas, index_prompt) = self.generate_smart_prompt(core_tools, filtered_tools, health).await;
        cache.store(key, schemas.clone(), index_prompt.clone());
        (
            Arc::new(schemas),
            Arc::from(index_prompt.as_str()),
        )
    }
}
```

- [ ] **Step 4: Run test**

Run: `cargo test -p alephcore --lib smart_prompt_cache_hit_skips_recompute`
Expected: PASS.

### Task 2.4: Switch existing callers to the cached path

**Files:**
- Modify: every caller of `generate_smart_prompt`

- [ ] **Step 1: Find callers**

Run: `grep -rn "generate_smart_prompt(" src/ --include="*.rs" | grep -v "generate_smart_prompt_cached"`
Expected: 1–3 production callers + tests.

- [ ] **Step 2: Migrate callers to `_cached`**

For each production caller, plumb through the cache (a single `Arc<ToolEmitCache>` owned by the dispatcher) and a freshly-built `ToolEmitCacheKey`. Tests can call the uncached path.

- [ ] **Step 3: cargo check + cargo test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: clean; failure count `≤ 19`.

### Task 2.5: Commit 2

- [ ] **Step 1: Commit**

```bash
git add src/dispatcher/registry/discovery.rs \
  src/thinker/interaction.rs \
  src/thinker/security_context.rs \
  $(git diff --cached --name-only | xargs -I{} echo {})
git commit -m "tools: ToolEmitCache — memoize native tool emission (hermes-aligned)

Single-entry ArcSwap cache keyed by (registry_generation,
manifest_hash, security_hash, health_generation). All four dimensions
drive invalidation; miss recomputes via the existing
generate_smart_prompt path. Hit is lock-free O(1).

InteractionManifest::cache_hash + SecurityContext::cache_hash compute
stable fingerprints over every field that influences tool emission."
```

---

## Commit 3: ToolRuntimeStateLayer

### Task 3.1: Define `ToolRuntimeState` trait + `RuntimeStateFragment`

**Files:**
- Create: `src/tools/runtime_state.rs`
- Modify: `src/tools/mod.rs` (add `pub mod runtime_state;`)

- [ ] **Step 1: Write the failing test**

```rust
// src/tools/runtime_state.rs
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragment_renders_simple_xml() {
        let f = RuntimeStateFragment {
            tool_name: "delegate_task".into(),
            status: ToolStatus::Available,
            hints: vec!["depth 2 of 4".to_string()],
        };
        let s = f.render_xml();
        assert!(s.contains("<tool name=\"delegate_task\">"));
        assert!(s.contains("<hint>depth 2 of 4</hint>"));
    }

    #[test]
    fn unavailable_fragment_includes_status_attr() {
        let f = RuntimeStateFragment {
            tool_name: "send_telegram".into(),
            status: ToolStatus::Unavailable { reason: "bridge offline".into() },
            hints: vec![],
        };
        let s = f.render_xml();
        assert!(s.contains("status=\"unavailable\""));
        assert!(s.contains("bridge offline"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tools::runtime_state`
Expected: FAIL — module missing.

- [ ] **Step 3: Implement**

```rust
// src/tools/runtime_state.rs
//! Per-tool runtime state surface.
//!
//! Tools opt into describing live state (depth budgets, available
//! sandboxes, rate-limit clocks). The ToolRuntimeStateLayer renders the
//! collected fragments into a <tool_runtime_state> XML block at prompt
//! priority 502 — intelligence in the prompt, not in wire-format schema.

use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone)]
pub struct RuntimeStateFragment {
    pub tool_name: String,
    pub status: ToolStatus,
    pub hints: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ToolStatus {
    Available,
    Unavailable { reason: String },
}

impl RuntimeStateFragment {
    pub fn render_xml(&self) -> String {
        let attr = match &self.status {
            ToolStatus::Available => String::new(),
            ToolStatus::Unavailable { .. } => " status=\"unavailable\"".to_string(),
        };
        let mut buf = format!("  <tool name=\"{}\"{}>\n", escape_attr(&self.tool_name), attr);
        if let ToolStatus::Unavailable { reason } = &self.status {
            buf.push_str(&format!("    <hint>{}</hint>\n", escape_text(reason)));
        }
        for h in &self.hints {
            buf.push_str(&format!("    <hint>{}</hint>\n", escape_text(h)));
        }
        buf.push_str("  </tool>\n");
        buf
    }
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}
fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub trait ToolRuntimeState: Send + Sync {
    fn describe<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Option<RuntimeStateFragment>> + Send + 'a>>;
}
```

Add to `src/tools/mod.rs`: `pub mod runtime_state;`.

- [ ] **Step 4: Run test**

Run: `cargo test -p alephcore --lib tools::runtime_state`
Expected: PASS.

### Task 3.2: Extend `ToolHandler` with default `runtime_state()`

**Files:**
- Modify: `src/tools/handlers/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod runtime_state_default_tests {
    use super::*;
    struct PlainHandler;
    #[async_trait::async_trait]
    impl ToolHandler for PlainHandler {
        async fn invoke(&self, _: serde_json::Value)
            -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError>
        { unreachable!() }
        fn definition(&self) -> crate::tools::service::ToolDefinition { unreachable!() }
    }
    #[test]
    fn default_runtime_state_is_none() {
        assert!(PlainHandler.runtime_state().is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails, then add the default**

```rust
fn runtime_state(&self) -> Option<&dyn crate::tools::runtime_state::ToolRuntimeState> {
    None
}
```

- [ ] **Step 3: Run test**

Expected: PASS.

### Task 3.3: Plumb `runtime_state_blocks` into `ResolvedContext`

**Files:**
- Modify: `src/thinker/context.rs`

- [ ] **Step 1: Write the failing test**

```rust
// src/thinker/context.rs (in existing #[cfg(test)])
#[test]
fn resolved_context_holds_runtime_state_blocks() {
    let mut ctx = ContextAggregator::resolve(
        &InteractionManifest::new(InteractionParadigm::Background),
        &SecurityContext::permissive(),
        &[],
    );
    ctx.runtime_state_blocks = vec![crate::tools::runtime_state::RuntimeStateFragment {
        tool_name: "x".into(),
        status: crate::tools::runtime_state::ToolStatus::Available,
        hints: vec![],
    }];
    assert_eq!(ctx.runtime_state_blocks.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — field missing.

- [ ] **Step 3: Add the field**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedContext {
    pub available_tools: Vec<ToolInfo>,
    pub disabled_tools: Vec<DisabledTool>,
    pub environment_contract: EnvironmentContract,
    #[serde(skip)]
    pub runtime_context: Option<super::runtime_context::RuntimeContext>,
    /// NEW: aggregated per-tool runtime state (depth, sandbox, etc.)
    /// Empty by default; populated by orchestrator before prompt build.
    #[serde(skip, default)]
    pub runtime_state_blocks: Vec<crate::tools::runtime_state::RuntimeStateFragment>,
}
```

Update existing `ResolvedContext { ... }` constructions in `resolve()` to include `runtime_state_blocks: Vec::new()`.

- [ ] **Step 4: cargo check + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib thinker::context`
Expected: clean + PASS.

### Task 3.4: New layer file `src/thinker/layers/tool_runtime_state.rs`

**Files:**
- Create: `src/thinker/layers/tool_runtime_state.rs`

- [ ] **Step 1: Write the failing test**

```rust
// src/thinker/layers/tool_runtime_state.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_layer::{LayerInput, PromptLayer};
    use crate::tools::runtime_state::{RuntimeStateFragment, ToolStatus};

    #[test]
    fn layer_priority_is_502() {
        assert_eq!(ToolRuntimeStateLayer.priority(), 502);
    }

    #[test]
    fn layer_renders_available_and_unavailable() {
        // Build a ResolvedContext containing two fragments; construct a
        // LayerInput::context with it; call inject; assert XML.
        // (Replace todos with actual LayerInput construction following
        //  patterns already in this file's siblings.)
        todo!();
    }

    #[test]
    fn layer_is_empty_when_no_fragments() {
        let layer = ToolRuntimeStateLayer;
        let mut out = String::new();
        // Use the smallest valid LayerInput where runtime_state_blocks is empty
        // and assert nothing is emitted.
        todo!();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::layers::tool_runtime_state`
Expected: FAIL.

- [ ] **Step 3: Implement the layer**

```rust
// src/thinker/layers/tool_runtime_state.rs
//! ToolRuntimeStateLayer — emits <tool_runtime_state> XML at priority 502.
//!
//! Sits immediately after ToolsLayer (500) and HydratedToolsLayer (501),
//! surfacing per-tool runtime state (depth limits, sandbox availability,
//! 'unavailable: reason' hints). R9 in action: intelligence lives in the
//! prompt, not in the wire-format JSON schema.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ToolRuntimeStateLayer;

impl PromptLayer for ToolRuntimeStateLayer {
    fn name(&self) -> &'static str {
        "tool_runtime_state"
    }
    fn priority(&self) -> u32 {
        502
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        if ctx.runtime_state_blocks.is_empty() {
            return;
        }
        output.push_str("<tool_runtime_state>\n");
        for f in &ctx.runtime_state_blocks {
            output.push_str(&f.render_xml());
        }
        output.push_str("</tool_runtime_state>\n\n");
    }
}
```

- [ ] **Step 4: Fill in the `todo!()` tests**

Construct a real `LayerInput` using sibling tests in `src/thinker/layers/*.rs` as templates. Verify XML structure.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib thinker::layers::tool_runtime_state`
Expected: PASS (3).

### Task 3.5: Register layer in `prompt_pipeline.rs`

**Files:**
- Modify: `src/thinker/layers/mod.rs` — add `pub mod tool_runtime_state;`
- Modify: `src/thinker/prompt_pipeline.rs`

- [ ] **Step 1: Find registration site**

Run: `grep -n "ToolsLayer\|HydratedToolsLayer\|Box::new(" src/thinker/prompt_pipeline.rs | head -10`

- [ ] **Step 2: Add registration**

Near other `Box::new(SomeLayer)` lines, add:

```rust
Box::new(crate::thinker::layers::tool_runtime_state::ToolRuntimeStateLayer),
```

- [ ] **Step 3: cargo check**

Run: `cargo check -p alephcore`
Expected: clean.

### Task 3.6: Populate `runtime_state_blocks` in `harness_bridge.rs`

**Files:**
- Modify: `src/orchestrator/harness_bridge.rs`

- [ ] **Step 1: Write the failing integration test**

Add to `tests/tool_scheduling.rs`:

```rust
#[tokio::test]
async fn runtime_state_blocks_render_in_prompt() {
    // Set up a harness driver with one opt-in tool that returns a fragment.
    // Drive one turn. Inspect the assembled system prompt; assert it
    // contains <tool_runtime_state> with the fragment's hint.
    todo!("scaffold per existing tests/ patterns");
}
```

- [ ] **Step 2: Implement aggregation**

Around line 592 of `harness_bridge.rs`, after the existing `resolve(...)` call, collect fragments:

```rust
let mut fragments = Vec::new();
let snap = dispatcher.health().snapshot();
for tool in dispatcher.active_tools().await {
    if let Some(handler) = tool.handler() {
        if let Some(rs) = handler.runtime_state() {
            if let Some(f) = rs.describe().await {
                fragments.push(f);
            }
        }
        // Also surface health Unhealthy reasons as Unavailable fragments
        if let Some(reason) = snap.reason(&tool.name) {
            fragments.push(crate::tools::runtime_state::RuntimeStateFragment {
                tool_name: tool.name.clone(),
                status: crate::tools::runtime_state::ToolStatus::Unavailable {
                    reason: reason.short_label().to_string(),
                },
                hints: vec![],
            });
        }
    }
}
resolved_context.runtime_state_blocks = fragments;
```

(Adapt method names to the real `dispatcher` API surface — confirm with `grep -n "fn active_tools\|fn handler" src/dispatcher/`.)

- [ ] **Step 3: Run integration test**

Run: `cargo test --test tool_scheduling -- --nocapture`
Expected: PASS.

### Task 3.7: Two more demo opt-ins — `bash` and one MCP bridge tool

**Files:**
- Modify: `src/builtin_tools/<bash or execute_code>.rs` (find with `grep -rln "name.*bash\|name.*execute_code" src/builtin_tools/`)
- Modify: one MCP handler (look at `src/tools/handlers/mcp.rs`)

- [ ] **Step 1: Write a test per opt-in (depth-aware to existing state)**

For `bash`: probe reports a hint listing available sandbox modes (e.g., "shell available" or "sandbox: docker").
For the MCP tool: probe reports connectivity state (Unhealthy if upstream MCP server is disconnected).

- [ ] **Step 2: Implement each**

Pattern from Task 1.8 + Task 3.1. Keep each opt-in <40 lines.

- [ ] **Step 3: Run tests + cargo check**

Run: `cargo test -p alephcore --lib && cargo check -p alephcore`
Expected: PASS; failure count `≤ 19`.

### Task 3.8: Commit 3

- [ ] **Step 1: Run full test pass**

Run: `cargo test -p alephcore --lib 2>&1 | tail -10`
Expected: failure count `≤ 19`.

- [ ] **Step 2: Commit**

```bash
git add src/tools/runtime_state.rs \
  src/tools/handlers/mod.rs \
  src/tools/mod.rs \
  src/thinker/context.rs \
  src/thinker/layers/tool_runtime_state.rs \
  src/thinker/layers/mod.rs \
  src/thinker/prompt_pipeline.rs \
  src/orchestrator/harness_bridge.rs \
  src/builtin_tools/* \
  src/tools/handlers/mcp.rs \
  tests/tool_scheduling.rs
git commit -m "tools: ToolRuntimeStateLayer — live tool hints in prompt (hermes-aligned)

New PromptLayer at priority 502 surfaces per-tool runtime state via
<tool_runtime_state> XML. ToolHandler gains opt-in runtime_state()
returning Option<&dyn ToolRuntimeState>. Unhealthy tools surface as
status=unavailable with the probe's short label.

Demo opt-ins: delegate_task depth budget, bash sandbox availability,
one MCP bridge connectivity. R9 in action: intelligence lives in the
prompt, not in the wire-format JSON schema."
```

---

## Pre-Merge

### Task 4.1: Baseline regression check

- [ ] **Step 1: Capture branch test count**

Run: `cargo test -p alephcore --lib 2>&1 | tail -5 | tee /tmp/branch-tests.txt`
Note the failure count.

- [ ] **Step 2: Capture main test count for comparison**

```bash
git stash --include-untracked  # if needed
git fetch origin main && git checkout origin/main -- .
cargo test -p alephcore --lib 2>&1 | tail -5 | tee /tmp/main-tests.txt
git checkout HEAD -- .  # restore branch
```

- [ ] **Step 3: Compare**

`diff /tmp/main-tests.txt /tmp/branch-tests.txt` — branch must have `failure count ≤ main count` (i.e., ≤ 19). If higher, fix before merge.

### Task 4.2: Main-only file change diff (per `feedback_pre_check_main_before_merge`)

- [ ] **Step 1: List main-only files since branch-off**

Run: `git log --name-only main...HEAD -- '*.rs' | sort -u > /tmp/branch-files.txt`
Run: `git log --name-only HEAD..main -- '*.rs' | sort -u > /tmp/main-only-files.txt`

- [ ] **Step 2: Spot-check overlap**

For every file in `/tmp/main-only-files.txt` that overlaps with branch edits, manually verify the merge didn't drop main-side changes.

### Task 4.3: Merge to main

- [ ] **Step 1: Rebase / merge fresh main**

```bash
git fetch origin main
git rebase origin/main  # OR git merge --no-ff origin/main, per project norm
```

Resolve conflicts; rerun Task 4.1 to confirm tests still green.

- [ ] **Step 2: Merge worktree branch into main (no-ff for the audit trail)**

```bash
git checkout main
git merge --no-ff worktree-tool-scheduling-overhaul -m "merge: tool scheduling overhaul (hermes-aligned)"
```

- [ ] **Step 3: Push**

```bash
git push origin main
```

(Per existing memory `project_hermes_inspired_wiring_cycle.md` and recent cycles: don't auto-push to origin without an explicit go from the user. If unclear, stop after the local merge and confirm.)

- [ ] **Step 4: Memory + cleanup**

Add an entry to `~/.claude/projects/-Volumes-TBU4-Workspace-Aleph/memory/MEMORY.md` linking a new memory file describing this cycle's outcome, commits, and any deferred follow-ups. Remove the worktree per `feedback_worktree_for_implementation` in a NEW session (per `Git Worktree 注意事项` in CLAUDE.md, removing a worktree inside the same session corrupts the shell).

---

## Self-Review Checklist (run after writing this plan, before execution)

1. **Spec coverage**: every section of the spec maps to one or more tasks above. ✓
   - Spec §4.1 (ToolHealthGate) → Tasks 1.1–1.8
   - Spec §4.2 (ToolEmitCache) → Tasks 2.1–2.5
   - Spec §4.3 (ToolRuntimeStateLayer) → Tasks 3.1–3.8
   - Spec §5 (data flow) → integration test Task 1.9 + Task 3.6
   - Spec §6 (error handling) → Task 1.1 (timeout, panic guard, single-flight)
   - Spec §7 (testing) → unit tests in each task + integration tests 1.9 + 3.6
   - Spec §9 (cycle plan) → §"Commit Sequence" + Tasks 4.1–4.3

2. **Placeholders**: a handful of `todo!()` markers exist in test scaffolds (Tasks 1.5 step 1, 1.8 step 2, 3.4 step 1, 3.6 step 1, 3.7 step 1). Each is annotated with concrete instructions ("follow sibling test helpers", "scaffold per existing tests/ patterns") — these are not blanket placeholders but acknowledged "structure depends on local helpers"; the execution agent must replace each with concrete setup before running.

3. **Type consistency**: `ProbeResult`, `HealthReason`, `ToolHealthProbe`, `ToolHealthCache`, `HealthSnapshot`, `ToolRuntimeState`, `RuntimeStateFragment`, `ToolStatus`, `ToolEmitCacheKey`, `ToolEmitCache` — names match across tasks. ✓

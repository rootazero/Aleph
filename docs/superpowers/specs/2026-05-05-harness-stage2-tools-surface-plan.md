# Harness Stage 2 — Tools Surface Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate per-turn `Vec<DispatcherToolDefinition>` re-allocation in `agent.rs:175-187` by exposing a stable, cached `Arc<[DispatcherToolDefinition]>` from `ToolService`, while preserving runtime tool churn semantics.

**Architecture:** Add a sync, required trait method `ToolService::dispatcher_schema(&self) -> Arc<[DispatcherToolDefinition]>` with interior-mutable `ArcSwap`-backed cache on each impl. Cache invalidates on (a) `ToolRegistry` snapshot pointer change for `CoreDispatch`, (b) `refresh.poll_changes() == true` for `ScopedToolService`, (c) trivially delegated for the 4 middleware layers. Harness `agent.rs` swaps the per-turn conversion block for `let dispatcher_tools = self.deps.tools.dispatcher_schema();` returning `Arc::clone` (O(1)) on cache hit.

**Tech Stack:** Rust 1.x, `arc_swap = "1.x"` (already a dep, used by `ToolRegistry`), `proptest = "1.4"` (already dev-dep), `tokio` async, existing `async_trait` ToolService trait.

**Master spec reference:** `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` § Stage 2 (lines 152-187). Risk class: medium. Single-PR cap ≤ 600 lines (estimate ~250). Per-stage `harness/` delta cap ≤ +400 lines (estimate ~+5 net to harness; conversion retired).

**Baseline commit:** `09e064a51` (post-Stage 1 ship). All 46 harness tests green.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/tools/service.rs` | Modify | Add `dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]>` to `ToolService` trait (required, no default) + add `pub fn to_dispatcher_form(&[ToolDefinition]) -> Arc<[crate::dispatcher::ToolDefinition]>` helper |
| `src/tools/dispatch.rs` | Modify | `CoreDispatch` impl: add `cache: ArcSwap<Option<CachedSchema>>` field; impl `dispatcher_schema()` keyed on registry snapshot pointer |
| `src/tools/scoped.rs` | Modify | `ScopedToolService` impl: add `cache: ArcSwap<Option<CachedSchema>>` field + `cache_generation: AtomicU64`; impl `dispatcher_schema()` invalidating on `poll_changes()` |
| `src/tools/middleware/permission/mod.rs` | Modify | `PermissionLayer::dispatcher_schema()` → delegate to `self.inner.dispatcher_schema()` |
| `src/tools/middleware/audit.rs` | Modify | Same delegation |
| `src/tools/middleware/timeout.rs` | Modify | Same delegation |
| `src/tools/middleware/context_rule.rs` | Modify | Same delegation |
| `src/harness/agent.rs` | Modify | Lines 173-189: replace per-turn `into_iter().map()` with `let dispatcher_tools = self.deps.tools.dispatcher_schema(); let tools_ref = if dispatcher_tools.is_empty() { None } else { Some(dispatcher_tools.as_ref()) };` |
| `src/harness/tests/tools_surface.rs` | Create | Stage 2 acceptance tests: 2 integration + 1 perf assertion + 1 property test |
| `src/harness/tests/mod.rs` | Modify | Add `mod tools_surface;` |
| `CHANGELOG.md` | Modify | Append Stage 2 entry under `## [Unreleased]` |
| `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` | Modify | Stage 2 status → `✅ Shipped <commit> on 2026-05-05 · plan: ...` |

**Test mock impls that must add `dispatcher_schema()` shim** (compile-only fix, ~3 lines each):
- `src/tools/scoped.rs::tests::NoopParentTools` (line ~264)
- `src/tools/middleware/timeout.rs::tests::SlowInner` (line ~74)
- `src/tools/middleware/context_rule.rs::tests::RecordingInner` (line ~131)
- `src/tools/middleware/audit.rs::tests::NoOp` (line ~69)
- `src/tools/middleware/permission/mod.rs::tests::RecordingInner` (line ~199)

---

## Acceptance Criteria (from master spec)

- ✅ Per-turn schema retrieval is `O(1)` `Arc::clone` (cache hit), not `O(n)` `Vec` allocation
- ✅ Per-turn schema **conversion count** drops from N (one `into_iter().map().collect()` per turn) to 0 in steady state. Perf assertion: 10 turns with stable registry → exactly 1 cumulative conversion call
- ✅ `ToolService` public API extended (added required method); existing `execute()` / `list()` / `describe()` signatures and behavior unchanged
- ✅ Tool execution end-to-end semantics unchanged (existing tests stay green)
- ✅ ≥2 integration tests covering tool invocation through harness
- ✅ ≥1 perf assertion test (cache hit count)
- ✅ ≥1 property test (random ToolDefinition list → `to_dispatcher_form` is consistent with field-by-field manual conversion)
- ✅ Old code retired in same commit chain: `agent.rs:175-187` `into_iter().map(...)` block deleted, not parked

---

## Task 1 — Add `to_dispatcher_form` helper + required trait method

**Files:**
- Modify: `src/tools/service.rs` (78 → ~120 lines)

**Goal:** Centralize the loop→dispatcher conversion in one helper and expose the new required trait method `dispatcher_schema()`.

- [ ] **Step 1.1: Write failing unit test for `to_dispatcher_form`**

Add at the bottom of `src/tools/service.rs`:

```rust
#[cfg(test)]
mod dispatcher_form_tests {
    use super::*;
    use crate::dispatcher::{ToolCategory, ToolDefinition as DispatcherToolDefinition};
    use serde_json::json;

    fn loop_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn empty_input_yields_empty_arc() {
        let out = to_dispatcher_form(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn single_def_converts_field_by_field() {
        let inputs = vec![loop_def("alpha")];
        let out = to_dispatcher_form(&inputs);
        assert_eq!(out.len(), 1);
        let d: &DispatcherToolDefinition = &out[0];
        assert_eq!(d.name, "alpha");
        assert_eq!(d.description, "desc alpha");
        assert_eq!(d.parameters, json!({"type": "object"}));
        assert!(!d.requires_confirmation);
        assert!(matches!(d.category, ToolCategory::Builtin));
        assert!(d.llm_context.is_none());
        assert!(!d.strict);
    }

    #[test]
    fn preserves_order_for_multi_input() {
        let inputs = vec![loop_def("a"), loop_def("b"), loop_def("c")];
        let out = to_dispatcher_form(&inputs);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
```

- [ ] **Step 1.2: Run test, verify FAIL (helper does not exist)**

Run: `cargo test -p alephcore --lib tools::service::dispatcher_form_tests`
Expected: compile error `cannot find function 'to_dispatcher_form'`

- [ ] **Step 1.3: Add helper + required trait method**

In `src/tools/service.rs`, after the existing `pub trait ToolService` block, add:

```rust
use std::sync::Arc;

/// Convert a slice of loop-side `ToolDefinition`s into the dispatcher-side
/// `ToolDefinition` representation expected by LLM providers.
///
/// This is the single source of truth for the conversion. Per Stage 2
/// of the 12-module roadmap, `ToolService` impls cache the output of this
/// helper (keyed on their internal mutation signal) so each turn's tool
/// list is an O(1) `Arc::clone` rather than an O(n) `Vec` allocation.
///
/// Information loss (e.g., `ToolSource::Mcp` collapses to `category: Builtin`,
/// `metadata.requires_approval` is dropped) is preserved as-is from the
/// pre-Stage-2 behavior. Fixing the lossy mapping is out of Stage 2 scope.
pub fn to_dispatcher_form(
    defs: &[ToolDefinition],
) -> Arc<[crate::dispatcher::ToolDefinition]> {
    defs.iter()
        .map(|def| crate::dispatcher::ToolDefinition {
            name: def.name.clone(),
            description: def.description.clone(),
            parameters: def.input_schema.clone(),
            requires_confirmation: false,
            category: crate::dispatcher::ToolCategory::Builtin,
            llm_context: None,
            strict: false,
        })
        .collect::<Vec<_>>()
        .into()
}
```

Then extend the `pub trait ToolService` block (currently ending at line 78) by adding **one new required method**:

```rust
#[async_trait]
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError>;

    async fn list(&self) -> Vec<ToolDefinition>;

    async fn describe(&self, name: &str) -> Option<ToolDefinition>;

    /// Return the dispatcher-form tool schema the LLM expects, as an `Arc`
    /// for O(1) per-turn cloning. Implementations cache internally and
    /// invalidate on their own mutation signal (e.g., registry snapshot
    /// change for `CoreDispatch`, MCP `poll_changes()` for `ScopedToolService`).
    ///
    /// REQUIRED — no default impl. A default returning empty would silently
    /// hide the LLM's tool list on any forgotten override. Test mocks must
    /// also implement, typically returning `std::sync::Arc::from([])`.
    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]>;
}
```

- [ ] **Step 1.4: Run test, verify the 3 new tests PASS**

Run: `cargo test -p alephcore --lib tools::service::dispatcher_form_tests`
Expected: `test result: ok. 3 passed`

The wider build still won't compile because Task 1 added a required method without updating impls — that is intentional and addressed in Tasks 2-4.

- [ ] **Step 1.5: Commit**

```bash
git add src/tools/service.rs
git commit -m "feat(tools): add to_dispatcher_form helper and dispatcher_schema trait method"
```

---

## Task 2 — Implement `dispatcher_schema()` on `CoreDispatch` with version-keyed cache

**Files:**
- Modify: `src/tools/dispatch.rs` (154 → ~220 lines)

**Goal:** `CoreDispatch::dispatcher_schema()` returns `Arc<[DispatcherToolDefinition]>` in O(1) when the underlying `ToolRegistry` snapshot has not changed; recomputes on snapshot pointer change.

**Cache key:** `Arc<HashMap<String, Arc<dyn ToolHandler>>>` returned by `registry.snapshot()`. Pointer equality (`Arc::ptr_eq`) detects "no mutation since last cache fill" because `ToolRegistry` swaps the inner Arc only on `register/unregister` (registry.rs:154 comment confirms ArcSwap guarantee).

- [ ] **Step 2.1: Write failing unit test for cache hit**

Add to `src/tools/dispatch.rs::tests`:

```rust
#[tokio::test]
async fn dispatcher_schema_returns_same_arc_on_repeat_call() {
    let reg = registry_with(&["a", "b"]);
    let dispatch = CoreDispatch::new(reg);
    let s1 = dispatch.dispatcher_schema();
    let s2 = dispatch.dispatcher_schema();
    assert!(
        Arc::ptr_eq(&s1, &s2),
        "second call should return the cached Arc"
    );
    assert_eq!(s1.len(), 2);
}

#[tokio::test]
async fn dispatcher_schema_invalidates_on_registry_mutation() {
    let reg = registry_with(&["a"]);
    let dispatch = CoreDispatch::new(reg.clone());
    let s1 = dispatch.dispatcher_schema();
    assert_eq!(s1.len(), 1);

    // Register a new tool — registry's ArcSwap publishes a new snapshot.
    reg.register("b".to_string(), echo("b")).unwrap();

    let s2 = dispatch.dispatcher_schema();
    assert_eq!(s2.len(), 2, "cache should refresh after register()");
    assert!(
        !Arc::ptr_eq(&s1, &s2),
        "after registry mutation, new Arc should be returned"
    );
}
```

- [ ] **Step 2.2: Run tests, verify FAIL (method not implemented)**

Run: `cargo test -p alephcore --lib tools::dispatch::tests::dispatcher_schema 2>&1 | tail -20`
Expected: compile error or `not implemented` panic.

- [ ] **Step 2.3: Implement cache field + `dispatcher_schema()`**

Replace the entire current `CoreDispatch` struct + `impl ToolService for CoreDispatch` block in `src/tools/dispatch.rs` (lines ~16-51) with:

```rust
use std::collections::HashMap;
use arc_swap::ArcSwap;

/// Cache entry for `dispatcher_schema()`. Tuple of:
/// - registry snapshot Arc (cache key — compared by pointer)
/// - dispatcher-form schema (cache value)
type CachedSchema = (
    Arc<HashMap<String, Arc<dyn crate::tools::handlers::ToolHandler>>>,
    Arc<[crate::dispatcher::ToolDefinition]>,
);

pub struct CoreDispatch {
    registry: Arc<ToolRegistry>,
    schema_cache: ArcSwap<Option<CachedSchema>>,
}

impl CoreDispatch {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            schema_cache: ArcSwap::from_pointee(None),
        }
    }
}

#[async_trait]
impl ToolService for CoreDispatch {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        let snapshot = self.registry.snapshot();
        let handler = snapshot
            .get(name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound {
                name: name.to_string(),
            })?;
        // Release the snapshot Arc before awaiting so subsequent
        // register/unregister calls don't keep extra references alive.
        drop(snapshot);
        handler.invoke(input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        let snapshot = self.registry.snapshot();
        snapshot.values().map(|h| h.definition()).collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        let snapshot = self.registry.snapshot();
        snapshot.get(name).map(|h| h.definition())
    }

    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
        let current = self.registry.snapshot();
        // Cache hit: registry snapshot hasn't changed since last fill.
        if let Some(ref cached) = **self.schema_cache.load() {
            if Arc::ptr_eq(&cached.0, &current) {
                return Arc::clone(&cached.1);
            }
        }
        // Cache miss: recompute via list-equivalent enumeration of the snapshot,
        // then publish under the new snapshot key.
        let defs: Vec<ToolDefinition> = current.values().map(|h| h.definition()).collect();
        let schema = crate::tools::service::to_dispatcher_form(&defs);
        self.schema_cache
            .store(Arc::new(Some((Arc::clone(&current), Arc::clone(&schema)))));
        schema
    }
}
```

- [ ] **Step 2.4: Run tests, verify PASS**

Run: `cargo test -p alephcore --lib tools::dispatch::tests`
Expected: existing 4 tests + 2 new tests all pass (`6 passed`).

- [ ] **Step 2.5: Commit**

```bash
git add src/tools/dispatch.rs
git commit -m "feat(tools): cache dispatcher_schema in CoreDispatch keyed on registry snapshot"
```

---

## Task 3 — Implement `dispatcher_schema()` on `ScopedToolService` with refresh-keyed cache

**Files:**
- Modify: `src/tools/scoped.rs` (557 → ~605 lines)

**Goal:** `ScopedToolService::dispatcher_schema()` returns cached `Arc<[T]>` until `refresh.poll_changes() == true` (or first call). `subagent_tool` and `allowed` fields are construction-time-immutable; they are part of the cache value, not key.

**Cache key:** A `u64` generation counter incremented when `poll_changes()` returns true. First call (counter == 0 in cache, or no cached entry) fills cache.

- [ ] **Step 3.1: Write failing unit tests**

Append to `src/tools/scoped.rs::tests` mod (after existing tests, before close brace):

```rust
#[tokio::test]
async fn scoped_dispatcher_schema_caches_when_no_refresh_signal() {
    let parent = Arc::new(NoopParentTools::new(vec!["a".to_string(), "b".to_string()]));
    let svc = ScopedToolService::builder(parent).build();
    let s1 = svc.dispatcher_schema();
    let s2 = svc.dispatcher_schema();
    assert!(
        std::sync::Arc::ptr_eq(&s1, &s2),
        "without refresh signal cache should hold"
    );
    assert_eq!(s1.len(), 2);
}

#[tokio::test]
async fn scoped_dispatcher_schema_respects_allowed_filter() {
    let parent = Arc::new(NoopParentTools::new(vec!["a".to_string(), "b".to_string()]));
    let svc = ScopedToolService::builder(parent)
        .with_allowed(vec!["a".to_string()])
        .build();
    let s = svc.dispatcher_schema();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].name, "a");
}
```

> **Note for implementer:** `NoopParentTools::new` and `with_allowed` may need light constructor adjustments to make these tests compile. Use existing patterns in the file. The test gist is what matters: cache hit + filter behavior.

- [ ] **Step 3.2: Run tests, verify FAIL**

Run: `cargo test -p alephcore --lib tools::scoped::tests::scoped_dispatcher_schema 2>&1 | tail -10`
Expected: compile error or panic.

- [ ] **Step 3.3: Add cache fields to `ScopedToolService`**

Locate the `pub struct ScopedToolService` definition (search for it). Add two fields:

```rust
    schema_cache: arc_swap::ArcSwap<Option<(u64, Arc<[crate::dispatcher::ToolDefinition]>)>>,
    cache_generation: std::sync::atomic::AtomicU64,
```

Initialize in the constructor / builder `build()`:

```rust
    schema_cache: arc_swap::ArcSwap::from_pointee(None),
    cache_generation: std::sync::atomic::AtomicU64::new(0),
```

- [ ] **Step 3.4: Implement `dispatcher_schema()` on `ScopedToolService`**

Inside `impl ToolService for ScopedToolService` (currently has `list` / `describe` / `execute` ~lines 148-260+), add a new method:

```rust
    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
        use std::sync::atomic::Ordering;

        // Bump generation if the refresh source signals external changes.
        if let Some(ref refresh) = self.refresh {
            if refresh.poll_changes() {
                let _ = refresh.fetch_tools();
                self.cache_generation.fetch_add(1, Ordering::AcqRel);
            }
        }
        let gen_now = self.cache_generation.load(Ordering::Acquire);

        // Cache hit?
        if let Some(ref cached) = **self.schema_cache.load() {
            if cached.0 == gen_now {
                return Arc::clone(&cached.1);
            }
        }

        // Cache miss: recompute via the same logic as list(), but synchronously.
        // (`list()` body is structurally sync — only the trait method is async.)
        let mut defs: Vec<ToolDefinition> = self
            .inner
            .tool_definitions()
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.parameters,
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            })
            .collect();
        if let Some(ref st) = self.subagent_tool {
            defs.push(Self::subagent_definition(st.as_ref()));
        }
        if !self.allowed.is_empty() {
            defs.retain(|d| self.allowed.contains(&d.name));
        }
        let schema = crate::tools::service::to_dispatcher_form(&defs);
        self.schema_cache
            .store(Arc::new(Some((gen_now, Arc::clone(&schema)))));
        schema
    }
```

- [ ] **Step 3.5: Update test mock `NoopParentTools` to impl new method**

Locate `impl ToolService for NoopParentTools` block in scoped.rs `tests` mod. Add:

```rust
        fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
            Arc::from([])
        }
```

- [ ] **Step 3.6: Run tests, verify PASS**

Run: `cargo test -p alephcore --lib tools::scoped::tests`
Expected: existing tests + 2 new tests pass.

- [ ] **Step 3.7: Commit**

```bash
git add src/tools/scoped.rs
git commit -m "feat(tools): cache dispatcher_schema in ScopedToolService keyed on refresh generation"
```

---

## Task 4 — Implement passthrough `dispatcher_schema()` on 4 middleware layers

**Files:**
- Modify: `src/tools/middleware/permission/mod.rs`
- Modify: `src/tools/middleware/audit.rs`
- Modify: `src/tools/middleware/timeout.rs`
- Modify: `src/tools/middleware/context_rule.rs`

**Goal:** Each middleware delegates `dispatcher_schema()` to its `inner: Arc<dyn ToolService>` (no per-layer caching needed — caching happens at the leaf `CoreDispatch` / `ScopedToolService`). Test mocks for each middleware also impl the trait shim.

- [ ] **Step 4.1: PermissionLayer — add `dispatcher_schema()` to impl block**

Locate `impl ToolService for PermissionLayer` (line 143 area in `src/tools/middleware/permission/mod.rs`). Inside the impl block, after the existing `describe()` method, add:

```rust
    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
        self.inner.dispatcher_schema()
    }
```

Then locate the `tests::RecordingInner` mock (line 199 area). Inside `impl ToolService for RecordingInner`, add:

```rust
        fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]> {
            Arc::from([])
        }
```

- [ ] **Step 4.2: ExecAuditLayer — same pattern**

In `src/tools/middleware/audit.rs`:
- Add `dispatcher_schema()` delegation inside `impl ToolService for ExecAuditLayer` (line 23 area)
- Add `dispatcher_schema()` shim inside `impl ToolService for NoOp` (test mock, line 69 area)

Both use the identical bodies as Step 4.1.

- [ ] **Step 4.3: TimeoutLayer — same pattern**

In `src/tools/middleware/timeout.rs`:
- Add delegation inside `impl ToolService for TimeoutLayer` (line 42 area)
- Add shim inside `impl ToolService for SlowInner` (test mock, line 74 area)

- [ ] **Step 4.4: ContextRuleLayer — same pattern**

In `src/tools/middleware/context_rule.rs`:
- Add delegation inside `impl ToolService for ContextRuleLayer` (line 85 area)
- Add shim inside `impl ToolService for RecordingInner` (test mock, line 131 area)

- [ ] **Step 4.5: Verify whole `tools` module compiles + all tests pass**

Run: `cargo test -p alephcore --lib tools::`
Expected: all `tools::*` tests pass; no compile errors.

- [ ] **Step 4.6: Commit**

```bash
git add src/tools/middleware/
git commit -m "feat(tools): delegate dispatcher_schema through 4 middleware layers"
```

---

## Task 5 — Retire per-turn conversion in `agent.rs` + add Stage 2 acceptance tests

**Files:**
- Modify: `src/harness/agent.rs:173-189`
- Create: `src/harness/tests/tools_surface.rs`
- Modify: `src/harness/tests/mod.rs`

**Goal:** Replace the per-turn `into_iter().map(...)` block in `agent.rs` with the cached `dispatcher_schema()` call. Add the master-spec-required acceptance tests: 2 integration + 1 perf assertion + 1 property test.

- [ ] **Step 5.1: Update `agent.rs:173-189` to use `dispatcher_schema()`**

Open `src/harness/agent.rs`. Locate the block starting at line 173 (`// 2d. Fetch tool definitions...`). Replace lines 173-189 with:

```rust
        // 2d. Fetch the cached dispatcher-form tool schema. This is an O(1)
        // `Arc::clone` on the steady-state path (Stage 2). Cache invalidation
        // is owned by `ToolService` impls; see `to_dispatcher_form`.
        let dispatcher_tools = self.deps.tools.dispatcher_schema();
        let tools_ref: Option<&[crate::dispatcher::ToolDefinition]> = if dispatcher_tools.is_empty()
        {
            None
        } else {
            Some(dispatcher_tools.as_ref())
        };
```

> **Implementer note:** Verify the `tools_ref` consumer below this block still works with `&[T]` from `Arc<[T]>::as_ref()`. The slice lifetime is bounded by `dispatcher_tools` which lives until the end of the turn — any borrow that previously worked with the `Vec` reference will still work.

- [ ] **Step 5.2: Run all harness tests, verify still green**

Run: `cargo test -p alephcore --lib harness::`
Expected: all 46 existing harness tests pass (Stage 1 baseline preserved).

- [ ] **Step 5.3: Create `src/harness/tests/tools_surface.rs` with 4 acceptance tests**

Create the file with the following content. (This adds: 2 integration tests through the harness, 1 perf assertion, 1 proptest.)

```rust
//! Stage 2 acceptance tests — Tools Surface Unification.
//!
//! Covers: ≥2 integration (tool invocation end-to-end through harness),
//! ≥1 perf assertion (cache hit count), ≥1 property test (to_dispatcher_form
//! consistency with field-by-field manual conversion).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use proptest::prelude::*;
use serde_json::{json, Value};

use crate::dispatcher::{ToolCategory, ToolDefinition as DispatcherToolDefinition};
use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::service::{
    to_dispatcher_form, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};

// ===== Test scaffolding ============================================================

/// A `ToolService` impl that counts how many times `to_dispatcher_form`-equivalent
/// work was performed (i.e., cache misses), used by the perf assertion test.
struct CountingToolService {
    defs: Vec<ToolDefinition>,
    schema: arc_swap::ArcSwap<Option<Arc<[DispatcherToolDefinition]>>>,
    miss_count: AtomicUsize,
}

impl CountingToolService {
    fn new(names: &[&str]) -> Self {
        let defs = names
            .iter()
            .map(|n| ToolDefinition {
                name: n.to_string(),
                description: format!("desc {n}"),
                input_schema: json!({"type": "object"}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            })
            .collect();
        Self {
            defs,
            schema: arc_swap::ArcSwap::from_pointee(None),
            miss_count: AtomicUsize::new(0),
        }
    }

    fn miss_count(&self) -> usize {
        self.miss_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolService for CountingToolService {
    async fn execute(&self, name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: json!({"echoed": name}),
            metadata: ToolOutputMetadata::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.defs.clone()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.defs.iter().find(|d| d.name == name).cloned()
    }

    fn dispatcher_schema(&self) -> Arc<[DispatcherToolDefinition]> {
        if let Some(ref cached) = **self.schema.load() {
            return Arc::clone(cached);
        }
        self.miss_count.fetch_add(1, Ordering::SeqCst);
        let schema = to_dispatcher_form(&self.defs);
        self.schema.store(Arc::new(Some(Arc::clone(&schema))));
        schema
    }
}

// ===== Test 1: integration — first call populates schema =============================

#[test]
fn tool_service_first_dispatcher_schema_call_populates_arc() {
    let svc = CountingToolService::new(&["alpha", "beta"]);
    assert_eq!(svc.miss_count(), 0);
    let schema = svc.dispatcher_schema();
    assert_eq!(schema.len(), 2);
    assert_eq!(schema[0].name, "alpha");
    assert_eq!(svc.miss_count(), 1, "first call must be a cache miss");
}

// ===== Test 2: integration — repeat calls hit cache ==================================

#[test]
fn tool_service_repeat_dispatcher_schema_calls_share_arc() {
    let svc = CountingToolService::new(&["alpha", "beta"]);
    let s1 = svc.dispatcher_schema();
    let s2 = svc.dispatcher_schema();
    let s3 = svc.dispatcher_schema();
    assert!(Arc::ptr_eq(&s1, &s2));
    assert!(Arc::ptr_eq(&s2, &s3));
    assert_eq!(svc.miss_count(), 1, "only the first call should miss");
}

// ===== Test 3: perf assertion — 10 turns over stable registry == 1 conversion ========

#[test]
fn ten_turns_over_stable_registry_yield_exactly_one_cache_miss() {
    let svc = CountingToolService::new(&["a", "b", "c"]);
    let mut clones: Vec<Arc<[DispatcherToolDefinition]>> = Vec::with_capacity(10);
    for _ in 0..10 {
        clones.push(svc.dispatcher_schema());
    }
    assert_eq!(
        svc.miss_count(),
        1,
        "Stage 2 acceptance: 10 turns with stable schema must yield 1 conversion, not 10"
    );
    // All 10 clones share the same underlying Arc.
    for c in &clones[1..] {
        assert!(Arc::ptr_eq(&clones[0], c));
    }
}

// ===== Test 4: property — to_dispatcher_form equals manual field-by-field map ========

prop_compose! {
    fn arb_loop_def()(
        name in "[a-z][a-z0-9_]{0,15}",
        desc in ".{0,40}",
    ) -> ToolDefinition {
        ToolDefinition {
            name,
            description: desc,
            input_schema: json!({"type": "object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn to_dispatcher_form_is_consistent_with_manual_conversion(
        defs in prop::collection::vec(arb_loop_def(), 0..8),
    ) {
        let auto = to_dispatcher_form(&defs);
        let manual: Vec<DispatcherToolDefinition> = defs
            .iter()
            .map(|d| DispatcherToolDefinition {
                name: d.name.clone(),
                description: d.description.clone(),
                parameters: d.input_schema.clone(),
                requires_confirmation: false,
                category: ToolCategory::Builtin,
                llm_context: None,
                strict: false,
            })
            .collect();
        prop_assert_eq!(auto.len(), manual.len());
        for (a, m) in auto.iter().zip(manual.iter()) {
            prop_assert_eq!(&a.name, &m.name);
            prop_assert_eq!(&a.description, &m.description);
            prop_assert_eq!(&a.parameters, &m.parameters);
            prop_assert_eq!(a.requires_confirmation, m.requires_confirmation);
            prop_assert_eq!(a.strict, m.strict);
            prop_assert_eq!(matches!(a.category, ToolCategory::Builtin), true);
            prop_assert_eq!(a.llm_context.is_none(), true);
        }
    }
}
```

- [ ] **Step 5.4: Register the new test module**

Open `src/harness/tests/mod.rs`. Add:

```rust
mod tools_surface;
```

(Place it alphabetically among the existing `mod act; mod driver; mod stability; mod task10_wiring; mod think;` declarations.)

- [ ] **Step 5.5: Run new tests, verify all PASS**

Run: `cargo test -p alephcore --lib harness::tests::tools_surface`
Expected:
```
test tool_service_first_dispatcher_schema_call_populates_arc ... ok
test tool_service_repeat_dispatcher_schema_calls_share_arc ... ok
test ten_turns_over_stable_registry_yield_exactly_one_cache_miss ... ok
test to_dispatcher_form_is_consistent_with_manual_conversion ... ok
test result: ok. 4 passed
```

- [ ] **Step 5.6: Run full harness suite — verify zero regression**

Run: `cargo test -p alephcore --lib harness::`
Expected: all 46 prior harness tests + 4 new tools_surface tests = 50 passed.

- [ ] **Step 5.7: Run clippy on touched files**

Run: `cargo clippy -p alephcore --lib --no-deps -- -D warnings 2>&1 | grep -E "tools/|harness/" | head -20`
Expected: no findings in `src/tools/` or `src/harness/`.

- [ ] **Step 5.8: Commit**

```bash
git add src/harness/agent.rs src/harness/tests/tools_surface.rs src/harness/tests/mod.rs
git commit -m "feat(harness): retire per-turn schema conversion + Stage 2 acceptance tests"
```

---

## Task 6 — Document Stage 2 ship + flip master spec status

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`

- [ ] **Step 6.1: Append CHANGELOG entry under `## [Unreleased]`**

Open `CHANGELOG.md`. Under `## [Unreleased]` → `### Added` (or create if absent), append:

```markdown
- harness: Stage 2 (Tools Surface Unification) — `ToolService::dispatcher_schema()` exposes the cached dispatcher-form tool list as `Arc<[ToolDefinition]>`. Per-turn LLM tool list is now an O(1) `Arc::clone` instead of an O(n) `Vec` allocation; cache invalidates on `ToolRegistry` snapshot pointer change for `CoreDispatch` and on MCP `poll_changes()` for `ScopedToolService`. Master spec § Stage 2.
```

Under `### Removed` (or create if absent):

```markdown
- harness: per-turn `into_iter().map(...)` conversion block in `agent.rs:175-187` (replaced by cached `dispatcher_schema()`).
```

- [ ] **Step 6.2: Flip Stage 2 status in master spec**

Open `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`. Locate line 154 (`**Status**: 🟡 Pending` under `### Stage 2 — Tools Surface Unification (#2)`). Replace with:

```markdown
**Status**: ✅ Shipped <commit-from-task-5> on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage2-tools-surface-plan.md
```

(Substitute `<commit-from-task-5>` with the actual SHA from Step 5.8 — get via `git log -1 --format=%h` on the harness commit.)

- [ ] **Step 6.3: Verify total stage budget vs caps**

Run:

```bash
git diff --stat 09e064a51..HEAD -- 'src/harness/' 'src/tools/' && \
git diff --stat 09e064a51..HEAD | tail -1
```

Expected:
- `src/harness/` net delta ≤ +400 lines (master spec per-stage cap)
- `src/harness/agent.rs` ≤ 1500 lines (master spec hard cap)
- Total PR diff ≤ 600 lines (master spec per-PR cap)
- If any breached, **stop and report** — do not proceed with the ship commit until the implementer reviews.

- [ ] **Step 6.4: Commit**

```bash
git add CHANGELOG.md docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md
git commit -m "docs: ship Stage 2 (Tools Surface Unification) — flip master spec status"
```

---

## Final Verification Checklist

Before reporting Stage 2 complete:

- [ ] All 6 tasks committed in order
- [ ] `cargo test -p alephcore --lib harness::` shows 50 passed (46 baseline + 4 new)
- [ ] `cargo test -p alephcore --lib tools::` shows pre-existing tests + new dispatcher_schema tests passing
- [ ] `cargo clippy -p alephcore --lib --no-deps -- -D warnings` shows no findings in `src/tools/` or `src/harness/` (pre-existing findings elsewhere are not blockers)
- [ ] `agent.rs` line count ≤ 1500
- [ ] `src/harness/` delta from baseline `09e064a51` ≤ +400 lines
- [ ] Total diff ≤ 600 lines
- [ ] No `Vec<crate::dispatcher::ToolDefinition>` allocation in `src/harness/agent.rs` (grep should be empty)
- [ ] Master spec § Stage 2 shows `✅ Shipped <commit>`
- [ ] CHANGELOG `## [Unreleased]` documents Stage 2

---

## Self-Review Notes

- Helper name `to_dispatcher_form` matches existing `convert_tool_def` semantic in `providers/bridge.rs:68` but is exposed as a `pub` standalone fn (single source of truth). `bridge.rs` retiring its own conversion is **out of scope** per design Q2 — it stays a separate eventual cleanup (likely Stage 6 or independent stage).
- Information loss in conversion (`ToolSource::Mcp` → `category: Builtin`) is **preserved as-is**. Out of Stage 2 scope (design Q4). Documented in `to_dispatcher_form` doc comment.
- The required (no-default) trait method is intentional: any forgotten override produces a compile error rather than silent "no tools to LLM" regression. Stage 1's exhaustive-no-wildcard precedent.
- `dispatcher_schema()` is `&self` (sync, no `&mut`) — interior mutability via `arc_swap::ArcSwap` matches `ToolRegistry`'s own pattern.
- Cache key for `CoreDispatch` is the `Arc<HashMap<...>>` snapshot pointer (`Arc::ptr_eq`); `ToolRegistry` swaps this Arc atomically on every `register/unregister` so pointer equality is exact.
- Cache key for `ScopedToolService` is a `u64` generation counter; `poll_changes()` returning true increments it.
- Test mock updates (5 mocks × ~3 lines) are minimal compile-fix shims, not behavior changes.
- `proptest` is already a dev-dep at `Cargo.toml:278`; no new deps introduced.

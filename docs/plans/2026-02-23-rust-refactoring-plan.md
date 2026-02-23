# Rust Refactoring: Compilation Fix + Dead Code Elimination — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix 6 compilation errors in alephcore, then systematically eliminate ~132 `#[allow(dead_code)]` annotations, deleting confirmed dead code and properly attributing the rest.

**Architecture:** Bottom-up approach: fix compilation first (so `cargo check` works as our oracle), then batch-remove `#[allow(dead_code)]` annotations and let the compiler guide deletion. Each batch is verified independently before moving to the next.

**Tech Stack:** Rust, cargo check, cargo clippy, cargo test

**Design doc:** `docs/plans/2026-02-23-rust-refactoring-design.md`

---

## Task 1: Fix swarm_events Type Conflict

The `events.rs` and `swarm_events.rs` files define identical types (`AgentLoopEvent`, `InsightSeverity`). Both are re-exported from `mod.rs`, causing E0252. The `agent_loop.rs` imports from `swarm_events`, but `mod.rs` re-exports from both, creating type mismatches (E0308).

**Files:**
- Delete: `core/src/agent_loop/swarm_events.rs`
- Modify: `core/src/agent_loop/events.rs` — add serde tag attribute
- Modify: `core/src/agent_loop/mod.rs` — remove swarm_events module and duplicate re-export
- Modify: `core/src/agent_loop/agent_loop.rs:20` — fix import path

**Step 1: Add serde tag attribute to events.rs**

The `swarm_events.rs` version has `#[serde(tag = "type", rename_all = "snake_case")]` which the actual usage in `agent_loop.rs` depends on. Add this to `events.rs`:

```rust
// In core/src/agent_loop/events.rs, change line 11:
// FROM:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentLoopEvent {
// TO:
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLoopEvent {
```

**Step 2: Fix import in agent_loop.rs**

```rust
// In core/src/agent_loop/agent_loop.rs, change line 20:
// FROM:
use super::swarm_events::AgentLoopEvent;
// TO:
use super::events::AgentLoopEvent;
```

**Step 3: Remove swarm_events from mod.rs**

```rust
// In core/src/agent_loop/mod.rs:
// 1. Remove line 74:
mod swarm_events;

// 2. Remove lines 111-112 (the duplicate re-export):
// Re-export swarm events
pub use swarm_events::{AgentLoopEvent, InsightSeverity};
```

**Step 4: Delete swarm_events.rs**

```bash
rm core/src/agent_loop/swarm_events.rs
```

**Step 5: Verify compilation**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -30
```

Expected: E0252 and E0308 errors gone. E0599 (`from_builder`) may still remain.

**Step 6: Commit**

```bash
git add core/src/agent_loop/events.rs core/src/agent_loop/agent_loop.rs core/src/agent_loop/mod.rs
git rm core/src/agent_loop/swarm_events.rs
git commit -m "agent_loop: merge swarm_events into events module to fix E0252 type conflict"
```

---

## Task 2: Add AgentLoop::from_builder Constructor

The `AgentLoopBuilder::build()` calls `AgentLoop::from_builder()` which doesn't exist. Need to add a `pub(crate)` constructor that accepts all builder fields.

Also: the builder tests access `agent_loop.swarm_coordinator` which is a private field. Need to make it `pub(crate)` to match `config` and `overflow_detector` visibility.

**Files:**
- Modify: `core/src/agent_loop/agent_loop.rs` — add `from_builder` method + fix field visibility

**Step 1: Make swarm_coordinator pub(crate)**

```rust
// In core/src/agent_loop/agent_loop.rs, change line 128:
// FROM:
    swarm_coordinator: Option<Arc<crate::agents::swarm::coordinator::SwarmCoordinator>>,
// TO:
    pub(crate) swarm_coordinator: Option<Arc<crate::agents::swarm::coordinator::SwarmCoordinator>>,
```

**Step 2: Add from_builder constructor**

Add this method inside the `impl<T, E, C> AgentLoop<T, E, C>` block, after the `with_unified_session` constructor (after line 216):

```rust
    /// Create a new AgentLoop from builder components
    ///
    /// This is called by `AgentLoopBuilder::build()` to construct an AgentLoop
    /// with all optional Swarm Intelligence components.
    pub(crate) fn from_builder(
        thinker: Arc<T>,
        executor: Arc<E>,
        compressor: Arc<C>,
        config: LoopConfig,
        event_bus: Option<Arc<EventBus>>,
        overflow_detector: Option<Arc<OverflowDetector>>,
        swarm_coordinator: Option<Arc<crate::agents::swarm::coordinator::SwarmCoordinator>>,
    ) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config,
            compaction_trigger: OptionalCompactionTrigger::new(event_bus),
            overflow_detector,
            swarm_coordinator,
        }
    }
```

**Step 3: Verify compilation**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -30
```

Expected: 0 errors. All 6 original compilation errors resolved.

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib agent_loop 2>&1 | tail -20
```

Expected: all agent_loop tests pass (including builder tests).

**Step 5: Commit**

```bash
git add core/src/agent_loop/agent_loop.rs
git commit -m "agent_loop: add from_builder constructor for AgentLoopBuilder integration"
```

---

## Task 3: Fix build.rs for --all-features

The `build.rs` uses `Path` and `Command` inside `#[cfg(feature = "control-plane")]` without importing them.

**Files:**
- Modify: `core/build.rs`

**Step 1: Add imports inside cfg block**

```rust
// In core/build.rs, change the cfg block to add imports:
// After line 8 (`{`), add:
        use std::path::Path;
        use std::process::Command;
```

**Step 2: Verify**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --all-features 2>&1 | head -20
```

Expected: 0 errors from build.rs.

**Step 3: Commit**

```bash
git add core/build.rs
git commit -m "build: add missing imports in control-plane cfg block"
```

---

## Task 4: Verify Clean Compilation Baseline

Before starting dead code elimination, establish a clean baseline.

**Files:** None (verification only)

**Step 1: Full compilation check**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1
```

Expected: 0 errors. Warnings are acceptable at this stage.

**Step 2: Clippy check**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore 2>&1 | grep -E "^(warning|error)" | sort | uniq -c | sort -rn
```

Record baseline warning count.

**Step 3: Count current dead_code annotations**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -r '#\[allow(dead_code)\]' core/src/ | wc -l
```

Expected: ~132. Record exact count.

**Step 4: Run test suite**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -5
```

Record baseline test count and pass rate.

---

## Task 5: Dead Code Batch 1 — extension/

Remove `#[allow(dead_code)]` from all files in `core/src/extension/`, then triage compiler warnings.

**Files:**
- Modify: all files in `core/src/extension/` that contain `#[allow(dead_code)]`

**Step 1: Identify targets**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -rn '#\[allow(dead_code)\]' core/src/extension/
```

**Step 2: Remove all `#[allow(dead_code)]` annotations in extension/**

For each file found, remove the `#[allow(dead_code)]` line. Do NOT delete any actual code yet.

**Step 3: Compile and collect warnings**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | grep "warning.*dead_code\|warning.*unused" | grep "extension/"
```

**Step 4: Triage each warning**

For each warning, determine classification:

- **Struct field unused** (e.g., `config: ExtensionConfig` in `ExtensionManager`): Check if the field is read anywhere in the struct's impl blocks. If never read → delete field + remove from constructors. If read only under certain features → add `#[cfg_attr(not(feature = "X"), allow(dead_code))]`.

- **Method unused** (e.g., `convert_v2_hooks`): Search for callers with `grep -rn "convert_v2_hooks" core/src/`. If zero callers → delete the method. If called only in tests → it's fine (test code is gated).

- **Entire struct/enum unused**: Search for all references. If only constructed in tests → keep. If zero references → delete.

**Step 5: Apply deletions**

Delete confirmed dead code. For each deletion, verify compilation:

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -10
```

**Step 6: Verify tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib extension 2>&1 | tail -10
```

**Step 7: Commit**

```bash
git add core/src/extension/
git commit -m "extension: remove dead code and unused allow(dead_code) annotations"
```

---

## Task 6: Dead Code Batch 2 — providers/ + generation/

Same workflow as Task 5, targeting `core/src/providers/` and `core/src/generation/`.

**Files:**
- Modify: all files in `core/src/providers/` and `core/src/generation/` with `#[allow(dead_code)]`

**Step 1: Identify targets**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -rn '#\[allow(dead_code)\]' core/src/providers/ core/src/generation/
```

**Step 2: Remove annotations, compile, triage**

Follow the same workflow as Task 5 Steps 2-5.

**Common patterns in this batch:**
- Provider response structs with fields only used by serde deserialization (e.g., `done: bool` in `OllamaGenerateResponse`) — these are NOT dead code, they're needed for JSON parsing. Re-add `#[allow(dead_code)]` with comment: `// Deserialized from JSON response`.
- Provider-specific config fields only used under certain feature flags.

**Step 3: Verify and commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib providers 2>&1 | tail -10
git add core/src/providers/ core/src/generation/
git commit -m "providers: remove dead code and clean up allow(dead_code) annotations"
```

---

## Task 7: Dead Code Batch 3 — gateway/

Same workflow targeting `core/src/gateway/`.

**Files:**
- Modify: all files in `core/src/gateway/` with `#[allow(dead_code)]`

**Step 1: Identify targets**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -rn '#\[allow(dead_code)\]' core/src/gateway/
```

**Step 2-5: Remove, compile, triage, delete, verify**

Follow Task 5 workflow.

**Common patterns:**
- Channel-specific fields (iMessage, WhatsApp, Telegram) gated behind features — use `#[cfg_attr(not(feature = "telegram"), allow(dead_code))]`
- Adapter/bridge fields stored but only used in specific code paths
- Handler methods reserved for future RPC endpoints

**Step 6: Commit**

```bash
git add core/src/gateway/
git commit -m "gateway: remove dead code and clean up allow(dead_code) annotations"
```

---

## Task 8: Dead Code Batch 4 — memory/ + dispatcher/

Same workflow targeting `core/src/memory/` and `core/src/dispatcher/`.

**Files:**
- Modify: all files in `core/src/memory/` and `core/src/dispatcher/` with `#[allow(dead_code)]`

**Step 1: Identify targets**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -rn '#\[allow(dead_code)\]' core/src/memory/ core/src/dispatcher/
```

**Step 2-5: Follow Task 5 workflow**

**Common patterns:**
- LanceDB schema fields needed for Arrow conversion but not directly read in Rust
- Dispatcher config fields for future scheduling strategies
- Index/retrieval helper methods

**Step 6: Commit**

```bash
git add core/src/memory/ core/src/dispatcher/
git commit -m "memory, dispatcher: remove dead code and clean up allow(dead_code) annotations"
```

---

## Task 9: Dead Code Batch 5 — agent_loop/ + thinker/ + poe/

Same workflow targeting core agent modules.

**Files:**
- Modify: all files in `core/src/agent_loop/`, `core/src/thinker/`, `core/src/poe/` with `#[allow(dead_code)]`

**Step 1: Identify targets**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -rn '#\[allow(dead_code)\]' core/src/agent_loop/ core/src/thinker/ core/src/poe/
```

**Step 2-5: Follow Task 5 workflow**

**Common patterns:**
- Telemetry/tracing fields stored but not yet consumed
- POE validation types for future phases
- Thinker cache fields

**Step 6: Commit**

```bash
git add core/src/agent_loop/ core/src/thinker/ core/src/poe/
git commit -m "agent_loop, thinker, poe: remove dead code and clean up allow(dead_code) annotations"
```

---

## Task 10: Dead Code Batch 6 — Remaining Modules

Final sweep of all remaining `#[allow(dead_code)]` annotations.

**Files:**
- Modify: all remaining files with `#[allow(dead_code)]` in `core/src/`

**Step 1: Identify all remaining targets**

```bash
cd /Volumes/TBU4/Workspace/Aleph && grep -rn '#\[allow(dead_code)\]' core/src/ | grep -v "extension/\|providers/\|generation/\|gateway/\|memory/\|dispatcher/\|agent_loop/\|thinker/\|poe/"
```

This covers: `browser/`, `daemon/`, `engine/`, `exec/`, `intent/`, `config/`, `resilience/`, `perception/`, `permission/`, `runtimes/`, `wizard/`, `cron/`, `cli/`, `title_generator.rs`, `mcp/`, `skill_evolution/`, `prompt/`, and other scattered files.

**Step 2-5: Follow Task 5 workflow for each file**

**Step 6: Commit**

```bash
git add core/src/
git commit -m "core: final dead code cleanup across remaining modules"
```

---

## Task 11: Clippy Cleanup + Final Verification

Clean up remaining Clippy warnings and verify all success criteria.

**Files:**
- Modify: any files with remaining Clippy warnings

**Step 1: Run Clippy**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore 2>&1 | grep "^warning"
```

**Step 2: Fix remaining warnings**

Common fixes:
- `unnecessary mut` → remove `mut`
- `unused import` → remove import line
- `useless conversion` → remove `.into()`
- `needless return` → remove `return` keyword

**Step 3: Final verification**

```bash
# All must pass:
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | grep "^error"
# Expected: empty (0 errors)

cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore 2>&1 | grep "^warning" | wc -l
# Expected: 0 or very few

cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -3
# Expected: all tests pass

cd /Volumes/TBU4/Workspace/Aleph && grep -r '#\[allow(dead_code)\]' core/src/ | wc -l
# Expected: < 20
```

**Step 4: Commit**

```bash
git add core/src/
git commit -m "core: final clippy cleanup, all warnings resolved"
```

---

## Summary

| Task | Description | Risk | Est. Files |
|------|-------------|------|------------|
| 1 | Fix swarm_events type conflict | Low | 4 |
| 2 | Add from_builder constructor | Low | 1 |
| 3 | Fix build.rs imports | Low | 1 |
| 4 | Verify clean baseline | None | 0 |
| 5 | Dead code: extension/ | Medium | ~8 |
| 6 | Dead code: providers/ + generation/ | Medium | ~12 |
| 7 | Dead code: gateway/ | Medium | ~10 |
| 8 | Dead code: memory/ + dispatcher/ | Medium | ~12 |
| 9 | Dead code: agent_loop/ + thinker/ + poe/ | Medium | ~8 |
| 10 | Dead code: remaining modules | Medium | ~37 |
| 11 | Clippy + final verification | Low | ~5 |

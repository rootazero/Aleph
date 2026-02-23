# Rust Codebase Refactoring: Compilation Fix + Dead Code Elimination

**Date**: 2026-02-23
**Scope**: alephcore (core/src/) only
**Strategy**: Bottom-Up (fix compilation → compiler-guided dead code removal → Clippy cleanup)

---

## Diagnostic Summary

| Metric | Value |
|--------|-------|
| Total files | 1,249 .rs files |
| Total LOC | 386,190 |
| Compilation errors | 6 (swarm_events conflict + from_builder missing) |
| `#[allow(dead_code)]` | 132 across 87 files |
| `.clone()` calls | 2,800+ |
| `.unwrap()` calls | 2,000+ |
| Clippy warnings | 2 (alephcore only) |
| `Box<dyn>` usage | 149 (architecture-appropriate) |

---

## Phase 1: Fix Compilation Errors

### 1.1 swarm_events Type Conflict (E0252 + E0308)

**Problem**: `agent_loop/mod.rs` re-exports both `events::AgentLoopEvent` and `swarm_events::AgentLoopEvent`, causing name collision.

**Fix**:
1. Merge `swarm_events.rs` variants (`DecisionMade`, `ActionInitiated`, `ActionCompleted`) into `events.rs` enum
2. Move `InsightSeverity` from `swarm_events.rs` to `events.rs`
3. Delete `swarm_events.rs`, remove `mod swarm_events` from `mod.rs`
4. Update `agent_loop.rs` to use unified event types

**Verification**: `cargo check -p alephcore` — 0 E0252/E0308 errors

### 1.2 `AgentLoop::from_builder` Missing (E0599)

**Problem**: `builder.rs:113` calls `AgentLoop::from_builder()` which doesn't exist.

**Fix**:
1. Inspect builder's `build()` method intent
2. Either add `AgentLoop::from_builder()` or redirect to existing constructor (`new()` / `with_event_bus()`)

**Verification**: `cargo check -p alephcore` — 0 E0599 errors

### 1.3 build.rs Import Errors (--all-features)

**Problem**: Missing `std::path::Path` and `std::process::Command` imports.

**Fix**: Add necessary imports and type annotations.

**Verification**: `cargo check -p alephcore --all-features` — 0 errors

---

## Phase 2: Dead Code Elimination

### 2.1 Batch Schedule

| Batch | Directory | Count | Typical Content |
|-------|-----------|-------|-----------------|
| 1 | `extension/` | ~15 | trait methods, optional plugin fields |
| 2 | `providers/` + `generation/` | ~20 | provider config, TTS/image generation |
| 3 | `gateway/` | ~15 | channel-specific fields, adapters |
| 4 | `memory/` + `dispatcher/` | ~20 | storage fields, scheduling config |
| 5 | `agent_loop/` + `thinker/` + `poe/` | ~15 | telemetry, worker fields |
| 6 | Remaining modules | ~47 | miscellaneous |

### 2.2 Per-Batch Workflow

```
1. Remove #[allow(dead_code)]
2. cargo check — collect compiler warnings
3. Classify each "unused" warning:
   a. Confirmed dead → DELETE
   b. Feature-gated → #[cfg_attr(not(feature="X"), allow(dead_code))]
   c. Architectural reserve (has TODO/design doc) → KEEP + add comment
   d. Used via trait indirection → KEEP (compiler false positive)
4. cargo check — confirm no new errors
5. cargo test — confirm no regressions
```

### 2.3 Classification Criteria

| Classification | Condition | Action |
|---------------|-----------|--------|
| **Confirmed dead** | No callers, not in pub trait, no feature gate | Delete |
| **Feature-gated** | Used only under specific feature | Add conditional compilation |
| **Trait obligation** | Required by trait impl, not directly called | Keep |
| **Architectural reserve** | Has corresponding design doc or TODO | Keep + comment |

---

## Phase 3: Clippy Cleanup

After dead code elimination:
1. Fix remaining Clippy warnings in alephcore (unused imports, unnecessary mut)
2. Most warnings will be auto-resolved by Phase 1-2 changes

---

## Guardrails

### Immutable Boundaries

- **Public API unchanged**: All `pub fn`, `pub struct`, `pub enum` signatures preserved
- **Send/Sync unchanged**: No new `!Send` or `!Sync` constraints
- **Error types unchanged**: No Error enum variants removed
- **Feature flags unchanged**: Existing feature gate semantics preserved
- **Zero runtime cost**: No additional `clone()`, `Box<T>`, or dynamic dispatch

### Verification Checkpoints

| Checkpoint | Command | Pass Criteria |
|-----------|---------|---------------|
| Compilation | `cargo check -p alephcore` | 0 errors |
| Clippy | `cargo clippy -p alephcore` | 0 errors, ≤2 warnings |
| Tests | `cargo test -p alephcore` | All existing tests pass |
| All features | `cargo check -p alephcore --all-features` | 0 errors |

---

## Success Criteria

1. `cargo check -p alephcore` — 0 errors, 0 warnings
2. `cargo clippy -p alephcore` — 0 errors, 0 new warnings
3. `cargo test -p alephcore` — all tests pass
4. `#[allow(dead_code)]` count: 132 → <20 (only justified retentions)
5. Net code reduction: 1,000-3,000 lines of dead code removed

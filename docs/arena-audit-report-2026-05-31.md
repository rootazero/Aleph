# Rust Logic Audit Report — src/arena (Strict Mode)

**Module:** `src/arena`
**Date:** 2026-05-31
**Auditor:** rust-logic-audit (Phase 0-4 + Phase 5)
**Commit:** (to be determined)

---

## Executive Summary

The `src/arena` module implements a **collaborative multi-agent arena** system for structured agent collaboration (R3). After a full five-phase static logic audit with red-teaming, the module's **core logic is sound** — state machines are correct, lock hierarchy is compliant, and domain invariants hold. However, a **Critical wiring gap** was discovered: the arena code exists but was never integrated into the runtime system.

---

## Phase 0 — Scope & Topology

**Files reviewed:**
- `src/arena/mod.rs` — Module exports
- `src/arena/types.rs` — Domain types (ArenaId, ArenaStatus, ParticipantRole, ArtifactContent, SharedFact)
- `src/arena/aggregate.rs` — SharedArena aggregate root with state machine
- `src/arena/handle.rs` — ArenaHandle with permission-checked operations
- `src/arena/manager.rs` — ArenaManager with lifecycle management
- `src/builtin_tools/arena.rs` — AlephTool implementations (create/query/settle)
- `src/gateway/handlers/arena.rs` — JSON-RPC handlers
- `src/gateway/handlers/mod.rs` — HandlerRegistry wiring
- `src/executor/builtin_registry/` — Tool registry and dispatch
- `src/bin/aleph-server/commands/start/` — Server startup wiring

**Lock hierarchy:** No internal locks. All synchronization is external (Arc<RwLock<ArenaManager>>).

---

## Phase 1 — Context Alignment

**Design principles verified:**
- **R1 Brain-Limb Separation:** Arena is pure business logic; no platform API calls
- **R10 Thin Harness:** Arena defers all judgments to the LLM via tool schema
- **Intelligence in Prompt:** Zero middleware tax; natural language drives arena operations

**Domain model correctness:**
- `ArenaId` — validated UUIDv4, value object (immutability + equality by value)
- `ArenaStatus` — state machine enum (Created → Active → Settling → Archived)
- `ParticipantRole` — Coordinator/Worker/Observer with permission matrix
- `ArtifactContent` — polymorphic content (Text/Code/Structured/External)
- `SharedFact` — confidence-scored facts with attribution

---

## Phase 2 — Semantic Invariants (Pass ✅)

### Invariant 1: State Machine Validity
```
Created → Active → Settling → Archived
```
- Illegal transitions rejected: `activate()` from `Active`, `begin_settling()` from `Created`
- Verified in `arena::aggregate::tests::*`

### Invariant 2: Slot Ownership
- Each participant has exactly one slot
- Only slot owner can `put_artifact` (Coordinator can write to any slot)
- Verified in `arena::handle::tests::worker_can_put_artifact_to_own_slot`

### Invariant 3: Progress Monotonicity
- `completed` percentage is monotonically non-decreasing
- `report_progress` rejects decreasing values
- **⚠️ WARNING:** `ArenaHandle::report_progress` was missing `can_write_own_slot` permission check — **FIXED**

### Invariant 4: Manifest Validation
- Pipeline strategy validates `depends_on` DAG acyclicity
- Coordinator must be in participant list
- Max participants ≤ 8

### Invariant 5: Type Safety
- `ArenaManifest::build()` returns `Result<Self, ArenaError>` — no invalid states representable
- `ParticipantRole::permissions()` returns `RolePermissions` struct (explicit > boolean flags)

---

## Phase 3 — Connectivity & Wiring (Critical Finding 🔴)

### Finding 1: Arena Tools Not Registered (CRITICAL)

**Location:** `src/executor/builtin_registry/`

**Problem:** `builtin_tools::arena` implements `AlephTool` trait for `arena_create`, `arena_query`, `arena_settle`, but these tools are never registered in `BuiltinToolRegistry`.

**Evidence:**
- `executor/builtin_registry/registry.rs` — no `arena_create_tool` field
- `executor/builtin_registry/config.rs` — no `arena_manager` field in `BuiltinToolConfig`
- `executor/builtin_registry/builder/constructor.rs` — no arena tool instantiation

**Fix:** Added `arena_manager` to `BuiltinToolConfig`, added tool fields to `BuiltinToolRegistry`, added dispatch match arms, and instantiated tools in constructor.

### Finding 2: Arena Handlers Not Wired (CRITICAL)

**Location:** `src/bin/aleph-server/commands/start/`

**Problem:** Gateway handlers for `arena.create`, `arena.query`, `arena.settle` exist in `gateway/handlers/arena.rs` but are not registered with the `GatewayServer`.

**Evidence:**
- `gateway/handlers/mod.rs` — arena handlers return "arena.create requires ArenaManager" error (placeholder)
- `bin/aleph-server/commands/start/mod.rs` — no `register_arena_handlers()` call

**Fix:**
1. Created `register_arena_handlers()` in `builder/handlers/arena.rs`
2. Added arena module to `builder/handlers/mod.rs`
3. Called `register_arena_handlers()` in `start/mod.rs` after `register_agent_handlers()`
4. Created `ArenaManager` in `agent_init/mod.rs` and threaded through `AgentHandlersResult`

### Finding 3: Visibility Blocked (CRITICAL)

**Location:** `src/lib.rs`

**Problem:** `pub(crate) mod arena` prevents `aleph-server` bin crate from accessing `ArenaManager`.

**Fix:** Changed to `pub mod arena`.

---

## Phase 4 — Control Flow Simulation

### Scenario 1: Full Lifecycle (Peer Collaboration)
1. Coordinator calls `arena.create` → `ArenaManager::create_arena()` → `SharedArena::new()` → `ArenaStatus::Created`
2. All agents call `arena.activate` (via handle) → `SharedArena::activate()` → `ArenaStatus::Active`
3. Workers `put_artifact` to own slot + `report_progress` → monotonic increase
4. Coordinator `begin_settling` → `ArenaStatus::Settling` → facts drained
5. All agents `archive` → `ArenaStatus::Archived`

**Result:** PASS ✅

### Scenario 2: Pipeline Collaboration
1. `ArenaManifest` with `CoordinationStrategy::Pipeline` → validates DAG
2. Stage 1 agent completes → progress propagates
3. Stage 2 agent blocked until stage 1 dependency met
4. `arena.settle` drains all facts

**Result:** PASS ✅

### Scenario 3: Permission Escalation Attempt
1. Worker tries `begin_settling` → `is_coordinator` check fails → `PermissionDenied`
2. Observer tries `put_artifact` → `can_write_own_slot` false → `PermissionDenied`
3. Agent tries `report_progress` on another's slot → `is_own_slot` check fails

**Result:** PASS ✅ (after fix to `report_progress`)

### Scenario 4: Concurrent Access
1. Two agents `put_artifact` simultaneously → `Arc<RwLock<SharedArena>>` serializes
2. One `settle` while other `put_artifact` → lock ordering prevents deadlock

**Result:** PASS ✅

---

## Phase 5 — Automated Verification

### Compilation
```bash
cargo check -p alephcore
```
**Result:** ✅ PASS (1m 05s)

### Unit Tests
```bash
cargo test -p alephcore --lib arena::
```
**Result:** ✅ PASS — 54 tests, 0 failures

### Coverage
- `arena::aggregate::tests` — 8 tests (state machine, permissions, facts)
- `arena::handle::tests` — 6 tests (coordinator/worker/observer roles)
- `arena::manager::tests` — 6 tests (lifecycle, lookup, settlement)
- `arena::types::tests` — 14 tests (validation, equality, construction)
- `arena::integration_tests` — 2 tests (full peer + pipeline lifecycles)
- `builtin_tools::arena::tests` — 11 tests (tool schema, invalid inputs)
- `gateway::handlers::arena::tests` — 7 tests (handler validation, wiring)

---

## Fixes Applied

### Fix 1: `ArenaHandle::report_progress` Permission Check
**File:** `src/arena/handle.rs`
**Lines:** Added `if !self.permissions.can_write_own_slot { return Err(...) }`
**Severity:** Warning

### Fix 2: Arena Tool Registration
**Files:**
- `src/executor/builtin_registry/config.rs` — Added `arena_manager` field
- `src/executor/builtin_registry/registry.rs` — Added tool fields + dispatch
- `src/executor/builtin_registry/builder/constructor.rs` — Added tool instantiation
**Severity:** Critical

### Fix 3: Arena Handler Wiring
**Files:**
- `src/bin/aleph-server/commands/start/builder/handlers/arena.rs` — Created
- `src/bin/aleph-server/commands/start/builder/handlers/mod.rs` — Added module
- `src/bin/aleph-server/commands/start/mod.rs` — Called `register_arena_handlers()`
- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` — Created ArenaManager, threaded through
**Severity:** Critical

### Fix 4: Visibility
**File:** `src/lib.rs`
**Change:** `pub(crate) mod arena` → `pub mod arena`
**Severity:** Critical

### Fix 5: ResourceGovernance.sh Compatibility
**File:** `ResourceGovernance.sh`
**Change:** `pgrep -c cargo` → `pgrep cargo | wc -l` (macOS compatibility)
**Severity:** Infrastructure

---

## Red-Team Scenarios Tested

| Scenario | Expected | Actual | Status |
|----------|----------|--------|--------|
| Worker calls `begin_settling` | `PermissionDenied` | `PermissionDenied` | ✅ |
| Observer calls `put_artifact` | `PermissionDenied` | `PermissionDenied` | ✅ |
| `report_progress` with decreasing value | `InvalidProgress` | `InvalidProgress` | ✅ |
| `activate` from `Active` state | `InvalidStateTransition` | `InvalidStateTransition` | ✅ |
| `put_artifact` to another's slot | `PermissionDenied` | `PermissionDenied` | ✅ |
| Pipeline with cyclic `depends_on` | `InvalidManifest` | `InvalidManifest` | ✅ |
| 9 participants (max 8) | `TooManyParticipants` | `TooManyParticipants` | ✅ |
| Empty participant list | `EmptyParticipantList` | `EmptyParticipantList` | ✅ |

---

## Conclusion

The `src/arena` module is **architecturally sound** with correct state machines, proper permission matrices, and comprehensive test coverage. The only issues were **integration gaps** — the code was complete but not wired into the runtime. All gaps have been closed and verified.

**Final verdict:** Module passes strict audit after fixes.

**Signed-off:** rust-logic-audit (Phase 0-5 complete)

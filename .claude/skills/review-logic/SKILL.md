---
name: review-logic
description: >
  Deep logic bug review for Aleph Rust codebase. Five-phase analysis: (1) context
  alignment against reference docs, (2) semantic invariant checking (state machines,
  error propagation, unwrap/panic, locks, atomics), (3) control flow simulation,
  (4) red-teaming with adversarial inputs, (5) automated L1 proptest + L2 loom
  verification. Use when: reviewing code for logic bugs, state machine errors,
  error propagation issues, concurrency defects, or before merging significant
  changes. Also use after implementing new concurrency patterns, modifying DAG
  scheduling, or touching memory/gateway/agent_loop modules.
  Trigger: /review-logic [module|commit] [--strict]
---

# Logic Review — Three-Layer Defense System

This skill is the L3 (AI Semantic Audit) layer of a three-layer defense system. The five phases below are the L3 methodology:

| Layer | Method | Coverage | Command |
|-------|--------|----------|---------|
| **L1** | proptest (property-based) | 77 tests across 9 files — invariant violations, boundary errors | `just test-proptest` |
| **L2** | loom (concurrency) | 21 tests across 5 modules — deadlocks, data races, atomics bugs | `just test-loom` |
| **L3** | AI semantic audit (this skill) | Logic bugs, state errors, design flaws | `/review-logic` |

Run all layers together: `just test-logic`.

Follow the five phases below strictly and in order.

## Arguments

Parse the user's input:
- No args → review uncommitted changes (`git diff` + `git diff --cached`)
- Module name (e.g., `dispatcher`, `gateway`) → review that module's recent changes
- Commit hash → review that specific commit (`git show <hash>`)
- `--strict` flag → lower the Warning threshold (report more potential issues)

## Phase 1: Context Alignment

Before reading any code, establish what "correct" means:

1. **Identify the target module** from the diff/commit
2. **Read the reference doc** using this mapping:

| Module | Reference Doc |
|--------|--------------|
| agent_loop | docs/reference/AGENT_SYSTEM.md |
| dispatcher | docs/reference/AGENT_SYSTEM.md |
| gateway | docs/reference/GATEWAY.md |
| memory | docs/reference/MEMORY_SYSTEM.md |
| poe | docs/plans/2026-02-01-poe-architecture-design.md |
| tools | docs/reference/TOOL_SYSTEM.md |
| exec | docs/reference/SECURITY.md |
| extension | docs/reference/EXTENSION_SYSTEM.md |
| domain | docs/reference/DOMAIN_MODELING.md |

3. **Extract business invariants** from the doc (state machine rules, data constraints, ordering guarantees)
4. **Read the change intent** from commit message or PR description

## Phase 2: Semantic Invariant Checking

For each changed function, check:

### State Machine Legality
- Find all `enum` types used in the changed code
- Enumerate all variant transitions in `match` arms
- Flag any transition that skips intermediate states
- Flag any `_ => {}` that silently swallows new variants

### Error Propagation
- Trace every `?` operator — does the caller handle the error type correctly?
- Check `map_err` chains — is context preserved or silently lost?
- On error paths: is cleanup/rollback performed? Are resources released?

### unwrap/panic Audit
- Flag every `.unwrap()`, `.expect()`, `panic!()`, `unreachable!()`
- For each: is it in a test-only path, or could it trigger in production?
- Rate: SAFE (proven by type system), RISKY (depends on runtime state), CRITICAL (user-facing path)

### Lock & Concurrency Analysis

**Imports check:**
- Verify sync primitives import from `crate::sync_primitives`, NOT directly from `std::sync`
- Exceptions: `OnceLock`, `LazyLock`, `Once`, `Weak`, `AtomicU8`, `std::sync::mpsc` (not in sync_primitives)
- Exception: static `AtomicU64`/`AtomicU32` (loom's `new()` is not `const fn`, must use `std::sync::atomic`)
- Exception: types that interface with external crate APIs (e.g., `rhai::Locked` requires `std::sync::RwLock`)

**Lock hierarchy** (defined in `core/src/sync_primitives.rs`):
- Level 0: StateDatabase (resilience/database)
- Level 1: MemoryStore (memory/)
- Level 2: ToolRegistry, ChannelRegistry (dispatcher/, gateway/)
- Level 3: UI state, progress monitors
- Flag any code that acquires locks out of order

**Lock scope:**
- Find all `Mutex::lock()`, `RwLock::read()`, `RwLock::write()`
- Check: is the guard held across an `.await` point? (deadlock risk with std::sync::Mutex)
- Check: are multiple locks acquired? If so, is the order consistent across all call sites?

**Atomic operations:**
- Check ordering (`Relaxed` vs `SeqCst` vs `Acquire`/`Release`) — is it strong enough?
- Check TOCTOU patterns: load → check → store with separate critical sections is a race
- Flag unprotected read-modify-write sequences (should use `fetch_add`, `compare_exchange`, etc.)

**Loom test coverage** (21 tests across 5 modules):

| Module | Test file | # | Tests |
|--------|-----------|---|-------|
| dispatcher | `dispatcher/loom_concurrency.rs` | 4 | registry R/W, pause/resume flags, atomic counter, progress snapshot |
| gateway | `gateway/loom_concurrency.rs` | 5 | seq counter, connection state, request ID, chunk reset, run limit TOCTOU |
| agent_loop | `agent_loop/loom_concurrency.rs` | 3 | anchor store, state flags, Arc ref counting |
| memory | `memory/loom_concurrency.rs` | 5 | singleton init, compression trigger, timestamps, metrics, provider swap |
| resilience | `resilience/loom_concurrency.rs` | 4 | lane counter, token budget, per-task seq, mutex contention |

If the reviewed code touches concurrency patterns in these modules, check if existing loom tests cover the change. If not, suggest adding a loom test.

### Type Coercion
- Find all `as` casts — can they truncate or overflow?
- Prefer `try_into()` or `From` implementations

## Phase 3: Control Flow Simulation

For each changed function, mentally execute it:

### Branch Coverage
- List every `if/else`, `match`, and `?` branch
- For each: what happens on the "other" path?
- Flag: `if condition { do_something() }` with no `else` — is the implicit else correct?

### Loop Boundaries
- What happens when the collection is empty?
- What happens when the collection has millions of items?
- Is there a guaranteed termination condition?

### Option/Result Chains
- Trace `.map().and_then().unwrap_or()` chains
- Where does `None` propagate to? Is the final default value correct?

### Async Ordering
- For `tokio::spawn` or `join_all`: does the order of completion matter?
- For channels: can messages arrive out of order? Is that handled?

## Phase 4: Red-teaming

Switch to attacker mindset:

1. **Malicious inputs**: What if the input is:
   - Empty string / zero-length slice
   - Extremely long string (> 1MB)
   - Invalid UTF-8 (if accepting bytes)
   - NaN or Infinity (for floats)
   - Negative numbers where positive expected
   - Null/None where Some expected

2. **Abnormal timing**:
   - Network timeout mid-operation
   - Two identical requests arrive simultaneously
   - Component crashes between step 1 and step 2 of a multi-step operation

3. **Generate test suggestions**: Write 3-5 concrete test cases (as Rust code snippets) that target the most likely failure modes discovered above.

## Phase 5: Automated Verification (L1 + L2)

If the reviewed code touches any of these, run automated tests:

| Code changed | Run command | What it verifies |
|-------------|-------------|------------------|
| Concurrency (Mutex, RwLock, atomics, Arc) | `just test-loom` | 21 loom tests across 5 modules |
| Business logic, data structures, parsing | `just test-proptest` | 77 proptest tests across 9 files |
| Both or unclear | `just test-logic` | All proptest + loom tests |

**Proptest coverage** (77 tests across 9 files):

| Module | Test files |
|--------|-----------|
| agent_loop | `proptest_decision.rs` (10), `proptest_state.rs` (10) |
| dispatcher | `agent_types/proptest_graph.rs` (8), `agent_types/proptest_task.rs` (7) |
| gateway | `proptest_channel.rs` (7), `proptest_protocol.rs` (7) |
| memory | `proptest_enums.rs` (7) |
| poe | `proptest_types.rs` (11), `proptest_budget.rs` (10) |

Report the test results in the findings section. If a loom or proptest test fails, escalate to **Critical**.

If the change introduces a NEW concurrency pattern not covered by existing loom tests, add a **[Suggested Test]** with a loom test template:

```rust
#[test]
fn loom_new_pattern_name() {
    loom::model(|| {
        // Setup shared state using loom::sync types
        let shared = Arc::new(Mutex::new(initial_state));

        // Spawn threads to exercise the pattern
        let t1 = loom::thread::spawn(move || { /* ... */ });

        // Join and verify invariants
        t1.join().unwrap();
        // assert!(...)
    });
}
```

New loom tests go in `core/src/<module>/loom_concurrency.rs`, gated with `#[cfg(all(test, feature = "loom"))]` in the module's `mod.rs`.

## Report Format

Output the report in this exact format:

```markdown
# Logic Review Report
**Module**: <module name>
**Scope**: <what was reviewed>
**Date**: <today>
**Mode**: <normal|strict>

## Findings

### [Critical] <title>
- **Location**: `file:line`
- **Trigger condition**: <how to trigger>
- **Expected behavior**: <what should happen>
- **Actual behavior**: <what actually happens>
- **Suggested fix**: <how to fix>

### [Warning] <title>
- **Location**: `file:line`
- **Risk**: <what could go wrong>
- **Current impact**: <low/medium/high>
- **Suggestion**: <what to do>

### [Suggested Test] <title>
\```rust
#[test]
fn test_name() {
    // test code
}
\```

## Summary
| Level | Count |
|-------|-------|
| Critical | N |
| Warning | N |
| Suggested Test | N |
```

## Quality Bar

- **Critical**: Must be filed. The code WILL produce wrong results under described conditions.
- **Warning**: Should be addressed. The code MIGHT produce wrong results under edge conditions, or is fragile to future changes.
- **Suggested Test**: Nice to have. A test that would increase confidence in the logic.

In `--strict` mode: lower the bar for Warning (include more "might be a problem" items).

## Known Bug Patterns

Patterns confirmed by full-codebase review — always check for these:

- **UTF-8 byte slicing**: `&s[..n]` panics on multi-byte chars — use `s.get(..n)`, `char_indices()`, `strip_suffix()`
- **Lock poisoning**: `lock().unwrap()` cascades panics — use `.unwrap_or_else(|e| e.into_inner())`
- **SQL filter injection**: LanceDB DataFusion filters built via `format!()` — must escape with `escape_sql_string()`
- **`expect()`/`unwrap()` on user-facing paths**: home_dir, timestamps, HTTP clients — use fallbacks
- **`static mut`**: unsound in Rust — use `OnceLock` or `Lazy`
- **HashMap iteration order**: non-deterministic for security rules — sort explicitly

## Aleph-Specific Invariants

Project-specific rules to always check:

1. **R1 Brain-Limb Separation**: No platform-specific APIs (AppKit, Vision, windows-rs) in `core/src/`
2. **DAG Acyclicity**: Any code touching TaskGraph must preserve the no-cycle invariant
3. **POE Budget Monotonicity**: `current_attempt` and `tokens_used` must only increase
4. **Session Key Determinism**: Same input must always route to the same session
5. **Memory Namespace Isolation**: Facts in namespace A must never leak to namespace B queries
6. **Approval Flow Integrity**: exec approval cannot be bypassed by crafted tool names
7. **Gateway Auth State Machine**: Connection must authenticate before any RPC call succeeds
8. **Sync Primitives Import Rule**: Use `crate::sync_primitives` for `Arc`, `Mutex`, `RwLock`, and atomics — never `std::sync` directly (except documented exceptions: static atomics, OnceLock, external API interop)
9. **Lock Hierarchy Compliance**: Acquire locks in Level 0→1→2→3 order; reverse acquisition is a deadlock risk
10. **TOCTOU Prevention**: Check-then-act on shared state must happen within the same lock scope (see gateway/execution_engine TOCTOU fix as reference)

---
name: rust-logic-audit
description: >
  Deep logic bug review for Rust codebases. Portable engine for logic chain
  analysis including: (1) Topology Reconnaissance, (2) Context Alignment,
  (3) Semantic Invariant checking, (4) Connectivity & Wiring audit,
  (5) Control flow simulation, (6) Red-teaming, (7) Five-layer automated verification (L1-L5).
  Trigger: /rust-logic-audit [module|commit] [--strict]
---

# Rust Logic Audit — Five-Layer Defense System

This skill is a portable, deep-audit engine for Rust logic chains. It provides both semantic AI analysis (L3) and integrates with automated verification layers (L1, L2, L4, L5).

## Portability & Integration

This skill is designed to be **portable**. It carries its own execution engine in `./Scripts/logic-audit.sh`.
- **Integrated Mode**: If the project has a `justfile` with matching tasks (e.g., `test-kani`), it will use those.
- **Standalone Mode**: If running in a raw Rust project, it falls back to standard `cargo` commands.

### Commands
- **AI Audit**: `/rust-logic-audit`
- **Automated Pipeline**: `just audit-logic` or `./Scripts/logic-audit.sh`


| Layer | Method | Coverage | Command |
|-------|--------|----------|---------|
| **L1** | proptest (property-based) | 77 tests across 9 files — invariant violations, boundary errors | `just test-proptest` |
| **L2** | loom (concurrency) | 21 tests across 5 modules — deadlocks, data races, atomics bugs | `just test-loom` |
| **L3** | AI semantic audit (this skill) | Logic bugs, state errors, design flaws, wiring gaps | `/rust-logic-audit` |

Run all layers together: `just test-logic`.

Follow the five phases below strictly and in order.

## Arguments

Parse the user's input:
- No args → review uncommitted changes (`git diff` + `git diff --cached`)
- Module name (e.g., `dispatcher`, `gateway`) → review that module's recent changes
- Commit hash → review that specific commit (`git show <hash>`)
- `--all` flag → full repo scan (all modules under `core/src/`)
- `--strict` flag → lower the Warning threshold (report more potential issues)

## Scope Management

Before starting the five phases, assess scope and choose a batch strategy.

**Estimate scope:**

| Input type | Measure with |
|-----------|-------------|
| Uncommitted changes | `git diff --stat` |
| Commit | `git show --stat <hash>` |
| Module | `find core/src/<module> -name '*.rs' \| wc -l` |
| `--all` | All 63 modules under `core/src/` (~1200 .rs files) |

**Choose strategy:**

| Scope | Strategy |
|-------|----------|
| ≤ 10 changed files | **Single pass** — run five phases directly |
| 11-30 changed files | **Module batching** — group by module, run phases per batch |
| > 30 files or `--all` | **Subagent dispatch** — parallel subagents per module |

### Module batching (medium scope)

1. Group changed files by module using the Phase 1 module→doc mapping
2. Run all five phases for each module batch sequentially
3. Track cross-module concerns (see below) across batches
4. After all batches: merge findings into one consolidated report

### Subagent dispatch (large scope)

1. **Tier 1 — high-risk modules** (review first):
   agent_loop, dispatcher, gateway, memory, poe, exec, resilience, extension, thinker
2. **Tier 2**: all remaining `core/src/` directories
3. Dispatch one subagent per module. For tiny modules (≤ 5 files), group 3-4 into one subagent
4. Each subagent prompt: module name, reference doc path (from Phase 1 table), full five-phase instructions, flags
5. Collect all subagent reports → produce consolidated summary

### Cross-module tracking

These issues only surface when comparing findings across batches — actively track them:

- **Lock hierarchy violations** spanning modules (e.g., gateway holds Level 2 while calling memory at Level 1)
- **Error context loss** at module boundaries (`map_err` strips inner context)
- **Shared state inconsistency** — same data accessed from different modules with different lock patterns
- **API contract drift** — caller assumes invariant the callee doesn't guarantee
- **Wiring gaps across module boundaries** — module A exposes new `pub fn` but no module B calls it; trait implemented but never dyn-dispatched; event defined but never subscribed
- **Registration inconsistency** — feature code exists in module A but registration (tool map, handler dispatch, provider factory) is missing in module B

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
- Level 2: ToolRegistry, ChannelRegistry (tool_metadata/, gateway/)
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
| tool_metadata | `tool_metadata/loom_concurrency.rs` | 4 | registry R/W, pause/resume flags, atomic counter, progress snapshot |
| gateway | `gateway/loom_concurrency.rs` | 5 | seq counter, connection state, request ID, chunk reset, run limit TOCTOU |
| agent_loop | `agent_loop/loom_concurrency.rs` | 3 | anchor store, state flags, Arc ref counting |
| memory | `memory/loom_concurrency.rs` | 5 | singleton init, compression trigger, timestamps, metrics, provider swap |
| resilience | `resilience/loom_concurrency.rs` | 4 | lane counter, token budget, per-task seq, mutex contention |

If the reviewed code touches concurrency patterns in these modules, check if existing loom tests cover the change. If not, suggest adding a loom test.

### Connectivity & Wiring Analysis

**Critical insight**: A perfectly correct function that is never called is still a bug — it means the feature doesn't exist at runtime. Check for "dead logic at the seams."

#### Entry Point Audit
- **Main / binary entry points**: Is the new module/function reachable from `main()`, `lib.rs`, or the appropriate binary `main.rs`?
- **Plugin/extension registration**: If adding a plugin, tool, or extension — is it registered in the plugin manifest, tool registry, or extension loader? (Common gap: code exists but not in `register_plugins()` or equivalent)
- **Gateway handlers**: New JSON-RPC or WebSocket handlers must be added to the router dispatch table. Check `handlers/mod.rs` or router setup code.
- **Event subscribers**: New event handlers must subscribe to the event bus. Is `event_bus.subscribe(handler)` called?
- **Background task spawning**: New daemons or loops must be spawned in the system bootstrap (e.g., `tokio::spawn(daemon_task)`).

#### Visibility Traps
- **`pub` functions with no callers**: A `pub fn` in a module that is never called from outside — compiler won't warn because it's `pub`. Check if it should be `pub(crate)` or if callers are missing.
- **`pub(crate)` in leaf modules**: If a module is only used internally, `pub(crate)` functions that are uncalled indicate dead code.
- **Trait implementations**: A type implements a trait, but is the trait used anywhere? E.g., `impl MyTrait for Foo {}` but no `dyn MyTrait` or `<T: MyTrait>` bounds reference `Foo`.
- **Unused associated functions**: `impl Foo { pub fn new_feature() {} }` — is `new_feature()` ever invoked?

#### Registration Patterns (Aleph-Specific)

Check these registries when reviewing related code:

| Component | Registry Location | What to verify |
|-----------|-------------------|----------------|
| Built-in tools | `builtin_tools/mod.rs` or `tool_server` registration | Tool is added to the tool map |
| Gateway handlers | `gateway/handlers/mod.rs` | Handler enum variant + dispatch match arm |
| Channel interfaces | `gateway/interfaces/mod.rs` | Channel type registered in factory/loader |
| MCP tools | MCP client initialization | Tool schema registered with MCP server |
| Memory providers | Memory system initialization | Provider added to provider enum/factory |
| LLM providers | `providers/mod.rs` | Provider registered in provider resolution |
| Sandbox drivers | `sandbox/mod.rs` | Driver registered in driver selection logic |
| Compression strategies | `compressor/mod.rs` | Strategy added to strategy selector |
| Event handlers | Event bus initialization | Handler subscribed to relevant event topic |

#### Cross-Module Wiring
- **Service composition**: If module A depends on module B's new feature, does A's factory/constructor pass the new dependency?
- **Configuration wiring**: New config fields must be: (1) defined in config struct, (2) parsed from file/env, (3) passed to the component that uses it.
- **Feature flags**: Code behind `#[cfg(feature = "...")]` — is the feature enabled in any build configuration? Is the feature gate tested in CI?
- **WASM exports**: New WASM-facing functions need `#[wasm_bindgen]` and must be exported from the WASM module boundary.

#### Diagnostic Commands
Use these to detect wiring gaps:

```bash
# Find pub functions with no callers (requires cargo-udeps or manual audit)
# Check for unused dependencies that might indicate orphaned features
cargo udeps

# Build with aggressive dead code warnings
RUSTFLAGS="-W dead_code" cargo check -p alephcore

# Check if feature-gated code compiles
cargo check -p alephcore --features "feature-name"
```

**Red flags**:
- A commit adding a module but no changes to any registry, router, or main loop
- `pub fn` added with no corresponding caller in the same PR
- Trait impl added but no usage in `dyn` or generic contexts
- New enum variant in a registry type but no match arm handling it
- Event type defined but no subscriber
- Config struct field added but never read

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

For batched reviews, append cross-module section and batch summary:

```markdown
## Cross-Module Findings

### [Critical] <title spanning modules>
- **Modules**: `module_a/file.rs` → `module_b/file.rs`
- **Risk**: <description>
- **Suggested fix**: <how to fix>

## Batch Summary
| Module | Critical | Warning | Suggested Test |
|--------|----------|---------|----------------|
| gateway | 2 | 5 | 3 |
| dispatcher | 0 | 3 | 1 |
| ... | ... | ... | ... |
| **Cross-module** | **1** | **0** | **0** |
| **Total** | **N** | **N** | **N** |
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
- **Orphaned logic**: `pub fn` or trait impl added but no caller / no `dyn` usage — feature exists in source but not at runtime. Always verify the "last mile" wiring to registries, routers, event buses, or main loops
- **Partial registration**: New enum variant or handler added but missing from the dispatch `match` or factory function — compiles but silently unreachable

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
11. **Wiring Completeness**: Every new `pub fn`, trait impl, handler, tool, provider, or event type must have a verified caller / registration / subscriber. Unwired code is dead code — even if it compiles

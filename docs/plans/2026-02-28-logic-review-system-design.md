# Logic Review System Design

> Date: 2026-02-28
> Status: Approved

## Overview

A three-layer defense-in-depth system for detecting logic bugs in the Aleph codebase. Each layer operates independently, targeting different classes of defects.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                  Logic Review System                      │
│                                                          │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────────┐ │
│  │  L1: 属性墙  │  │ L2: 并发网  │  │ L3: AI 语义审计  │ │
│  │  proptest    │  │   loom      │  │ Claude Code Skill│ │
│  │             │  │             │  │                  │ │
│  │ cargo test  │  │ cargo test  │  │ /review-logic    │ │
│  │ (CI 自动)   │  │ --features  │  │ (手动触发)       │ │
│  │             │  │  loom (CI)  │  │                  │ │
│  └─────────────┘  └─────────────┘  └──────────────────┘ │
│                                                          │
│  L1/L2 = Mathematical proof (automatic, deterministic)   │
│  L3    = Semantic understanding (deep, flexible)         │
└──────────────────────────────────────────────────────────┘
```

## L1: Property Testing Wall (proptest)

### Integration

```toml
# core/Cargo.toml
[dev-dependencies]
proptest = "1.4"
```

No feature flag needed. proptest tests run as regular `#[test]` with `cargo test`.

### Property Categories

| Category | Description | Example |
|----------|-------------|---------|
| Roundtrip | serialize → deserialize = original | Gateway message codec |
| Invariant | property holds before and after operation | TaskGraph DAG acyclicity |
| Idempotent | f(x) = f(f(x)) | Config hot-reload, memory dedup |
| Monotonic | value only increases (or only decreases) | Event sequence numbers, POE budget |
| Commutative | order of operations doesn't affect result | Concurrent write consistency |

### Coverage Priority

1. **dispatcher** — DAG invariants, task state transition legality
2. **gateway** — message serialization roundtrip, RPC parameter validation
3. **memory** — read/write consistency, namespace isolation
4. **poe** — budget monotonic decrease, evaluation result invariance
5. **routing** — session key routing determinism
6. All other modules as needed

### Regression Seeds

proptest saves failure seeds to `proptest-regressions/`. These files MUST be committed to git to prevent regressions.

## L2: Concurrency Safety Net (loom)

### Integration

```toml
# core/Cargo.toml
[features]
loom = ["dep:loom"]

[dev-dependencies]
loom = "0.7"
```

### Conditional Compilation

A unified sync primitives module provides zero-cost aliases:

```rust
// core/src/sync_primitives.rs
#[cfg(loom)]
pub(crate) use loom::sync::{Arc, Mutex, RwLock};
#[cfg(loom)]
pub(crate) use loom::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(loom))]
pub(crate) use std::sync::{Arc, Mutex, RwLock};
#[cfg(not(loom))]
pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
```

Modules use `use crate::sync_primitives::*;` instead of `use std::sync::*;`.

### Detection Targets

| Defect | Description | High-risk areas |
|--------|-------------|-----------------|
| Deadlock | Two locks acquired in different order | gateway connection mgmt, dispatcher task queue |
| Race Condition | Read/write order causes logic error | agent_loop state transitions, memory concurrent writes |
| Starvation | A thread can never acquire lock | resilience recovery vs normal task contention |

### Coverage Priority

1. **dispatcher** — concurrent task submit/cancel/query
2. **gateway** — connection pool acquire/release/timeout cleanup
3. **agent_loop** — multi-agent concurrent state transitions
4. **memory** — concurrent fact write consistency
5. **resilience** — recovery vs normal flow contention

### Runtime Constraints

`LOOM_MAX_PREEMPTIONS=3` to prevent combinatorial explosion. Sufficient for most deadlock patterns.

## L3: AI Semantic Audit (Claude Code Skill)

### Trigger

```bash
/review-logic                    # Audit uncommitted changes
/review-logic dispatcher         # Audit specific module
/review-logic abc1234            # Audit specific commit
/review-logic --strict           # Strict mode: lower Warning threshold
```

### Four-Phase Workflow

#### Phase 1: Context Alignment

- Read target module's `docs/reference/` documentation
- Read relevant `docs/plans/` design documents
- Analyze git diff or commit message for change intent
- Extract explicit and implicit business invariants

#### Phase 2: Semantic Invariant Checking

| Check | Action |
|-------|--------|
| State machine legality | Enumerate all enum variant transitions, flag unreachable or illegal jumps |
| Error propagation | Trace every `?` and `map_err` chain, check for missing cleanup/rollback on error paths |
| unwrap audit | Flag all `.unwrap()`, `.expect()`, `panic!()`, assess production path risk |
| Lock scope | Check `Mutex::lock()` hold range, flag lock held across `.await` |
| Type coercion | Check `as` casts for truncation or overflow |

#### Phase 3: Control Flow Simulation

- **match completeness**: `_ => {}` swallowing variants that should be handled
- **if/else symmetry**: success path does A, failure path forgets to undo A
- **Loop boundaries**: empty collection behavior, million-item performance
- **Option chains**: None propagation through `.map().and_then().unwrap_or()` chains

#### Phase 4: Red-teaming

- Construct malicious inputs: invalid UTF-8, oversized strings, zero-length slices, NaN/Infinity
- Simulate abnormal timing: network timeout mid-operation, simultaneous requests
- Generate 3-5 edge case test suggestions with pseudocode

### Report Format

```markdown
# Logic Review Report
**Module**: <module>
**Scope**: <commit or diff description>
**Date**: <date>

## Findings

### [Critical] <title>
- **Location**: `file:line`
- **Trigger condition**: ...
- **Expected behavior**: ...
- **Actual behavior**: ...
- **Suggested fix**: ...

### [Warning] <title>
- **Location**: `file:line`
- **Risk**: ...
- **Current impact**: ...
- **Suggestion**: ...

### [Suggested Test] <title>
```rust
// test code
```

## Summary
| Level | Count |
|-------|-------|
| Critical | N |
| Warning | N |
| Suggested Test | N |
```

### Aleph-Specific Check Rules

| Module | Specific checks |
|--------|----------------|
| dispatcher | DAG acyclicity invariant, task state machine completeness, concurrent submit safety |
| gateway | WebSocket message serialization roundtrip, RPC handler error code consistency, auth state machine |
| agent_loop | OTAF cycle completeness, state machine cannot skip phases |
| memory | LanceDB read/write consistency, embedding dimension match, namespace isolation |
| poe | budget monotonic decrease, SuccessManifest immutability, evaluation idempotency |
| exec | Shell command injection check, approval flow cannot be bypassed |
| resilience | SQLite transaction atomicity, recovery idempotency |

## CI Integration

### Pipeline

```yaml
# .github/workflows/rust-core.yml

# Existing test job runs proptest automatically (they're regular #[test])
# Increase coverage in CI:
test:
  env:
    PROPTEST_CASES: 1024

# New job for loom
loom-tests:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo test --features loom --lib
      env:
        RUSTFLAGS: "--cfg loom"
        LOOM_MAX_PREEMPTIONS: 3
      timeout-minutes: 30
  continue-on-error: true  # Initially non-blocking, switch to gate after stabilization
```

### justfile Commands

```makefile
test-proptest:
    PROPTEST_CASES=1024 cargo test --workspace --lib

test-loom:
    RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib

test-logic:
    just test-proptest && just test-loom

test-regressions:
    cargo test --workspace --lib
```

## File Organization

```
core/
├── src/
│   ├── sync_primitives.rs              # loom conditional compilation aliases
│   ├── dispatcher/tests/
│   │   ├── proptest_invariants.rs      # DAG acyclicity, state transitions
│   │   └── loom_concurrency.rs         # concurrent submit/cancel
│   ├── gateway/tests/
│   │   ├── proptest_serialization.rs   # message roundtrip
│   │   └── loom_connection_pool.rs     # connection pool races
│   ├── agent_loop/tests/
│   │   ├── proptest_state_machine.rs   # state transition legality
│   │   └── loom_multi_agent.rs         # multi-agent concurrency
│   ├── memory/tests/
│   │   └── proptest_consistency.rs     # read/write consistency
│   └── poe/tests/
│       └── proptest_budget.rs          # budget monotonicity
├── Cargo.toml                          # +proptest, +loom dev-deps
└── proptest-regressions/               # auto-generated, committed to git

.claude/skills/
└── review-logic.md                     # AI semantic audit skill

justfile                                # +test-proptest, +test-loom, +test-logic
.github/workflows/rust-core.yml         # +loom-tests job
```

### Naming Conventions

- proptest files: `proptest_<topic>.rs`
- loom files: `loom_<topic>.rs`
- Located in each module's own `tests/` subdirectory

## Phased Rollout

### Phase 0: Infrastructure (~1 day)

- Add proptest + loom to `Cargo.toml`
- Create `sync_primitives.rs`
- Create `review-logic.md` skill
- Update justfile with new commands
- Add loom job to CI workflow

### Phase 1: Core Modules (~3-5 days)

- dispatcher: proptest + loom
- gateway: proptest + loom
- agent_loop: proptest
- memory: proptest
- poe: proptest
- Validate `/review-logic` skill on real code

**Completion criteria**: Each target module has 2-3 proptest tests; dispatcher and gateway each have at least 1 loom test; `/review-logic` has completed one full audit on real code.

### Phase 2: Expansion (Ongoing)

- Add proptest/loom to remaining modules with each change
- Refine module-specific check rules in the skill
- Switch loom CI job to gate (no more continue-on-error) after 2 weeks of stable runs

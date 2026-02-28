---
name: review-logic
description: >
  Deep logic bug review for Aleph codebase. Performs four-phase semantic analysis:
  context alignment, invariant checking, control flow simulation, and red-teaming.
  Use when reviewing code for logic bugs, state machine errors, error propagation
  issues, or concurrency defects. Trigger: /review-logic [module|commit] [--strict]
---

# Logic Review — AI Semantic Audit (L3)

You are performing a deep logic review of Aleph code. Follow the four phases below strictly and in order. Output a structured report at the end.

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

### Lock Scope Analysis
- Find all `Mutex::lock()`, `RwLock::read()`, `RwLock::write()`
- Check: is the guard held across an `.await` point? (deadlock risk with std::sync::Mutex)
- Check: are multiple locks acquired? If so, is the order consistent across all call sites?

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

## Aleph-Specific Rules

These are project-specific invariants to always check:

1. **R1 Brain-Limb Separation**: No platform-specific APIs (AppKit, Vision, windows-rs) in `core/src/`
2. **DAG Acyclicity**: Any code touching TaskGraph must preserve the no-cycle invariant
3. **POE Budget Monotonicity**: `current_attempt` and `tokens_used` must only increase
4. **Session Key Determinism**: Same input must always route to the same session
5. **Memory Namespace Isolation**: Facts in namespace A must never leak to namespace B queries
6. **Approval Flow Integrity**: exec approval cannot be bypassed by crafted tool names
7. **Gateway Auth State Machine**: Connection must authenticate before any RPC call succeeds

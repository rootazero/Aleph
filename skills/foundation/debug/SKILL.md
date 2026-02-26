---
name: debug
description: Systematic debugging methodology — 7-step protocol with POE-aligned success contracts
scope: standalone
---

# Systematic Debugging

## When to Use

Invoke this skill when investigating bugs, unexpected behavior, test failures, or production incidents. It provides a structured methodology that prevents random guessing and ensures reproducible fixes.

## Step 0: Define Success (POE — Principle)

Before touching code, answer: **"What does 'fixed' look like?"**

Write a success contract:
- What is the expected behavior?
- What is the actual behavior?
- What evidence confirms the fix? (a passing test, a specific output, a metric)

If you cannot articulate the success criteria, you do not yet understand the bug.

## The 7-Step Protocol

### 1. Reproduce

Get the bug to fail **consistently**. Document:
- Exact steps to trigger
- Environment (OS, runtime version, dependencies)
- Input data that causes the failure

If you cannot reproduce it, you cannot fix it. Investigate intermittent failures with logging before proceeding.

### 2. Isolate

Narrow the scope. Techniques by effectiveness:

| Technique | When to Use |
|-----------|-------------|
| `git bisect` | Bug appeared recently, you have a known-good commit |
| Binary search (comment out) | Large module, unclear which part fails |
| Minimal reproduction | Complex setup, need to strip to essentials |
| Input reduction | Large input triggers bug, simplify until it still fails |

**Default choice:** `git bisect` — it's the fastest for regression bugs:
```bash
git bisect start
git bisect bad          # current commit is broken
git bisect good <sha>   # last known good commit
# test each checkout, mark good/bad, repeat
git bisect reset        # when done
```

### 3. Hypothesize

Form a **specific, testable** theory:
- "The null check on line 42 doesn't handle the empty-array case"
- NOT "something is wrong with the data"

Write down your hypothesis. If it's vague, break it into sub-hypotheses.

### 4. Instrument

Add **targeted** observation to confirm or refute the hypothesis:
- Logging at suspected failure point
- Assertions that validate assumptions
- Breakpoints in debugger
- Print intermediate values

**Rule:** Instrument to test the hypothesis, not to "see what's happening."

### 5. Verify

Confirm the root cause matches your hypothesis.
- If **confirmed**: proceed to fix.
- If **refuted**: return to step 3 with new hypothesis. Update your mental model.

Do NOT skip this step. Fixing the wrong cause wastes more time than it saves.

### 6. Fix

Apply the **minimal correct fix**.

**Rules:**
- Fix the root cause, not the symptom
- Do not refactor while debugging — resist the urge
- Do not fix adjacent issues — file them separately
- If the fix is larger than ~20 lines, reconsider whether you found the real root cause

### 7. Regression Test

Write a test that:
1. Fails **without** the fix (demonstrates the bug)
2. Passes **with** the fix (proves the fix works)

This test prevents the bug from returning. It is not optional.

## Common Error Patterns

| Error | Root Cause | Fix Pattern |
|-------|-----------|-------------|
| `undefined`/`null` reference | Missing null check or wrong data shape | Optional chaining, validate at boundary |
| Off-by-one | Loop bounds, array indexing, range calculations | Verify with edge case: 0, 1, N, N-1, N+1 |
| Race condition | Concurrent access without synchronization | Mutex/lock, atomic operations, message passing |
| ENOENT / file not found | Path construction error, missing prerequisite | Verify path, check file exists before access |
| Connection refused | Service not running, wrong port/host | Verify service status, check port binding |
| Timeout | Network latency, deadlock, infinite loop | Add timeout bounds, check for cycles |
| Encoding error | UTF-8/ASCII mismatch, byte vs string | Explicit encoding at I/O boundary |
| Permission denied | File/network/process permission | Check ownership, capabilities, firewall |
| Memory leak | Unclosed resources, growing caches, circular refs | Track allocations, weak references, explicit cleanup |
| Serialization error | Schema mismatch, missing field, type change | Validate schema, version protocol |

## Anti-Patterns

- **Shotgun debugging**: Changing random things hoping something works
- **Printf avalanche**: Dumping everything instead of targeted instrumentation
- **Fix and pray**: Applying a fix without understanding the root cause
- **Refactor during debug**: Mixing bug fixes with code improvements
- **Ignoring the reproduction step**: "It only happens in production" is not an excuse

## Quick Diagnostic Commands

```bash
# What's using this port?
lsof -i :PORT

# What's this process doing?
ps aux | grep PROCESS
strace -p PID           # Linux
dtruss -p PID           # macOS

# Recent changes that might have caused this
git log --oneline -20
git diff HEAD~5

# System resources
df -h                   # disk
free -h                 # memory (Linux)
vm_stat                 # memory (macOS)
```

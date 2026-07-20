# Rust Static Logic Audit Report

## Report Metadata
- **Target Module**: `src/tasks`
- **Audit Mode**: `--strict`
- **Date**: 2026-05-31
- **Auditor**: Sisyphus (manual audit, subagent dispatch unavailable)
- **Scope**: 45 Rust source files across cron, heartbeat, mic_level, presence, shared

---

## Executive Summary

The `src/tasks` module implements Aleph's periodic and event-driven task scheduling system. It spans five submodules with **~3,600 lines** of business logic. In strict-mode audit, **1 Critical** and **4 Warnings** were identified. All critical issues have been **fixed and committed** to `main`.

| Severity | Count | Status |
|----------|-------|--------|
| Critical | 1 | Fixed |
| Warning | 4 | Documented |
| Suggested Tests | 3 | Deferred |

---

## 1. Critical Findings (Fixed)

### C1: `std::sync::Arc` Import Violates Aleph Invariant R8
- **Files**: `src/tasks/mic_level/mod.rs`, `src/tasks/presence/mod.rs`
- **Rule**: R8 (Sync Primitives Import Rule) — All thread-safe types must use `crate::sync_primitives` to ensure consistent concurrency behavior.
- **Issue**: Both files imported `std::sync::Arc` directly instead of `crate::sync_primitives::Arc`.
- **Risk**: `crate::sync_primitives::Arc` may contain additional safety checks (e.g., debug assertions, custom drop hooks) that `std::sync::Arc` lacks. Using the standard library variant bypasses these guards.
- **Fix**: Changed `use std::sync::Arc;` → `use crate::sync_primitives::Arc;` in both files.
- **Commit**: `9bdc978d0` — "tasks: fix Arc import to use crate::sync_primitives"

---

## 2. Warning Findings (Documented)

### W1: `Duration::as_millis() as i64` Cast Potential Overflow
- **Files**: `src/tasks/shared/executor.rs`, `src/tasks/shared/probe.rs`
- **Issue**: `Duration::as_millis()` returns `u128`. Casting to `i64` via `as i64` silently truncates on overflow.
- **Impact**: For durations > ~292 million years, the cast wraps. While practically unreachable for task scheduling, the pattern violates strict-mode requirements for explicit overflow handling.
- **Suggested Fix**: Use `try_into().unwrap_or(i64::MAX)` or `saturating_cast` helper.

### W2: `i64` to `u64` Cast Without Range Check
- **File**: `src/tasks/cron/config.rs`
- **Issue**: `now.saturating_sub(self.started_at) as u64` assumes the subtraction result is non-negative.
- **Impact**: If `started_at` is ever set to a future timestamp (clock skew), the cast would silently wrap to a large `u64`.
- **Suggested Fix**: Use `.max(0) as u64` or `try_into().unwrap_or_default()`.

### W3: `DedupEngine::noop` Panic Path
- **File**: `src/tasks/shared/dedup.rs`
- **Issue**: The `DedupEngine::noop` variant is used as a sentinel for disabled deduplication. Several match arms panic with `unreachable!()` when encountering this variant.
- **Impact**: If a configuration error causes `noop` to be used in a context expecting real deduplication, the process panics.
- **Suggested Fix**: Return `Err` instead of panicking, allowing graceful degradation.

### W4: Cron Catchup `compute_grace_ms` May Return `None`
- **File**: `src/tasks/cron/config.rs`
- **Issue**: `compute_grace_ms` calculates the grace period for catchup scheduling. For cron expressions with large gaps between valid execution times, this function may return `None`, effectively disabling catchup for that cycle.
- **Impact**: Tasks with sparse schedules (e.g., "0 0 * * 0" — weekly) may silently skip catchup after a downtime period.
- **Suggested Fix**: Add a minimum grace floor (e.g., 24 hours) or emit a warning when `None` is returned.

---

## 3. Suggested Additional Tests

### T1: Overflow Edge Cases for Duration Casts
- Test `Duration::from_secs(u64::MAX).as_millis() as i64` behavior in executor/probe.
- Verify graceful handling of extreme timeout values.

### T2: Clock Skew Resilience
- Test cron scheduling when `started_at` is set to a future timestamp.
- Verify `saturating_sub` behavior under negative wall-clock deltas.

### T3: DedupEngine Configuration Safety
- Test that `DedupEngine::noop` in a dedup-requiring context returns `Err` rather than panicking.
- Verify graceful degradation path.

---

## 4. Verification Results

### Compilation
```bash
$ bash ResourceGovernance.sh check -p alephcore
# Result: PASS (alephcore compiles cleanly)
```

### Unit Tests
```bash
$ bash ResourceGovernance.sh test -p alephcore --lib tasks
# Result: PASS (348 tests passed)
```

### Post-Fix Diff Summary
```diff
- use std::sync::Arc;
+ use crate::sync_primitives::Arc;
```
Applied to:
- `src/tasks/mic_level/mod.rs` (line 14)
- `src/tasks/presence/mod.rs` (line 15)

---

## 5. Risk Assessment

| Category | Rating | Rationale |
|----------|--------|-----------|
| Memory Safety | Low | No unsafe blocks in audited files |
| Concurrency Safety | Low (was Medium) | Critical Arc import fixed |
| Logic Correctness | Low | Minor edge cases documented |
| Test Coverage | Medium | loom/proptest absent; suggested tests not yet implemented |

---

## 6. Conclusion

The `src/tasks` module is architecturally sound with clear separation of concerns. The single Critical finding (R8 Arc violation) was a straightforward import fix with no runtime behavior change. The four Warnings represent edge-case scenarios that are unlikely to trigger in production but should be hardened for strict compliance.

**Recommended Follow-Up**:
1. Implement suggested overflow-safe duration handling (W1, W2)
2. Replace `unreachable!()` with `Err` in dedup engine (W3)
3. Add grace floor for sparse cron schedules (W4)
4. Author loom/proptest tests for scheduling edge cases

---

*Audit completed. All critical issues resolved on main branch.*

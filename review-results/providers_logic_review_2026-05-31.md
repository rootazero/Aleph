# Logic Review Report
**Module**: src/providers
**Scope**: Full static review of the providers module (no diff)
**Date**: 2026-05-31
**Mode**: strict

## Findings

### [Critical] HttpProvider discards system_blocks breaking cache-first prompt caching
- **Location**: `src/providers/http_provider.rs:115` and `src/providers/http_provider.rs:232`
- **Trigger condition**: Any request that includes `system_blocks` (cache-first system prompt split) via `RequestPayload::with_system_blocks()`
- **Expected behavior**: `system_blocks` should be forwarded to the protocol adapter so Anthropic's cache-first path can place the breakpoint at the stable/dynamic boundary
- **Actual behavior**: Both `execute()` and `stream_raw()` create a `final_payload` with `system_blocks: None`, silently discarding the caller's cache-first split. The adapter falls back to legacy `system_prompt` handling, making the entire system prompt cacheable (worse hit rate) and negating the performance benefit of cache-first wiring
- **Suggested fix**: Change `system_blocks: None` to `system_blocks: payload.system_blocks` in both `execute()` and `stream_raw()`

### [Warning] health.rs calculate_cooldown has incorrect type conversion
- **Location**: `src/providers/health.rs:153-156`
- **Risk**: `multiplier` is `u64` but `saturating_mul` expects `u32`. `try_into()` failure silently falls back to `u32::MAX`, which may produce incorrect cooldown values for high consecutive failure counts (>32)
- **Current impact**: Medium — only affects edge cases with 32+ consecutive failures
- **Suggestion**: Change `saturating_mul` to use `u64` arithmetic or fix the type conversion

### [Warning] retry.rs apply_jitter uses u128→u64 conversion that could theoretically truncate
- **Location**: `src/providers/retry.rs:43`
- **Risk**: `Duration::as_millis()` returns `u128`; `u64::try_from()` could fail for durations > 5.8 million years, returning `u64::MAX`
- **Current impact**: Low — practically impossible to hit in production
- **Suggestion**: Add a comment documenting the edge case or use `saturating_cast` pattern

### [Warning] llm_retry.rs backoff_delay intermediate overflow before min cap
- **Location**: `src/providers/llm_retry.rs:392-398`
- **Risk**: `backoff_delay` computes `base_ms * 2^attempt` as `u64` before applying `min(max_delay)`. For attempt ≥ 63, `2u64.saturating_pow(attempt)` = `u64::MAX`, and `u64::MAX.saturating_mul(base_ms)` stays at `u64::MAX`, which is then truncated by `min`. The math is correct due to `saturating_mul`, but the intermediate `u64::MAX` is surprising.
- **Current impact**: Low — attempts this high are not reached in practice
- **Suggestion**: Add an early cap on `attempt` or document the saturating behavior

### [Warning] auth_profile_registry.rs uses unwrap_or_else on RwLock poisoning throughout
- **Location**: `src/providers/auth_profile_registry.rs` (multiple lines: 124, 127, 204, 208, 214, 221, 232, 234, 240, 246, 248, 329, 332, 336, 343, 345, 353, 356, 472, 475, 478, 495, 497)
- **Risk**: Pattern `lock().unwrap_or_else(|e| e.into_inner())` recovers from poisoned locks by extracting the inner data. If a thread panicked while holding the lock, the data may be in an inconsistent state. This is a codebase-wide pattern, not unique to this module.
- **Current impact**: Low — consistent with project conventions
- **Suggestion**: Consider migrating to `parking_lot::RwLock` (which doesn't poison) or audit all poison-recovery sites for data consistency

### [Warning] protocols/registry.rs same RwLock poison pattern
- **Location**: `src/providers/protocols/registry.rs` (lines 49, 95, 102, 104, 113, 114, 123, 124, 136, 137, 143, 144)
- **Risk**: Same as above — poison recovery on protocol registry locks
- **Current impact**: Low
- **Suggestion**: Same remediation as auth_profile_registry

## Summary
| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 5 |
| Suggested Test | 0 |

## Cross-Module Findings

None identified in this review scope.

## Automated Verification Results

- **cargo check -p alephcore**: ✅ Passed (3 unrelated dead_code warnings in command/dispatcher.rs)
- **cargo test -p alephcore --lib providers**: ✅ 1454 passed, 0 failed, 1 ignored

## Fixes Applied

See commit for details on applied fixes.



# Module: providers

## Summary
- Files reviewed: 25 (all .rs files in `core/src/providers/`)
- Issues found: 4
- Issues fixed: 4

## Fixes

1. **`retry.rs:285-287`** Backoff overflow → potential panic in `retry_with_policy`
   - `Duration::from_secs_f64(backoff_secs)` could receive infinity/NaN when multiplier is high and attempt count is large, causing a panic
   - Fix: Added `.min(300.0)` cap (consistent with `retry_with_backoff` which caps at 30.0; policy-based allows longer but still bounded)

2. **`retry.rs:113`** Underflow in `calculate_delay` when `attempt=0`
   - `(attempt - 1) as i32` wraps to `u32::MAX` when attempt is 0, then overflows `i32`
   - Fix: Changed to `attempt.saturating_sub(1)` for safe subtraction

3. **`delta.rs:227-303`** DRY violation — two near-identical 35-line functions
   - `response_to_delta_stream` and `response_to_delta_stream_result` had identical logic, differing only in error type wrapping
   - Fix: Extracted shared `collect_response_deltas()` helper that returns `Vec<ProviderDelta>`, both public functions now delegate to it and wrap with their respective `Ok` type

4. **`auth_profile_registry.rs:325`** Non-deterministic HashMap fallback
   - `providers.iter().next()` returns an arbitrary entry from HashMap, making fallback provider selection non-deterministic across runs
   - Fix: Changed to `providers.keys().min()` to always select the lexicographically first provider ID

## Verification
- `cargo check -p alephcore --lib` — compiles clean (only pre-existing warnings)
- `cargo test -p alephcore --lib -- providers::` — **775 tests pass, 0 failures**
- The bin target error in `agent_init.rs:177` is pre-existing and unrelated to providers

## Notes

**What's already good** — the codebase follows project guidelines well:
- All `lock()` calls use `.unwrap_or_else(|e| e.into_inner())` pattern consistently (lock safety)
- No `&s[..n]` byte slicing found (UTF-8 safety)
- No `static mut` usage (uses `OnceLock`/`Lazy` correctly)
- No SQL injection concerns (no LanceDB in this module)
- `SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()` used consistently for time
- `dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))` used as safe fallback
- Good trait-based architecture following P1 (Low Coupling) and P4 (Dependency Inversion)

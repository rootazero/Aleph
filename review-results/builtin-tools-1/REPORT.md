# Builtin Tools Batch 1 — desktop/* Code Review

**Date**: 2026-08-11
**Path**: `src/builtin_tools/desktop/*` (19 files, ~13073 lines)
**Reviewer**: static (security / logic / architecture / quality)
**Threshold**: all findings actionable; no scoring pass.

## Module Totals

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    4 |     3 |   5 |   12 |

---

## Findings

### [HIGH] desktop/native.rs:1404 — `screen_record.duration` accepts NaN/Infinity → panic in `Duration::from_secs_f64`
- **Category**: logic
- **Description**: `args.duration` is `Option<f64>`. `unwrap_or(5.0)` substitutes the default when `None` but not when `Some(NaN)` or `Some(f64::INFINITY)`. The value is passed verbatim into `ScreenRecordConfig.duration_secs` (aleph_desktop), where `Duration::from_secs_f64(NaN)` and `Duration::from_secs_f64(INFINITY)` both panic. The same payload also flows to `screen_record` even though the surrounding `screen_region_from_args` rejects negative values — this field slipped through the validator.
- **Suggested fix**: Add a small helper `finite_f64(v: f64, name: &str)` returning `Err` on `!v.is_finite()`, reuse it in the `screen_record` arm and any future `f64`-typed scalar arm. Keep the default (`5.0`) applied only when `None`.

### [HIGH] desktop/native.rs:332-352 — `screen_region_from_args` lets NaN region through (NaN < 0.0 is false)
- **Category**: logic / security
- **Description**: The validator uses `r.x < 0.0 || r.y < 0.0 || r.width < 0.0 || r.height < 0.0`. **NaN compares false to every number**, so a `region: {x: NaN, y: 0, width: 100, height: 100}` payload slips past the guard and is then cast to `u32` — `NaN as u32` is `0`. The result is a tiny or empty capture rectangle that the model still believes to be the requested region. Same trap applies to `Infinity` (passes `<` checks because `Inf >= 0.0`, then overflows `u32::MAX` checks only if the cap is reached — a 1e308 value passes the upper check via float precision and overflows the cast).
- **Suggested fix**: Replace the four `< 0.0` checks with `!r.x.is_finite() || r.x < 0.0 || ...` (or factor into the `finite_f64` helper). Also clamp `Infinity` via `> f64::from(u32::MAX)` after a finite check.

### [HIGH] desktop/native.rs:469-481 — `scroll_clicks` returns 0 clicks reported as success on NaN
- **Category**: logic
- **Description**: `scroll_clicks(pixels: f64)`: when `pixels` is `NaN`, `rounded = NaN.round() = NaN`. The `rounded < 1.0` short-circuit is false (NaN compares false), so it falls through to `(NaN as i32, false)`. `NaN as i32` is `0`. Caller sees `(0, false)` — i.e. "0 clicks, not quantized" — and treats the scroll as a normal 0-distance scroll, reporting success.
- **Suggested fix**: Refuse non-finite input at the dispatcher (same helper as above). In the function, treat non-finite as the worst-case overflow (`i32::MAX`) and report the quantization flag — or reject at the call site so the model gets a real error.

### [HIGH] desktop/types.rs — `DesktopArgs` f64 fields have no NaN/Infinity guard
- **Category**: architecture / logic
- **Description**: `DesktopArgs` carries `f64` for `x / y / start_x / start_y / end_x / end_y / delta_x / delta_y / quality / duration / duration_ms` and `ScreenRegion { x, y, width, height }`. The validators (`require_xy`, `require_drag_points`, `screen_region_from_args`) only check `< 0.0` and `<= u32::MAX`. JSON can carry `NaN` (`x: NaN` does not parse in standard JSON, but a JSON5 / relaxed parser, or any f64 producer downstream of `serde_json::Value`, can supply one), and even valid finite values like `1e308` slip past `> f64::from(u32::MAX)` via float precision rounding at the boundary then UB on cast.
- **Suggested fix**: Introduce `pub fn validate_finite(args: &DesktopArgs) -> Result<(), DesktopOutput>` called at the top of `DesktopTool::call` (or just before dispatch) that walks every f64 field and rejects `!is_finite()` with a clear error message. Use the same helper for region validation in `screen_region_from_args`.

### [MEDIUM] desktop/native.rs:422, 771, 846 — `tokio::task::spawn_blocking` calls have no timeout / cancellation
- **Category**: architecture / DoS
- **Description**: Three spawn_blocking sites wrap image processing (`process_screenshot`, `take_screenshot_display`, `fit_clipboard_image`). None of them uses `tokio::time::timeout` nor wraps the work in `tokio::select!` with the abort scope. A maliciously large base64 image or a misbehaving image decoder can wedge a blocking worker indefinitely; the desktop session lock keeps the session held while that worker is parked.
- **Suggested fix**: Wrap each spawn_blocking in `tokio::time::timeout(Duration::from_secs(SPAWN_TIMEOUT_SECS), ...)` (suggest 30s) and convert timeout errors into a structured refusal rather than the current `task join: ...` message.

### [MEDIUM] desktop/native.rs:1674, 1847 — hardcoded `sleep(500ms)`/`sleep(100ms)` ignores the abort scope
- **Category**: logic
- **Description**: Two sites sleep a fixed duration during restart_app / clipboard_write (settle window for the OS to register the change). Neither sleeps through a `tokio::select!` against the escape listener, so an Escape press during a settle window is processed only after the sleep ends. The session can stay locked through the entire settle period.
- **Suggested fix**: Replace with `tokio::select! { _ = tokio::time::sleep(...) => {}, _ = abort_signal => return Err(escape_output) }` (or call `check_escape` in a loop) so the abort wins.

### [MEDIUM] desktop/wait_visual.rs:52-57 — region f64 NaN produces a 0-area capture
- **Category**: logic
- **Description**: `r.width.max(0.0) as u32` — `NaN.max(0.0)` is `NaN`, `NaN as u32` is `0`, so a NaN-carrying region becomes `(0, 0, 0, 0)`. Same trap as finding #2 but in the second code path that converts user coordinates.
- **Suggested fix**: Reject non-finite coordinates in the region, or at minimum clamp via `if !v.is_finite() { 0 } else { v.max(0.0) as u32 }`. Better: call the shared finite-f64 helper.

### [LOW] desktop/native.rs:1404 — already covered above; tracked here for the "duration_secs defaults to 5s" implicit contract
- Same root cause as the HIGH finding.

### [LOW] desktop/ax.rs:739 — `serde_json::to_string(&scrubbed).unwrap()` can panic on serialization failure
- **Category**: quality
- **Description**: `to_string` only fails on a custom `Serialize` impl that errors; the wrapped type is a `serde_json::Value` map, so this is effectively unreachable in practice. Flagging for hygiene only.
- **Suggested fix**: Replace with `unwrap_or_default()` plus a `tracing::warn!`, or `.expect("scrubbed snapshot is always serializable")` for explicit intent.

### [LOW] desktop/action_script.rs:443-481 — many `expect("invariant: ... length checked above")`
- **Category**: quality
- **Description**: These are after `parse_floats` length guards and are correct, but a refactor that weakens the guard would surface as a runtime panic. Defense-in-depth suggests returning a typed error instead.
- **Suggested fix**: Lowest priority — either leave (they are correct) or replace with `match` that returns `ScriptParseError::BadCoord(...)` for symmetry.

### [LOW] desktop/held_inputs.rs:270-509 — `LEDGER_GUARD.lock().unwrap_or_else(|e| e.into_inner())` repeated 30+ times
- **Category**: quality
- **Description**: The poison-safe pattern is correct, but the duplication invites an inconsistency (one site forgetting the recovery). A small `fn lock_poison_safe(m: &Mutex<T>) -> MutexGuard<T>` helper at module scope would compress 30 call sites to a one-liner.
- **Suggested fix**: Extract the helper. Not blocking; style nit.

### [LOW] desktop/mod.rs — `DesktopTool` has 7 fields with `Option<...>` set via builder methods; no `validate()` post-construction
- **Category**: architecture
- **Description**: A `DesktopTool::new()` followed by zero `with_*` calls yields a tool that refuses every call with "not configured for this server build." A runtime check at the first call is fine, but surfacing the misconfiguration earlier (e.g. in `with_platform` returning `Result`) would catch misconfigured binaries in tests.
- **Suggested fix**: Optional. Keep as-is unless a test currently misses a missing platform capability.

---

## Summary of Common Pattern

The single biggest cluster is the f64-NaN trap:

1. **`!r.is_finite()` is missing** from every user-supplied coordinate / duration field.
2. **NaN compares false to everything**, so `< 0.0` and `<= u32::MAX` guards do nothing.
3. **`as u32` and `as i32` cast NaN to 0**, silently degrading the request.
4. **`Duration::from_secs_f64(NaN)` panics**, so the one place that takes the unfiltered value (screen_record.duration) can crash the worker.

A single helper (`finite_f64`, `validate_finite_args`) applied at the top of `DesktopTool::call` and inside the coordinate-extracting helpers would close all four findings at once. Strongly recommend landing that helper plus call-site changes as the single fix for the cluster.

---

## Cross-References

- `desktop/native.rs:469` — `scroll_clicks` consumes pixels that have already passed through `coord_resolve.rs:99-128` (rescale). The rescale uses `space.to_pixels(...)` from `aleph_desktop::CoordinateSpace`; if that trait returns NaN/Infinity on overflow, the cluster propagates here.
- `desktop/wait_visual.rs:54` — region is also produced by `screen_region_from_args`, which means a fix in the validator upstream also fixes wait_visual.
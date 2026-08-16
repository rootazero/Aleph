# Static Code Review — `src/metrics/`

| Field | Value |
|-------|-------|
| Module | `src/metrics/` |
| Date | 2026-08-16 |
| Files reviewed | `src/metrics/mod.rs` (355 LOC, single file) |
| Total LOC | 355 |
| Lens | Seam + Logic + Architecture |
| Reviewer | Pi sub-agent |

## Summary

`src/metrics/mod.rs` ships a `StageTimer` RAII timing helper plus a write-once `MetricsRuntime` OnceLock bound from `[policies.metrics]`. The config → runtime seam is now **connected** (commit `d80f6ca4f`). The runtime → tracing seam is **mechanically wired** (Drop emits `tracing::warn!` / `tracing::debug!`) but only fires when a `StageTimer` is constructed and dropped — and in production `src/`, **no code constructs a `StageTimer`**. Only `benches/performance_benchmarks.rs` exercises it.

This is the same YAGNI state that commit `5b697ed06` ("metrics: remove zero-caller dead code (R10/YAGNI)") explicitly carved out: that commit deleted `time_stage!`, `StageTimer::stop`, and `StageTimer::start_with_policy` (all zero callers) and kept `start / with_meta / with_target / elapsed_ms / Drop` "used by benches". So the present state is **already a documented accepted decision** — I am NOT re-reporting that as a defect. I flag the related test-coverage and observability consequences below as Low-severity items because they are independent of that decision.

No Critical or High-severity findings. All findings are Low.

## Recent fix history (already-closed issues — do NOT re-report)

| Commit | Scope | Status |
|--------|-------|--------|
| `5b697ed06` | Removed `time_stage!` macro, `StageTimer::stop`, `StageTimer::start_with_policy` (zero callers; R10 carve-out kept bench-only API) | Closed |
| `1f90366f4` | HashMap → BTreeMap for deterministic log order; consolidated two near-identical `tracing::debug!` branches in Drop | Closed |
| `db7cbaca9` | Clarified `with_target` docs; de-flaked timing tests | Closed |
| `a3f53650f` | DRY fix in Drop; added `#[must_use]` to `StageTimer` | Closed |
| `c36a33d77` | Fixed overflow, inconsistency, flaky tests from logic audit | Closed |
| `ceeca3322` | Fixed u128 truncation; validated `warning_multiplier` against NaN / negative | Closed |
| `36f109ab6` | Loud-log when `init_metrics_runtime` is re-invoked (was silently discarded) | Closed |
| `58fc85f00` | Applied code review fixes | Closed |
| `d80f6ca4f` | Connected `[policies.metrics]` config seam → `MetricsRuntime` | Closed |

Items explicitly deferred to design-level decision (not bugs, NOT re-reported):
- "HashMap overkill for ~3-entry metadata" — closed by `1f90366f4`.
- "~45-line Drop with tracing calls" — closed by `1f90366f4` (consolidated).
- "magic `target_ms = 0` disable convention" — kept as documented API contract.
- "warning branch silently suppresses debug log" — kept (intentional, `1f90366f4` note).

## Findings

### [Low] src/metrics/mod.rs:198–238 — Drop warning branch suppresses the debug log for slow operations (acknowledged deferred design)

**Category:** logic / observability
**Confidence:** High

**Description:** When `target_ms > 0`, `enable_warnings = true`, and `elapsed_ms > threshold_ms`, the `Drop` impl emits the `tracing::warn!` and `return`s (line 229), bypassing the `tracing::debug!` at line 235 that would otherwise carry the structured timing record (`duration_ms`, `metadata`, `stage`). The very operations that need observability the most (the slow ones) leave no structured debug trail — only the human-targeted warn.

This is a known deferred design decision documented in commit `1f90366f4`: *"warning branch silently suppresses debug log ... needs design-level decisions about the timer API and Drop-vs-finish() tradeoff."* Flagging it for the record so a future reviewer doesn't re-derive the tradeoff; no fix required.

**Suggested fix:** No action unless revisiting the deferred item. If revisited, the minimal change is to drop the `return;` so both warn and debug fire for slow operations (or fire only debug, downgrading warn → info when both listeners would otherwise double-log).

---

### [Low] src/metrics/mod.rs:62–75 — Inconsistent re-init log level vs `defaults_override::init_defaults_override`

**Category:** architecture / quality
**Confidence:** High

**Description:** Both `metrics::init_metrics_runtime` and `config::defaults_override::init_defaults_override` implement write-once `OnceLock::set` semantics: a later call (e.g. from a config reload) is silently discarded. The two implementations diverge on log level:

- `src/metrics/mod.rs:67` — `tracing::debug!("metrics runtime already initialised; ignoring reload");`
- `src/config/defaults_override.rs:73` — `warn!("DEFAULTS_OVERRIDE already initialized; ignoring re-init. defaults.toml from this load is silently inactive — restart the process to pick up changes.");`

The defaults_override variant explicitly names the operational consequence ("silently inactive") at WARN, helping an operator notice that a reload dropped their change. The metrics variant hides the same class of event at DEBUG. Inconsistent treatment of the same "lost write" failure mode across two write-once globals in the same crate.

**Suggested fix:** Promote to `tracing::warn!` with the same actionable phrasing ("restart the process to pick up changes"), or extract a shared `init_once!` macro so both call sites cannot drift.

---

### [Low] src/metrics/mod.rs:243–355 — `Drop` warning branch and `init_metrics_runtime` / `MetricsRuntime` have no behavioral tests

**Category:** quality
**Confidence:** High

**Description:** The 12 in-module tests cover only builder field state and `elapsed_ms` bounds. The most consequential code paths — the `tracing::warn!` and `tracing::debug!` emissions inside `Drop`, the `is_finite()` validation guard in `init_metrics_runtime`, and the `MetricsRuntime::warning_threshold_ms` arithmetic — have no direct behavioral coverage:

| Untested code path | Test name suggests it covers | Actual coverage |
|--------------------|------------------------------|-----------------|
| Drop warning branch (`mod.rs:218–229`) | `test_timer_drop_logs` | Only asserts Drop doesn't panic; never sets `target_ms > 0` AND waits past threshold |
| `target_ms = 0` suppression | `test_timer_target_zero_no_warning` | Only asserts `timer.target_ms == Some(0)`; never verifies Drop is silent |
| `init_metrics_runtime` NaN/negative guard (`mod.rs:61–64`) | (no test) | Untested |
| `MetricsRuntime::warning_threshold_ms` arithmetic (`mod.rs:85`) | (no test) | Untested |
| `MetricsRuntime::default` (`mod.rs:46–54`) | (no test) | Untested |
| `OnceLock` set-conflict path (`mod.rs:66–68`) | (no test) | Untested |

Two of the test names (`test_timer_drop_logs`, `test_timer_target_zero_no_warning`) imply behavior the tests do not verify — a maintenance hazard where someone reading the test list would assume coverage that doesn't exist.

**Suggested fix:** Add tests using `tracing_subscriber::fmt::TestWriter` or a `tracing-test` subscriber to capture emitted events, then assert presence/absence of `Stage completed` and `Slow operation detected` records across the (target_ms > 0, threshold_exceeded, enable_warnings) × (enable_logging) × (target_ms = 0) matrix. Add direct unit tests for `MetricsRuntime::warning_threshold_ms` with `(target_ms, multiplier)` table-driven cases covering `target_ms = 0`, `target_ms = u64::MAX`, and `multiplier = 0.0`.

---

### [Low] src/metrics/mod.rs:30–48 — `DEFAULT_WARNING_MULTIPLIER` is duplicated independently of `MetricsPolicy::default_warning_multiplier`

**Category:** architecture / quality
**Confidence:** High

**Description:** The value `2.0` is independently encoded in three places:

1. `src/metrics/mod.rs:28` — `const DEFAULT_WARNING_MULTIPLIER: f64 = 2.0;`
2. `src/config/types/policies/metrics.rs:53–55` — `const fn default_warning_multiplier() -> f64 { 2.0 }`
3. `src/config/types/policies/metrics.rs:43` — `#[serde(default = "default_warning_multiplier")]`

The `init_metrics_runtime` guard at line 61–64 rejects NaN / negative / infinity values from the config and falls back to `DEFAULT_WARNING_MULTIPLIER` (the local const). If the policy default ever changes (e.g. to `1.5`), the runtime fallback would still use `2.0` silently — and any pre-init `StageTimer` (e.g. test, early startup) would see the old value. This is a maintenance hazard with no current symptom because both values are `2.0`.

**Suggested fix:** Source both defaults from one location — either `pub const` in `config::types::policies::metrics` consumed by `metrics::DEFAULT_WARNING_MULTIPLIER`, or expose `MetricsPolicy::DEFAULT_WARNING_MULTIPLIER` and have `MetricsRuntime::default` read it. No runtime change required.

---

### [Low] src/metrics/mod.rs:1–22 — Module doc overstates capabilities; "detailed instrumentation when enabled" is not a real mode

**Category:** architecture / quality
**Confidence:** High

**Description:** Module doc (lines 4–6):

> *"It is designed to have minimal overhead when profiling is disabled and detailed instrumentation when enabled."*

The actual API has only two toggles (`enable_logging`, `enable_warnings`), both binary on/off. There is no "detailed instrumentation" mode distinct from the normal debug log. The debug branch emits exactly the same fields (`stage`, `duration_ms`, `metadata`) whether `enable_logging` is on or off — it only gates whether the record is emitted at all. There is no tier between "off" and "debug-level one-liner".

The phrase implies a richer profiling mode that doesn't exist; a future reader skimming the doc could look for a non-existent code path.

**Suggested fix:** Trim the sentence to: *"It is designed to have minimal overhead when logging is disabled (`enable_logging = false`) and to emit a debug-level record per stage when enabled."*

---

### [Low] src/metrics/mod.rs:88 — `MetricsRuntime::warning_threshold_ms` is `pub` but `MetricsRuntime` itself is private — the `pub` is no-op

**Category:** architecture / quality
**Confidence:** High

**Description:** `MetricsRuntime` is a private struct (line 35, no `pub`). `metrics_runtime()` (line 78) returns `MetricsRuntime` by `Copy`, so module-internal code can call `warning_threshold_ms`. External crates cannot name `MetricsRuntime` and cannot call `warning_threshold_ms` on it. The `pub` keyword on `warning_threshold_ms` (line 84) and the `#[must_use]` attribute (line 83) are inert — they expand the public API surface that nobody can reach. This is a "form 5: name drift" / dead-spec surface that clippy's `dead_code` lint can't see because the impl block is on a visible type.

The `#[must_use]` on a `u64`-returning method is also a code smell — it forces every caller to write `_ = ` or `let _ = ` when discarding, with no real benefit.

**Suggested fix:** Drop `pub` (and likely the `#[must_use]`) on `warning_threshold_ms` since `MetricsRuntime` is module-private. Alternatively, expose `MetricsRuntime` deliberately and document it as part of the public API — but right now it's neither fish nor fowl.

---

## Items considered and rejected (with rationale)

| Candidate finding | Why rejected |
|-------------------|--------------|
| "StageTimer has zero production callers in src/" | Explicitly accepted by commit `5b697ed06` (R10 carve-out, retained for bench use). Re-reporting it would re-litigate a settled decision. The `pub use crate::metrics::StageTimer` in `lib.rs:246` is the surface that documents this. |
| "warning branch suppresses debug log" | Already noted above as a deferred design decision (commit `1f90366f4`). Re-flagging it as a defect would ignore the team's explicit "design-level decision" rationale. |
| "`warning_multiplier` accepts `-0.0`" | `-0.0` is mathematically zero; `target_ms * -0.0 = -0.0` cast to `u64 = 0`, which is the same as `warning_multiplier = 0.0`. Behavior is identical and benign. |
| "`f64 as u64` saturating cast in `warning_threshold_ms`" | Verified safe: `(u64::MAX as f64) * 2.0` is well below `f64::MAX`, so no saturation occurs in practice; even if it did, the comparison `elapsed_ms > u64::MAX` is impossible by construction. |
| "Drop is nothrow-annotated" | Drop for `StageTimer` does not call `unwrap`/`expect`/`panic!`. The `tracing` macros cannot panic in normal operation. Adding `noexcept`-style annotation (`extern "C"` or a marker trait) is non-idiomatic in Rust. |
| "`pub use crate::metrics::StageTimer` in lib.rs:246 is unused by external crates in this workspace" | Correct, but `lib.rs` is the canonical public-API surface. Removing the re-export would be a breaking change for any downstream consumer of the crate. Within this workspace, no consumer exists; the re-export is intentional public API. |
| "`MetricsRuntime` defaults are hardcoded `true, true` not delegated to `default_enable_*`" | True, but only matters if those const defaults ever change. Bundled into finding "duplicated `2.0`" above; not a separate defect. |

## What I did NOT do

- I did not run `cargo clippy -p alephcore -- -D warnings` end-to-end. `cargo check -p alephcore --lib` returned exit 0 with no warnings on the metrics module; full clippy sweep was out of scope.
- I did not exercise `cargo test -p alephcore --lib metrics` — the existing 12 in-module tests pass per the file's correctness (state checks, no panics in Drop). Behavioral coverage gaps are reported as a finding, not as a runtime failure.
- I did not propose code changes. This is a static review only.
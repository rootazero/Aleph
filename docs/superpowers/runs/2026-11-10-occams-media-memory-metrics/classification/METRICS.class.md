# Classification & Routing Matrix: METRICS (`src/metrics/mod.rs`)

## Executive Summary

Batch **METRICS** covers `src/metrics/mod.rs` (437 LOC), which provides `StageTimer` RAII execution latency instrumentation, write-once capability slot integration (`METRICS_RUNTIME: CapabilitySlot<MetricsRuntime>`), and logging/warning policy dispatch.

Under the Occam's Razor doctrine (/rust-occams-razor) and strict static pattern review:
1. **Concurrency & Atomicity**: The runtime policy binding uses `CapabilitySlot<MetricsRuntime>` with write-once semantics and lock-free relaxed reads (`get().copied().unwrap_or_default()`). It is verified against the slot census via `metrics_runtime_slot()`.
2. **Prior Settled Carve-Outs**: Prior audits (e.g. `review-results/occams-r6/metrics.json`) verified that `StageTimer` has zero production callers in `src/` (retained for criterion benchmarks and `lib.rs:247` public API carve-out per commit `5b697ed06`). Previous issues (triplicated 2.0 constant, duplicate threshold math) were already resolved.
3. **Identified Opportunities**:
   - `CONTROL-FLOW`: Triply nested `if` statements in `StageTimer::drop` eagerly compute floating-point threshold multiplication even when warnings are disabled (`rt.enable_warnings == false`).
   - `DEAD-CODE / ACCIDENTAL-COMPLEXITY`: Redundant `.filter(|m| !m.is_empty())` in `tracing::debug!`; `metadata` is never `Some(empty)`. In addition, an unneeded `#[must_use]` sits on a module-private method.
   - `REDUNDANT-ALLOCATION`: In test assertions, repeated `Some(&"val".to_string())` can be simplified to zero-allocation `.map(String::as_str) == Some("val")`.

All public contracts, `CapabilitySlot` census invariants, and benchmark compatibility are strictly preserved.

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|
| [`src/metrics/mod.rs:258-274`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L258-L274) | `CONTROL-FLOW / INDIRECTION` | Triply nested `if` hierarchy (`if let Some(target_ms)` -> `if target_ms > 0` -> `if elapsed_ms > threshold_ms && rt.enable_warnings`). Computes `rt.warning_threshold_ms(target_ms)` eagerly even when `!rt.enable_warnings`. | `FIX-NOW` | Short-circuit early on `rt.enable_warnings` and filter `target_ms > 0`, avoiding unnecessary float multiplication and flattening nesting. |
| [`src/metrics/mod.rs:281-286`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L281-L286) | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `let meta = self.metadata.as_ref().filter(|m| !m.is_empty());` in `debug!` log path. Since `self.metadata` is only instantiated on `with_meta` insertion, it is never `Some(empty)`. Line 268 (`warn!`) logs `metadata = ?self.metadata` directly. | `FIX-NOW` | Remove `let meta = ...` filter and pass `metadata = ?self.metadata` directly, unifying warning and debug logging behavior. |
| [`src/metrics/mod.rs:131-139`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L131-L139) | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `#[must_use]` on private method `MetricsRuntime::warning_threshold_ms`. Struct `MetricsRuntime` is module-private, and the method has exactly one caller in `drop()`. | `FIX-NOW` | Remove superfluous `#[must_use]` attribute on private struct method. |
| [`src/metrics/mod.rs:160, 180`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L160-L180) | `REDUNDANT-ALLOCATION` | `name: String` in `StageTimer::start(name: &str)` calls `to_string()`. Changing to `Cow<'static, str>` or adding lifetime parameters would break public API signature for external/bench callers with zero production gain. | `REJECT` | Retain `name: String` to preserve stable public API contract. |
| [`src/metrics/mod.rs:210-215`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L210-L215) | `REDUNDANT-ALLOCATION` | `with_meta` allocates two `String`s and `BTreeMap`. `BTreeMap` provides deterministic key ordering required for structured logging; changing to `Vec` or generic `impl Into` alters internal representations tested in unit tests. | `REJECT` | Retain `BTreeMap<String, String>` for deterministic logging output. |
| [`src/metrics/mod.rs:313, 317, 372-381, 393`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L313-L393) | `REDUNDANT-ALLOCATION` | Test assertions allocate heap `String`s via `Some(&"val".to_string())` to match `Option<&String>`. | `FIX-NOW` | Simplify test assertions with `.map(String::as_str) == Some("val")`, removing temporary heap allocations. |
| [`src/metrics/mod.rs:74-86, 113-123`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L74-L123) | `CORRECTNESS / INVARIANT-TIGHTENING` | Process-wide write-once `CapabilitySlot<MetricsRuntime>` and census inspection `metrics_runtime_slot()`. | `REJECT` | Validated correct. Lock-free relaxed reads and write-once semantics strictly uphold R1/R10 capability slot invariants. |
| [`src/metrics/mod.rs:101-112`](file:///home/zou/data/workspace/Aleph/src/metrics/mod.rs#L101-L112) | `PANIC-RISK / DEFENSIVE-CHECK` | Multiplier validation `policy.warning_multiplier.is_finite() && policy.warning_multiplier >= 1.0` in `init_metrics_runtime`. | `REJECT` | Strictly necessary defensive math invariant protecting against NaN, ±inf, and false-positive warning floods. |

---

## Invariant Safety Pre-Flight

1. **Capability Slot Invariants (`METRICS_RUNTIME`)**:
   - Registered in capability slot census via `metrics_runtime_slot()` and verified by `the_accessor_exposes_this_handle_to_the_roster`.
   - `MissingSemantics::IndistinguishableDefault` tied directly to `DEFAULT_WARNING_MULTIPLIER` (2.0x).
2. **Timing Math & Saturation (`warning_threshold_ms`)**:
   - Preserves saturation to `u64::MAX` on float overflow (`raw >= u64::MAX as f64`).
   - Sourced canonical `DEFAULT_WARNING_MULTIPLIER` from `crate::config::types::policies::metrics`.
3. **Public API & Downstream Compatibility**:
   - `StageTimer::start`, `with_meta`, `with_target`, `elapsed_ms` signatures and return types are completely unchanged.
   - Preserves benchmark harness integration in `benches/performance_benchmarks.rs`.

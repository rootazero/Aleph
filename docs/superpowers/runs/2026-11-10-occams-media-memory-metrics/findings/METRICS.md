# Occam's Razor Findings & Remediation: Batch METRICS

**Module**: `src/metrics/mod.rs` (436 LOC)  
**Date**: 2026-11-10  
**Doctrine**: `/rust-occams-razor`

---

## 1. Executive Summary

Static review of `src/metrics/mod.rs` verified `StageTimer` RAII latency tracking and `CapabilitySlot<MetricsRuntime>` concurrency semantics.
The following Occam's Razor simplifications were applied:
1. **Control Flow Flattening & Early Short-Circuit (`StageTimer::drop`)**: Short-circuit immediately on `rt.enable_warnings` and filter `target_ms > 0`, avoiding unnecessary floating-point multiplication (`rt.warning_threshold_ms(target_ms)`) when warnings are globally disabled.
2. **Accidental Complexity Elimination (`debug!` logging)**: Removed dead `.filter(|m| !m.is_empty())` on `self.metadata` (since `metadata` is only allocated when inserting key/value pairs) and unified field format with `warn!`.
3. **Dead Attribute Removal**: Removed redundant `#[must_use]` on module-private `MetricsRuntime::warning_threshold_ms`.
4. **Zero-Allocation Test Assertions**: Replaced `Some(&"val".to_string())` heap allocations across unit tests with `.map(String::as_str) == Some("val")`.

All public contracts, capability slot census invariants, and benchmark integrations remain preserved.

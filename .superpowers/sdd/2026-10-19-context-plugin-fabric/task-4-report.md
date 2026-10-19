# Task 4 implementation report — fold ownership

Status: implementation and independent review complete; combined cargo verification pending. No test pass is claimed here.

## Changes / 变更

- `src/session/events.rs`: optional serde-compatible FoldRecorded.owner; absent/None preserves historical events.
- `src/context/compact/folds.rs`: owner-preserving FoldRecord and recovery route; None/builtin uses builtin, foreign owner is kept explicit; recorded spans suppress duplicate legacy derivation without erasing distinct explicit owners.
- `src/context/compact/manual.rs`: the builtin owner is stamped in the existing atomic CompactionPerformed/FoldRecorded event batch.
- `src/builtin_tools/decompress.rs`: historical owner determines routing; an unavailable foreign adapter reports degraded Core recovery, bounded by the existing pagination/byte and soft/hard-retirement checks. Actual external dispatch is a Task 6 integration seam.
- `src/diagnostics/checks/fold_economics.rs`, `src/harness/agent/think.rs`, `src/context/compact/fold_ledger.rs`: migrate existing constructors/patterns without new behavior; preserve native metrics and event order.

## Review / 审查

Independent reviewer `7c41db8e-56ec-459` APPROVED: optional serde compatibility, same-batch owner persistence, recorded-vs-derived deduplication, historical-owner routing and Retire::From denial checked. No Critical/Important found in the scoped review. Reviewer did not run cargo or a whole-repo constructor search.

## Verification / 验证

Combined Task 1/2/4 tests run under `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0` with >=4GiB MemAvailable before each cargo invocation. The first compilation caught an omitted `owner: None` in the fold-ledger test constructor, now fixed. The retry is still compiling; final results will be appended before acceptance.

## Remaining scope / 未做

External-owner adapter invocation and lifecycle notifications remain Tasks 5/6. This task provides provenance and explicit Core fallback, not a second recovery store. No full-suite result or arbitrary-duplicate-fold-id guarantee claimed.

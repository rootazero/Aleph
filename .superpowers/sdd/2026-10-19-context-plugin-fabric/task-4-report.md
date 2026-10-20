# Task 4 implementation report — fold ownership

Status: implementation and independent review complete; targeted fold/manual/ledger, decompress, diagnostics and event compatibility tests passed. A pre-existing R10 line-count overrun remains assigned to Task 5.

## Changes / 变更

- `src/session/events.rs`: optional serde-compatible FoldRecorded.owner; absent/None preserves historical events.
- `src/context/compact/folds.rs`: owner-preserving FoldRecord and recovery route; None/builtin uses builtin, foreign owner is kept explicit; recorded spans suppress duplicate legacy derivation without erasing distinct explicit owners.
- `src/context/compact/manual.rs`: the builtin owner is stamped in the existing atomic CompactionPerformed/FoldRecorded event batch.
- `src/builtin_tools/decompress.rs`: historical owner determines routing; an unavailable foreign adapter reports degraded Core recovery, bounded by the existing pagination/byte and soft/hard-retirement checks. Actual external dispatch is a Task 6 integration seam.
- `src/diagnostics/checks/fold_economics.rs`, `src/harness/agent/think.rs`, `src/context/compact/fold_ledger.rs`: migrate existing constructors/patterns without new behavior; preserve native metrics and event order.

## Review / 审查

Independent reviewer `7c41db8e-56ec-459` APPROVED: optional serde compatibility, same-batch owner persistence, recorded-vs-derived deduplication, historical-owner routing and Retire::From denial checked. No Critical/Important found in the scoped review. Reviewer did not run cargo or a whole-repo constructor search.

## Verification / 验证

Combined validation uses `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0` and >=4GiB MemAvailable before each cargo invocation. The omitted fold-ledger test constructor was fixed.

Actual results: folds 9 passed, manual compaction 23 passed, fold ledger 5 passed in the context batch; decompression 9 passed; fold economics diagnostics 4 passed; session events 31 passed. The context batch's only failure was the separate Task 1 digest-boundary fixture, corrected for the next shared run.

R10: the exact twelve-file guard passed. The line guard failed at 5570 against CEILING 5545. Running the guard's exact count algorithm over git blobs proved the original baseline `79a2494f6`, Task 1 `c65f50d3a`, Task 2 `a97dc0f52` and Task 4 `b30a9c778` all have 5570 production lines: Task 4 added zero budgeted lines. This pre-existing 25-line overrun is recorded for real scaffolding extraction in Task 5; no ceiling was raised and the guard is not reported green.

## Remaining scope / 未做

External-owner adapter invocation and lifecycle notifications remain Tasks 5/6. This task provides provenance and explicit Core fallback, not a second recovery store. No full-suite result or arbitrary-duplicate-fold-id guarantee claimed.

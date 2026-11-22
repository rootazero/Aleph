# Phase8 / NSH Final SPEC Review / Phase8 / NSH 最终 SPEC 审阅

**Date / 日期:** 2026-11-15
**Repository / 仓库:** local `main`; planning-only review
**Verdict / 结论:** **PASS WITH RECORDED LIMITS**
**Reviewer / 审阅者:** independent read-only `reviewer` subagent, sequential after the parent repairs
**Scope / 范围:** Phase8 Integration & Security plan/matrix and NSH v0.5 migration inventory/upstream evidence/Panel parity spec/plan/R2 decision proposal

## Review basis / 审阅依据

The parent ran Graphify before the review and supplied the reviewer the result identifying `docs/superpowers/plans/2026-11-15-phase8-integration-security-and-nsh-restart.md` at L1. The reviewer then read the planning artifacts and the cited current-source contracts. It did not modify files, run Cargo, build, run tests, or perform network operations.

Reviewed artifacts:

- `docs/superpowers/plans/2026-11-15-phase8-integration-security-implementation.md`
- `docs/superpowers/runs/2026-11-15-phase8-security-evidence-matrix.md`
- `docs/superpowers/runs/2026-11-15-phase8-entry-inventory.md`
- `docs/superpowers/runs/2026-11-15-nsh05-migration-inventory.md`
- `docs/superpowers/runs/2026-11-15-nsh05-upstream-evidence.md`
- `docs/superpowers/specs/2026-11-15-nsh-panel-parity-slice.md`
- `docs/superpowers/plans/2026-11-15-nsh-panel-parity-slice.md`
- `docs/superpowers/specs/2026-11-15-nsh-r2-native-ui-decision.md`
- `docs/superpowers/plans/2026-11-15-phase8-integration-security-and-nsh-restart.md`

## Accepted review conclusions / 已接受结论

- F1–F5 have named ingress, mutation/barrier, terminal assertion, test target and future receipt paths.
- F2 preserves the crate boundary: the captured `OwnedPtySession` race is in the crate-internal `src/capability/zahir_facade.rs` seam and calls `ZahirFacade::invoke_terminal_gateway(..., captured: Some(captured))`; the public Gateway ingress remains a current-capture test. The existing provider-restart fixture relationship is recorded.
- F4 preserves the current subscription contract: caller cursors are ignored, subscriptions anchor to the current snapshot, lag requires invalidation and a fresh snapshot, and an unchanged lease is not rejected by lag alone.
- The pinned NSH v0.5 commit and exact HostChannel, EngineConfig/runtime, AppView, access/media, Window/WindowRef and declaration paths are recorded without machine-specific checkout paths.
- The Chat + Artifacts route has session/history, queued/accepted/streaming/terminal run states, `artifacts.list`, `session.artifact` invalidation/refetch, reconnect replacement snapshots and deduplication rules.
- Phase0–6 status and ADR-003–009 status are reconciled. R2 remains a proposed decision gate; detached daemon ownership is explicitly outside the first slice; Tauri/WebChat removal remains behind Phase4 parity and a two-week dual run.
- The performance protocol defines a controlled prebuild, application-level launch marker, process set, sampling, 10 warmups, 30 measurements, p95 rank 29, thresholds, schema and exit codes.
- Planning artifacts explicitly remain non-runtime evidence. No implementation, dependency migration, lockfile change, Tauri removal, release or push is authorized by this review.

## Repairs made during review / 审阅期间修复

The independent review initially found blockers in cursor semantics, the F2 capture seam, incomplete v0.5 API mapping, incomplete Artifacts/run data flow, missing Phase0–6/ADR reconciliation, incomplete performance reproducibility, invalid commands, portable path leakage, stale source anchors and an under-specified performance launcher. The parent repaired those items in the planning documents.

After the final independent verdict, two P2 source-anchor corrections were applied mechanically:

- `docs/superpowers/runs/2026-11-15-phase8-security-evidence-matrix.md` now cites `src/gateway/capability_terminal_tests.rs:1157` without a malformed double-colon line reference.
- `docs/superpowers/plans/2026-11-15-nsh-panel-parity-slice.md` now cites `interfaces/webchat/src/components/artifacts/mod.rs:250-257` for the session-change/refetch path and `:271-279` for invalidation handling.

The final documents were checked for stale absolute workspace paths, the old receipt filename, the old F2 symbol, the old queued/artifact anchors and invalid `bun check`/root `bun run check` references. No Cargo or runtime validation was performed as required for this planning stage.

## Recorded limits / 记录的限制

This PASS WITH RECORDED LIMITS is a planning verdict. It does not prove NSH v0.5 compilation or runtime behavior, canonical Panel projection, security test results, performance thresholds, cross-platform support, Phase4 parity, the two-week dual run, R2 approval, or Tauri removal. The proposed harness, schema, Phase8 integration target and execution receipts do not yet exist. The current shell remains the old v0.4/absolute-path/mock implementation described by the migration inventory.

The next gate is the independent QUALITY review of the final planning package. After QUALITY review, work stops for human approval before any production implementation or NSH dependency migration.

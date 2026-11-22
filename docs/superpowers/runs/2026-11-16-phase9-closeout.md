# Phase9 closeout / Phase9 结项

**Phase9 PASS; Phase10 READY. / Phase9 完整验收通过，Phase10 准入条件已满足。** Scope is the approved macOS debug slice. Final evidence SPEC → different-model QUALITY completed without blocking findings; admission preserves every documented limit.

## Requirement audit / 逐项验收

| Gate | Result | Authoritative evidence |
|---|---|---|
| A — tests | PASS: 54 terminal invocations; 22,402 passed, zero failed, 20 existing ignored | [Test matrix](2026-11-16-phase9-test-matrix-verified.json) |
| B — dependencies | PASS: real resolver 1,572-entry lock; pinned NSH revision | [Dependency decision](2026-11-16-phase9-dependency-decision.md) |
| C — existing Panel runtime | PASS: canonical Chat/stream/Artifacts/reconnect and real ingress guards; Panel 1,568, Tauri 190, NSH 5 tests | [Acceptance receipt](2026-11-18-phase9-acceptance.json) |
| D — debug parity | PASS: complete fresh 80 runs, original reducer VALID / ACCEPTED, independent immutable audit PASS | [Original parity receipt](2026-11-18-phase9-parity-receipt.json) |
| E — final review | PASS: source and final evidence SPEC → different-model QUALITY; no blocking findings | [Machine receipt](2026-11-18-phase9-acceptance.json) |

A's retained graph run took 656.84 s. B's adopted graph has its own full 22,402 / 0 / 20 run in 661.14 s; these are separate executions. / 不以 retained 图测试替代 adopted 图测试。

## Final performance block / 最终性能块

`isolated-matrix-40-20261118T185504`: Tauri first 40, then NSH 40; each 10 warmups + 30 measured; p95 one-based rank 29. Each round owns a fresh Gateway/provider/session fixture. Original reducer and limits remain unchanged.

| Metric | Tauri median | NSH median | NSH comparison | Result |
|---|---:|---:|---:|---|
| `first_frame_ms` | 5357.9510 | 4933.0425 | 0.9207× | PASS |
| `route_ms` | 33.0000 | 33.0000 | 1.0000× | PASS |
| `input_ms` | 198.5000 | 198.5000 | 1.0000× | PASS |
| `reconnect_ms` | 1041.5000 | 1022.5000 | 0.9818× | PASS |
| `close_ms` | 47.6790 | 48.9210 | 1.0260× | PASS |
| `rss_mb` | 678.1016 | 640.8438 | 0.9451× | PASS |
| `threads` | 76.0000 | 68.0000 | -8 threads | PASS |

Latency values are ms; RSS is MB. Close p95 is 77.444 → 61.365 ms. Every latency median ≤1.10× and p95 ≤1.20×, RSS median ≤1.10×, thread median delta ≤2. All seven metrics pass.

The immutable auditor SHA-256 `1d358b50a1417c9ccad34e3fceaa7a042d44485355c8b7bd5b6c313865dfdc94` matched before and after execution, exit 0, `failures: []`. It revalidated 80 raw captures, exact order, 80 unique fixture roots/run IDs, same assets/binaries, complete observed coalition resources and terminal settlement, and independently re-exported metrics and reran the unchanged reducer. / 独立重验未拼接旧轮次。

## Changes and boundaries / 变更与边界

NSH hosts the existing Leptos/WASM Panel; business state and Gateway transport remain there. QA uses public NSWorkspace per-instance coalition ownership, shared monotonic native/resource clocks, browser event timestamps for route/input/reconnect, actual kernel native exit status, and all observed member settlement. Every final catalog and hashed terminal tail uses the same validator, including when the host record disappears. Public `quit_app()` exercises standalone application exit and retains engine/window shutdown plus `run_app` return.

The first complete `174308` block is preserved as VALID / NOT_ACCEPTED (close median ratio 1.13434). All invalid and interrupted blocks remain in the private historical inventory. / 保留历史失败、审查撤回与原始日志，不改判、不拼接。

## Integrity and handoff / 完整性与交接

[Current 52-file source manifest](2026-11-18-phase9-source-manifest.json), [recursive default asset manifest](2026-11-18-phase9-default-manifest.json), and [final machine integrity](2026-11-16-phase9-final-integrity.json) bind the acceptance evidence. Nine default files restored exactly; default WASM/JS have no QA ingress, reconnect callback or history witness. Base/HEAD stays `3892804c112fd6e574166a4c03a7c40a77d0b239`, branch `runtime-proof`, work uncommitted. Protected config and AGENTS/CLAUDE git status is unchanged.

Private raw evidence is retained under `.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/isolated-matrix-40-20261118T185504`; prior authority files are archived in `.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/pre-final-authority-20261118T1913`. [Phase10 entry brief](2026-11-18-phase10-entry-brief.md) states the admission boundary.

## Recorded limits / 未做与边界

- Debug macOS slice only; no release, other-platform, full embedded-core package or global workspace/all-features/clippy acceptance.
- Application exit is covered; user WindowClose callbacks are not established by the QA quit path.
- Resource settlement covers all members visible in retained coalition samples and terminal catalogs; sub-interval unsampled members are not inventoried. Unrelated applications remain outside the target name scope.
- Terminal absence receipts record contemporaneous public OS observations; they are not proof against rewriting an entire evidence set.
- Queue coverage uses a separate real Gateway wire → actual Panel decoder/dispatcher/ChatState integration; performance rounds honestly report queue not observed.
- Twenty existing Core ignored tests, existing whole-repository rustfmt differences, duplicate synthetic tail fixture maintenance and a dedicated final-listing exception regression remain recorded limits; no new skips were added.
- No commit/merge/push/release, root/private API, OS policy change, detached daemon takeover, Tauri/WebChat removal, Safe Replay or external-effect exactly-once claim.

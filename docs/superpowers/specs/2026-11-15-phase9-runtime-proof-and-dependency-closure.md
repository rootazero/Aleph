# Phase9 Candidate Specification: Runtime Proof & Dependency Closure
# Phase9 候选规格：运行时证据与依赖闭合

**Status / 状态:** planning proposal only; production implementation deferred to the next session
**Precondition / 前置:** Phase8 `PASS WITH RECORDED LIMITS`

## Problem / 问题

Phase8 established the bounded capability security slices and current-head build provenance. The candidate Phase9 scope carries forward four unresolved evidence edges: the full alephcore test process has no terminal result, the NSH v0.5 shell has no live native Gateway/window proof, the Cargo.lock transition has broad resolver churn, and the approved **debug QA** performance receipt has not run. Release-profile performance remains a separate future gate and is intentionally recorded as `RECORDED_LIMIT` here.

Phase9 must close the debug integration evidence edges with real terminal outcomes while preserving current authorities and the existing R2 boundary.

Phase8 已建立受控 capability security 切片与当前 HEAD 构建 provenance。候选 Phase9 承接四条未闭合证据边界：alephcore 全量测试没有终态、NSH v0.5 shell 没有 live native Gateway/window 证据、Cargo.lock 发生较大 resolver churn，以及批准的 **debug QA** 性能 receipt 尚未执行。release-profile 性能属于独立的未来闸门，本候选阶段明确记录为 `RECORDED_LIMIT`。

Phase9 应在保持现有 authority 与 R2 边界的前提下，关闭 debug integration 证据并产生真实终态。

## Observable contract / 可观察合同

A timeout is classified separately from its cause and is never silently green. Use these terminal states:

- `PASS`: the check completed and its acceptance predicate passed;
- `RECORDED_LIMIT`: the check completed but the approved scope intentionally excludes its conclusion, such as release-profile performance;
- `BLOCKED`: a required environment, dependency or external service was unavailable, and the implementation contract was not disproven;
- `FAIL`: source evidence or a completed run proves that the implementation violated an acceptance predicate, including a lifecycle/teardown contract;
- `UNRESOLVED_TIMEOUT`: the process stopped without a terminal result and the cause is not yet proven. This state blocks closeout and cannot be converted to `BLOCKED` merely to make the report green.

A valid measurement that misses a threshold is `VALID` plus `NOT_ACCEPTED`, never an invalid receipt.

timeout 必须与原因分开分类，不能静默算绿：

每个 Phase9 测量面必须产生明确终态：`PASS`、`RECORDED_LIMIT`、`BLOCKED`、`FAIL` 或阻塞 closeout 的 `UNRESOLVED_TIMEOUT`。缺失环境/依赖且没有证据证明实现违反合同，才是 `BLOCKED`；已经证明 lifecycle/teardown 违反合同，必须是 `FAIL`。有效但未达阈值的结果必须表示为 `VALID` + `NOT_ACCEPTED`，不能伪装成 invalid receipt。

## Work surfaces / 工作面

### Full test closure / 全量测试闭合

Use the existing `alephcore` lib test binary and preserve the six Phase8 focused tests as positive controls. The Phase8 execution receipt records the original full-suite run and its nine long-running tests; the next session must copy the exact log into its run artifact rather than depend on a temporary `/tmp` path. The nine tests are:

- `extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind`
- `extension::claude_cache_tests::an_unreadable_index_forgets_nothing`
- `extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history`
- `extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy`
- `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`
- `extension::claude_cache_tests::the_model_may_disable_but_not_enable_a_claude_cache_row`
- `extension::claude_cache_tests::the_rpc_face_enables_and_disables_a_claude_cache_row`
- `extension::claude_cache_tests::two_broken_claude_code_plugins_at_one_version_are_two_error_rows`
- `extension::context_adapter::tests::lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement`

The candidate receipt is `docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md`. Each test must name its source file, exact filter command, timeout, fixture/environment, final stdout/stderr, and classification. A full-suite timeout remains `UNRESOLVED_TIMEOUT` until these per-test runs prove `PASS`, `BLOCKED` or `FAIL`; a source-backed teardown defect is `FAIL`; `BLOCKED` requires evidence that a required environment, dependency or external service is unavailable. An unexplained wait remains `UNRESOLVED_TIMEOUT` and continues to block closeout. `BLOCKED` 必须有环境、依赖或外部服务不可用的证据；原因未证明的等待继续记为 `UNRESOLVED_TIMEOUT`。

Do not hide slow tests with a skip list and call the suite green.

### Dependency closure / 依赖闭合

Audit the generated `Cargo.lock` against a frozen pre-NSH baseline, not a future moving `HEAD`. The baseline is the lock blob from the Phase8 migration starting point: commit `e08af3789dbf194f02f08461c215eb3c7701557f`, file `Cargo.lock`, SHA-256 to be recorded before any next-session change. The retained generated lock must also record its SHA-256 and the NSH pin `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.

The Phase9 receipt must contain machine-readable package identity deltas for the frozen original migration (`added`, `removed`, `changed`) and a separate baseline for any Phase9 candidate. The acceptance predicate is a resolver-valid, reproducible `--locked` graph with an explicit decision about broad version churn. No hand-edited lock splice is accepted without `cargo metadata --locked --offline --no-deps` plus target-package compile/test evidence against the candidate lock.

### Live NSH proof / NSH live 证据

Use the approved debug-only QA profile. The launcher command is `python3 qa/nsh_panel_launcher.py --repo-root . --target-dir .phase9-target --fixture-root qa/fixtures/nsh-panel-parity --receipt docs/superpowers/runs/2026-11-15-phase9-nsh-runtime-receipt.json`, run from the repository root after the debug-only build. It derives executable paths from the passed repository/target paths and launches with Python argv arrays and `shell=False`. The Tauri child uses the existing `--qa-root`/`--qa-nonce` owned-root protocol; the first NSH child has no such internal contract and is supervised by launcher-level pid/exit/cleanup evidence, consistent with the NSH parity plan. No repository absolute path or shell-variable expansion is part of the contract.

The first live slice must prove:

1. v0.5 macOS window opens through the pinned `AppView`/`run_app` boundary;
2. a canonical Chat `/` request reaches Gateway and produces accepted/stream/complete state;
3. `session.artifact` causes `artifacts.list` refresh for the matching session;
4. reconnect rebuilds state without accepting stale session/artifact data;
5. the fixed runtime schema validates lifecycle events and its value-only metrics feed the existing `qa/nsh_panel_parity.py` reducer;
6. the `closed` barrier records both shell child exit codes, no remaining Tauri child under its owned root, and no remaining NSH launcher-supervised child.

A valid threshold miss is `valid: true` plus `threshold_status: NOT_ACCEPTED`; `invalid_reason` is only for invalid measurements and `BLOCKED` only for unavailable environment/dependency. This does not authorize daemon ownership or Tauri removal.

### Performance / 性能

The debug QA measurement keeps the approved order Tauri first, NSH second; ten warmups followed by thirty measured runs; 100ms RSS/thread sampling; p95 rank 29 one-based; latency median/p95 limits 1.10x/1.20x, RSS median 1.10x and thread median +2. It closes only debug integration/performance evidence. Release-profile performance remains a separate future gate and is a deliberate `RECORDED_LIMIT`, not an unresolved Phase9 acceptance predicate.

## Authority and safety boundaries / authority 与安全边界

- `Zahir`, `ZahirFacade`, `OwnershipTree`, `PolicyEngine`, existing event/projection/UI/PTY authorities remain canonical.
- No second registry, descriptor map, owner map or runtime authority.
- R2 remains in force: NSH owns native shell/lifecycle/transport; Panel/Leptos/WASM remains business UI authority until a separate ADR changes that boundary.
- The detached Gateway/daemon remains external to the NSH process ownership tree.
- No new public wire API is proposed by this specification.

## Acceptance / 验收

Phase9 planning is complete only when the implementation plan names exact files, commands, barriers, receipts, rollback boundaries and terminal states. Phase9 implementation is complete only after the approved live evidence exists; this session does not execute it.

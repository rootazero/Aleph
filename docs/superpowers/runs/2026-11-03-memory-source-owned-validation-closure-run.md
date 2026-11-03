# Memory source-owned validation closure run / Memory source-owned 验证闭合运行记录

- Date / 日期: 2026-11-03
- Status / 状态: In progress (Task 1 started) / 进行中（已开始 Task 1）
- Branch / 分支: local `main`; no remote operation authorized / 本地 `main`；未授权远端操作
- Plan / 计划: [validation closure plan](../plans/2026-11-03-memory-source-owned-validation-closure-plan.md)
- Prior evidence / 前序证据: [source-owned boundary run](./2026-11-03-memory-source-owned-boundary-run.md)
- Scope / 范围: validation only — closes Admin HTTP effect-test gaps where existing deterministic fixtures allow, and attributes the previous full-suite failures to the pinned pre-slice baseline / 仅验证——在现有确定性 fixture 允许的范围内补齐 Admin HTTP 效果测试，并将前序全量失败归因到固定 pre-slice 基线

## Starting state / 起始状态

- Git root: `/Volumes/TBU4/Workspace/Aleph`; branch `main`; single worktree; clean tree.
- Task-start HEAD: `98b71deb4 docs(memory): plan validation closure`. Prior slice commits `a4fa8088f` (docs), `07d9f43ce`, `c412be694`, `7ae4ee417` (impl) are ancestors; `4b6da265a` is an ancestor.
- Baseline object to compare against: `063345f2f30bf999b28ee5d84aac860be8432fa7` (pinned pre-slice tip; local object only, no fetch).
- Memory gate verified before the first Cargo command: `python qa/browser_live/check_memory.py` → `available_bytes=6770262016 required_bytes=4294967296 ALLOW`.
- Prior slice residual gaps (the ones this task targets / 本任务针对的前序残余缺口):
  1. No deterministic HTTP effect evidence for the `204 No Content` branch (owned reconciler's first scan).
  2. No HTTP sweep of the unavailable runtime states (`Starting` / `Draining` / `Failed`) — accessor-level evidence only.
  3. No report-payload differentiation across a restart — only handler identity was asserted.

## Task 1 — Admin API effect evidence / Admin API 效果证据

New deterministic real-router evidence, all through the real `crate::gateway::admin_api::router` with the real `MemoryRuntime` and the real runtime-owned reconciler daemon. No new public or production test-control API was added; no sleep, no scheduler race, no global mock. / 全部通过真实 admin router、真实 `MemoryRuntime` 与真实 runtime-owned reconciler daemon 观测；未新增公开或生产 test-control API，无 sleep、无调度竞态、无全局 mock。

| Behavior / 行为 | Test / 测试 | Evidence / 证据 |
|---|---|---|
| `204` while a current reconciler has not published a report / 当前 reconciler 尚未发布报告时返回 `204` | `gateway::admin_api::reconciler::tests::get_latest_returns_204_until_the_owned_reconciler_publishes_a_report` | The owned daemon's real first scan is blocked inside the shared `StateDatabase` SQLite handle (crate-visible `std::sync::Mutex`), so no report can exist yet; the router answers `204`, and after the lock is released the same router answers `200` once the daemon publishes. |
| `503` while the runtime is Draining / runtime 排空中返回 `503` | `gateway::admin_api::reconciler::tests::get_latest_returns_503_while_the_runtime_is_draining` | A real admitted operation keeps the drain open; the request is served while `status().state == Draining` and answers `503`. |
| `503` while the runtime is Starting / runtime 启动中返回 `503` | `memory::runtime::tests::admin_reconciler_endpoint_is_unavailable_while_the_runtime_is_starting` | The runtime is pinned in `Starting` by the existing cfg(test) initialization gate; the router answers `503`. |
| `503` after a real stop failure (Failed) / 真实停止失败后（Failed）返回 `503` | `memory::runtime::tests::admin_reconciler_endpoint_is_unavailable_after_a_real_stop_failure` | A genuine unfinalized SQLite statement makes `Connection::close` return `SQLITE_BUSY`, so `stop()` fails and the runtime stays `Failed`; the router answers `503`. |
| A restart serves only the current generation's report / 重启后只提供当前代的报告 | `gateway::admin_api::reconciler::tests::get_latest_serves_only_the_current_generation_report_after_restart` | A real fact is written through the production path between the two generations; the first generation reported `scanned_facts == 0`, the restarted generation's response reports `scanned_facts == 1` — the difference is visible in the response body, not in handler identity. |

Command and counts (memory gate `ALLOW` before each Cargo command; `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; `--test-threads=1`) / 命令与计数：

- `cargo test -p alephcore --lib gateway::admin_api::reconciler::tests -- --test-threads=1` → **7 passed; 0 failed; 21903 filtered out** (1.48s). Selected: `get_latest_returns_204_until_the_owned_reconciler_publishes_a_report`, `get_latest_returns_503_while_the_runtime_is_draining`, `get_latest_serves_only_the_current_generation_report_after_restart`, `get_latest_returns_503_after_runtime_stop`, `get_latest_returns_503_when_no_handler_wired`, `get_latest_returns_503_when_reconciler_disabled`, `get_latest_returns_clean_report_after_scan`.
- `cargo test -p alephcore --lib memory::runtime::tests -- --test-threads=1` → **16 passed; 0 failed; 21894 filtered out** (2.40s). Selected: `admin_reconciler_endpoint_is_unavailable_while_the_runtime_is_starting`, `admin_reconciler_endpoint_is_unavailable_after_a_real_stop_failure`, `stop_error_is_sticky_but_cleanup_retry_reaches_off`, `admin_reconciler_state_matrix_is_fail_closed`, `admin_reconciler_resolves_current_runtime_generation_and_rejects_stale_report`, `blocking_work_outlives_cancelled_request`, `cancelled_start_keeps_partial_resources_until_stop_finishes`, `cancelled_stop_keeps_transition_and_exact_task_ids`, `child_panic_keeps_the_first_error_and_observes_its_sibling`, `cold_off_opens_no_memory_resources`, `concurrent_stop_waiters_share_one_drain`, `failed_initialization_closes_the_partial_backend`, `restart_never_revives_old_handle`, `restart_reads_the_canonical_activation_snapshot`, `running_reaper_observes_children_and_clones_share_one_admission`, `starting_factory_restores_dream_health_before_public_admission`.

Test-only support changes / 仅测试支撑改动：

- `src/gateway/admin_api/mod.rs`: one `#[cfg(test)] pub(crate) fn test_admin_state(...)` next to the existing `#[cfg(test)] pub(crate) fn test_session_store(...)`, so the admin router fixture is shared by the gateway tests and the Memory runtime tests instead of duplicated. It does not exist in production builds.
- `src/memory/runtime/tests.rs`: the existing unfinalized-statement setup was extracted into `fn unfinalized_statement(backend: &SqliteMemoryBackend) -> BusyStatement` and reused by the new Failed-state test; `stop_error_is_sticky_but_cleanup_retry_reaches_off` keeps its original assertions.

Production change / 生产改动: **none** — no endpoint contract defect was observed, so no production code was modified / 未观察到端点契约缺陷，因此未修改任何生产代码。

Remaining gaps / 剩余缺口: the `204` evidence blocks the owned daemon's scan at the event-log read rather than inside the reconciler handler (the handler's own cfg(test) gate cannot be installed on the runtime-created handler); `duration_ms` is not asserted in the restart test because the restarted daemon may publish concurrently with the test's own scan; the `Failed`-state HTTP evidence still relies on a real SQLite close failure rather than an injected lifecycle fault / `204` 证据把 owned daemon 的扫描阻塞在事件日志读取处，而非 reconciler handler 内部（handler 自身的 cfg(test) gate 无法安装到 runtime 创建的 handler 上）；重启测试不断言 `duration_ms`，因为重启后的 daemon 可能与测试自身的扫描并发发布；`Failed` 状态的 HTTP 证据仍依赖真实 SQLite close 失败，而非注入的生命周期故障。

## Task 2 — Full-suite failure attribution / 全量失败归因

- Pending.

## Task 3 — Final evidence, review, delivery / 最终证据、审阅与交付

- Pending.

---

**Historical evidence boundary / 历史证据边界:** the prior slice's test totals and its aggregate `21834 passed; 51 failed; 19 ignored` figure are recorded in the prior run; they are context for this comparison, not new evidence / 前序切片的测试总数与聚合计数记录在前序运行记录中，仅作为本次对照背景，不算本切片的新证据。

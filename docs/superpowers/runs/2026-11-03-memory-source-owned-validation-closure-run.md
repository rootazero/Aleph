# Memory source-owned validation closure run / Memory source-owned 验证闭合运行记录

- Date / 日期: 2026-11-03
- Status / 状态: Complete — Tasks 1–3 delivered locally; the delivery commit is the local commit that carries this record / 已完成——Task 1–3 已本地交付；交付提交即携带本记录的那次本地提交
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

New deterministic real-router evidence, all through the real `crate::gateway::admin_api::router` with the real `MemoryRuntime` and the runtime-owned reconciler handler. Only the `204` evidence is driven by the **owned daemon** (its real first scan is what gets blocked); the clean-report and restart-payload assertions use direct `reconcile_once()` scans on the same runtime-owned handler, so they prove the endpoint's report contract, not daemon scheduling. No new public or production test-control API was added; no sleep, no scheduler race, no global mock. / 全部通过真实 admin router、真实 `MemoryRuntime` 与 runtime-owned reconciler handler 观测；仅 `204` 证据由 **owned daemon** 驱动（被阻塞的是它的真实首轮扫描），clean-report 与 restart-payload 断言使用同一 runtime-owned handler 上的直接 `reconcile_once()` 扫描，因此它们证明端点的报告契约，而非 daemon 调度。未新增公开或生产 test-control API，无 sleep、无调度竞态、无全局 mock。

| Behavior / 行为 | Test / 测试 | Evidence / 证据 |
|---|---|---|
| `204` while a current reconciler has not published a report / 当前 reconciler 尚未发布报告时返回 `204` | `gateway::admin_api::reconciler::tests::get_latest_returns_204_until_the_owned_reconciler_publishes_a_report` | The owned daemon's real first scan is blocked inside the shared `StateDatabase` SQLite handle (crate-visible `std::sync::Mutex`), so no report can exist yet; the router answers `204`, and after the lock is released the same router answers `200` once the daemon publishes. |
| `503` while the runtime is Draining / runtime 排空中返回 `503` | `gateway::admin_api::reconciler::tests::get_latest_returns_503_while_the_runtime_is_draining` | A real admitted operation keeps the drain open; the request is served while `status().state == Draining` and answers `503`. |
| `503` while the runtime is Starting / runtime 启动中返回 `503` | `memory::runtime::tests::admin_reconciler_endpoint_is_unavailable_while_the_runtime_is_starting` | The runtime is pinned in `Starting` by the existing cfg(test) initialization gate; the router answers `503`. |
| `503` after a real stop failure (Failed) / 真实停止失败后（Failed）返回 `503` | `memory::runtime::tests::admin_reconciler_endpoint_is_unavailable_after_a_real_stop_failure` | A genuine unfinalized SQLite statement makes `Connection::close` return `SQLITE_BUSY`, so `stop()` fails and the runtime stays `Failed`; the router answers `503`. |
| A restart serves only the current generation's report / 重启后只提供当前代的报告 | `gateway::admin_api::reconciler::tests::get_latest_serves_only_the_current_generation_report_after_restart` | A real fact is written through the production path between the two generations; the first generation's scan reported `scanned_facts == 0`, the restarted generation's response reports `scanned_facts == 1` — the difference is visible in the response body, not in handler identity. Both generations are scanned directly on their runtime-owned handler; the restarted daemon may publish concurrently, which is why the assertion avoids `duration_ms`. |

Command and counts (memory gate `ALLOW` before each Cargo command; `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; `--test-threads=1`) / 命令与计数：

- `cargo test -p alephcore --lib gateway::admin_api::reconciler::tests -- --test-threads=1` → **7 passed; 0 failed; 21903 filtered out** (1.48s). Selected: `get_latest_returns_204_until_the_owned_reconciler_publishes_a_report`, `get_latest_returns_503_while_the_runtime_is_draining`, `get_latest_serves_only_the_current_generation_report_after_restart`, `get_latest_returns_503_after_runtime_stop`, `get_latest_returns_503_when_no_handler_wired`, `get_latest_returns_503_when_reconciler_disabled`, `get_latest_returns_clean_report_after_scan`.
- `cargo test -p alephcore --lib memory::runtime::tests -- --test-threads=1` → **16 passed; 0 failed; 21894 filtered out** (2.40s). Selected: `admin_reconciler_endpoint_is_unavailable_while_the_runtime_is_starting`, `admin_reconciler_endpoint_is_unavailable_after_a_real_stop_failure`, `stop_error_is_sticky_but_cleanup_retry_reaches_off`, `admin_reconciler_state_matrix_is_fail_closed`, `admin_reconciler_resolves_current_runtime_generation_and_rejects_stale_report`, `blocking_work_outlives_cancelled_request`, `cancelled_start_keeps_partial_resources_until_stop_finishes`, `cancelled_stop_keeps_transition_and_exact_task_ids`, `child_panic_keeps_the_first_error_and_observes_its_sibling`, `cold_off_opens_no_memory_resources`, `concurrent_stop_waiters_share_one_drain`, `failed_initialization_closes_the_partial_backend`, `restart_never_revives_old_handle`, `restart_reads_the_canonical_activation_snapshot`, `running_reaper_observes_children_and_clones_share_one_admission`, `starting_factory_restores_dream_health_before_public_admission`.

Test-only support changes / 仅测试支撑改动：

- `src/gateway/admin_api/mod.rs`: one `#[cfg(test)] pub(crate) fn test_admin_state(...)` next to the existing `#[cfg(test)] pub(crate) fn test_session_store(...)`, so the admin router fixture is shared by the gateway tests and the Memory runtime tests instead of duplicated. It does not exist in production builds.
- `src/memory/runtime/tests.rs`: the existing unfinalized-statement setup was extracted into `fn unfinalized_statement(backend: &SqliteMemoryBackend) -> BusyStatement` and reused by the new Failed-state test; `stop_error_is_sticky_but_cleanup_retry_reaches_off` keeps its original assertions.

Production change / 生产改动: **none** — no endpoint contract defect was observed, so no production code was modified / 未观察到端点契约缺陷，因此未修改任何生产代码。

Remaining gaps / 剩余缺口: the `204` evidence blocks the owned daemon's scan at the event-log read rather than inside the reconciler handler (the handler's own cfg(test) gate cannot be installed on the runtime-created handler), so it proves "a current reconciler without a published report" rather than "the scan has not started"; the clean-report and restart-payload rows are direct `reconcile_once()` scans, not daemon-driven, so daemon tick timing is not pinned by them; `duration_ms` is not asserted in the restart test because the restarted daemon may publish concurrently with the test's own scan; the `Failed`-state HTTP evidence still relies on a real SQLite close failure rather than an injected lifecycle fault / `204` 证据把 owned daemon 的扫描阻塞在事件日志读取处，而非 reconciler handler 内部（handler 自身的 cfg(test) gate 无法安装到 runtime 创建的 handler 上），因此它证明的是"存在当前 reconciler 但尚未发布报告"，而非"扫描尚未开始"；clean-report 与 restart-payload 两行是直接 `reconcile_once()` 扫描而非 daemon 驱动，因此它们没有钉住 daemon 的 tick 时序；重启测试不断言 `duration_ms`，因为重启后的 daemon 可能与测试自身的扫描并发发布；`Failed` 状态的 HTTP 证据仍依赖真实 SQLite close 失败，而非注入的生命周期故障。

## Task 2 — Full-suite failure attribution / 全量失败归因

Two revisions were compared with the identical command, on the same host, with serial execution (`--test-threads=1`), `CARGO_BUILD_JOBS=1`, `CARGO_INCREMENTAL=0`, and the mandatory memory gate in front of every Cargo invocation. The two suites were never running at the same time. / 两个版本使用完全相同的命令、同一主机、串行执行；每次 Cargo 调用前都通过强制内存门；两套测试从未并发运行。

| Revision / 版本 | Where / 位置 | Command / 命令 | Result / 结果 |
|---|---|---|---|
| Current `main` at `2fcd93970` (task-start `98b71deb4` plus the Task 1 test commit) / 当前 `main` | primary checkout `/Volumes/TBU4/Workspace/Aleph` | `cargo test -p alephcore --lib -- --test-threads=1` | `21840 passed; 51 failed; 19 ignored; 0 measured; 0 filtered out` (1235.22s, exit 101) |
| Pinned pre-slice baseline `063345f2f30bf999b28ee5d84aac860be8432fa7` / 固定 pre-slice 基线 | detached worktree `/tmp/aleph-validation-closure/baseline`, isolated `CARGO_TARGET_DIR=/tmp/aleph-validation-closure/baseline-target` | identical command | `21827 passed; 51 failed; 19 ignored; 0 measured; 0 filtered out` (1171.17s, exit 101) |

Memory gate readings (the check must print `ALLOW` with at least `4294967296` available bytes) / 内存门读数：start of the task `available_bytes=6770262016`, current-suite run `available_bytes=7023968256`, baseline run #1 `available_bytes=8399568896`, baseline run #2 also `ALLOW` (its byte reading was not transcribed). The check was re-run after the suites and printed `ALLOW` again.

Named-failure comparison (fully qualified test names, not aggregate counts) / 按完整测试名对照（不是聚合计数）:

- Failing at both revisions: **51** / 两版同时失败：**51**。
- Failing only at the current revision: **0** / 仅当前版本失败：**0**。
- Failing only at the baseline: **0** (after the baseline worktree was made comparable, see below) / 仅基线失败：**0**（在基线 worktree 可比化之后，见下）。

So every one of the 51 failures of the current tree also fails at the pinned pre-slice baseline, and the local delta introduced no new full-suite failure. 因此当前树的 51 项失败在固定 pre-slice 基线上同样失败；本地增量没有引入任何新的全量失败。

The 51 shared failing names / 共同失败的 51 个测试名：

```text
agents::swarm::tasks::acceptance::tests::criterion_newlines_cannot_break_out_of_the_list_item
agents::swarm::tasks::store::tests::test_acquire_and_release_lock
browser::cdp_backend::navigate::tests::a_new_loader_id_resets_the_tabs_ref_table
browser::cdp_backend::recording::tests::a_preexisting_stale_file_is_never_reported_complete
browser::cdp_backend::routes::tests::a_tab_rule_for_another_tab_does_not_arm_this_tab
browser::cdp_backend::routes::tests::closing_a_tab_drops_its_rules_and_its_loop
browser::cdp_backend::routes::tests::profile_rules_are_armed_before_the_tabs_first_navigation
browser::cdp_backend::routes::tests::removing_the_last_rule_disarms_the_tab
browser::cdp_backend::routes::tests::zero_rules_means_no_fetch_enable_and_no_loop
browser::cdp_backend::tabs::tests::an_empty_target_enumeration_does_not_hide_the_tab_we_created
browser::cdp_backend::tabs::tests::open_tab_records_the_tabs_identity_and_landed_url
browser::chromium_resolve::tests::a_finished_install_beats_a_newer_half_finished_one
browser::chromium_resolve::tests::the_newest_installed_sibling_wins_when_the_dry_run_directory_is_absent
browser::engine::registry::tests::a_surviving_displaced_engine_blocks_relaunch_and_keeps_its_record
browser::page_state::fetch_chromium::tests::a_grandchild_cross_origin_frame_is_carried_rather_than_refusing_the_page
browser::page_state::fetch_chromium::tests::a_grandchild_whose_id_collides_with_a_parent_element_is_still_confessed
builtin_tools::browser_tools::qa::tests::a_dimension_the_driver_cannot_serve_lands_in_unverified_not_pass
builtin_tools::browser_tools::qa::tests::a_failing_run_with_screenshot_requested_attaches_the_evidence
builtin_tools::browser_tools::qa::tests::a_met_expectation_with_all_checks_off_passes_cleanly
builtin_tools::browser_tools::qa::tests::a_passing_run_takes_no_evidence_screenshot
builtin_tools::browser_tools::qa::tests::a_selector_that_stays_fails_gone_selector
builtin_tools::browser_tools::qa::tests::evidence_tails_carry_only_the_new_lines
builtin_tools::browser_tools::qa::tests::failed_expectations_return_a_successful_tool_result_with_passed_false
builtin_tools::browser_tools::qa::tests::the_wire_shape_matches_the_spec
builtin_tools::browser_tools::runtime_manage::tests::install_returns_a_job_id_immediately_instead_of_awaiting_the_download
extension::lifecycle::tests::b3_cancelled_waiter_does_not_release_guest_permit_and_starting_stop_joins_without_deadlock
extension::lifecycle::tests::b3_publication_cleanup_error_retains_exact_failed_owner_evidence
extension::lifecycle::tests::b3_publication_failure_and_missing_producer_cleanup_fail_closed
extension::lifecycle::tests::b3_ready_real_guest_reload_and_old_receipt_are_incarnation_fenced
extension::lifecycle::tests::b4_production_canonical_reload_rejects_admitted_a_before_guest_and_runs_b
extension::lifecycle::tests::real_wasm_boot_concurrent_ensure_loaded_yields_one_owner_and_one_publication
extension::lifecycle::tests::stage_e_wasm_load_reads_guest_bytes_and_drops_exact_instance_after_unmount
extension::lifecycle::tests::stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner
extension::lifecycle::tests::stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation
extension::panel::tests::accepts_semver_build_metadata_in_the_canonical_entry
extension::projection::tests::publishing_plugin_projections_has_exactly_one_author
extension::tests::a_project_tool_survives_another_plugin_claiming_the_same_short_name
gateway::execution_engine::run_loop::tests::every_run_producer_answers_the_fire_time_authority_question
gateway::handlers::markdown_skills::tests::containment_is_component_wise
gateway::server::browser_viewer_route::tests::viewer_credential_cannot_cross_scope
gateway::server::browser_viewer_route::tests::ws_full_session_frames_input_and_revoke
gateway::server::browser_viewer_route::tests::ws_origin_mismatch_is_unauthorized
gateway::server::browser_viewer_route::tests::ws_rejects_malformed_binary_and_oversize_hello
gateway::server::browser_viewer_route::tests::ws_unknown_grant_is_unauthorized
gateway::server::browser_viewer_route::tests::ws_without_hello_times_out
looping::tests::commit_field_update_never_clobbers_owner_scope
looping::tests::commit_field_update_never_clobbers_the_telecom_origin
looping::tests::commit_field_update_preserves_live_pending_not_stale
looping::tests::commit_field_update_reschedule_clears_pending
memory::store::markdown::store_impl::tests::test_append_transcript
resilience::database::memory_events::tests::each_memory_event_reader_has_only_the_consumers_it_was_built_for
```

The first baseline run reported `21823 passed; 55 failed` with four extra names / 第一次基线运行为 `21823 passed; 55 failed`，多出四项：`config::tests::skill_doc_drift::no_bundled_snippet_hardcodes_the_aleph_config_path`, `config::tests::skill_doc_drift::the_bundled_self_skill_names_no_phantom_config_keys`, `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`, `gateway::execution_engine::btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question`. Their baseline panic messages identify them as artifacts of the fresh worktree, not code differences / 它们的基线 panic 信息表明这是新建 worktree 的环境产物，而非代码差异: "only 0 file(s) in BUNDLED_SKILLS — the `skills/` submodule is not checked out, so this guard is not running at all", "scanned /private/tmp/aleph-validation-closure/baseline/plugins and found no plugin manifests — the scan, not the plugins, is what broke", "the bundled walk parsed 0 skill(s) and 0 plugin director(ies) — `skills/` and `plugins/` are git submodules and this checkout has not initialised them". Both revisions pin the same submodule commits (`plugins` `b35ad47fe8eec3196b2dee70e51eadeacf3a620c`, `skills` `32c5b5ebde27863aed4f38ad23af223e83848e11`) and those objects already existed locally in `.git/modules`, so the baseline worktree was populated **offline** — `git config submodule.skills.url <local .git/modules/skills>`, `git config submodule.plugins.url <local .git/modules/plugins>`, `git -c protocol.file.allow=always submodule update --init --recursive` — and the suite was re-run, which is the `21827 passed; 51 failed` row above. No network operation was performed. / 两个版本固定相同的 submodule commit，且对象已在本地 `.git/modules` 中，基线 worktree 以**离线**方式补齐后重跑；全程无网络操作。

Attribution boundary / 归因边界: this attributes the 51 failures to the **whole local delta** (the five prior-slice commits plus this task's test commit) as a set, not to the Memory commits alone. Individual failures were not re-bisected; none of them is in the Memory subsystem except `memory::store::markdown::store_impl::tests::test_append_transcript`, which fails identically at the baseline (it renames `metadata.tmp` in a temporary directory that is absent). / 该结论把 51 项失败归因于**整体本地增量**（前序切片五个提交 + 本任务测试提交），不是单独归因于 Memory 提交；未做逐项 bisect。

## Task 3 — Final evidence, review, delivery / 最终证据、审阅与交付

### Documentation, formatting and diff checks / 文档、格式与 diff 检查

Exact commands and outputs / 精确命令与输出：

```text
python qa/docs/check_markdown_links.py --self-test   → self-test: PASS
python qa/docs/check_markdown_links.py               → doc_files=1646 valid=929 external_or_protocol=65 absolute_or_private=0 placeholder=0 code_or_example=157 missing_repository_targets=0
python qa/browser_live/check_docs.py --self-test      → check_docs self-test: OK
rustfmt --edition 2021 --check --config skip_children=true src/gateway/admin_api/reconciler.rs src/memory/runtime/tests.rs → (no output, clean)
git diff --check                                      → (no output, clean)
git diff --cached --check                             → (no output, clean)
```

`bun run check` was **not** run: `grep -n '"check"' package.json` finds no `check` script in this checkout, the same limitation the prior slice recorded, so no substitute result is claimed. Linux/Windows native execution was not available on this host and was not attempted; no substitute result is claimed. / 本 checkout 没有 `check` script（与前序切片同一限制），因此未运行 `bun run check`，也不宣称替代结果；本机不具备 Linux/Windows 原生执行条件，未执行，同样不伪造结果。

### Fresh independent review / 全新独立审阅

One fresh read-only whole-diff review was obtained against the task-start parent `98b71deb4` (`git diff 98b71deb4`, including the Task 1 commit and this record) with a supported review model. The reviewer did not run Cargo and performed no network operation. / 针对任务起始父提交 `98b71deb4` 取得一次全新只读 whole-diff 审阅；审阅者未运行 Cargo，未做任何网络操作。

Verdict: **no production or test correctness defect found**. The reviewer confirmed the `204` barrier is deterministic, that `/reconciler/latest` resolves the handler only through `MemoryRuntime::current_reconciler()` (Running-only, so every other state fails closed with `503`), that the Draining and Failed tests use a real admitted operation and a real SQLite close failure, that the restart payload differentiation is substantive (`scanned_facts` 0 → 1 in the response body), that the recorded counts are consistent with the 51-name intersection, and that the new `#[cfg(test)] pub(crate) fn test_admin_state` is not a production API or lifecycle authority.

Two documentation findings were raised and fixed / 提出并已修复两处文档问题：

1. The run record's status line still said "In progress (Task 1 started)" after Tasks 1–2 were complete → the status now states the slice is complete.
2. The Task 1 evidence wording claimed every new row used "the real runtime-owned reconciler daemon", but the restart test calls `reconcile_once()` directly on the runtime-owned handler → the Task 1 intro, the restart table row and the gaps paragraph now distinguish the daemon-driven `204` row from the direct-scan clean/restart-payload rows, and state that the direct scans do not pin daemon tick timing.

One risk was raised and **deferred** / 提出但**延后**：`#[cfg(test)] pub(crate) fn test_admin_state` drops its `TempDir` before returning `AdminApiState`; on this Unix host the open SQLite/token state stays usable, but the fixture is less portable to platforms with stricter open-file deletion semantics. The reviewer itself recommended no production code change, the same pattern existed in the previous local helper, and changing the fixture signature would widen this validation-only slice — so the behaviour is recorded here instead of being changed. / 该 fixture 的 `TempDir` 在返回前即被 drop；本机（Unix）上已打开的 SQLite/token 状态仍可用，但在对「已打开文件删除」更严格的平台上可移植性较差。此项按审阅建议不做代码改动，改为记录。

### Reference-document calibration / reference 文档校准

`docs/reference/FEATURE_LOCATOR.md` — the "Memory source-owned boundary" closure section no longer says that a deterministic first-scan HTTP `204` effect test is absent; a new "Memory source-owned validation closure" subsection records the `204` / `503` / restart-content evidence, the direct-scan-versus-daemon nuance, the 51-versus-51 baseline attribution and its boundary, with a link to this run record. `docs/reference/MEMORY_SYSTEM.md` — the bounded source-owned closure section gained a paragraph pointing at this validation-only evidence. Both edits are factual calibration only; no new capability, lifecycle authority or product claim was added. / 两处均为事实校准，未新增能力、lifecycle authority 或产品主张。

### Cleanup and delivery / 清理与交付

The only temporary state this task created was the detached baseline worktree `/tmp/aleph-validation-closure/baseline` and its isolated `CARGO_TARGET_DIR=/tmp/aleph-validation-closure/baseline-target`, plus the suite logs under `/tmp/aleph-validation-closure/`. The worktree was removed with `git worktree remove --force` after the results were recorded; the primary checkout, its branch and its history were never reset, merged, rebased, fetched or pushed. / 本任务创建的唯一临时状态是 detached 基线 worktree 及其隔离 target 目录与日志；记录结果后仅移除了自己创建的 worktree，从未对主 checkout 执行 reset/merge/rebase/fetch/push。

Delivery commit / 交付提交: `test(memory): close reconciler evidence gaps` (local `main`; the commit that carries this record). The tree is clean after it, and `4b6da265a` remains an ancestor.

### Non-claims / 不宣称

This slice does not claim universal all-writer Memory stop, native unload, RSS reclamation, process isolation, external exactly-once, daemon tick-timing determinism, or Linux/Windows native execution. The `51` full-suite failures are attributed to the whole local delta as a set and were **not** repaired; none of them was fixed here, and none of them is claimed to be Memory-specific. / 本切片不宣称 universal all-writer Memory stop、native unload、RSS 回收、进程隔离、外部 exactly-once、daemon tick 时序确定性或 Linux/Windows 原生执行。51 项全量失败按整体本地增量归因且**未**修复，未在本切片修复任何一项，也不宣称它们属于 Memory 子系统。

---

**Historical evidence boundary / 历史证据边界:** the prior slice's test totals and its aggregate `21834 passed; 51 failed; 19 ignored` figure are recorded in the prior run; they are context for this comparison, not new evidence / 前序切片的测试总数与聚合计数记录在前序运行记录中，仅作为本次对照背景，不算本切片的新证据。

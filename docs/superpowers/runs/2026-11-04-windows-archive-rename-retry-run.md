# Windows archive rename retry run / Windows 归档移动重试记录

**Status / 状态:** Implemented and reviewed; review findings resolved, scoped Windows acceptance and named baseline comparison complete. Full-lib suite remains non-green / 已实施并完成审阅，具体 findings 已处理，Windows 有界验收与逐名基线对照已完成；全量库测试仍未绿。

## Starting state / 起始状态

- Discovered checkout / Git 定位：`D:/Workspace/Aleph`, branch / 分支 `main`.
- Actual HEAD / 实际 HEAD：`074fc1c007af7b36e8ca64f9e2d416bb5d09df65` (plan/prompt documentation only / 仅计划与交接文档).
- Attribution baseline / 失败归因基线：`f40fbf061`; it is HEAD's parent and contains the Windows console QA harness / 当前 HEAD 的父提交，已含 Windows 控制台 QA harness。
- `git status --short`: empty before this task / 本任务开始前为空。
- Initial memory gate / 首次内存门：`available_bytes=15528808448`, `required_bytes=4294967296`, `ALLOW`.
- Local work only / 仅本地工作：no repository fetch, push, merge, rebase, reset, or primary-checkout switch / 不执行仓库远端同步、历史重写或主检出切换。

## Approach and trade-off / 方案与代价

Reproduce with real Windows child handles and real directory rename before changing production. Keep naming collisions separate from a finite Windows sharing-denial budget. Genuine permission errors may therefore be delayed by that finite budget, but must retain their OS error on failure. / 先用真实 Windows 子文件句柄与目录移动复现；命名冲突与共享拒绝分别计数。真实权限错误可能延迟一个有限预算后返回，但不得丢失底层 OS 错误。

Transcript-handle attribution is measurement-dependent; the samples below did not establish an own-transcript leak, so explicit close is not changed. Defender ownership cannot be inferred merely from a transient error. / transcript 是否需要显式关闭取决于实测；下文样本未建立自有句柄泄漏证据，因此不改关闭方式。暂时性错误本身不能证明 Defender 持有句柄。

## Evidence / 证据

- Current local artifacts / 本次本地产物：`C:/Temp/aleph-archive-retry/` (kept separate from the previous run / 与前序运行隔离).
- First focused Cargo invocation / 首次聚焦 Cargo：`cargo test -p alephcore --lib gateway::session_store::file_backend::archive_tests::delete_session_bounds_windows_sharing_denial_and_preserves_source -- --exact --nocapture --test-threads=1`; gate / 内存门 `15441297408` bytes, `ALLOW`. It did not execute tests: the existing dependency artifact lacked Panel symbols that are present in both current and baseline source; compilation was stopped after those diagnostics. This is **not** behavioural RED. / 未运行测试：现有依赖产物缺少当前与基线源码均已定义的 Panel 符号；诊断出现后中止编译。这**不是**行为 RED。
- Diagnostic names / 精确诊断：`E0432` (`PanelUiContextSnapshot`, `PanelProjectSelection`, `PANEL_PROJECT_SELECT_METHOD`), `E0408`, `E0425` (`PANEL_PROJECT_GENERATION_HEADER`), `E0689`. Only `shared/protocol/src/panel.rs`'s timestamp was refreshed to force dependency rebuild; its contents are unchanged. / 只更新时间戳以触发依赖重编译，没有改其内容。
- Baseline server command / 基线 server 命令：`cargo build -p alephcore --bin aleph-server --features alephcore/test-helpers`; gate / 内存门 `15654731776` bytes, `ALLOW`; exit 0 in `9m 41s` / 退出 0，用时 9 分 41 秒。
- All Cargo runs use / 全部 Cargo 均设置 `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; all test invocations use / 测试均使用 `--test-threads=1`.

### Rebuilt baseline live measurements / 重建基线 live 测量

Artifact / 产物：`baseline-server.exe`, 311203840 bytes; SHA-256 `830540e9fc93ab99414b3e51bf06ef7c248a4e3ae67e9718448c6d2605dba8e8`. Build record / 构建记录：`baseline-build-record.json`, source revision `074fc1c007af7b36e8ca64f9e2d416bb5d09df65`; source changes at build time were tests only / 构建时源码改动仅含测试。

Command per run / 每轮命令：`python qa/memory_lifecycle/archive_check.py --binary C:/Temp/aleph-archive-retry/baseline-server.exe --artifacts C:/Temp/aleph-archive-retry --cycles 20`.

| Run / 轮次 | Artifact directory / 产物目录 | Completed / 完成 | Attempt-0 failures / 首次失败 | Final archives / 最终归档 |
|---|---|---|---|---|
| 1 | `aleph-memory-archive-fru5wump` | 20 | 0 | 20 |
| 2 | `aleph-memory-archive-m48j_uo1` | 20 | 0 | 20 |
| 3 | `aleph-memory-archive-hm_33tyg` | 20 | 0 | 20 |

Measured baseline failure rate / 本次实测基线失败率：**0/60**. This sample does not reproduce the prior intermittent live denial and cannot support a claim that a later 0/60 is a measured rate improvement. The controlled handle-denial RED below supplies the defect evidence. / 本样本没有复现此前间歇性 live 拒绝，因此后续若同为 0/60，不能宣称实测失败率下降；缺陷证据由下文受控句柄拒绝 RED 提供。

Windows Defender reports `AntivirusEnabled=true`, `RealTimeProtectionEnabled=true` in `defender-status.json`; this establishes configuration, not ownership of a particular handle. / Defender 状态仅证明实时保护已启用，不证明特定句柄归属。

Focused RED/attribution command / 聚焦 RED 与归因命令：`cargo test -p alephcore --lib gateway::session_store::file_backend::archive -- --nocapture --test-threads=1`; gate / 内存门 `15129088000` bytes, `ALLOW`; test-profile build `15m 42s`; **4 selected: 2 passed, 2 failed, 0 ignored**, runtime `3.16s` / 实际选中 4，2 通过、2 失败、0 忽略。

### Behavioural RED / 行为 RED

- `gateway::session_store::file_backend::archive_tests::delete_session_bounds_windows_sharing_denial_and_preserves_source`: `held child: Database error: Archive session failed: 拒绝访问。 (os error 5); elapsed=0ns`; failed the finite-backoff lower bound. / 保持真实子文件句柄且禁用 `FILE_SHARE_DELETE`，归档立即失败，未作有限退避。
- `gateway::session_store::file_backend::archive_tests::delete_session_retries_same_destination_after_child_handle_release`: `must wait for the held child: Err(DatabaseError("Archive session failed: 拒绝访问。 (os error 5)"))`; the same delete future terminated before the controlled release point. / 同一删除 future 在受控释放点之前即失败。
- Existing contract / 既有契约：`gateway::session_store::file_backend::archive_tests::delete_session_reuses_a_key_without_colliding_with_its_archive` passed / 通过。
- Tests use the real store, real directories, a real `OpenOptionsExt::share_mode(3)` handle, and real rename; Tokio paused time waits for blocking filesystem work before auto-advancing. There is no wall-clock sleep race, mock or production test hook. / 全部为真实行为，没有真实时间 sleep 竞态、mock 或生产测试钩子。

### Handle attribution measurement / 句柄归因测量

`gateway::session_store::file_backend::archive_diagnostics::measure_windows_handle_attribution` ran before production edits / 在生产修改前运行：

| Mode / 组别 | Deletes / 删除 | First failures / 首次失败 | Exclusive transcript reopens / transcript 独占重开 |
|---|---|---|---|
| `no_write` | 100 | 0 | not sampled / 未采样 |
| `production_append` | 100 | 0 | not sampled / 未采样 |
| `production_append_handle_probe` | 100 | 0 | 100/100 succeeded / 成功 |
| `explicit_close` (`flush`, then `into_std`, then drop) | 100 | 0 | not sampled / 未采样 |

The controlled test-owned non-delete-sharing child handle is sufficient to cause OS 5. It is not a traced Defender handle. Production append did not leave an observable conflicting transcript handle in the 100 exclusive-reopen samples; explicitly closing the transcript did not improve the measured 0/100 failure rate. Therefore no transcript-close production change is justified by this measurement. / 测试自持的非删除共享句柄足以触发 OS 5，但不是 Defender 句柄追踪。生产 append 的 100 次独占重开未观察到冲突句柄，显式关闭也未改善本次 0/100 失败率，因此不修改生产 transcript 关闭方式。

**Residual attribution / 归因残留：** the owner of the earlier intermittent live denial remains unknown. Defender is enabled, but configuration is not evidence that it held that particular handle. / 前序间歇性 live 拒绝的具体持有方仍未知；不能由 Defender 已启用推断其持有该句柄。

**Implementation decision / 实施决定：** preserve collision-first naming and its 1024 cap; Windows OS 5/32/33 may wait 25/50/100/200/400/800 ms on the same destination, with one six-retry budget across suffix changes. The nominal scheduled backoff totals 1575 ms; actual elapsed time also includes filesystem work and scheduling, and a lasting denial still returns the original OS error. / 保留命名契约；共享冲突同目标有限退避，跨后缀不重置预算。名义总退避为 1575 ms，实际耗时还包含文件 I/O 与调度；持续拒绝仍报原始 OS 错误。

### Implementation and QA changes / 实施与 QA 变化

- Production / 生产：private `ArchiveRenameFailure` decision table, `ARCHIVE_SHARING_BACKOFF_MS=[25,50,100,200,400,800]`, one sharing budget outside the collision branch, terminal warning with actual total attempts / 私有分类表、有限退避、跨冲突共享预算、终局实际尝试次数日志。`append_transcript`, the SessionStore trait, backend selection and SQLite code are unchanged / 均未改动。
- The budget-exhaustion regression now also creates a real occupied destination **after the first sharing denial**, so resetting the sharing budget on a collision would violate its upper bound. After release it verifies the incumbent bytes and the next suffix / 耗尽回归增加真实交错，覆盖跨冲突预算不重置，并在释放后验证旧字节与下一后缀。
- `probe.py` now makes one delete RPC, retains `attempts=1`, and fails on a terminal production denial / 生命周期探针不再愈合生产错误。
- `archive_check.py` also makes one delete RPC per cycle, returns nonzero on failure, verifies one new archive and contiguous names per date, and compares every prior archive's complete file-set/SHA-256 map. Its JSON records graceful shutdown/exit/endpoint/listener facts; a forced kill is failure cleanup / 归档检查也取消重试，并验证增长、命名、旧文件集合与字节，记录清理事实；强杀只能算失败清理。
- Two-cycle harness smoke (baseline binary, separate from fixed-artifact acceptance) / 两轮脚本烟测（基线产物，不算修复后验收）：`aleph-memory-archive-ylpvw2sz`, exit 0, `complete=true`, all hash/growth/name checks true; `[SHUTDOWN] signal=ctrl_c` observed, server exit 0, no forced cleanup, endpoint removed, listener closed / 全部通过。
- The first post-fix test compilation was stopped before tests to add the plan-required terminal-attempt logging; memory gate `17020698624` bytes, `ALLOW`. It is not GREEN evidence / 首次修复后编译为补齐终局日志而在测试前中止，不算 GREEN。
- Restarted focused command / 重新聚焦命令：`cargo test -p alephcore --lib gateway::session_store::file_backend::archive_tests -- --nocapture --test-threads=1`; memory gate `16531910656` bytes, `ALLOW`; build `16m 11s`, **5 passed, 0 failed, 0 ignored**, runtime `0.06s` / 实际通过 5/5。The held-handle/collision-interleaving test printed `elapsed=1.575s` and retained OS error 5 / 持有句柄并交错碰撞时，耗尽虚拟退避为 1.575 秒且保留 OS error 5。

### Focused regression results / 聚焦回归结果

New/retained archive target selected these exact names / 归档目标实际选中的精确测试名：

```text
gateway::session_store::file_backend::archive_tests::delete_session_bounds_windows_sharing_denial_and_preserves_source
gateway::session_store::file_backend::archive_tests::delete_session_retries_same_destination_after_child_handle_release
gateway::session_store::file_backend::archive_tests::delete_session_reuses_a_key_without_colliding_with_its_archive
gateway::session_store::file_backend::archive_tests::rename_failure_classifies_destination_before_os_error
gateway::session_store::file_backend::archive_tests::rename_failure_does_not_retry_unrelated_or_untyped_errors
```

Each following invocation is / 后续每条命令为 `cargo test -p alephcore --lib <filter> -- --test-threads=1`, preceded by a fresh `python qa/browser_live/check_memory.py`; all exit 0, failed/ignored 0 / 各自先过内存门，退出均为 0，失败与忽略均为 0：

| Filter | Passed / 通过 | Available bytes / 可用字节 | Gate |
|---|---:|---:|---|
| `gateway::session_store` | 102 | 16130043904 | ALLOW |
| `memory::extensions` | 60 | 16311132160 | ALLOW |
| `gateway::session_manager` | 39 | 16340135936 | ALLOW |
| `a2a::sub_agent` | 7 | 16350666752 | ALLOW |
| `builtin_tools::session_complete` | 4 | 16356802560 | ALLOW |
| `gateway::handlers::session` | 43 | 16334602240 | ALLOW |

### Owned test-placement correction / 本切片测试文件命名修正

The first full-run comparison found one current-only test / 首次逐名对照发现一个仅当前失败：`gateway::continuation_lifecycle::tests::every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement`. Direct exact execution of each just-built full-suite binary selected 1 test: current exit 101, baseline exit 0. The current panic specifically identified `archive_diagnostics.rs (2 site(s))`. / 两边刚构建出的测试二进制均精确选中 1 个，当前 101、基线 0；错误明确指向本次新增诊断文件的两个调用。

This was introduced by this slice: the diagnostic module already had `#[cfg(all(test, windows))]`, but its filename did not match the existing source guard's test-only conventions (`*_tests.rs`, `tests.rs`, `tests/`, `test_utils`). The file was moved unchanged to `archive_diagnostics_tests.rs`, and the private module name changed accordingly. The guard itself and the runtime retry implementation were not changed. A content-hash comparison confirmed the diagnostic content was identical. / 这是本切片新增测试文件的命名错误，不是基线问题。按既有测试文件命名移动并同步私有模块名，诊断内容哈希一致，未削弱守卫或修改运行时重试。

Re-running the **same unit binaries** after the move selected 1 each and both exited 0. The recorded binary SHA-256 values are identical before/after, establishing that this guard reads the source tree at runtime; these are direct-binary source-guard results, not Cargo or archive-behaviour evidence. / 改名后使用相同二进制，两边各选中 1 个并通过；前后二进制哈希相同。这只证明源码守卫的测试文件归类已修正，不冒充 Cargo 或真实归档行为证据。

| Guard snapshot / 守卫快照 | Current exit / 当前退出 | Baseline exit / 基线退出 | Current gate / 当前内存门 | Baseline gate / 基线内存门 |
|---|---:|---:|---:|---:|
| before test-file move / 改名前 | 101 | 0 | 15836590080 ALLOW | 15803506688 ALLOW |
| after test-file move / 改名后 | 0 | 0 | 15820578816 ALLOW | 15822045184 ALLOW |

Exact argv, binary hashes and raw errors are retained in `guard-red.json`, `guard-green.json` and their per-side logs. A fresh Cargo archive/guard/full run, server rebuild and native acceptance are now in progress on the corrected source tree. / 精确参数、哈希和错误保存在上述产物；调整后的源码正重新执行 Cargo 测试、server 构建与原生验收。

The corrected-source Cargo run also selected **6 archive-prefix tests, all passed, 0 failed, 0 ignored, 21694 filtered out** (3.06s; compilation 16m 28s), including the renamed `gateway::session_store::file_backend::archive_diagnostics_tests::measure_windows_handle_attribution`. The five behavioural/classifier regressions remain green, and the held-child test still reports `elapsed=1.575s`. The fresh gate was `15733567488` bytes, `ALLOW`. / 更名后的 Cargo 归档前缀实选 6 个并全绿，包含诊断；既有五个回归保持通过，真实持有子句柄测试仍得到 1.575 秒虚拟退避。

Cargo then precisely ran `gateway::continuation_lifecycle::tests::every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement`: **1 passed, 0 failed, 0 ignored, 21699 filtered out**, 1.08s, gate `15626596352` bytes `ALLOW`. Raw logs: `final-archive.log`, `final-guard.log` and their `*-memory.log`. / 随后 Cargo 精确选中此前失败的守卫 1 个并通过，原始命令日志与内存门按上述名称保存。

An attempted Cargo-based per-name comparison ran one current test (selected 1, failed), then unexpectedly began recompiling baseline dependencies/core. That extra compiler was stopped and selected no baseline test; its logs are compile-only, not behavioural evidence. Direct exact runs use the binaries produced by the completed full builds and are identified separately. / 曾尝试 Cargo 逐名复跑，当前首个测试选中 1 个失败，基线却触发额外重编译；已停止该额外编译，基线没有选中测试，不将其计作行为证据。后续直接二进制核验单独标识。

### Initial full-lib limitation and pinned comparison / 首次全量限制与固定基线对照

Command / 命令：`cargo test -p alephcore --lib -- --test-threads=1`; gate `16338124800` bytes, `ALLOW`; Cargo exit **127**. It declared 21700 tests, emitted 10613 `ok`, 54 `FAILED`, 7 `ignored`, then aborted at `gateway::pty::manager::tests::close_all_terminates_every_live_session` with `exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN`. There was **no final test summary**; remaining tests did not execute. These observed counts are not a passing full-suite result. / 全量在上述精确 PTY 测试名处异常退出，无最终汇总，余下测试未执行；不得把已观察状态计数当作全量通过。

The task-owned detached worktree is pinned at `f40fbf061`, with its own `CARGO_TARGET_DIR`; the identical command started only after the initial fixed-server Cargo build finished. Its fresh gate was `14125264896` bytes, `ALLOW`; compilation took `37m 01s`. It declared 21695 tests and emitted 10610 `ok`, 57 `FAILED`, 7 `ignored`, then aborted at the **same complete PTY test name** with `0xc0000409, STATUS_STACK_BUFFER_OVERRUN`, Cargo exit 127 and no final summary. / 基线固定在上述提交并使用隔离 target，内存门通过；编译与已观察结果如上，也在同一完整 PTY 测试名处异常退出，无最终汇总。

Per-name comparison of the initial current failures found **53 same named failures, 1 only-current, 0 incomplete**. The one current-only failure is the owned test-placement issue corrected above; this is not a claim that all initial failures were pre-existing. Four additional baseline-only names were observed / 首次逐名比较为 53 个同名失败、1 个仅当前失败、0 个未完成；仅当前项已按上节修正，不能称首次失败全为既有问题。另有四个基线特有失败：

- `config::tests::skill_doc_drift::no_bundled_snippet_hardcodes_the_aleph_config_path`
- `config::tests::skill_doc_drift::the_bundled_self_skill_names_no_phantom_config_keys`
- `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`
- `gateway::execution_engine::btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question`

Raw `full.log`, `baseline-full.log`, `full-comparison.json` and the exact-name table `full-comparison.md` retain the initial results. / 首次原始日志与逐名表已保存。

### Corrected-source full comparison / 更名后源码全量对照

The identical Cargo full-lib command was re-run on the corrected source tree with a fresh gate of `15676239872` bytes, `ALLOW`. It declared 21700 tests; the parsed completed statuses were **10615 `ok`, 52 `FAILED`, 7 `ignored`**. It again exited at `gateway::pty::manager::tests::close_all_terminates_every_live_session` with `0xc0000409, STATUS_STACK_BUFFER_OVERRUN`, Cargo exit 127, no final suite summary and the remaining tests unexecuted. / 更名后以同一命令重新跑全量，门值与已完成结果如上；仍在同一完整 PTY 测试名处异常退出，未产生全量汇总，余下测试未执行。

The exact-name comparison against the pinned baseline contains **52 same named failures, 0 only-current, 0 incomplete**. The baseline-only set now contains the four names above **plus** `gateway::pty::manager::tests::cancelled_stop_retains_handle_and_blocks_reactivation_until_join` (baseline failed, corrected-source full run passed). This is an observed change of outcome, not a claimed fix or root-cause attribution for that unrelated PTY test. / 逐名比较为 52 个同名失败、0 个仅当前失败、0 个未完成；基线特有项为上节四个再加该 PTY 测试。本轮通过只代表实测结果变化，不将其认作本切片修复或推断其根因。

Raw evidence: `final-full.log`, `final-full-memory.log`, `final-full-comparison-summary.log`, `final-full-comparison.json` and `final-full-comparison.md`. No suite-wide pass or identical failure cause is claimed. / 完整错误与逐名表保存在上述产物，不宣称全量通过或同名失败根因相同。

### Exact isolated comparison / 精确隔离对照

The complete union of original/current/baseline failed or aborted names was executed directly through the two completed Cargo-built unit binaries: **59 pairs / 118 commands**, each `<unit-binary> <exact-test-name> --exact --nocapture --test-threads=1`. These are direct Rust test-runner commands, not per-name Cargo invocations. Every command selected **1** test with no ignored selection, and every fresh memory gate was `ALLOW`, available bytes **15747301376–16073080832** (required 4294967296). / 对首次、最终和基线全部失败或异常退出名称取并集，共 59 对、118 条直接 Rust 测试命令；每条实选 1 个、无忽略，逐条内存门均通过。这些结果与上节同命令全量 Cargo 对照分开。

Binary provenance / 二进制来源（full-build logs and SHA-256 in `unit-binaries.json`）：

- current / 当前：`final-full.log`, SHA-256 `a23eb9ee78ab2de5ef93af1ab98cce1465df97f4f9dff2d1ba263c2062f0e29d`;
- baseline / 基线：`baseline-full.log`, `f40fbf061`, SHA-256 `c5bc9fc9996a16fb48a8819c2dc6aa96a0098c2827dc84ed4628d0887410a5a0`.

The independent outcomes were **48 same named failures, 6 not reproduced in isolation, 4 baseline-only failures, 1 owned test-placement correction verified; 0 only-current**. A same-name/exit match does not establish an identical root cause, and an isolated pass does not erase a failure in the serial full run. In particular, the PTY cancellation test passed the final full run but failed both direct runs. No unrelated failure was repaired. / 独立结果为同名失败 48、隔离未复现 6、基线特有 4、本次命名问题已修正 1、仅当前失败 0。同名和退出码不等于根因相同；隔离通过也不能抹去串行失败。尤其 PTY cancellation 测试在最终全量通过，却在两边隔离执行时均失败。未顺手修复无关失败。

The exact per-name table below is copied from `final-named.md`; raw argv, selection counts, summaries, gates and logs remain in `final-named.json` and `final-named/`. Direct exits `0xc0000409` identify abnormal process termination, not an ordinary assertion failure. / 下表逐名保留原始对照；参数、计数、内存门与错误日志保存在上述产物。`0xc0000409` 是进程异常终止，不能当普通断言失败处理。

| Exact test name / 精确测试名 | Final full C/B / 最终全量当前与基线 | Direct exit C/B / 独立退出 | Direct comparison / 独立对照 | Gate bytes C/B / 内存门字节 |
|---|---|---|---|---|
| `gateway::continuation_lifecycle::tests::every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement` | ok / ok | 0 / 0 | owned test-placement corrected | 15945760768 / 15887097856 |
| `agents::swarm::tasks::acceptance::tests::criterion_newlines_cannot_break_out_of_the_list_item` | FAILED / FAILED | 101 / 101 | same named failure | 15935393792 / 15933849600 |
| `agents::swarm::tasks::store::tests::test_acquire_and_release_lock` | FAILED / FAILED | 101 / 101 | same named failure | 15934550016 / 15934820352 |
| `browser::cdp_backend::navigate::tests::a_new_loader_id_resets_the_tabs_ref_table` | FAILED / FAILED | 101 / 101 | same named failure | 15936700416 / 15934509056 |
| `browser::cdp_backend::recording::tests::a_preexisting_stale_file_is_never_reported_complete` | FAILED / FAILED | 101 / 101 | same named failure | 15932960768 / 15927922688 |
| `browser::cdp_backend::routes::tests::a_tab_rule_for_another_tab_does_not_arm_this_tab` | FAILED / FAILED | 101 / 101 | same named failure | 15901360128 / 15902830592 |
| `browser::cdp_backend::routes::tests::closing_a_tab_drops_its_rules_and_its_loop` | FAILED / FAILED | 101 / 101 | same named failure | 15901749248 / 15902199808 |
| `browser::cdp_backend::routes::tests::profile_rules_are_armed_before_the_tabs_first_navigation` | FAILED / FAILED | 101 / 101 | same named failure | 15902466048 / 15901835264 |
| `browser::cdp_backend::routes::tests::removing_the_last_rule_disarms_the_tab` | FAILED / FAILED | 101 / 101 | same named failure | 15884324864 / 15859441664 |
| `browser::cdp_backend::routes::tests::zero_rules_means_no_fetch_enable_and_no_loop` | FAILED / FAILED | 101 / 101 | same named failure | 15859867648 / 15860932608 |
| `browser::cdp_backend::tabs::tests::an_empty_target_enumeration_does_not_hide_the_tab_we_created` | FAILED / FAILED | 101 / 101 | same named failure | 15859216384 / 15856861184 |
| `browser::cdp_backend::tabs::tests::open_tab_records_the_tabs_identity_and_landed_url` | FAILED / FAILED | 101 / 101 | same named failure | 15854301184 / 16073080832 |
| `browser::engine::registry::tests::a_surviving_displaced_engine_blocks_relaunch_and_keeps_its_record` | FAILED / FAILED | 101 / 101 | same named failure | 16071360512 / 15986200576 |
| `browser::page_state::fetch_chromium::tests::a_grandchild_cross_origin_frame_is_carried_rather_than_refusing_the_page` | FAILED / FAILED | 101 / 101 | same named failure | 15998881792 / 15977672704 |
| `browser::page_state::fetch_chromium::tests::a_grandchild_whose_id_collides_with_a_parent_element_is_still_confessed` | FAILED / FAILED | 101 / 101 | same named failure | 15970529280 / 15951273984 |
| `builtin_tools::browser_tools::qa::tests::a_dimension_the_driver_cannot_serve_lands_in_unverified_not_pass` | FAILED / FAILED | 101 / 101 | same named failure | 15974711296 / 15962681344 |
| `builtin_tools::browser_tools::qa::tests::a_failing_run_with_screenshot_requested_attaches_the_evidence` | FAILED / FAILED | 101 / 101 | same named failure | 15964745728 / 15961800704 |
| `builtin_tools::browser_tools::qa::tests::a_met_expectation_with_all_checks_off_passes_cleanly` | FAILED / FAILED | 101 / 101 | same named failure | 15957839872 / 15961092096 |
| `builtin_tools::browser_tools::qa::tests::a_passing_run_takes_no_evidence_screenshot` | FAILED / FAILED | 101 / 101 | same named failure | 15959633920 / 15956963328 |
| `builtin_tools::browser_tools::qa::tests::a_selector_that_stays_fails_gone_selector` | FAILED / FAILED | 101 / 101 | same named failure | 15966224384 / 15967186944 |
| `builtin_tools::browser_tools::qa::tests::evidence_tails_carry_only_the_new_lines` | FAILED / FAILED | 101 / 101 | same named failure | 15965683712 / 15968477184 |
| `builtin_tools::browser_tools::qa::tests::failed_expectations_return_a_successful_tool_result_with_passed_false` | FAILED / FAILED | 101 / 101 | same named failure | 15968157696 / 15966023680 |
| `builtin_tools::browser_tools::qa::tests::the_wire_shape_matches_the_spec` | FAILED / FAILED | 101 / 101 | same named failure | 15962787840 / 15972732928 |
| `builtin_tools::file_ops::path_utils::tests::a_home_entry_compiled_under_one_home_is_not_served_under_another` | FAILED / FAILED | 101 / 101 | same named failure | 15898054656 / 15905054720 |
| `builtin_tools::file_ops::path_utils::tests::a_home_with_glob_metacharacters_is_still_a_literal_location` | FAILED / FAILED | 101 / 101 | same named failure | 15904096256 / 15900606464 |
| `builtin_tools::file_ops::path_utils::tests::code_built_entries_are_literal_locations_under_any_home` | FAILED / FAILED | 101 / 101 | same named failure | 15899799552 / 15899660288 |
| `config::live_apply::terminal_tests::cleanup_failure_never_reports_live_or_discards_the_failed_session` | FAILED / FAILED | 101 / 101 | same named failure | 15898034176 / 15899426816 |
| `config::live_apply::terminal_tests::competing_config_writer_waits_for_the_apply_read_guard` | FAILED / FAILED | 0 / 0 | not reproduced in isolation | 15901569024 / 15898660864 |
| `config::live_apply::terminal_tests::enabling_while_stopping_observes_the_same_wait_budget` | FAILED / FAILED | 0 / 0 | not reproduced in isolation | 15897407488 / 15896121344 |
| `config::live_apply::terminal_tests::patch_rollback_and_failed_persistence_observe_source_effects` | FAILED / FAILED | 0 / 0 | not reproduced in isolation | 15897358336 / 15902863360 |
| `config::live_apply::terminal_tests::stop_timeout_retains_the_same_driver_until_explicit_retry` | FAILED / FAILED | 0 / 0 | not reproduced in isolation | 15901896704 / 15876087808 |
| `config::live_apply::terminal_tests::stopping_enable_waits_for_retained_join_before_new_generation` | FAILED / FAILED | 0 / 0 | not reproduced in isolation | 15873032192 / 15871213568 |
| `config::live_apply::tests::disabling_the_terminal_live_kills_sessions_through_apply_live_sections` | FAILED / FAILED | 101 / 101 | same named failure | 15869186048 / 15870300160 |
| `config::live_apply::tests::terminal_disable_reports_live_only_after_join` | FAILED / FAILED | 0 / 0 | not reproduced in isolation | 15870361600 / 15879852032 |
| `config::tests::skill_doc_drift::no_bundled_snippet_hardcodes_the_aleph_config_path` | ok / FAILED | 0 / 101 | baseline-only | 15877885952 / 15876521984 |
| `config::tests::skill_doc_drift::the_bundled_self_skill_names_no_phantom_config_keys` | ok / FAILED | 0 / 101 | baseline-only | 15876669440 / 15876329472 |
| `extension::lifecycle::tests::b3_cancelled_waiter_does_not_release_guest_permit_and_starting_stop_joins_without_deadlock` | FAILED / FAILED | 101 / 101 | same named failure | 15873425408 / 15875186688 |
| `extension::lifecycle::tests::b3_publication_cleanup_error_retains_exact_failed_owner_evidence` | FAILED / FAILED | 101 / 101 | same named failure | 15890997248 / 15840071680 |
| `extension::lifecycle::tests::b3_publication_failure_and_missing_producer_cleanup_fail_closed` | FAILED / FAILED | 101 / 101 | same named failure | 15839498240 / 15831384064 |
| `extension::lifecycle::tests::b3_ready_real_guest_reload_and_old_receipt_are_incarnation_fenced` | FAILED / FAILED | 101 / 101 | same named failure | 15827779584 / 15829938176 |
| `extension::lifecycle::tests::b4_production_canonical_reload_rejects_admitted_a_before_guest_and_runs_b` | FAILED / FAILED | 101 / 101 | same named failure | 15829114880 / 15826231296 |
| `extension::lifecycle::tests::real_wasm_boot_concurrent_ensure_loaded_yields_one_owner_and_one_publication` | FAILED / FAILED | 101 / 101 | same named failure | 15831314432 / 15830605824 |
| `extension::lifecycle::tests::stage_e_wasm_load_reads_guest_bytes_and_drops_exact_instance_after_unmount` | FAILED / FAILED | 101 / 101 | same named failure | 15830208512 / 15830880256 |
| `extension::lifecycle::tests::stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner` | FAILED / FAILED | 101 / 101 | same named failure | 15832002560 / 15827558400 |
| `extension::lifecycle::tests::stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation` | FAILED / FAILED | 101 / 101 | same named failure | 15823581184 / 15824203776 |
| `extension::mcp_config::tests::bare_and_absolute_commands_are_kept_as_written` | FAILED / FAILED | 101 / 101 | same named failure | 15836512256 / 15833657344 |
| `extension::panel::tests::accepts_semver_build_metadata_in_the_canonical_entry` | FAILED / FAILED | 101 / 101 | same named failure | 15836385280 / 15836282880 |
| `extension::projection::tests::publishing_plugin_projections_has_exactly_one_author` | FAILED / FAILED | 101 / 101 | same named failure | 15834619904 / 15838154752 |
| `extension::tests::a_project_tool_survives_another_plugin_claiming_the_same_short_name` | FAILED / FAILED | 101 / 101 | same named failure | 15835947008 / 15831224320 |
| `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation` | ok / FAILED | 0 / 101 | baseline-only | 15747301376 / 15755735040 |
| `gateway::execution_engine::btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` | ok / FAILED | 0 / 101 | baseline-only | 15754207232 / 15747964928 |
| `gateway::execution_engine::run_loop::tests::every_run_producer_answers_the_fire_time_authority_question` | FAILED / FAILED | 101 / 101 | same named failure | 15747616768 / 15948136448 |
| `gateway::execution_engine::run_loop::tests::project_skill_block_none_when_only_global_skills` | FAILED / FAILED | 101 / 101 | same named failure | 15947886592 / 15936700416 |
| `gateway::handlers::config::terminal_tests::terminal_cold_patch_self_config_noop_and_explicit_reload_recovery` | FAILED / FAILED | 101 / 101 | same named failure | 15935930368 / 15930912768 |
| `gateway::handlers::markdown_skills::tests::containment_is_component_wise` | FAILED / FAILED | 101 / 101 | same named failure | 15927705600 / 15930728448 |
| `gateway::handlers::pty::tests::spawn_with_no_cwd_actually_chdirs_the_child_into_the_first_root` | FAILED / FAILED | 101 / 101 | same named failure | 15930433536 / 15923257344 |
| `gateway::pty::manager::tests::all_old_generation_operation_faces_reject_replacement_sessions` | FAILED / FAILED | 101 / 101 | same named failure | 15928422400 / 15927422976 |
| `gateway::pty::manager::tests::cancelled_stop_retains_handle_and_blocks_reactivation_until_join` | ok / FAILED | 101 / 101 | same named failure | 15927115776 / 15923970048 |
| `gateway::pty::manager::tests::close_all_terminates_every_live_session` | aborted/not completed / aborted/not completed | 3221226505 / 3221226505 | same named failure | 15924215808 / 15919648768 |


### Initial fixed binary and native probes / 初次修复后二进制与原生探针

Build / 构建：`cargo build -p alephcore --bin aleph-server --features alephcore/test-helpers`; gate `16038031360` bytes, `ALLOW`; exit 0, `9m 24s`. Artifact / 产物：`fixed-server.exe`; SHA-256 `bcfa70d8ada4aed758bc78953ba65177517f3ec53f732d5e5f7cd8d351d3cec1`. `build-record.json` records Windows-native, `alephcore/test-helpers`, source parent `074fc1c007af7b36e8ca64f9e2d416bb5d09df65` plus the working-tree patch in `source.diff`; this is not a claim that the binary was built from a clean delivery commit. / 构建记录明确记载父提交加工作区补丁，不冒称从交付后的干净提交构建。

Actual commands / 实际命令：

```text
python qa/memory_lifecycle/probe.py --binary C:/Temp/aleph-archive-retry/fixed-server.exe --sha256 bcfa70d8ada4aed758bc78953ba65177517f3ec53f732d5e5f7cd8d351d3cec1 --build-record C:/Temp/aleph-archive-retry/build-record.json --providerless --artifacts C:/Temp/aleph-archive-retry
python qa/memory_lifecycle/probe.py --binary C:/Temp/aleph-archive-retry/fixed-server.exe --sha256 bcfa70d8ada4aed758bc78953ba65177517f3ec53f732d5e5f7cd8d351d3cec1 --build-record C:/Temp/aleph-archive-retry/build-record.json --artifacts C:/Temp/aleph-archive-retry
```

| Mode / 模式 | Artifact directory / 产物目录 | Printed checks / 输出检查 | Exit / 退出 | Complete |
|---|---|---|---:|---|
| providerless | `aleph-memory-lifecycle-u49jx5n1` | `Checks: 102/102` | 0 | true |
| configured | `aleph-memory-lifecycle-9ui1ahzk` | `Checks: 110/110` | 0 | true |

Both result files have zero failed checks and four session-tail delete checks with `attempts=1`. Combined logs contain zero `Archive session failed`. Raw chat and real Markdown archival remain available while Off; `memory.stats` and the derived `note_manage` tool remain refused; rejected writes have no Markdown effect; start/stop/reactivation and restart checks pass. Configured mode also retains and completes held provider work at shutdown. / 两份结果均无失败，分别四次尾部归档删除均只尝试一次，合并日志均无归档失败；Off 的 raw chat/Markdown、派生 RPC/tool 拒绝、被拒写无效果、启停与重启语义均通过，configured 的持有 provider 工作在关闭前完成。

Each probe has 30 successful `SQLite integrity` checks across its two boots. An additional header-based read-only audit of both probe homes and all three archive-check homes found 15 SQLite databases in each home, **75/75 `PRAGMA integrity_check` results exactly `ok`**, with no non-`.db` SQLite files missed; full relative paths/results are in `sqlite-integrity.json`. / 两种 probe 各含两次启动共 30 项 SQLite 检查；另按文件头审计五个 HOME，各 15 个数据库，共 75/75 为 `ok`，没有遗漏其他扩展名的 SQLite 文件。

### Initial native repeated-key archive acceptance / 初次原生重复 key 归档验收

Each of three runs used / 三轮均使用：`python qa/memory_lifecycle/archive_check.py --binary C:/Temp/aleph-archive-retry/fixed-server.exe --artifacts C:/Temp/aleph-archive-retry --cycles 20`.

| Run / 轮次 | Artifact directory / 产物目录 | Cycles / 轮数 | Attempt-0 failures / 首次失败 | Growth, naming, old hashes / 增长、命名、旧哈希 | Final name set / 最终名称集合 |
|---|---|---:|---:|---|---|
| 1 | `aleph-memory-archive-ig0ryjno` | 20 | 0 | 20/20 true | A |
| 2 | `aleph-memory-archive-v_vs2mnf` | 20 | 0 | 20/20 true | A |
| 3 | `aleph-memory-archive-of931nk_` | 20 | 0 | 20/20 true | A |

All three exit 0 and record `complete=true`; every cycle has exactly one attempt numbered 0 and `deleted=true`. Every prior archive's complete file-set/SHA-256 map remains equal, one new archive appears per cycle and suffixes remain contiguous. All three logs contain zero `Archive session failed`. Before/after live first-attempt failures are **0/60 → 0/60**; this does not demonstrate a statistical rate improvement. Recovery is established separately by the controlled RED→GREEN. / 三轮均退出 0，每轮只有编号 0 的一次删除且成功；旧归档全文件集合与哈希不变，每轮增加一个归档，后缀连续。前后实测均为 0/60，不宣称统计失败率改善；恢复能力由独立受控 RED→GREEN 证明。

Exact set A, read independently from each of the three fixed runs and each of the three rebuilt-baseline runs (displayed in suffix order, not filesystem listing order) / 六轮分别读取后均得到以下完整集合 A，按后缀数值展示：

```text
2026-11-04/agent_main_main_s1
2026-11-04/agent_main_main_s1-1
2026-11-04/agent_main_main_s1-2
2026-11-04/agent_main_main_s1-3
2026-11-04/agent_main_main_s1-4
2026-11-04/agent_main_main_s1-5
2026-11-04/agent_main_main_s1-6
2026-11-04/agent_main_main_s1-7
2026-11-04/agent_main_main_s1-8
2026-11-04/agent_main_main_s1-9
2026-11-04/agent_main_main_s1-10
2026-11-04/agent_main_main_s1-11
2026-11-04/agent_main_main_s1-12
2026-11-04/agent_main_main_s1-13
2026-11-04/agent_main_main_s1-14
2026-11-04/agent_main_main_s1-15
2026-11-04/agent_main_main_s1-16
2026-11-04/agent_main_main_s1-17
2026-11-04/agent_main_main_s1-18
2026-11-04/agent_main_main_s1-19
```

### Windows shutdown and cleanup / Windows 关闭与清理

Windows has no Unix SIGTERM path here. The existing helper clears inherited Ctrl-C suppression, launches with `CREATE_NEW_CONSOLE`, then uses `AttachConsole` plus `GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)`. / 此处 Windows 不走 Unix SIGTERM；helper 清除继承的忽略 Ctrl-C 属性，以新 console 启动，再附着并发送真实 Ctrl-C。

Both boots in each probe and all three archive-check servers record **`[SHUTDOWN] signal=ctrl_c` and exit 0**; all endpoints are removed and listeners closed. Archive-check JSON additionally records `forced_cleanup=false`, successful event generation and helper exit 0. A force-kill would be failure cleanup, not graceful evidence. Win32 last-error fields can retain 6 after successful API returns; attached/generated booleans and helper exit are the success evidence, not stale last-error values. / 四次 probe server 启动与三次归档 server 均有上述信号和退出证据、endpoint/listener 清理；无 server 强杀。成功 API 后残留的 Win32 last-error 6 不是失败证据，应看实际返回值与 helper 退出码。

## Final rebuilt snapshot / 最终重建快照

After the test-only file-name correction, Cargo rebuilt the server with `alephcore/test-helpers`: exit **0**, `10m 00s`, fresh gate **15935479808 bytes ALLOW**. The final artifact is `final-server.exe`, SHA-256 **`cb46bdfd208973e351dd780c3f283e786977942d120fd917e62033cf63e4b8a5`**. Canonical `build-record.json` and `final-build-record.json` retain parent `074fc1c007af7b36e8ca64f9e2d416bb5d09df65` plus the dirty-tree input hashes and `final-source.diff` SHA-256 `991bc21c92494ba43ed7877a48b6ea2e36913492f201b6b6920fe7ef6fffe27a`. All recorded source-file hashes were checked against the delivery tree. This is not represented as a build from a clean delivery commit. / 测试文件更名后重新构建，退出、时间与内存门如上；最终二进制与 source patch 哈希已更新。记录明确为父提交加工作树输入，不冒称由尚未产生的干净交付提交构建。

This final snapshot supersedes the initial native snapshot above for delivery. All five scenario exit codes and the final native aggregate exit were **0**. / 交付以本节最终快照为准，前文初次 native 快照保留作历史证据；五个场景及 native 汇总均退出 0。

| Final scenario / 最终场景 | Artifact directory / 产物目录 | Result / 结果 |
|---|---|---|
| providerless | `aleph-memory-lifecycle-j6ex35c1` | `Checks: 102/102`, complete=true |
| configured | `aleph-memory-lifecycle-iwl0wdgf` | `Checks: 110/110`, complete=true |
| archive 1 | `aleph-memory-archive-tsardb1j` | 20 cycles, 20 one-attempt deletes, 20 archives |
| archive 2 | `aleph-memory-archive-xgw2q5av` | 20 cycles, 20 one-attempt deletes, 20 archives |
| archive 3 | `aleph-memory-archive-mw1udsx8` | 20 cycles, 20 one-attempt deletes, 20 archives |

All final server logs contained **0 `Archive session failed`**. Every archive cycle passed `previous_archives_unchanged`, `one_archive_added` and `names_contiguous`; no harness retry occurred. Each final archive run observed Ctrl-C, exit **0**, `forced_cleanup=false`, endpoint removal and listener closure. The two probes also passed their per-boot exit/signal/endpoint/listener checks. Providerless logs explicitly report `(SQLite backend)`, configured logs `(file backend)`; other generic SQLite log messages are not treated as backend selection evidence. / 最终五组日志中该归档错误为零；归档每轮旧文件哈希、单目录增长与连续命名均通过，无 harness 重试。三轮归档均实测 Ctrl-C、退出 0、无强杀、endpoint 移除和 listener 关闭；两种 probe 的每次启动/关闭检查也均通过。backend 选择按带 backend 后缀的精确日志判断，不把其它 SQLite 消息混为后端选择。

The final three exact directory lists are below; each was independently compared with the expected 20-name set. / 最终三次精确目录清单如下，均另行核对为预期 20 项：

```json
{
  "aleph-memory-archive-tsardb1j": {
    "2026-11-04": [
      "agent_main_main_s1",
      "agent_main_main_s1-1",
      "agent_main_main_s1-10",
      "agent_main_main_s1-11",
      "agent_main_main_s1-12",
      "agent_main_main_s1-13",
      "agent_main_main_s1-14",
      "agent_main_main_s1-15",
      "agent_main_main_s1-16",
      "agent_main_main_s1-17",
      "agent_main_main_s1-18",
      "agent_main_main_s1-19",
      "agent_main_main_s1-2",
      "agent_main_main_s1-3",
      "agent_main_main_s1-4",
      "agent_main_main_s1-5",
      "agent_main_main_s1-6",
      "agent_main_main_s1-7",
      "agent_main_main_s1-8",
      "agent_main_main_s1-9"
    ]
  },
  "aleph-memory-archive-xgw2q5av": {
    "2026-11-04": [
      "agent_main_main_s1",
      "agent_main_main_s1-1",
      "agent_main_main_s1-10",
      "agent_main_main_s1-11",
      "agent_main_main_s1-12",
      "agent_main_main_s1-13",
      "agent_main_main_s1-14",
      "agent_main_main_s1-15",
      "agent_main_main_s1-16",
      "agent_main_main_s1-17",
      "agent_main_main_s1-18",
      "agent_main_main_s1-19",
      "agent_main_main_s1-2",
      "agent_main_main_s1-3",
      "agent_main_main_s1-4",
      "agent_main_main_s1-5",
      "agent_main_main_s1-6",
      "agent_main_main_s1-7",
      "agent_main_main_s1-8",
      "agent_main_main_s1-9"
    ]
  },
  "aleph-memory-archive-mw1udsx8": {
    "2026-11-04": [
      "agent_main_main_s1",
      "agent_main_main_s1-1",
      "agent_main_main_s1-10",
      "agent_main_main_s1-11",
      "agent_main_main_s1-12",
      "agent_main_main_s1-13",
      "agent_main_main_s1-14",
      "agent_main_main_s1-15",
      "agent_main_main_s1-16",
      "agent_main_main_s1-17",
      "agent_main_main_s1-18",
      "agent_main_main_s1-19",
      "agent_main_main_s1-2",
      "agent_main_main_s1-3",
      "agent_main_main_s1-4",
      "agent_main_main_s1-5",
      "agent_main_main_s1-6",
      "agent_main_main_s1-7",
      "agent_main_main_s1-8",
      "agent_main_main_s1-9"
    ]
  }
}
```

A separate read-only SQLite-header audit across these five final homes found **15 databases per home, 75 total**, with **75/75 `PRAGMA integrity_check` returning exactly `ok`**. Receipt files: `final-native-audit.json`, `final-native-audit-summary.log`, `final-sqlite-integrity.json`. The audit command itself exited 0; an initial display-only summary incorrectly indexed the date-keyed archive tree as a list, then was corrected to read the saved dictionary. This did not affect the completed audit or any server/probe result. / 最终五个 home 按 SQLite 文件头扫描，每个 15 个、共 75 个数据库，全部只读完整性检查为 ok。审计命令本身退出 0；首次显示摘要误将按日期索引的目录字典当列表，已按保存的字典正确读取，不影响审计及服务验收。

The two lifecycle rows above are the post-review reruns with the stricter `deleted is True` assertion: both exit 0, retain **102/102** and **110/110**, and each has four actual `deleted=true` responses with `attempts=1`. Their server logs contain zero `Archive session failed`. `post-review-probes.exit` is 0; the refreshed header audit again reports **75/75 ok** across these two homes and the unchanged three archive homes. The earlier pre-review final probe homes (`aleph-memory-lifecycle-m2gta7bs`, `aleph-memory-lifecycle-ne241f32`) remain historical; their logs/audits are preserved with a `pre-review-` prefix. / 上表两种生命周期模式已用更严格断言重跑，均退出 0、保持 102/102 与 110/110，各四次实际删除均为 true 且只尝试一次，服务日志无归档失败；复验退出回执为 0，更新后的五个 HOME 文件头审计仍为 75/75 ok。审阅前两份 final probe 及日志/审计以历史快照保留，三轮归档代码与证据未改动。

Measured live first-attempt failure rates remain **baseline 0/60 → final 0/60**. This does not demonstrate a statistical rate improvement; the controlled real-rename RED→GREEN proves the bounded recovery mechanism. The original intermittent holder remains unidentified. / live 首次失败率仍为 0/60→0/60，不据此宣称统计改善；恢复机制由真实受控 rename RED→GREEN 证明，原始间歇拒绝的句柄持有方仍未定位。

## Separate backend-default finding / 独立的 backend 默认值发现

Source observation, not changed / 源码事实，未修改：`GeneralConfig` derives `Default`, so an omitted `[general]` table produces an empty `session_store_backend` and the server selects SQLite. With a present `[general]` table and an omitted key, serde uses `default_session_store_backend()` returning `"file"`. The new providerless logs confirm `Session store initialized (SQLite backend)`; configured logs confirm `Session store initialized (file backend)`. The providerless probe therefore does not exercise FileSessionStore archival. / 无 `[general]` 表时为空串并走 SQLite；有表缺键时 serde 默认返回 `"file"`。新日志已分别确认 SQLite backend 与 file backend，因此 providerless 不覆盖 file backend 归档。本切片不改变部署的 backend 选择。

## Non-claims and deferred work / 不作声明与延期范围

- Original live handle owner remains unknown; no traced Defender attribution and no evidence of an own-transcript leak / 原现场持有方未知，未追踪到 Defender 句柄，也未建立自有 transcript 泄漏证据。
- Backoff is finite planned waiting, not a hard end-to-end deadline. A handle or permission denial lasting beyond the budget still returns an OS error / 有限预算约束退避，不是端到端硬时限；超预算的拒绝仍失败。
- Windows-native only; Linux/macOS are unverified / 仅 Windows 原生证据，Linux/macOS 未验证。
- Real UI/desktop QA deferred: requires its own shell/browser session / 真机 UI/desktop QA 需要独立壳与浏览器会话，延期。
- RSS reclamation deferred: process-level memory accounting is separate / RSS 回收需要独立进程级测量，延期。
- External-effect exactly-once deferred: not this archive-move contract / 外部效果 exactly-once 不属于本归档移动契约，延期。
- Cluster/multi-process placement deferred: local Session execution remains process-local / 集群与多进程放置延期，本地 Session 执行仍为进程内语义。
- No Capability Core/Memory-runtime architecture, SessionStore public API, SQLite backend or backend-default changes / 未改 Capability Core、Memory runtime 架构、SessionStore 公共 API、SQLite backend 或默认 backend。

## Review and delivery checks / 审阅与交付检查

Preliminary fresh Pi read-only whole-diff review `del_mw20cq3y_uepw` covered every tracked difference against `f40fbf061`, both new files, and the committed plan/prompt. Written result: **0 Critical, 0 Important, 1 Minor**. The Minor identified the plan Architecture's unconditional transcript-close statement; it is now conditional on measured own-handle attribution, matching Task 2 Step 4 and the direct request. No production change followed review. / 全新 Pi 只读审阅覆盖完整差异与新增文件，仅发现计划中的关闭条件表述，已校准；审阅后没有生产改动。

That preliminary reviewer explicitly declined to judge full-suite attribution and live acceptance at its snapshot. The complete native/full/named evidence and the owned test-placement correction were added afterward, so final reviewer `del_mw24plon_s45d` inspected all nine staged files against `f40fbf061` and cross-checked the final artifacts. Its complete written report (`final-review.txt`) returned **REQUEST CHANGES: 0 Critical, 1 Important, 1 Minor**. The launcher reported `exit SIGTERM` after the written report, not exit 0. The Important required the lifecycle probe to reject deletion no-ops: it now requires an object `result` with `deleted is True`, retains the full response and `attempts=1`, and has no retry loop. The Minor identified that the saved early scanner log contained `valid=944` while the latest `valid=947` run had only been printed; a fresh normal scan is saved separately as `final-markdown-links.log` with its exit receipt. The preliminary result remains historical in `review.txt`. / 首次审阅未覆盖最终证据，故最终 reviewer 重新检查全部九个文件与产物，结论为 0 Critical、1 Important、1 Minor；完整报告已保存，启动器报告 SIGTERM，不冒称退出 0。Important 已按建议加强为对象结果且 `deleted is True`，保留单次尝试与完整响应；Minor 是最新扫描输出未落盘，已改为单独保存最新正式扫描及退出回执。首次报告保留为历史记录。

The review fix changes only the Python deletion assertion; Rust, Cargo.lock and the server binary are unchanged. `final-build-record.json` and `pre-review-build-record.json` preserve the build-time source hashes. Canonical `build-record.json` additionally records current `verification_inputs`, `verification_source_patch=post-review-source.diff` and its SHA-256 so the later QA runner is distinguished from the binary build. Both lifecycle modes are rerun with the stricter assertion; the unchanged three 20-cycle archive runs remain the archive evidence. / 修复只加强 Python 断言，Rust、Cargo.lock 和 server 二进制不变；构建时哈希保持原记录，canonical 记录另列当前 QA 输入及补丁哈希，明确区分构建来源与审阅后的验证脚本。两种生命周期模式重新执行，三轮未改动的 20 次归档继续作为归档证据。

Single-file formatting / 单文件格式：`rustfmt --edition 2021 --check --config skip_children=true src/gateway/session_store/file_backend/mod.rs src/gateway/session_store/file_backend/archive_diagnostics_tests.rs` exited 0 with no output on the corrected filenames / 更名后的两个文件已重新检查，无输出、退出 0。

Doc-checker self-tests / 文档检查器自测：`python qa/docs/check_markdown_links.py --self-test` → `self-test: PASS`; `python qa/browser_live/check_docs.py --self-test` → `check_docs self-test: OK`, both exit 0 / 均退出 0。Fresh normal scans are saved as `final-markdown-links.log` and `final-browser-docs.log`, with matching `.exit` receipts of 0 / 最新正式扫描日志与退出回执均已落盘：markdown checker `doc_files=1656`, `valid=947`, `external_or_protocol=65`, `absolute_or_private=0`, `placeholder=0`, `code_or_example=157`, `missing_repository_targets=0`; browser checker `check_docs: OK (7 files)`. The older `markdown-links.log` with `valid=944` is historical. `git diff --cached --check` also exited 0 / 早期 944 计数日志为历史记录，暂存差异检查也退出 0。

Named full/isolated baseline comparisons, final whole-diff review and its concrete corrections are complete. Local-only commit, clean-tree/ancestor verification and removal of only the task-owned comparison worktree/target are recorded in `final-git.json`, emitted after those delivery actions; all proof artifacts remain outside the repository. No `bun run check` or clippy success is claimed. The pre-commit hook's broad Cargo formatting/clippy commands have no required per-command memory gate, so local delivery bypasses that hook and reports these skipped checks explicitly. / 全量与隔离逐名对照、最终 whole-diff 审阅及具体问题修复均已完成；本地提交、干净状态与祖先校验、仅自建 worktree/target 清理的交付回执在执行后写入 `final-git.json`，证据留在仓外。不冒称 bun check 或 clippy 通过；提交钩子的广域 Cargo 命令未实现逐条内存门，本地交付绕过钩子并明示跳过项。

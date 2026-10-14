# Task 16 — Full Library Suite Static Attribution Report

## Bilingual Header / 双语标题

| | |
|---|---|
| **EN** | Full `--lib` suite static attribution for the current branch. Command: `target/debug/deps/alephcore-3f530ad86e8e47b1 --test-threads=4`. 21 107 total / 21 063 passed / 25 failed / 19 ignored / 1 328.33 s |
| **中文** | 当前分支全 `--lib` 套件静态归因。命令：`target/debug/deps/alephcore-3f530ad86e8e47b1 --test-threads=4`。21 107 总计 / 21 063 通过 / 25 失败 / 19 忽略 / 1 328.33 秒 |

---

## Contextual Evidence / 上下文证据

- **Command source**:
  ```
  target/debug/deps/alephcore-3f530ad86e8e47b1 --test-threads=4
  ```
  Source: `/home/zou/.pi/agent/sessions/--home-zou-data-workspace-Aleph--/2026-10-14T01-50-30-603Z_01a1381a-9ccb-76d9-a79f-44ac90ac1edb.jsonl` (line 6). The timestamp in the source message is `2026-10-14T01:50:31.200Z` — the timestamp of the message **reporting** completion, not the actual command end time. Absolute executable path, actual cwd, environment, timeout, and redirects are all **unknown**.
- **Worktree root**: `/home/zou/data/workspace/Aleph-sms-pstn-sdd` — reported source checkout; **not claimed** as execution-time cwd.
- **Exit status**: source records `exit 101`; log itself does not embed exit-status.
- **Reported counts**: `21107 total, 21063 passed, 25 failed, 19 ignored, 0 measured/filtered, 1328.33s` (log line 21374).
- **Static attribution source**: all line-number references in this report point to the **committed Git object at HEAD** (`d13c23c4c`), not to any concurrent worktree state. Evidence drawn from `/tmp/acp-delegate/del_mv7irk9g_9dub.out`.

---

## Git Object Comparison / Git 对象比较

| | |
|---|---|
| **HEAD** | `d13c23c4c8aeb44f17a5c7bd44fd586f9fe29cae` |
| **Merge-base** | `48d75f9b646773f6c3acba99cb6b61bdbebf7811` |

All attribution below is **static Git-object comparison** — no baseline run was performed, no concurrent worktree state was used. "Baseline" refers to the same-code, same-defect evidence in both commits, **not** a merge-base actual test run result.

---

## Classification Legend / 分类说明

| Tag | 含义 / Meaning |
|------|----------------|
| **B** | 基线缺陷 — 两提交间代码未变；静态比较确认同样违规存在于 HEAD 与 merge-base。分支未引入。 / Baseline defect — code unchanged at both commits; static comparison confirms same violation at HEAD and merge-base. Not introduced by the branch. |
| **E** | 环境 / 共享状态 / 竞态 — 静态分析无法确定失败是否由分支触发或早已存在。测试及周围代码未变；执行环境（HOME、子模块、PTY spawn、store）提供可能的解释，但归因未证实。 / Environment / shared-state / race condition — static analysis cannot determine if failure is branch-triggered or pre-existing. Environment and shared state provide possible explanations, not a proven attribution. |
| **Δ+B** | Delta + 基线 — 既有失败被分支新增违规代码扩大。不可将整个失败归因于分支，也不可忽略新增部分。需单独分支修复。 / Delta + Baseline — pre-existing failure amplified by branch-new violating code. Full failure not attributable to branch alone; new violations cannot be ignored. Separate branch repair required. |

---

## 25-Row Failure Table / 25 行失败表

> Log 列：`/tmp/aleph-telecom-task16/full-lib-d13c23c4c.log:<行号范围>`。Source 列：完整绝对路径，不使用别名。

| # | `module::test` | Log / 日志 | Source / 源码 | 错误与机制 / Error & Mechanism | 分类 / Class |
|---|---|---|---|---|
| 1 | `a2a::adapter::server::routes::tests::extract_credentials_bearer_mixed_case` | L114, 21145–21148 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/a2a/adapter/server/routes.rs:419` | 恐慌：`Expected BearerToken, got None`；`extract_credentials` 仅匹配小写 `"bearer "` 或精确 `"Bearer "`；混合大小写头无法匹配。/ panic: `extract_credentials` only matches lowercase `"bearer "` or exact `"Bearer "`; mixed-case header not matched. | **B** |
| 2 | `builtin_tools::terminal::tests::a_closed_session_read_preserves_the_old_error_text` | L3626, 21151–21154 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/builtin_tools/terminal/tests.rs:1044` | 恐慌：`spawn: "spawn_command failed: No such file or directory (os error 2)"`；测试在关闭会话文本断言之前到达 spawn。PTY/默认 shell 环境未指定；日志无法确认缺失二进制。代码未变。/ panic: test reaches spawn before closed-session text assertion; PTY/shell env unspecified. | **E** |
| 3 | `cluster::node_file_cmd::tests::file_read_rejects_missing` | L4004, 21156–21159 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/cluster/node_file_cmd.rs:362` | 恐慌：`file.read: No such file or directory (os error 2)`；测试要求错误信息包含 `"not found"`；实现直接格式化 OS 错误。/ panic: test requires `"not found"` in message; impl formats raw OS error. | **B** |
| 4 | `cluster::node_file_cmd::tests::file_read_rejects_oversize` | L4005, 21161–21164 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/cluster/node_file_cmd.rs:377` | 恐慌：`file.read: >8388608 bytes (cap enforced at read)`；测试要求 `"exceeds"`；容量限制有效但消息措辞不匹配。/ panic: cap enforcement works but message lacks `"exceeds"`. | **B** |
| 5 | `config::tests::skill_doc_drift::no_bundled_snippet_hardcodes_the_aleph_config_path` | L4284, 21166–21169 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/config/tests/skill_doc_drift.rs:948` | 恐慌：`only 0 file(s) in BUNDLED_SKILLS — the skills/ submodule not checked out`；嵌入技能树为空；自防御断言触发。/ panic: embedded skills tree empty; self-defense assertion fires. | **E** |
| 6 | `config::tests::skill_doc_drift::the_bundled_self_skill_names_no_phantom_config_keys` | L4286, 21171–21174 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/config/tests/skill_doc_drift.rs:541` | 恐慌：`self/SKILL.md is not in the embedded skills tree. Either the file moved or skills/ submodule not checked out`。/ panic: embedded file missing; skills/ submodule not initialised. | **E** |
| 7 | `config::tests::validation::test_invalid_regex_patterns_fail` | L4311, 21176–21179 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/config/tests/validation.rs:54` | 恐慌：`expected validation to reject this config: ()`；`validate_rules()` 返回 `Ok(())` 而不拒绝无效正则。测试与测试逻辑未变。/ panic: `validate_rules()` returns `Ok(())` without rejecting invalid regex; tests and test-adjacent logic unchanged. | **B** |
| 8 | `config::tests::validation::test_rule_referencing_unknown_provider_fails` | L4315, 21181–21184 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/config/tests/validation.rs:54` | 同上恐慌；`validate_rules()` 不拒绝未知 provider 引用。测试未变；validate.rs 因 public-history 变动而有变更，非全文件同 blob。/ Same panic; tests unchanged; `validate.rs` changed due to public-history additions — not blob-identical across commits. | **B** |
| 9 | `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation` | L6265, 21186–21189 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/extension/validation.rs:356` | 恐慌：扫描到 0 个 plugin manifest；`plugins/` gitlink 目录为空。/ panic: scan found 0 manifests; `plugins/` gitlink empty. | **E** |
| 10 | `gateway::execution_engine::btw_wire_tests::no_shipped_command_word_resolves_as_a_side_question` | L6827, 21191–21194 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/execution_engine/btw_wire_tests.rs:2180` | 恐慌：bundled walk 解析到 0 skill 和 0 plugin 目录；`skills/` 与 `plugins/` 子模块未初始化导致提前恐慌。/ panic: empty submodules cause premature panic before command-conflict assertion. | **E** |
| 11 | `gateway::execution_engine::run_loop::tests::project_skill_block_none_when_no_project_skills` | L6972, 21196–21199 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/execution_engine/run_loop/tests.rs:209` | 恐慌：`assertion failed: collect_project_skill_block(tmp.path()).is_none()`；函数仍读全局技能，仅排除 `$HOME/.aleph` 和 `$HOME/.claude`；遗漏 `$HOME/.agents`。/ panic: function excludes only `.aleph` and `.claude`; misses `.agents`; possibly affected by HOME/ALEPH_HOME. | **E** |
| 12 | `gateway::pty::manager::tests::spawning_a_pty_journals_intent_then_pid_and_its_exit_settles_the_row` | L9997, 21201–21204 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/pty/manager.rs:922` | 恐慌：`JobRecord { … phase: Settled, exit_code: Some(3), pid: None }`；reader 早于 manager 写 PID；若先 settle，PID stamp 对已结束行为 no-op。静态兼容此顺序，无法确认原 4 线程调度下该顺序是否发生。/ panic: reader may settle before manager writes PID; static analysis compatible with this order; cannot confirm from log alone. | **E** |
| 13 | `gateway::visibility::tests::every_ambient_owner_call_site_is_in_the_census` | L11014, 21206–21212 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/visibility.rs:1300` | **Δ+B**：恐慌列出 3 个不在 `AMBIENT_OWNER_CENSUS` 的调用点：<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/caller_identity.rs:375` — **既有**（merge-base 已存在）。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/orchestrator/harness_bridge/public_spend_tests.rs:87` — **分支新增**：测试函数名含 `_ambient_owner()`；substring 守卫误将函数名识别为调用。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/providers/metering.rs:1207` — **分支新增**：同上机制，函数名含 `_ambient_owner()`。<br>守卫扫描整个文件，不区分函数名与真实调用点。/ Δ+B: 3 sites; V13b/c are branch-new test-naming artifacts; V13a is pre-existing. | **Δ+B** |
| 14 | `harness::tests::budget::the_harness_line_budget_does_not_grow` | L12094, 21214–21217 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/harness/tests/budget.rs:871` | 恐慌：`src/harness/ grew to 5608 budgeted lines, over the frozen ceiling of 5545`（超 63）。两提交独立统计均为 5608；12 个计量文件全 blob 相同。`task10_wiring/mod.rs`（分支修改）不在计量集合。不是 telecom 引入。/ panic: both commits independently yield 5608; all 12 metered files blob-identical; not a telecom regression. | **B** |
| 15 | `memory::dreaming::stages::goal_lessons_promote::tests::no_goal_store_is_graceful_noop` | L13333, 21277–21282 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/memory/dreaming/stages/goal_lessons_promote.rs:302` | 恐慌：`assertion left == right failed: left: 4, right: 0`；测试假设默认 stage `None` 意味着"不从全局 store 读"，但 `None` 实际读全局 store。left=4/right=0 可能是全局 store 中有遗留数据；来源及排程 UNKNOWN，无法确定来自哪个测试。/ panic: `None` reads global store; left=4/right=0 may reflect prior data in global store; source and scheduling UNKNOWN. | **E** |
| 16 | `memory::dreaming::tests::a_project_sub_cycle_lands_in_the_audit_table_under_its_namespace` | L13464, 21284–21287 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/skill/mod.rs:827` | 恐慌：`default_skill_dirs resolved the developer's REAL Claude root (/home/zou/.claude/skills) inside a lib test`；HOME 未隔离；触发真实家目录保护。/ panic: real `.claude/skills` read inside a lib test; HOME not isolated. | **E** |
| 17 | `memory::dreaming::tests::every_corpus_family_is_maintained_under_the_default_config` | L13486, 21289–21292 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/skill/mod.rs:827` | 同 #16；相同机制。/ Same panic and mechanism as #16. | **E** |
| 18 | `memory::dreaming::tests::run_dream_executes_pipeline_not_stub` | L13488, 21294–21297 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/skill/mod.rs:827` | 同 #16；非 pipeline-stub 回归。/ Same panic; not a pipeline-stub regression. | **E** |
| 19 | `memory::dreaming::tests::run_now_persists_an_audit_row_with_its_decision` | L13508, 21299–21302 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/skill/mod.rs:827` | 同 #16；尚未到达 audit-row 断言。/ Same panic; not yet at audit-row assertion. | **E** |
| 20 | `memory::dreaming::tests::the_nightly_fan_out_stops_at_the_configured_budget` | L13532, 21304–21307 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/skill/mod.rs:827` | 同 #16；非 configured-budget 断言失败。/ Same panic; not a configured-budget assertion failure. | **E** |
| 21 | `tool_metadata::types::unified::builders::populate_safety_profile_tests::read_only_with_confirmation_still_irreversible_high_risk` | L19687, 21309–21314 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/tool_metadata/types/unified/builders.rs:228` | 恐慌：`assertion left == right failed: left: ReadOnly, right: IrreversibleHighRisk`；实现仅在 `!read_only && requires_confirmation` 时提升 safety level；测试要求 confirmation 始终优先。/ panic: impl only promotes safety when `!read_only && requires_confirmation`; test requires confirmation to always override. | **B** |
| 22 | `tool_output::compressor::tests::test_compress_screenshot_long_alphanumeric_not_base64` | L19713, 21316–21321 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/tool_output/compressor.rs:585` | 恐慌：50 000 个 A 被压缩为精确 500 个 A 加 "…"；测试要求保留原始 50 000。/ panic: 50 000 `A` compressed to exactly 500 `A` + "…"; test expects full 50 000 preserved. | **B** |
| 23 | `utils::source_scan::tests::no_module_hand_rolls_the_cfg_test_prefix_cut` | L20762, 21323–21332 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/utils/source_scan.rs:2687` | **Δ+B**：恐慌列出 6 个手工进行 `#[cfg(test)]` 前缀剪裁的站点：<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/busy_queue/durable.rs:1035` — **分支新增**。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/run_origin.rs:233` — **分支新增**。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/private_read.rs:255` — **分支新增**。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/persisted_marker.rs:403` — **分支新增**。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/persisted_marker.rs:405` — **分支新增**。<br>• `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:119` — **既有**（merge-base 未变）。<br>守卫本身（`src/utils/source_scan.rs:2659–2691`）两提交 blob 相同。/ Δ+B: 6 sites; V23a–V23e are branch-new; V23f is pre-existing; guard blob-identical. | **Δ+B** |
| 24 | `verification::stop_hooks::tests::is_shell_safe_accepts_windows_path_and_rejects_shell_metachars` | L20821, 21334–21337 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/verification/stop_hooks.rs:746` | 恐慌：`"" should be rejected`；测试先将空串放入拒绝列表，随后又断言空串应被允许；`chars().all(…)` 对空串返回 `true`。/ panic: test inserts empty string into reject list then asserts it should be allowed; `chars().all(…)` returns `true` for empty input. | **B** |
| 25 | `verification::stop_hooks::tests::safe_chars_snapshot_is_locked` | L20822, 21339–21344 | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/verification/stop_hooks.rs:725` | 恐慌：`assertion left == right failed`；两数组字符完全相同但顺序不同：`left` 以 `['a'…'z','A'…'Z'…]` 开头，`right` 以 `[' ','"','\'','/',…]` 开头。/ panic: identical characters in different order; `left` starts with alpha, `right` with whitespace/punct. | **B** |

---

## Fresh Environment Probes / 新鲜环境探针

主会话随后使用当前编译的 lib binary，针对全部 13 项 E 类失败逐项 `--exact --test-threads=1` 重跑；结果与命令记录在 `/tmp/aleph-telecom-task16/env-reruns/results.json`。/ The parent subsequently reran all 13 E-class failures individually against the current lib binary; each used `--exact --test-threads=1`.

- 原环境单独重跑：3 passed（terminal closed-session、PTY journaling、goal-lessons no-store），10 failed。这只证明单独可通过，不证明具体竞态根因。/ Three passed in isolation; this does not prove a particular race mechanism.
- 6 项原环境仍失败的测试，在每项独立临时 `HOME`、`ALEPH_HOME`、XDG 路径下全部通过：project-skill-empty 与 5 项 dreaming pipeline 测试。精确日志与临时目录记录在 `/tmp/aleph-telecom-task16/env-reruns/isolated-home-results.json`；不修改真实 home、项目代码或测试断言。/ All six passed with fresh per-test temporary home directories, without source/assertion changes.
- 其余 4 项仍失败：2 个 skill-doc 守卫、plugin installer validation、btw bundled-command census。错误明确报告缺失的 skills/plugins 子模块、扫描到 0 个文件／manifest。未初始化／下载子模块，未把零覆盖改成 skip。/ Four still fail on missing skills/plugins submodules; no submodule download or zero-coverage skip was performed.

修复新增守卫违规后的相关测试也用已重新编译的同一 binary 逐组串行重跑并通过：`gateway::telecom` 157、`gateway::busy_queue::durable` 16、`public_spend_tests` 2、`providers::metering` 15；日志依次位于 `/tmp/aleph-telecom-task16/telecom-guard-fixes.log`、`/tmp/aleph-telecom-task16/durable-guard-fixes.log`、`/tmp/aleph-telecom-task16/public-spend-guard-fixes.log`、`/tmp/aleph-telecom-task16/metering-guard-fixes.log`。/ The affected canary groups passed after rebuilding: 157, 16, 2 and 15 tests respectively.

这些定向探针增加了归因证据，**不替代完整当前快照测试，也不表示原 full-lib 结果变成通过**。/ These probes strengthen attribution but do not replace a full current-snapshot run or turn the original failing full suite green.

## Branch-Introduced Violations Only / 分支新增违规条目

The Δ+B rows carry two distinct subsets. The following are **only the branch-new violations**, stated without claiming the full test failure is branch-caused.

### #13 — `every_ambient_owner_call_site_is_in_the_census` (Δ+B)

| Violation | Source | Nature |
|-----------|--------|--------|
| V13a | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/caller_identity.rs:375` | **Pre-existing** — real call to `scope::ambient_owner()` present at merge-base. |
| V13b | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/orchestrator/harness_bridge/public_spend_tests.rs:87` | **Branch-new** — test function name ends with `_ambient_owner()`; substring guard mis-identifies function name as a call. Not a production permission leak. |
| V13c | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/providers/metering.rs:1207` | **Branch-new** — same mechanism: `async fn …_ambient_owner()` name string-matches the guard pattern. |

### #23 — `no_module_hand_rolls_the_cfg_test_prefix_cut` (Δ+B)

| Violation | Source | Nature |
|-----------|--------|--------|
| V23a | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/busy_queue/durable.rs:1035` | **Branch-new** — hand-rolls the production-prefix cut instead of calling `production_prefix()`. |
| V23b | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/run_origin.rs:233` | **Branch-new** — same category. |
| V23c | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/private_read.rs:255` | **Branch-new** — same category. |
| V23d | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/persisted_marker.rs:403` | **Branch-new** — same category. |
| V23e | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/persisted_marker.rs:405` | **Branch-new** — same category (adjacent line). |
| V23f | `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:119` | **Pre-existing** at merge-base (unchanged). |

---

## Summary Counts / 汇总

| Category | Count | Items |
|----------|-------|-------|
| **B** — 基线静态缺陷 / Baseline static defects | **10** | #1, #3, #4, #7, #8, #14, #21, #22, #24, #25 |
| **E** — 环境 / 共享状态 / 竞态 / Environment, shared state, races | **13** | #2, #5, #6, #9, #10, #11, #12, #15, #16, #17, #18, #19, #20 |
| **Δ+B** — 既有扩大 / Preexisting amplified | **2** | #13 (V13a pre-existing, V13b/c branch-new), #23 (V23a–V23e branch-new, V23f pre-existing) |
| **Total failed / 失败合计** | **25** | — |

---

## Historical Rerun Plan and Fresh Targeted Results / 原重跑计划与新鲜定向结果

文档作者未执行 cargo。主会话随后使用当前修复版本编译出的 lib test binary，分别以 `--test-threads=1` 运行下列两个过滤器；各实际运行 1 个测试，均 exit 101。/ The document author ran no cargo. The parent subsequently ran each filter against the lib test binary compiled with the current fixes; each executed one test and exited 101.

- `/tmp/aleph-telecom-task16/source-scan-guard-after-fix.log`：仅报告 `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:119`；本分支新增的五个违规站点已不再出现。/ Only the pre-existing MCP site remains; the five branch-new sites are absent.
- `/tmp/aleph-telecom-task16/visibility-guard-after-fix.log`：仅报告 `/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/caller_identity.rs:375`；两个新测试名已不再被识别为违规。/ Only the pre-existing caller identity site remains; the two new test names no longer trigger the guard.

这验证了新增违规已从守卫诊断中消除，**不表示两个守卫通过，也不表示全库全绿**。以下保留原计划命令；主会话实际命令用相同 filter、未带 `--exact`。/ This establishes removal of the new violations from guard diagnostics, not passing guards or a green full suite. The original planned commands remain below; the parent used the same filters without `--exact`.

### #23 — `no_module_hand_rolls_the_cfg_test_prefix_cut`
```
target/debug/deps/alephcore-3f530ad86e8e47b1 --test-threads=1 utils::source_scan::tests::no_module_hand_rolls_the_cfg_test_prefix_cut --exact
```
修复 V23a–V23e 后，`src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:119`（既有 MCP 违规）仍会导致测试失败，除非 main 批准单独基线修复。

After fixing V23a–V23e, the pre-existing MCP violation at `mcp.rs:119` will still cause the test to fail, unless main approves a separate baseline fix.

### #13 — `every_ambient_owner_call_site_is_in_the_census`
```
target/debug/deps/alephcore-3f530ad86e8e47b1 --test-threads=1 gateway::visibility::tests::every_ambient_owner_call_site_is_in_the_census --exact
```
修复 V13b/V13c 后，`src/gateway/caller_identity.rs:375`（既有调用）仍会导致测试失败，除非 main 批准单独基线修复。

After fixing V13b/V13c, the pre-existing call at `caller_identity.rs:375` will still cause the test to fail, unless main approves a separate baseline fix.

---

## 4-Thread Limitations / 4 线程局限性说明

原 4 线程调度的以下局限**已知但未验证**：

- `HOME` / `ALEPH_HOME` 跨测试共享
- 全局 goal store 跨测试污染
- PTY reader 与 manager 交错顺序未定义

静态分析无法确认 13 个 E 类失败在原 4 线程下的行为；不得断言"基线必失败"或"分支必无关"；1 线程计划不能证明原 4 线程无竞态。

The following limitations of the original 4-thread scheduling are **known but unverified**:

- `HOME` / `ALEPH_HOME` shared across tests
- Global goal store cross-test pollution
- PTY reader-manager interleaving ordering undefined

Static analysis cannot confirm the behaviour of the 13 E-class failures under the original 4-thread scheduling; "baseline must fail" or "branch unrelated" cannot be asserted; a 1-thread plan cannot prove the original 4-thread run had no races.

---

## Notes on New Prefix-Cut Violations / 新增 Prefix-Cut 违规说明

五处新增违规（V23a–V23e）均手工实现了生产代码前缀剪裁逻辑。根据 `src/utils/source_scan.rs:59,168`，正确做法是调用 `production_prefix()` 或 `production_text()`——这些函数已在同 crate 中定义。最小修复方案是将五处迁移至调用共享函数，而非扩展豁免名单。注意：既有 MCP 违规（`/home/zou/data/workspace/Aleph-sms-pstn-sdd/src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:119`）在 merge-base 即已存在，独立于 telecom 变更，需单独处理。

The five new violations (V23a–V23e) hand-roll production-prefix logic. Per `src/utils/source_scan.rs:59,168`, the correct approach is to call `production_prefix()` or `production_text()` — already defined in the same crate. Migrate the five sites to the shared function without expanding the exemption list. The pre-existing MCP violation at `mcp.rs:119` predates the telecom branch and requires separate handling.

---

## What Was NOT Done / 未做事项

以下明确排除在本任务之外：

- **No cargo test execution** / 未执行 cargo test — 未运行 `cargo test`、`cargo check`、`cargo clippy` 或任何变体。
- **No baseline run** / 未执行基线运行 — merge-base 套件未执行；所有"基线"评估均为静态 Git 对象比较。
- **No code repairs** / 未修复代码 — 未修改任何源文件。
- **No staging or committing** / 未 stage 或 commit — 未执行 git 操作（仅读取已提交对象）。
- **No real or simulated provider calls** / 未联系真实或模拟 provider — 未联系、购买或部署任何 live SMS/PSTN provider。
- **Only new report added; no existing docs or source modified** / 仅新增本报告；未修改任何现有文档或源码。
- **No reruns executed** / 未执行重跑 — 上文计划命令仅作记录，未执行。
- **No updates to protected docs** / 未更新受保护文档 — 以下文件保持原样：
  - `/home/zou/data/workspace/Aleph-sms-pstn-sdd/.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-3c-report.md`
  - `/home/zou/data/workspace/Aleph-sms-pstn-sdd/.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-4-loop-origin-fix-report.md`
  - `/home/zou/data/workspace/Aleph-sms-pstn-sdd/docs/superpowers/plans/2026-10-09-pstn-sms-thin-relay-implementation.md`
  - 以及任何 matrix、handoff、progress 或 voice 报告。
- **No 4-thread conclusions for E-class** / 4 线程局限未下结论 — 原 4 线程调度未完全表征；13 个 E 类失败无法断言"基线必失败"或"分支必无关"；1 线程计划不能证明原 4 线程无竞态。

---

## Delivery Status / 交付状态

| | |
|---|---|
| **File created** | `task-16-full-lib-attribution.md` — new document, add-only |
| **Other files modified** | None |
| **Full suite green** | **No** — 25 failures confirmed; this report does not establish Task 16 acceptance, does not establish code acceptance, and is not a production-ready completion assertion. |
| **Branch repairs pending** | Yes — #13 V13b/V13c and #23 V23a–V23e are flagged for separate worker tracking. The pre-existing MCP violation (V23f) and pre-existing caller_identity call (V13a) are out of scope for this branch; they require main-agent approval for separate baseline fixes. |

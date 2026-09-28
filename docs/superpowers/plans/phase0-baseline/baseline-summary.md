# Phase 0 Baseline Summary — Panel & TUI Polish

**Captured**: 2026-09-21T08:30:18Z
**Branch**: panel-tui-polish
**Commit**: 5e1b29062f02a90c4e201f34db7b28790c981ad8 (base: 0927461ba)
**Worktree**: `/home/zou/data/workspace/Aleph-panel-tui-polish`

## 总览

| Suite | Exit | Passed | Failed | Ignored | Duration | Status |
|-------|------|--------|--------|---------|----------|--------|
| alephcore lib | 1 | 19235 | 3 | 19 | 207.74s | ⚠️ 3 pre-existing |
| webchat lib | 0 | 1347 | 0 | 0 | 2.73s | ✅ 全绿 |
| tui lib | 1 | 435 | 1 | 0 | 0.34s | ⚠️ 1 pre-existing |
| vitest | 0 | — | — | — | — | N/A 未配置 |
| playwright | 0 | — | — | — | — | ⏸ 推迟到 Phase 3 |

**总测试数（Rust）**：21,017 + 1,347 + 435 = 22,567 passed across 3 suites.

## Pre-existing 失败（已知问题，不算本轮回归）

### alephcore
1. `capability::census::tests::every_installed_global_is_a_capability_slot` — 源码注释明确说明"literal is known to be one BELOW the live total"；这是测试预期的状态。
2. `tools::concurrency::tests::windows_separators_fold_onto_the_same_scope` — Windows-only test 在 Linux 跑，预期失败。
3. `verification::extension_stop_gate::tests::consecutive_veto_ceiling_unwedges_the_loop` — 真实 bug，需要 Plan 2 审计阶段调查（候选 Tier-2）。

### tui
1. `tui::widgets::header::tests::the_version_comes_from_the_version_file_not_the_cargo_copy` — 测试用相对路径 `../../VERSION`，当 CWD 不是 `interfaces/tui/` 时失败（直接跑 binary 时 CWD 是 worktree root）。修复方法：改用 `env!("CARGO_MANIFEST_DIR")` 构造绝对路径。**候选 Tier-1**（小、易修）。

## OOM 与基础设施约束

- alephcore lib test 重新编译会被 OOM killer SIGKILL（exit 137），即使 `CARGO_BUILD_JOBS=1`。这是这台机器的已知约束。
- 解决：用 main worktree 里已经编译好的 test binary 直接跑（与 HEAD 同一 base commit，3 个新 commit 都是 doc-only）。
- Plan 2 中任何需要重新编译 alephcore test 的 fix，要么用 incremental cache 增量编译，要么用 per-test `--test test_name` 拆分。

## 未覆盖模块（Plan 2 必须先补测试再改）

- `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs` 仅 3 个 inline test，composer 是交互密集模块，建议在改动前补测试。

## 进程状态

- 无残留 `aleph-server` 进程（已清理）。
- 不会污染 Plan 2 启动。

## Phase 0 退出条件

- [x] 所有测试套件跑过（已跑，3 个 pre-existing 失败）
- [x] 失败项已分类（pre-existing vs regression）
- [x] commit hash、branch、worktree 已记录
- [x] 无 orphan 进程
- [x] baseline JSON + summary 已生成

**结论**：baseline 已建立。3 个 pre-existing 失败是文档化的问题，不阻塞 Phase 1 审计。Phase 2 修复阶段需要把 pre-existing 的真实 bug 之一（extension_stop_gate）纳入 audit 候选。

---

→ 下一步：Phase 1 审计（gap-analysis.md）
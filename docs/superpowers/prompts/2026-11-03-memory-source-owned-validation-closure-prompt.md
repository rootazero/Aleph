# Restart Prompt: Memory source-owned validation closure / 重启提示：Memory source-owned 验证闭环

**Status / 状态:** Planned, not implemented / 已规划，尚未实施。

**Previous task merged to local main / 前序任务已提交到本地 main:** `a4fa8088f4dcd6f939909069da8e08266983accb docs(memory): record source-owned lifecycle boundaries`. The reconciler and compression implementation commits are its ancestors. The previous task was executed directly on `main`; no branch merge is needed. / reconciler 与 compression 实现提交均为其祖先。前序任务已直接在 `main` 上完成，无需伪造分支合并。

Copy the request below into a fresh session. Discover the active checkout from Git; do not assume an absolute path. / 将下方请求交给新会话。先从 Git 定位实际 checkout，不假设绝对路径。

---

请按 `docs/superpowers/plans/2026-11-03-memory-source-owned-validation-closure-plan.md` 执行下一有界任务，并先完整阅读前序证据记录 `docs/superpowers/runs/2026-11-03-memory-source-owned-boundary-run.md` 及本计划引用的 design。用户已授权在下一会话按计划补测试/证据；不要在本会话扩展范围或重新询问前序已批准的 source-owned Memory 架构。

## 目标

闭合前序 Memory source-owned boundary 切片留下的验证缺口，而不是继续扩展生产 lifecycle 架构：

1. 尽可能用真实 Admin API router/runtime fixture 补齐 `204` 首次 scan、Unavailable 状态和 restart 后 report-generation 区分的效果证据；只能使用现有确定性 fixture/barrier，不新增公开或生产 test-control API。无法安全确定性观测的行为明确记录为 gap，不通过 sleep/race 猜测。
2. 对前序 `cargo test -p alephcore --lib -- --test-threads=1` 的 51 个失败按完整测试名，在当前 `main` 与固定 pre-slice 基线 `063345f2f30bf999b28ee5d84aac860be8432fa7` 做同命令、串行对照。报告相同失败、仅当前失败、未复现和环境阻塞；不要仅用聚合计数归因，也不要修复无关失败。

## 起始状态和 Git 纪律

- 先核验 Git root、当前分支、worktree、HEAD 与 `4b6da265a` 祖先关系。前序已在本地 `main`，提交 `a4fa8088f`，无需再做 branch merge。
- 当前任务只在本地工作；禁止 fetch、push、merge、rebase、reset、切换 primary checkout。不要覆盖用户改动。
- 固定对照基线为 `063345f2f30bf999b28ee5d84aac860be8432fa7`。不 fetch 更新；若本地没有该对象，记录阻塞，不换成猜测基线。
- 对 baseline 只创建本任务拥有的 detached 临时 worktree 和隔离 `CARGO_TARGET_DIR`；不要并行运行两套 Cargo；完成后只清理自己创建的 worktree/临时文件。

## 必须遵守的测试纪律

- 每条 Cargo 命令前运行 `python qa/browser_live/check_memory.py`；必须得到 `ALLOW` 且 available bytes 至少 `4294967296`。设置 `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`；全量对照使用 `--test-threads=1`。
- 聚焦测试必须实际选中正数；零测试、编译错误和源码扫描都不算行为证据。
- 使用真实 runtime、router、store 与确定性 Notify/channel 握手；禁止 sleep race、global mocks、公开 test-only API、第二个 registry 或 lifecycle authority。
- 仅当真实 endpoint 行为先 RED 证明 defect 时才允许最小生产修复。不得修复 51 项中与此切片无关的失败。
- `bun run check` 在前序 checkout 中不存在；除非新 checkout 确实出现 `check` script，否则不要重复运行或声称通过。Linux/Windows native checks 在不可用主机上只记录未执行，不伪造替代结果。

## 范围红线

保持 raw/session archival 独立于 derived `MemoryRuntime`。不做 universal all-writer stop、native unload、RSS、process isolation、exactly-once 声明；不接入 Channel/Feishu/Terminal/MCP/EffectClaim/Safe Replay，不改 Memory producer 架构，不引入 supervisor。若测试需要新的公开或生产生命周期控制接口才能确定性运行，停下并记录理由，保留该行为为未闭合限制。

## 交付

先创建 `docs/superpowers/runs/2026-11-03-memory-source-owned-validation-closure-run.md`，记录起始状态。完成后记录精确测试名/计数、内存门结果、current-vs-baseline 失败集合、审查结论及未执行项；更新 docs 只限于事实校准。做文档 scanner/self-test、rustfmt、diff-check，获取 fresh supported read-only whole-diff review，修复具体 findings 后本地 conventional commit 并验证 clean。最终清楚区分证据与未完成项。

---

本 prompt 是下一会话的执行授权与范围交接，不代表本计划已实施。 / This prompt authorizes the next session's bounded execution; it is not evidence that the plan has run.

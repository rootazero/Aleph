# Restart prompt: Windows archive rename retry / 重启提示：Windows 归档 rename 重试

**Status / 状态:** Planned, not implemented / 已规划，尚未实施。

**Previous task merged to local `main` / 前序任务已合并到本地 `main`:** `f40fbf061 test(qa): add Windows console control harness for memory lifecycle probe` (pushed to `origin/main` as a fast-forward). The Windows-native Memory lifecycle validation that produced the defect evidence ran on a detached worktree at `4e53aafbe`; its harness changes are now on `main`. / Windows 原生 Memory 生命周期验证在 `4e53aafbe` 的 detached worktree 上跑完，其 harness 改动现已合并到 `main`。

Copy the request below into a fresh session. Discover the active checkout from Git; do not assume an absolute path. / 将下方请求交给新会话。先从 Git 定位实际 checkout，不假设绝对路径。

---

请按 `docs/superpowers/plans/2026-11-04-windows-archive-rename-retry-plan.md` 执行下一有界任务。用户已授权在下一会话按该计划实施本切片；不要把它扩成「所有平台归档都健壮」的总体工程，也不要顺手做计划里明确列为 deferred 的四项（真机 UI/desktop QA、RSS 回收验证、external-effect exactly-once、集群/多进程放置）。

## 目标

把 Windows 上 `FileSessionStore` 归档失败的缺口从 **harness 层移到生产层**，并让「复用同一个 session key 不会覆盖旧 archive」这条契约在 Windows 原生上成立。

用户原话：**「Windows 上 Archive session failed 的 rename 重试缺口仍在生产侧，仅由 harness 重试掩盖；建议后续在 file_backend 内对 ERROR_ACCESS_DENIED 增加重试。」**

已知的具体缺口：`src/gateway/session_store/file_backend/mod.rs` 的 `delete_session`（约 779-830 行）用 `tokio::fs::rename` 把会话目录搬进 `.archive/<date>/<sanitize_key_for_dir(key)>`。循环只在**目标已存在**（命名冲突）时换名 `<key>-1`、`<key>-2`…（上限 1024）；其它错误一律走终局分支 `Archive session failed: {error}`。Windows 上目录里若还有子句柄（服务自己的 transcript 句柄、外部扫描器如 Defender 实时保护），rename 会以 `ERROR_ACCESS_DENIED (os error 5)` 失败，此时目标并不存在 —— 于是走终局分支，`sessions.delete` 整体失败。

本切片要做四件事：

1. **先 RED，再修**：在 `file_backend` 内写出确定性的 Windows RED（真实复现 rename 被拒），并做句柄持有方归因（服务自身 transcript 句柄 / 外部扫描器 / 两者），归因结论必须有实测支撑，剩余无法解释的部分要如实列为残留。
2. **生产侧修复**：修复落在 `file_backend` 内 —— (a) 按 Task 1 的归因结果显式关闭 transcript 句柄（若归因指向它）；(b) 在 rename 循环里把「Windows 共享冲突」与「命名冲突」分开，前者对**同一目标**做有界退避重试，后者保持 `<key>-N` 契约与 1024 上限不变。重试预算有限、终局错误仍携带底层 OS 错误，真实权限问题不得被吞掉。
3. **拆掉 harness 的遮羞布**：删掉 `qa/memory_lifecycle/probe.py` 里那段会重试 `Archive session failed` 的循环，保留尝试次数记录；让缺陷再次出现时探针**大声失败**。
4. **Windows 原生复验**：重建 server（含 `alephcore/test-helpers`）、刷新 SHA-256 与 `build-record.json`，重跑 providerless 与 configured 两种 probe（configured 路径不得再出现任何 `Archive session failed`），并用 `qa/memory_lifecycle/archive_check.py --cycles 20` 至少三次证明归档目录单调增长 `agent_main_main_s1`、`-1`、`-2`… 且旧归档不被覆盖。

## 起始状态和 Git 纪律

- 本切片起始 commit 为 **`f40fbf061`**（已在本地 `main` 与 `origin/main`）。先核验 git root、当前分支、HEAD、`git status --short`；若工作区有与本切片无关的改动，先停下报告。
- 只在本地工作；除非本会话用户明确要求，不要 fetch、push、merge、rebase、reset 或切换 primary checkout；不要覆盖用户改动。
- 对照基线只允许创建本任务拥有的 detached 临时 worktree 与隔离 `CARGO_TARGET_DIR`；不要并行运行两套 Cargo；结束后只清理自己创建的 worktree/临时文件。
- 全量串行对照时，任何**新失败**都必须按完整测试名在 `f40fbf061` 上同命令复跑对照，报告「相同失败 / 仅当前失败 / 未复现」；不得用聚合计数下结论，也不得顺手修复无关失败。
- 前序 Windows 验证的证据在 `docs/superpowers/runs/` 之外的会话产物里（`C:\Temp\aleph-memory-native-windows\` 下的日志与 `build-record.json`）；若那些产物已不存在，就按计划重新生成，不要凭记忆引用。

## 必须遵守的测试纪律

- 每条 Cargo 命令前运行 `python qa/browser_live/check_memory.py`；必须得到 `ALLOW` 且 available bytes 至少 `4294967296`。设置 `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`；聚焦与全量都使用 `--test-threads=1`。
- 聚焦测试必须实际选中正数；零测试、编译错误与源码扫描都不算行为证据。
- 测真实行为：真实 file backend、真实目录、真实 rename。禁止 sleep race、global mocks、公开 test-only API、第二个 archive authority。
- 只有当真实行为先 RED 证明缺陷时，才允许最小生产修复。
- Windows 原生结果只代表 Windows；不得外推到 Linux/macOS，也不得把没跑的平台记成已验证。
- `bun run check` 在本 checkout 不存在，除非新 checkout 真的出现 `check` script，否则不要运行或声称通过。

## 范围红线

- 改动限于 `src/gateway/session_store/file_backend/`、`qa/memory_lifecycle/` 与事实性文档校准。不新增公开 API，不新增公开 test-only API，不引入第二个归档权威，不改归档命名契约。
- **不得削弱 Memory 生命周期契约**：Off 时 raw chat / Markdown 归档仍须可用，derived RPC 与 derived tool 在 Off 时仍须被拒，start/stop/restart 语义不变。重试不得把「被拒的写」变成「被容忍的写」，也不得吞掉不可能完成的移动所对应的 OS 错误。
- 重试必须有界：尝试预算、退避形状与终局错误都要在代码和测试里看得见。
- 不得顺手改「配置选择哪个 session store backend」这一事实（计划 Task 1 Step 5 只记录、不修）。
- 明确 **deferred、不得在本切片顺手做掉**：真机 UI/desktop QA、RSS 回收验证、external-effect exactly-once、集群/多进程放置；也不做 Capability Core / Memory runtime 架构改动，不动 SQLite backend 行为。
- 若某个行为必须新增公开或生产生命周期控制接口才能确定性运行，停下并记录理由，把该行为保留为未闭合限制。

## 交付

先创建 `docs/superpowers/runs/2026-11-04-windows-archive-rename-retry-run.md`，记录起始状态、RED 证据与归因测量。随后完成计划四个任务：记录精确测试名/计数、内存门读数、修复前后 live 失败率、两种 probe 的 `Checks: X/Y`、`archive_check.py` 三次的归档目录清单、SQLite integrity 与 endpoint/listener 清理结果、Windows 控制台信号方法（Windows 无 SIGTERM；优雅关闭证据是 `[SHUTDOWN] signal=ctrl_c` 与退出码 0；强制 kill 只能算失败清理）。做文档 scanner/self-test、rustfmt（单文件 `rustfmt --config skip_children=true`，不要用 `cargo fmt -p <crate>`）、diff-check；取一次全新受支持的只读 whole-diff 审阅并修复具体 findings；本地 conventional commit 并确认工作区干净；最后**按事实**校准 `docs/reference/MEMORY_SYSTEM.md`（以及 `FEATURE_LOCATOR.md` 中若有关于归档移动的断言）。最终清楚区分证据与未完成项。

---

本 prompt 是下一会话的执行授权与范围交接，不代表本计划已实施。 / This prompt authorizes the next session's bounded execution; it is not evidence that the plan has run.

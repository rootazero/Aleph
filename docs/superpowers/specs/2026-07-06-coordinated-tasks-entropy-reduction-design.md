# 4.4 协调任务 · 结构熵减（行为保持的模块拆分）

**日期**: 2026-07-06
**范围**: FEATURE_LOCATOR §4.4 Coordinated Tasks
**类型**: 纯结构重构（behavior-preserving）+ 搬运中的机会式死代码清理
**分支纪律**: 全程在新建 worktree 分支进行，不触碰 main

## 背景与动机

§4.4 是已 ✅ 且**领先参考项目**（codex-rs cloud-tasks / agent-graph-store、pi orchestrator）的成熟子系统：本地 CoordTask DAG 调度、有界重试 + 指数退避 + equal jitter、per-task 超时、僵尸/孤儿双回收、依赖失败级联（`Unsatisfiable`）均已连线并有测试覆盖。参考项目无可整体移植的更强任务协调模型。

因此本轮价值不在"抄缺失功能"，而在**内部结构熵减**：两个核心文件突破 Aleph 自己的红线（P2 / CODE_ORGANIZATION：>500 应拆、800 max）：

- `src/teams/dispatcher/schedule.rs` = **1261 行**
- `src/agents/swarm/tasks/store/mod.rs` = **1175 行**

拆分与最近提交 `2cdffbe92 refactor: split six oversized modules into directory submodules` 同节奏，遵循其"pure structural move; parents keep `pub mod <name>;`"范式（直接先例：`gateway/handlers/graph.rs` 1338 → mod.rs 85 + 5 主题文件）。

## 非目标（Out of Scope）

- 不改任何调度/重试/超时/回收的行为语义（纯搬运，测试跟随）。
- 不新增功能、不改 `CoordTaskStore` trait 契约、不改 SQL schema。
- 不做 correctness 深挖（priority aging / cancellation 传播等）——已在 brainstorm 中确认留待下轮。
- 不动 §4.4 之外的已知死代码（如 `tools/adapters/builtin_adapter.rs`）。
- 已核实 `runner.rs::execute_acp_member_task` **非**死代码（`runner.rs:146` 活体调用），不动。

## Part A — `dispatcher/schedule.rs` → `dispatcher/schedule/` 目录

`schedule.rs` 是「一组纯自由函数 + `impl TeamDispatcher` 的多个方法 + 测试」。**无 trait-impl 跨文件障碍**（inherent `impl TeamDispatcher` 可分布在同模块多个文件的多个 impl 块中）。

当前内容与去向：

| 现有符号 (行) | 新文件 | 理由 |
|---|---|---|
| `is_dispatcher_managed` (41) / `completion_status` (53) / `is_zombie` (79) / `select_schedulable` (128) + 对应单元测试（select ~9 个、zombie ~9 个，含 `task`/`in_progress_task` 测试 helper） | `schedule/select.rs` | 纯调度策略：无 I/O、可测的选择与僵尸判定谓词 |
| `reclaim_zombies` (391) / `reclaim_orphaned` (443) | `schedule/reclaim.rs` | 回收 janitor（僵尸/孤儿） |
| `fail_or_retry` (644) | `schedule/failure.rs` | 失败→重试连线（调用既有纯 `tasks/retry.rs::retry_decision`） |
| `dispatch_once` (199) / `run_task` (482) / `resolve_dispatch_target` (335) / `persist_artifact` (776) / `now_epoch` (376) | `schedule/mod.rs` | 核心 dispatch + 执行驱动 |

**结构规则**:
- 每个子文件用独立 `impl TeamDispatcher { … }` 块；`mod.rs` 顶部 `use` 拉入所需类型。
- 纯自由函数（select/zombie 一族）设 `pub(crate)` 或 `pub(super)`，保持现有可见性等价。
- 测试跟随其被测符号迁移（select/zombie 测试 → `select.rs` 的 `#[cfg(test)] mod tests`）。
- `dispatcher/mod.rs` 的 `pub mod schedule;` 声明不变。

**预期规模**: mod.rs ~350 行、select.rs ~350 行（含测试）、reclaim.rs ~150 行、failure.rs ~150 行。全部回到红线内。

## Part B — `store/mod.rs` → 拆 `impl CoordTaskStore`

障碍：`impl CoordTaskStore for SqliteCoordTaskStore` 是**单个 trait impl**，Rust 不允许跨文件拆分。

**方案**：主题模块放 `pub(super) async fn` **自由函数**（签名首参 `store: &SqliteCoordTaskStore`），`mod.rs` 保留唯一的薄 trait impl，每方法一行委派。用主题模块命名空间（`crud::create_task(self, input).await`）避免与 trait 方法同名递归歧义。

方法分组与去向（全部来自当前 `impl CoordTaskStore`）：

| 新文件 | 迁移的方法 |
|---|---|
| `store/crud.rs` | `create_task` / `get_task` / `update_task` / `list_tasks` / `delete_team_tasks` |
| `store/deps.rs` | `get_dependencies` / `get_dependents` / `get_newly_unblocked` |
| `store/locks.rs` | `acquire_lock` / `release_lock` / `release_stale_locks` |
| `store/runs.rs` | `start_task_run` / `finish_task_run` / `list_task_runs` / `record_run_review` |
| `store/comments.rs` | `add_task_comment` / `list_task_comments` |
| `store/journal.rs` | `upsert_task_journal` / `get_task_journal` / `list_team_journals` |
| `store/mod.rs`（保留） | `SqliteCoordTaskStore` struct + 构造器（`new`/`with_event_bus`/`connection_handle`）+ `emit_task_topic` + `migrate` + 薄 `impl CoordTaskStore`（~25 行委派） |

**结构规则**:
- 主题自由函数 `pub(super)`，仅 mod.rs 委派消费；不外泄。
- `emit_task_topic` 是多方法共享的 helper，留在 mod.rs（或若被多主题重度依赖，抽到 `store/helpers.rs` 已存在文件）——搬运时按实际调用点决定，默认留 mod.rs。
- 现有 `store/{helpers,row_decode,schema,tests}.rs` 不动；`tests` 内引用的私有项若因迁移改变可见性，用 `pub(super)`/`pub(crate)` 恢复等价。
- `tasks/mod.rs` 的 `pub mod store;` 已是目录模块声明，不变。

**取舍**：Part B 引入 ~25 行 trait 委派样板（trait impl 无法避免的成本），换来每个主题文件高内聚、mod.rs 降到 ~250 行。已确认接受。

**预期规模**: mod.rs ~250 行 + 6 个主题文件各 60–180 行，全部红线内。

## 验证策略

- **编译等价**: 收尾 `cargo check -p alephcore --lib` 至多一次（遵守 cargo 节制）。
- **测试等价**: 迁移后测试符号不变、断言不变；若本地允许再跑一次 `cargo test -p alephcore --lib coord`（可选，视系统负担）。
- **diff 审计**: 每个新文件的内容应能在旧文件中逐符号对应；除可见性修饰符与 `use`/`impl` 包裹外无逻辑改动。纯 `git diff` 应只体现"移动 + 包裹"。
- **红线核对**: 拆分后 `wc -l` 所有相关文件 < 800。

## 风险与回退

- **风险低**：纯搬运，无行为改动。主要风险是可见性/`use` 路径遗漏导致编译错误，由收尾 `cargo check` 兜底。
- **回退**：worktree 分支隔离；不合入 main 前可整分支丢弃。

## 实施顺序

1. 新建 worktree 分支。
2. Part A（高置信度净赢）：schedule.rs → schedule/ 四文件。
3. `cargo check --lib` 验证 A。
4. Part B：store/mod.rs → 6 主题文件 + 薄 trait impl。
5. 收尾 `cargo check --lib` 验证 A+B。
6. 提交（`refactor:` scope），worktree 合并回 main（同会话不删 worktree）。

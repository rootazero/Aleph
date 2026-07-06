# 4.4 协调任务 · 结构熵减 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 §4.4 两个突破 800 行红线的文件（`teams/dispatcher/schedule.rs` 1261 行、`agents/swarm/tasks/store/mod.rs` 1175 行）按职责拆成子模块目录，行为零改变。

**Architecture:** 纯结构搬运（behavior-preserving）。Part A 拆 `schedule.rs` 为 `schedule/{mod,select,reclaim,failure}.rs`（inherent `impl TeamDispatcher` 可跨文件多块）。Part B 拆 `store/mod.rs`：trait impl 无法跨文件，故各方法体下沉为主题模块的 `pub(super)` 自由函数，mod.rs 保留唯一薄 `impl CoordTaskStore` 逐方法委派。

**Tech Stack:** Rust 2021 / tokio / rusqlite（`crate::error::Result`）。

## Global Constraints

- **分支隔离**：全程在新建 worktree 分支，严禁触碰 main（用户协议 + CLAUDE.md 分支策略）。
- **cargo 节制**：每个 Part **至多一次** `cargo check -p alephcore --lib`（CLAUDE.md「极度节制 cargo 调用」）。不跑全量测试。
- **纯搬运**：不改任何逻辑、签名语义、SQL、schema、trait 契约。符号内容逐行对应旧文件；除 `use` / `impl` 包裹 / 可见性修饰符 / Part B 的 `self`→`store` 机械重命名外，无改动。
- **house style**：对齐 `2cdffbe92`——"parents keep `pub mod <name>;`"，每子文件单一职责。
- **格式**：`cargo fmt` 后再提交；行宽 100；4 空格缩进。
- **提交规范**：English，`refactor: <desc>`。

---

## Task 1: 建立 worktree 分支

**Files:** 无源码改动。

- [ ] **Step 1: 创建隔离工作区**

REQUIRED SUB-SKILL: Use `superpowers:using-git-worktrees` to create the worktree. 分支名建议 `refactor/coord-tasks-entropy`。

- [ ] **Step 2: 确认起点干净**

Run: `git -C <worktree> status --porcelain`
Expected: 空输出（clean）。

---

## Task 2: Part A — 拆 `dispatcher/schedule.rs` → `schedule/` 目录

**Files:**
- Delete: `src/teams/dispatcher/schedule.rs`（内容分散到下列 4 文件）
- Create: `src/teams/dispatcher/schedule/mod.rs`
- Create: `src/teams/dispatcher/schedule/select.rs`
- Create: `src/teams/dispatcher/schedule/reclaim.rs`
- Create: `src/teams/dispatcher/schedule/failure.rs`
- Unchanged: `src/teams/dispatcher/mod.rs:17` `pub mod schedule;`（目录模块声明等价，不改）

**符号 → 文件 → 可见性映射**（源行号指旧 `schedule.rs`）:

| 符号 (旧行) | 去向 | 可见性 | 依据 |
|---|---|---|---|
| `MANAGED_BY_KEY` (35) / `MANAGED_BY_DISPATCHER` (37) | select.rs | `pub` | dispatcher/mod.rs:34 re-export |
| `is_dispatcher_managed` (41) | select.rs | `pub` | 同上 re-export |
| `completion_status` (53) | select.rs | `pub(super)` | 被 mod.rs 的 `run_task` (旧573) 跨文件调用 → 从私有升级 |
| `is_zombie` (79) | select.rs | `pub` | 被 reclaim.rs 调用 + intra-doc 链接 `schedule::is_zombie` |
| `select_schedulable` (128) | select.rs | `pub` | 被 mod.rs 调用 + intra-doc 链接 `schedule::select_schedulable` |
| 测试 `task` helper (802) | select.rs | 私有(test) | select 测试依赖 |
| `completion_status_routes…` (834) | select.rs | test | 测 completion_status |
| select 测试组 (860–1160：selects_only_pending / skips_locked / orders_by_priority / respects_available_slots / spreads_slots / inflight_load_defers / max_per_owner_caps / fair_fill) | select.rs | test | 测 select_schedulable |
| `in_progress_task` helper (1163) + zombie 测试组 (1177–1257) | select.rs | test | 测 is_zombie |
| `reclaim_zombies` (391) | reclaim.rs | `pub(super)` | 被 mod.rs 的 dispatch_once 调用；无外部调用者 |
| `reclaim_orphaned` (443) | reclaim.rs | `pub(super)` | 同上 |
| `fail_or_retry` (644) | failure.rs | `pub(super)` | 仅 schedule 内部调用（handoff.rs 仅 doc 链接） |
| `fail_task` (751) | failure.rs | `pub(in crate::teams::dispatcher)` | **被 `clarify.rs:25/30/39/97` 调用**（dispatcher 兄弟，非 schedule）→ 必须保留 dispatcher 级可见（等价旧 `pub(super)` where super=dispatcher） |
| `dispatch_once` (199) | mod.rs | `pub(crate)` | 被 dispatcher/mod.rs:214/224 调用 |
| `resolve_dispatch_target` (335) | mod.rs | 私有 | 仅 mod.rs 内调用 |
| `now_epoch` (376) | mod.rs | `pub(super)` | 被 reclaim.rs(旧411)/failure.rs(旧690) 跨文件 `Self::now_epoch()` |
| `run_task` (482) | mod.rs | `pub(crate)` | 保持现状 |
| `persist_artifact` (776) | mod.rs | 私有 | 仅 mod.rs 内 run_task 调用 |

- [ ] **Step 1: 创建 `schedule/select.rs`**

搬入上表 select.rs 行的**逐字内容**。顶部 `use` 块（从旧 schedule.rs 头部提取本文件实际用到的项）：

```rust
//! Pure scheduling policy for the team dispatcher: which tasks are managed,
//! which are schedulable, and which are zombies. No I/O — exercisable without
//! a live dispatcher.

use std::collections::{HashMap, HashSet};

use crate::agents::swarm::tasks::acceptance::lead_review_required;
use crate::agents::swarm::tasks::timeout::read_task_timeout;
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus};
```

把 `fn completion_status` 改为 `pub(super) fn completion_status`；其余 `pub`/签名不变。测试 `mod tests` 整体搬入本文件（`use super::*;` 及测试内原有的 `use`）。

- [ ] **Step 2: 创建 `schedule/reclaim.rs`**

```rust
//! Janitor duties: reap zombie (worker-lost) and orphaned (crash-left) tasks
//! back to a schedulable or terminal state.

use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate};
use crate::sync_primitives::Arc;

use super::select::is_zombie;
use super::TeamDispatcher;
```

内容：`impl TeamDispatcher { pub(super) async fn reclaim_zombies(self: &Arc<Self>) {…}  pub(super) async fn reclaim_orphaned(self: &Arc<Self>) {…} }`，方法体逐字搬运（旧 391–481）。`reclaim_zombies` 内 `Self::now_epoch()` 调用不变（now_epoch 在 mod.rs `pub(super)`）。若实际 `use` 有缺漏，以编译器为准补齐——只补 `use`，不改逻辑。

- [ ] **Step 3: 创建 `schedule/failure.rs`**

```rust
//! Failure handling: bounded retry (delegating to the pure `tasks::retry`
//! decision) and terminal fail.

use crate::agents::swarm::tasks::retry::{
    jittered_backoff_secs, read_max_retries, retry_decision, with_retry_not_before, RetryDecision,
};
use crate::agents::swarm::tasks::{CoordTask, CoordTaskStatus, CoordTaskUpdate};

use super::TeamDispatcher;
```

内容：`impl TeamDispatcher { pub(super) async fn fail_or_retry(&self, task: &CoordTask, error: &str) {…}  pub(in crate::teams::dispatcher) async fn fail_task(&self, task: &CoordTask, error: &str) {…} }`，方法体逐字搬运（旧 644–775）。`fail_task` 的可见性从 `pub(super)` 改为 `pub(in crate::teams::dispatcher)`——这是全 Part 唯一非机械的关键点（保住 clarify.rs 调用）。`fail_or_retry` 内 `Self::now_epoch()` 不变。

- [ ] **Step 4: 创建 `schedule/mod.rs`**

顶部保留旧 schedule.rs 的 `//!` 模块文档（旧 1–6 行）。然后：

```rust
mod failure;
mod reclaim;
mod select;

pub use select::{
    is_dispatcher_managed, is_zombie, select_schedulable, MANAGED_BY_DISPATCHER, MANAGED_BY_KEY,
};

// ... 旧 schedule.rs 头部剩余 use（dispatch_once/run_task/resolve_dispatch_target/
// persist_artifact/now_epoch 实际用到的：OwnedSemaphorePermit、build_handoff_context、
// runner::{execute_member_task, MemberDispatchTarget, MemberRunStatus}、
// acceptance::lead_review_required、timeout::{effective_timeout_secs, read_task_timeout}、
// tasks::{CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate, TaskRunStatus}、
// Arc、artifacts::{...}、InboxContextProvider、TeamMemberKind、Duration、HashMap/HashSet）
```

`impl TeamDispatcher` 块含（逐字搬运旧 199–334 / 335–375 / 376–390 / 482–643 / 776–795）：
- `pub(crate) async fn dispatch_once(self: &Arc<Self>)`
- `async fn resolve_dispatch_target(...)`（私有）
- `pub(super) fn now_epoch() -> u64`（从私有升级）
- `pub(crate) async fn run_task(self: Arc<Self>, ...)`
- `async fn persist_artifact(...)`（私有）

`run_task` 内对 `completion_status` 的调用改为 `select::completion_status(&task)`（或加 `use select::completion_status;`）。

- [ ] **Step 5: 删除旧 `schedule.rs`**

Run: `git -C <worktree> rm src/teams/dispatcher/schedule.rs`（内容已全部迁出）。

- [ ] **Step 6: 格式化 + 编译验证（本 Part 唯一 cargo）**

Run: `cargo fmt -p alephcore && cargo check -p alephcore --lib`
Expected: 编译通过，无 warning 关于 unused import / unreachable。若报可见性或缺 `use` 错误，按 Step 说明补 `use`/调可见性，**不改逻辑**，再复查（此为同一次 check 的迭代，非新增调用）。

- [ ] **Step 7: 核对红线并提交**

Run: `wc -l src/teams/dispatcher/schedule/*.rs`
Expected: 每个文件 < 800。

```bash
git -C <worktree> add src/teams/dispatcher/schedule/ src/teams/dispatcher/schedule.rs
git -C <worktree> commit -m "refactor: split dispatcher schedule.rs into schedule/ submodules"
```

---

## Task 3: Part B — 拆 `store/mod.rs` 的 `impl CoordTaskStore`

**Files:**
- Modify: `src/agents/swarm/tasks/store/mod.rs`（保留 struct/构造器/emit_task_topic/migrate + 薄 trait impl + 现有 `#[cfg(test)] mod tests`）
- Create: `src/agents/swarm/tasks/store/crud.rs`
- Create: `src/agents/swarm/tasks/store/deps.rs`
- Create: `src/agents/swarm/tasks/store/locks.rs`
- Create: `src/agents/swarm/tasks/store/runs.rs`
- Create: `src/agents/swarm/tasks/store/comments.rs`
- Create: `src/agents/swarm/tasks/store/journal.rs`
- Unchanged: `store/{helpers,row_decode,schema,tests}.rs`；`tasks/mod.rs:13` `pub mod store;`

**Interfaces (委派契约):** 每主题文件导出 `pub(super) async fn <trait_method_name>(store: &SqliteCoordTaskStore, <原参数>) -> crate::error::Result<...>`，签名与原 trait 方法**逐字一致**，仅把接收者 `&self` 改成显式 `store: &SqliteCoordTaskStore`，方法体内 `self` → `store`（机械替换）。mod.rs 的 `impl CoordTaskStore` 每方法体改为单行 `<module>::<name>(self, <args>).await`。

**方法分组**（源行号指旧 store/mod.rs 的 `impl CoordTaskStore`）:

| 主题文件 | 方法 (旧行) |
|---|---|
| crud.rs | `create_task` (194) / `get_task` (262) / `update_task` (267) / `list_tasks` (366) / `delete_team_tasks` (915) |
| deps.rs | `get_dependencies` (494) / `get_dependents` (499) / `get_newly_unblocked` (514) |
| locks.rs | `acquire_lock` (553) / `release_lock` (584) / `release_stale_locks` (621) |
| runs.rs | `start_task_run` (638) / `finish_task_run` (651) / `list_task_runs` (677) / `record_run_review` (725) |
| comments.rs | `add_task_comment` (768) / `list_task_comments` (792) |
| journal.rs | `upsert_task_journal` (823) / `get_task_journal` (874) / `list_team_journals` (943) |

**保留在 mod.rs**：`SqliteCoordTaskStore` struct、`new`/`with_event_bus`/`connection_handle`（旧 46/56/67，`pub`，外部消费者：aleph-server `coord_stores.rs`、`teammates.rs`、`dag.rs` 测试——**不动**）、`emit_task_topic`（旧 78，私有 inherent；主题模块是 `store` 的**后代**，可访问父模块私有项，无需改可见性）、`migrate`（旧 180）、现有 `#[cfg(test)] mod tests`（旧 989+，调 trait 方法，经委派仍通）。

- [ ] **Step 1: 创建 6 个主题文件**

每文件模板（以 crud.rs 为例；其余同构，只换方法与 `use`）：

```rust
//! CRUD methods for `SqliteCoordTaskStore` (create/get/update/list/delete).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use super::SqliteCoordTaskStore;
use super::row_decode; // 若该方法用到；按实际引用增删
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskUpdate, NewCoordTask, // 按实际用到的类型
};

pub(super) async fn create_task(
    store: &SqliteCoordTaskStore,
    input: NewCoordTask,
) -> crate::error::Result<CoordTask> {
    // 旧 create_task 方法体逐字搬运，`self` → `store`
}
// get_task / update_task / list_tasks / delete_team_tasks 同法
```

要点：
- 方法体内对其它 store helper 的调用（`store.emit_task_topic(...)`、`store.connection_handle()`、`row_decode::…`、`schema::…`、`helpers::…`）随 `self→store` 机械替换即可；`use super::{row_decode, schema, helpers};` 按各文件实际引用补。
- 若某方法体内调用了另一 trait 方法（如 `update_task` 内部若调 `get_task`），改为调该方法的**新自由函数**（`crud::get_task(store, id).await`），不要走 `store.get_task(...)`（trait 委派）以免绕路；同组内直接函数名调用。

- [ ] **Step 2: 改写 mod.rs 的 `impl CoordTaskStore` 为薄委派**

保留 `#[async_trait]`（若原有）与 trait 方法签名逐字不变，方法体改为单行委派：

```rust
#[async_trait::async_trait] // 若原文件有此属性则保留原样
impl CoordTaskStore for SqliteCoordTaskStore {
    async fn create_task(&self, input: NewCoordTask) -> crate::error::Result<CoordTask> {
        crud::create_task(self, input).await
    }
    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>> {
        crud::get_task(self, id).await
    }
    // …其余 20 个方法逐一委派到对应模块…
    async fn list_team_journals(&self, /* 原参数 */) -> crate::error::Result</* 原返回 */> {
        journal::list_team_journals(self, /* 原参数名 */).await
    }
}
```

在 mod.rs 顶部加模块声明：`mod comments; mod crud; mod deps; mod journal; mod locks; mod runs;`（放在现有 `mod tests;` 一带，rustfmt 排序）。

- [ ] **Step 3: 格式化 + 编译验证（本 Part 唯一 cargo）**

Run: `cargo fmt -p alephcore && cargo check -p alephcore --lib`
Expected: 编译通过。常见修复（只补 `use`/可见性，不改逻辑）：主题文件缺类型 `use` → 补；`emit_task_topic` 若报私有不可达（不应发生，后代可访问）→ 退一步给它 `pub(super)`。

- [ ] **Step 4: 核对红线并提交**

Run: `wc -l src/agents/swarm/tasks/store/*.rs`
Expected: `mod.rs` 降到 ~250–300，其余各 < 800。

```bash
git -C <worktree> add src/agents/swarm/tasks/store/
git -C <worktree> commit -m "refactor: split coord-task store impl into topic submodules"
```

---

## Task 4: 收尾（可选测试 + 合并）

**Files:** 无源码改动。

- [ ] **Step 1: （可选）跑一次协调任务单测**

系统负担允许时：`cargo test -p alephcore --lib coord`
Expected: 迁移的测试全 PASS（断言未改，纯移动应绿）。负担高则跳过，依赖 Task2/3 的 `cargo check` + diff 审计。

- [ ] **Step 2: diff 审计（纯搬运证明）**

Run: `git -C <worktree> diff main --stat`
Expected: 只见「旧文件大减 + 新文件相应增」，净变化 ≈ 委派样板 + `use`/可见性行；无算法/SQL 行改动。逐符号可在旧文件找到对应。

- [ ] **Step 3: 完成分支**

REQUIRED SUB-SKILL: Use `superpowers:finishing-a-development-branch`（同会话只合并不删 worktree，见 CLAUDE.md Git Worktree 注意事项）。

---

## Self-Review 记录

- **Spec 覆盖**：Part A（schedule 四文件）↔ Task 2；Part B（store 六主题 + 薄 trait impl）↔ Task 3；worktree 纪律 ↔ Task 1；cargo 节制/红线核对 ↔ 各 Part 的 fmt+check+wc 步骤；死代码「不动 execute_acp_member_task」↔ 已在 spec 非目标固定，计划无触碰。
- **占位符扫描**：无 TBD/TODO；每步给出可执行命令、期望输出、具体 `use`/可见性。方法体「逐字搬运」而非重述属纯移动的正确表述（非占位）。
- **类型一致性**：`fail_task` 可见性 `pub(in crate::teams::dispatcher)`（Task2 表 + Step3 一致）；委派签名「`&self`→`store: &SqliteCoordTaskStore`、`self`→`store`」全 Part B 统一；再导出符号集 `{is_dispatcher_managed, is_zombie, select_schedulable, MANAGED_BY_KEY, MANAGED_BY_DISPATCHER}` 与 dispatcher/mod.rs:34 消费一致。

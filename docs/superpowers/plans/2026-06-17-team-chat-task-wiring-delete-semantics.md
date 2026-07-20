# 团队聊天任务连线 + 解散/删除语义修复 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让群聊 leader 自动编排产生可追踪任务（填满看板/计划/任务并把成员执行流式进群聊）、把侧栏「删除」正名为「解散」、修复团队概览的真正级联硬删除。

**Architecture:** Problem 1 是纯 prompt 写侧修复（强化 `build_member_input` 的 leader 段，R7/R9/R10，绝不加 dispatcher）——读侧（看板订阅 `team.*.task.*`）与成员执行 fanout（`TeamFanoutEmitter`）均已连通，随写侧修复自动成立。Problem 2 是前端 i18n label 改动。Problem 3 在 handler 编排层做跨 store 级联删除（先 `delete_team` 把关并移除团队行，再 best-effort 清理 5 个从属 store 的孤儿），并把前端删除错误从 console-only 改为可见。

**Tech Stack:** Rust (alephcore, async-trait, rusqlite/tokio Mutex), Leptos/WASM Panel (`interfaces/webchat`), JSON-RPC gateway。

## Global Constraints

- 红线 R7（LLM 主权）/R10（笨循环）：Problem 1 **严禁**新建意图分类器、任务规划管线、dispatcher。只改 prompt。
- 红线 R10：**禁止改动 `src/harness/`**。
- 提交规范：English commit messages，格式 `<scope>: <description>`。
- 单分支：直接在 `main` 上开发（项目约定）。
- **cargo 节制（用户强约束）**：默认不跑 cargo；后端任务的测试运行**批量化**——可把同一 Phase 内的多个后端单测推迟到该 Phase 末尾用一次 `cargo test -p alephcore --lib <module_path>` 跑完，而非每步一跑。报告 `NOT cargo-checked` 是可接受的诚实降级。
- 部署刷新链（rust_embed）：改 Panel 源码后必须 `just wasm` → 重编 `aleph-server` binary → 替换运行中 daemon（见 Task D）。仅 `just wasm` 不生效。
- 代码注释 English；对话/文档 Chinese。
- 现有 SQLite cascade 依赖 `PRAGMA foreign_keys`，**不假定其开启**——级联删除显式手删子表，不依赖 FK CASCADE。

---

## Phase A — Problem 2：侧栏「删除」正名为「解散」

### Task A1: 侧栏群聊菜单 label 与确认文案改为「解散」

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:1109`（菜单项 label）
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:993`（确认条文案）

**Interfaces:**
- Consumes: 已存在 i18n key `teams.disband`（zh:"解散" / en:"Disband"，locales 行 1468）、`common.confirm_dissolve`（zh:"确认解散？" / en:"Confirm disband?"，locales 行 27）。
- Produces: 无下游依赖。

**背景**：该按钮调 `do_delete_group` → `TeamsApi::disband()` → `teams.disband`（软删除），语义是解散，仅 label 错。后端不动。

- [ ] **Step 1: 改菜单项 label**

把 `chat_sidebar.rs:1109`：
```rust
>{move || t_string!(i18n, common.delete).to_string()}</button>
```
改为：
```rust
>{move || t_string!(i18n, teams.disband).to_string()}</button>
```

- [ ] **Step 2: 改确认条文案**

把 `chat_sidebar.rs:993`：
```rust
<span class="text-red-400 font-medium">{move || t_string!(i18n, chat.confirm_delete).to_string()}</span>
```
改为：
```rust
<span class="text-red-400 font-medium">{move || t_string!(i18n, common.confirm_dissolve).to_string()}</span>
```

- [ ] **Step 3: 构建校验（Panel WASM）**

Run: `cargo build -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -5`
Expected: 编译通过（无 `t_string!` key 缺失错误）。若 `teams.disband` 在该 i18n 宏作用域不可达，则改用 `common` 下等价 key，或在 zh.json/en.json 的 `common` 段补 `"disband"` 条目后引用 `common.disband`。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: rename group-chat sidebar 删除→解散 (disband, not hard delete)"
```

---

## Phase B — Problem 1：leader 自动编排 + 任务实时刷新

### Task B1: 强化 leader 编排 prompt（host-tested）

**Files:**
- Modify: `src/teams/broadcast/member_prompt.rs:16-21`（`leader_block`）
- Test: `src/teams/broadcast/member_prompt.rs`（已存在 `#[cfg(test)] mod tests`，扩充 `leader_prompt_appends_leader_identity`）

**Interfaces:**
- Consumes: `build_member_input(team_id, agent_id, role, roster, transcript, is_leader) -> String`（已存在纯函数）。
- Produces: 行为契约不变（签名不变），仅 leader 段文本更强。下游 `GroupChatBroadcaster` 无需改。

**设计意图**：把 leader 段从「可选、非义务」改为「实质工作时的预期默认」，但保留 LLM 判断（不硬编码意图规则，R7）。

- [ ] **Step 1: 更新失败测试（先改断言）**

在 `member_prompt.rs` 的 `leader_prompt_appends_leader_identity` 末尾追加断言：
```rust
        assert!(out.contains("team_delegate"), "leader 段提到 team_delegate");
        assert!(
            out.contains("拆成可追踪任务") || out.contains("派给成员"),
            "leader 段指示把实质工作拆成可追踪任务"
        );
```

- [ ] **Step 2: 运行测试确认失败（可批量，见 Global Constraints）**

Run: `cargo test -p alephcore --lib member_prompt::tests::leader_prompt_appends_leader_identity`
Expected: FAIL（当前 leader_block 无「拆成可追踪任务/派给成员」字样）。

- [ ] **Step 3: 强化 leader_block**

把 `member_prompt.rs:16-21` 的：
```rust
    let leader_block = if is_leader {
        "\n\n你还是这个群的 leader——除了平等参与讨论,当任务需要严肃编排时,\
         你可以用 `task_create` / `team_delegate` 派活给成员、汇总产出给用户。但这是你的判断,不是义务。"
    } else {
        ""
    };
```
改为：
```rust
    let leader_block = if is_leader {
        "\n\n你还是这个群的 leader。当用户给团队的是一项需要完成的实质工作\
         (而非寒暄或简单问答)时,默认先用 `team_delegate` / `task_create` 把它\
         拆成可追踪任务派给成员、再汇总产出回复用户——这样进度会显示在团队看板上。\
         是否拆解、如何拆解由你判断;闲聊或你能直接答的,就直接答。"
    } else {
        ""
    };
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore --lib member_prompt::tests`
Expected: PASS（两个测试都过；注意原 `leader_prompt_appends_leader_identity` 里 `assert!(out.contains("task_create"))` 仍成立）。

- [ ] **Step 5: Commit**

```bash
git add src/teams/broadcast/member_prompt.rs
git commit -m "teams: strengthen group-chat leader prompt to orchestrate tracked tasks"
```

### Task B2: per-chat 工作区「任务」tab 订阅 `team.*.task.*` 实时刷新

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`（`TeamTasksView` 的 Effect 区，约 468-475）

**Interfaces:**
- Consumes: `DashboardState::subscribe_topic` / `subscribe_events`（与 `kanban.rs` 同款，见 `interfaces/webchat/src/views/teams/kanban.rs:46-71`）；`chat.team_id`。
- Produces: 无下游。

**背景**：全局看板已订阅 `team.*.task.*` 实时刷新；per-chat `任务` tab 当前只在 `team_members` 变化时 refetch，leader 新建任务时不刷新。补一个同款订阅。

- [ ] **Step 1: 加 topic 订阅 + 事件刷新**

参照 `kanban.rs:46-71` 的写法，在 `TeamTasksView` 内（已有的 task-fetch Effect 之后）加入：
```rust
    // 订阅 team.*.task.* 让 leader 新建/更新任务时本 tab 实时刷新（对齐全局看板）。
    Effect::new(move |_| {
        if !chat_conn.is_connected.get() {
            return;
        }
        let dash2 = chat_conn;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("team.*.task.*").await;
        });
    });
    let _sub = chat_conn.subscribe_events(move |evt| {
        if !evt.topic.starts_with("team.") || !evt.topic.contains(".task.") {
            return;
        }
        // 复用已有的 task 拉取逻辑（refetch tasks for current chat.team_id）。
        refetch_tasks();
    });
```
说明：`chat_conn` 为本组件已 `expect_context::<DashboardState>()` 得到的句柄（若变量名不同，沿用组件内既有名）；`refetch_tasks` 为把现有 fetch Effect 主体抽出的闭包——若现状是内联 `spawn_local`，先抽成一个 `let refetch_tasks = move || { /* 原 teams.get/list_tasks 拉取体 */ };` 再在原 Effect 与新订阅里复用（DRY）。

- [ ] **Step 2: 构建校验**

Run: `cargo build -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -5`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs
git commit -m "panel: live-refresh team workspace 任务 tab on team.*.task.* events"
```

---

## Phase C — Problem 3：可见错误 + 真正的级联硬删除

> 各 store 新增方法签名（本 Phase 全局契约，后续任务引用）：
> - `MessageStore::delete_team_messages(&self, team_id: &str) -> crate::error::Result<usize>`
> - `CoordTaskStore::delete_team_tasks(&self, team_id: &str) -> crate::error::Result<usize>`
> - `EventLogStore::delete_team_events(&self, team_id: &str) -> crate::error::Result<usize>`
> - `ArtifactStore::delete_artifacts_for_tasks(&self, task_ids: &[String]) -> crate::error::Result<usize>`
> - `SqliteSnapshotStore::delete_team_snapshots(&self, team_id: &str) -> crate::error::Result<usize>`

### Task C1: 概览删除/解散错误可见化（debug-first）

**Files:**
- Modify: `interfaces/webchat/src/views/teams/overview.rs:90-130`（`handle_disband` / `handle_delete`）

**Interfaces:**
- Consumes: 组件已有 `error_msg: RwSignal<Option<String>>`（`overview.rs:42-53` reload 中使用）。
- Produces: 无下游。

**背景**：当前删除失败仅 `web_sys::console::error_1`，用户看不到 → 「点了没反应」。改为写入 `error_msg`。

- [ ] **Step 1: handle_delete 失败写 error_msg**

把 `overview.rs` `handle_delete`（约 112-130）的：
```rust
        if let Err(e) = TeamsApi::delete(&state, &team_id).await {
            web_sys::console::error_1(&format!("Delete failed: {e}").into());
        }
```
改为：
```rust
        if let Err(e) = TeamsApi::delete(&state, &team_id).await {
            web_sys::console::error_1(&format!("Delete failed: {e}").into());
            error_msg.set(Some(format!("删除失败: {e}")));
            return; // 不再继续刷新列表，让错误可见
        }
        error_msg.set(None);
```

- [ ] **Step 2: handle_disband 同样处理**

把 `handle_disband`（约 90-109）里对应的 `if let Err(e) = TeamsApi::disband(...)` 分支同样追加 `error_msg.set(Some(format!("解散失败: {e}")));` 与 `return;`，成功路径 `error_msg.set(None)`。

- [ ] **Step 3: 确认 error_msg 已在 UI 渲染**

检查 `overview.rs` 是否已有 `error_msg` 的渲染区（reload 用到了它，通常顶部有红条）。若无，加一处：
```rust
    {move || error_msg.get().map(|m| view! {
        <div class="px-3 py-2 rounded bg-danger/10 text-danger text-xs">{m}</div>
    })}
```

- [ ] **Step 4: 构建校验**

Run: `cargo build -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -5`
Expected: 编译通过。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/teams/overview.rs
git commit -m "panel: surface team disband/delete errors instead of console-only swallow"
```

### Task C2: `MessageStore::delete_team_messages`

**Files:**
- Modify: `src/teams/messages/store.rs`（trait 定义 61-131 加方法；`SqliteMessageStore` impl 加实现）
- Test: `src/teams/messages/store.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `delete_team_messages(&self, team_id: &str) -> crate::error::Result<usize>`（返回删除的 `team_messages` 行数）。

- [ ] **Step 1: trait 声明**

在 `MessageStore` trait（`store.rs:61-131`）内加：
```rust
    /// Hard-delete all messages (and their recipients) for a team. Returns rows deleted.
    async fn delete_team_messages(&self, team_id: &str) -> crate::error::Result<usize>;
```

- [ ] **Step 2: 写失败测试**

在 `store.rs` 的 `mod tests` 中（沿用本文件已有的 in-memory store + migrate 建库方式）：
```rust
    #[tokio::test]
    async fn delete_team_messages_removes_rows_and_recipients() {
        let store = test_store().await; // 沿用本文件既有 setup helper（migrate + SqliteMessageStore::new）
        // 写两条 team-A 消息 + 一条 team-B 消息（用本文件既有的 send_message helper/接口）
        seed_message(&store, "team-A", "alice").await;
        seed_message(&store, "team-A", "bob").await;
        seed_message(&store, "team-B", "carol").await;

        let n = store.delete_team_messages("team-A").await.unwrap();
        assert_eq!(n, 2, "删除 team-A 的两条消息");
        assert!(store.list_team_messages("team-A", 100).await.unwrap().is_empty());
        assert_eq!(store.list_team_messages("team-B", 100).await.unwrap().len(), 1, "不误删 team-B");
    }
```
（若本文件无 `test_store`/`seed_message` helper，则按文件内既有测试构造方式内联建库与插入。）

- [ ] **Step 3: 实现**

在 `impl MessageStore for SqliteMessageStore` 中加：
```rust
    async fn delete_team_messages(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        // 先删 recipients（无 FK cascade），再删消息行
        conn.execute(
            "DELETE FROM message_recipients WHERE message_id IN \
             (SELECT id FROM team_messages WHERE team_id = ?1)",
            params![team_id],
        )
        .map_err(db_err)?;
        let n = conn
            .execute("DELETE FROM team_messages WHERE team_id = ?1", params![team_id])
            .map_err(db_err)?;
        Ok(n)
    }
```
（确认本文件顶部已 `use rusqlite::params;` 与 `db_err` helper；若 `db_err` 不在本文件，沿用本文件既有错误映射方式。）

- [ ] **Step 4: 运行测试（可批量到 Phase 末）**

Run: `cargo test -p alephcore --lib teams::messages::store::tests::delete_team_messages_removes_rows_and_recipients`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/teams/messages/store.rs
git commit -m "teams: add MessageStore::delete_team_messages for cascade hard-delete"
```

### Task C3: `CoordTaskStore::delete_team_tasks`

**Files:**
- Modify: `src/agents/swarm/tasks/mod.rs`（`CoordTaskStore` trait，424-551 区）
- Modify: `src/agents/swarm/tasks/store/mod.rs`（`SqliteCoordTaskStore` impl）
- Test: `src/agents/swarm/tasks/store/tests.rs`

**Interfaces:**
- Produces: `delete_team_tasks(&self, team_id: &str) -> crate::error::Result<usize>`（返回删除的 `coord_tasks` 行数）。
- 不依赖 FK CASCADE：显式手删 runs/comments/journals/dependencies。

- [ ] **Step 1: trait 声明**

在 `CoordTaskStore` trait 加：
```rust
    /// Hard-delete all tasks for a team and their child rows
    /// (runs/comments/journals/dependencies). Returns coord_tasks rows deleted.
    async fn delete_team_tasks(&self, team_id: &str) -> crate::error::Result<usize>;
```

- [ ] **Step 2: 写失败测试**

在 `tests.rs`（沿用本文件 store 构造方式，参考既有 `team.team-T.task.created` 测试）：
```rust
    #[tokio::test]
    async fn delete_team_tasks_removes_tasks_and_children() {
        let store = new_test_store(); // 沿用本文件既有构造
        let t = store.create_task(NewCoordTask {
            team_id: Some("team-A".into()),
            subject: "s".into(), description: String::new(), owner: None,
            priority: Priority::Normal, blocked_by: Vec::new(), metadata: serde_json::json!({}),
        }).await.unwrap();
        store.add_comment(&t.id, "author", "body").await.unwrap(); // 用本文件既有 comment 接口

        let n = store.delete_team_tasks("team-A").await.unwrap();
        assert_eq!(n, 1);
        assert!(store.get_task(&t.id).await.unwrap().is_none(), "任务已删");
        assert!(store.list_task_comments(&t.id).await.unwrap().is_empty(), "子表 comments 已删");
    }
```
（comment/list 接口名以本文件既有为准；若不同则替换为等价接口。）

- [ ] **Step 3: 实现**

在 `impl CoordTaskStore for SqliteCoordTaskStore` 加（手删子表，不靠 FK pragma）：
```rust
    async fn delete_team_tasks(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let in_team = "SELECT id FROM coord_tasks WHERE team_id = ?1";
        for child in [
            "DELETE FROM coord_task_dependencies WHERE task_id IN (SELECT id FROM coord_tasks WHERE team_id = ?1) OR depends_on IN (SELECT id FROM coord_tasks WHERE team_id = ?1)",
            "DELETE FROM coord_task_runs WHERE task_id IN (SELECT id FROM coord_tasks WHERE team_id = ?1)",
            "DELETE FROM coord_task_comments WHERE task_id IN (SELECT id FROM coord_tasks WHERE team_id = ?1)",
            "DELETE FROM coord_task_journals WHERE task_id IN (SELECT id FROM coord_tasks WHERE team_id = ?1)",
        ] {
            conn.execute(child, params![team_id]).map_err(db_err)?;
        }
        let _ = in_team; // 文档用途
        let n = conn
            .execute("DELETE FROM coord_tasks WHERE team_id = ?1", params![team_id])
            .map_err(db_err)?;
        Ok(n)
    }
```
（`db_err`/`params` 沿用本 impl 文件既有引入。）

- [ ] **Step 4: 运行测试**

Run: `cargo test -p alephcore --lib delete_team_tasks_removes_tasks_and_children`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agents/swarm/tasks/mod.rs src/agents/swarm/tasks/store/mod.rs src/agents/swarm/tasks/store/tests.rs
git commit -m "coord: add CoordTaskStore::delete_team_tasks (explicit child cleanup)"
```

### Task C4: `EventLogStore::delete_team_events`

**Files:**
- Modify: `src/teams/events.rs`（trait 143-162 + `SqliteEventLogStore` impl）
- Test: `src/teams/events.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `delete_team_events(&self, team_id: &str) -> crate::error::Result<usize>`。

- [ ] **Step 1: trait 声明**

```rust
    /// Hard-delete all events for a team. Returns rows deleted.
    async fn delete_team_events(&self, team_id: &str) -> crate::error::Result<usize>;
```

- [ ] **Step 2: 写失败测试**

```rust
    #[tokio::test]
    async fn delete_team_events_removes_team_rows_only() {
        let store = test_store().await; // 沿用本文件既有构造
        store.log_event(new_event("team-A", "started")).await.unwrap(); // 用本文件既有 helper
        store.log_event(new_event("team-B", "started")).await.unwrap();
        let n = store.delete_team_events("team-A").await.unwrap();
        assert_eq!(n, 1);
        assert!(store.get_events("team-A", None, None).await.unwrap().is_empty());
        assert_eq!(store.get_events("team-B", None, None).await.unwrap().len(), 1);
    }
```

- [ ] **Step 3: 实现**

```rust
    async fn delete_team_events(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let n = conn
            .execute("DELETE FROM team_events WHERE team_id = ?1", rusqlite::params![team_id])
            .map_err(db_err)?;
        Ok(n)
    }
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p alephcore --lib teams::events::tests::delete_team_events_removes_team_rows_only`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/teams/events.rs
git commit -m "teams: add EventLogStore::delete_team_events for cascade hard-delete"
```

### Task C5: `ArtifactStore::delete_artifacts_for_tasks`

**Files:**
- Modify: `src/teams/artifacts.rs`（trait 281-293 + `SqliteArtifactStore` impl）
- Test: `src/teams/artifacts.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `delete_artifacts_for_tasks(&self, task_ids: &[String]) -> crate::error::Result<usize>`（按 task_id 列表删 `task_artifacts` + 其 dependencies；返回删除的 artifact 行数）。
- 设计：artifacts 只有 `task_id`（无 `team_id`），故由调用方（handle_delete）先用 `coord_store.list_tasks` 取 team 的 task_ids 再传入。

- [ ] **Step 1: trait 声明**

```rust
    /// Hard-delete all artifacts (and their dependency rows) belonging to the given tasks.
    /// Returns artifact rows deleted.
    async fn delete_artifacts_for_tasks(&self, task_ids: &[String]) -> crate::error::Result<usize>;
```

- [ ] **Step 2: 写失败测试**

```rust
    #[tokio::test]
    async fn delete_artifacts_for_tasks_removes_only_listed() {
        let store = test_store().await; // 沿用本文件既有构造
        let a = store.create_artifact(new_artifact("task-1")).await.unwrap(); // 用本文件既有 helper
        store.create_artifact(new_artifact("task-2")).await.unwrap();
        let n = store.delete_artifacts_for_tasks(&["task-1".to_string()]).await.unwrap();
        assert_eq!(n, 1);
        assert!(store.get_artifact(&a.id).await.unwrap().is_none());
        assert_eq!(store.get_artifacts_for_task("task-2").await.unwrap().len(), 1);
    }
```

- [ ] **Step 3: 实现（变长 IN 占位符）**

```rust
    async fn delete_artifacts_for_tasks(&self, task_ids: &[String]) -> crate::error::Result<usize> {
        if task_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().await;
        let placeholders = std::iter::repeat("?").take(task_ids.len()).collect::<Vec<_>>().join(",");
        let params: Vec<&dyn rusqlite::ToSql> =
            task_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        // 先删 dependencies（指向将被删的 artifact），再删 artifacts
        conn.execute(
            &format!(
                "DELETE FROM task_artifact_dependencies WHERE artifact_id IN \
                 (SELECT id FROM task_artifacts WHERE task_id IN ({placeholders}))"
            ),
            params.as_slice(),
        )
        .map_err(db_err)?;
        let n = conn
            .execute(
                &format!("DELETE FROM task_artifacts WHERE task_id IN ({placeholders})"),
                params.as_slice(),
            )
            .map_err(db_err)?;
        Ok(n)
    }
```
（`db_err` 沿用本文件既有错误映射；确认 `use rusqlite::ToSql;` 可用或全限定。）

- [ ] **Step 4: 运行测试**

Run: `cargo test -p alephcore --lib teams::artifacts::tests::delete_artifacts_for_tasks_removes_only_listed`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/teams/artifacts.rs
git commit -m "teams: add ArtifactStore::delete_artifacts_for_tasks for cascade hard-delete"
```

### Task C6: `SqliteSnapshotStore::delete_team_snapshots`

**Files:**
- Modify: `src/teams/snapshots/store.rs`（`SqliteSnapshotStore` inherent impl）
- Test: `src/teams/snapshots/store.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `delete_team_snapshots(&self, team_id: &str) -> crate::error::Result<usize>`（inherent 方法，非 trait）。

- [ ] **Step 1: 写失败测试**

```rust
    #[tokio::test]
    async fn delete_team_snapshots_removes_team_rows() {
        let store = test_store().await; // 沿用本文件既有构造（new_from_shared + migrate）
        seed_snapshot(&store, "team-A").await; // 用本文件既有 save/insert 接口
        seed_snapshot(&store, "team-B").await;
        let n = store.delete_team_snapshots("team-A").await.unwrap();
        assert_eq!(n, 1);
    }
```
（若本文件无快照写入 helper，则按既有快照保存接口插入两条不同 team 的记录。）

- [ ] **Step 2: 实现**

在 `impl SqliteSnapshotStore` 加：
```rust
    /// Hard-delete all snapshots for a team. Returns rows deleted.
    pub async fn delete_team_snapshots(&self, team_id: &str) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let n = conn
            .execute(
                "DELETE FROM coord_team_snapshots WHERE team_id = ?1",
                rusqlite::params![team_id],
            )
            .map_err(|e| crate::error::AlephError::ConfigError {
                message: format!("SnapshotStore: {e}"),
                suggestion: None,
            })?;
        Ok(n)
    }
```

- [ ] **Step 3: 运行测试**

Run: `cargo test -p alephcore --lib teams::snapshots::store::tests::delete_team_snapshots_removes_team_rows`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/teams/snapshots/store.rs
git commit -m "teams: add SqliteSnapshotStore::delete_team_snapshots for cascade hard-delete"
```

### Task C7: `handle_delete` 级联编排 + 注册注入多 store

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（`handle_delete` 147-164 重写）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/agents.rs:211`（`teams.delete` 注册改为手动闭包注入多 store）

**Interfaces:**
- Consumes: Task C2–C6 的 5 个新方法 + 既有 `TeamStore::delete_team`、`CoordTaskStore::list_tasks(CoordTaskFilter)`。
- `register_teams_handlers` 已有参数：`store`、`coord_store`（非 Option）、`event_store`/`artifact_store`/`snapshot_store`/`msg_store`（Option）。
- Produces: `teams.delete` RPC 行为 = 先 `delete_team`（把关 disbanded + 移除团队行），成功后 best-effort 清 5 个从属 store。

- [ ] **Step 1: 重写 `handle_delete` 签名与编排**

把 `src/gateway/handlers/teams.rs:147-164` 的 `handle_delete` 改为：
```rust
pub async fn handle_delete(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    msg_store: Arc<dyn crate::teams::messages::MessageStore>,
    event_store: Arc<dyn crate::teams::events::EventLogStore>,
    artifact_store: Arc<dyn crate::teams::artifacts::ArtifactStore>,
    snapshot_store: Arc<crate::teams::SqliteSnapshotStore>,
) -> JsonRpcResponse {
    debug!("Handling teams.delete request (cascade)");
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let team_id = params.team_id;

    // 1) 权威把关 + 移除团队行（disbanded 校验在 delete_team 内）。失败即返回，不级联。
    if let Err(e) = store.delete_team(&team_id).await {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete team '{team_id}': {e}"),
        );
    }

    // 2) best-effort 清理从属 store 的孤儿（单条失败仅告警，不影响整体成功）。
    let task_ids: Vec<String> = match coord_store
        .list_tasks(CoordTaskFilter { team_id: Some(team_id.clone()), status: None, owner: None })
        .await
    {
        Ok(tasks) => tasks.into_iter().map(|t| t.id).collect(),
        Err(e) => {
            warn!("teams.delete: list tasks for artifact cleanup failed: {e}");
            Vec::new()
        }
    };
    if let Err(e) = artifact_store.delete_artifacts_for_tasks(&task_ids).await {
        warn!("teams.delete: artifact cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = coord_store.delete_team_tasks(&team_id).await {
        warn!("teams.delete: task cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = snapshot_store.delete_team_snapshots(&team_id).await {
        warn!("teams.delete: snapshot cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = msg_store.delete_team_messages(&team_id).await {
        warn!("teams.delete: message cleanup failed for {team_id}: {e}");
    }
    if let Err(e) = event_store.delete_team_events(&team_id).await {
        warn!("teams.delete: event cleanup failed for {team_id}: {e}");
    }

    JsonRpcResponse::success(request.id, json!({ "success": true }))
}
```
确认 `teams.rs` 顶部已 `use` 了 `CoordTaskFilter`、`warn`（`tracing::warn`）；缺则补 `use tracing::warn;` 与 `use crate::agents::swarm::tasks::CoordTaskFilter;`（与 `handle_get`/`handle_list_tasks` 同源）。

- [ ] **Step 2: 改注册为手动多-store 闭包（macro 仅支持 ≤4 ctx，故手写）**

把 `agents.rs:211` 的：
```rust
register_handler!(server, "teams.delete", teams::handle_delete, store);
```
替换为（gate 在 4 个 Option store 全部 Some 时走级联，否则回退基础删除，绝不回归）：
```rust
match (event_store, snapshot_store, artifact_store, msg_store.clone()) {
    (Some(ev), Some(snap), Some(art), Some(msg)) => {
        let store_c = Arc::clone(store);
        let coord_c = Arc::clone(coord_store);
        let ev_c = Arc::clone(ev);
        let snap_c = Arc::clone(snap);
        let art_c = Arc::clone(art);
        let msg_c = Arc::clone(&msg);
        server.handlers_mut().register("teams.delete", move |req| {
            let store_c = Arc::clone(&store_c);
            let coord_c = Arc::clone(&coord_c);
            let ev_c = Arc::clone(&ev_c);
            let snap_c = Arc::clone(&snap_c);
            let art_c = Arc::clone(&art_c);
            let msg_c = Arc::clone(&msg_c);
            async move {
                teams::handle_delete(req, store_c, coord_c, msg_c, ev_c, art_c, snap_c).await
            }
        });
    }
    _ => {
        // 回退：仅 TeamStore 删除（旧行为），保证从属 store 缺席配置不崩。
        register_handler!(server, "teams.delete", teams::handle_delete_basic, store);
    }
}
```
说明：`snapshot_store` 形参类型是 `Option<&Arc<SqliteSnapshotStore>>`，`event_store`/`artifact_store` 同为 `Option<&Arc<dyn ...>>`，`msg_store` 是 `Option<Arc<dyn MessageStore>>`（已 owned）——`Arc::clone` 时注意引用层级（对 `&Arc` 用 `Arc::clone(x)` 得 `Arc`）。

- [ ] **Step 3: 保留旧基础删除为 `handle_delete_basic`**

在 `teams.rs` 保留原单-store 版本改名，供回退分支用：
```rust
/// Fallback used when subordinate stores are not all configured: removes the
/// team row only (legacy behavior). Cascade cleanup is skipped.
pub async fn handle_delete_basic(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match store.delete_team(&params.team_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete team '{}': {}", params.team_id, e),
        ),
    }
}
```

- [ ] **Step 4: 编译校验（一次性，高风险改动允许一次 `--lib`）**

Run: `cargo check -p alephcore --lib 2>&1 | tail -20`
Expected: 通过。重点核对 `Arc::clone` 引用层级、`CoordTaskFilter` 字段名（`team_id`/`status`/`owner`）、`warn!` 导入。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/teams.rs src/bin/aleph-server/commands/start/builder/handlers/agents.rs
git commit -m "teams: cascade hard-delete across stores in teams.delete handler"
```

---

## Phase D — 构建、部署、E2E 验证

### Task D1: 全量构建 + 部署 + 端到端验证

**Files:** 无（验证任务）。

- [ ] **Step 1: 后端批量测试（一次性）**

Run: `cargo test -p alephcore --lib teams:: member_prompt:: delete_team 2>&1 | tail -20`
Expected: Phase B/C 新增单测全 PASS。

- [ ] **Step 2: 重建 WASM + server binary**

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server 2>&1 | tail -5
```
Expected: 两者成功（rust_embed 把新 dist 烧进 binary）。

- [ ] **Step 3: 替换运行中 daemon（按 CLAUDE.md 刷新链）**

dev daemon：`./target/release/aleph-server stop` → `cargo run --release -p alephcore --bin aleph-server start`
（.app daemon 按 CLAUDE.md 的 mv/cp/kill 流程）。确认 `curl -s localhost:18790`（或实际端口）HTTP 200。

- [ ] **Step 4: E2E — Problem 1（任务编排 + 流式）**

开一个群聊（≥2 成员）→ 发实质需求（如「调研 X 的三种方案并产出对比报告」）。验证：
- 全局 `团队` tab 的 `看板`/`计划` 与 per-chat `任务` tab 出现对应 coord_task，状态从 pending→in_progress→completed 推进；
- 被派活成员的回复以归属气泡出现在群聊，名册状态在 working/done 切换。
- 若 leader 仍只闲聊不派活：检查所选 leader 是否被路由为发言者（无 @ 时 leader 默认接话），必要时复核 prompt 文案强度。

- [ ] **Step 5: E2E — Problem 2（解散正名）**

侧栏群聊行 `⋯` 菜单显示「解散」，确认条显示「确认解散？」；点击后团队从侧栏 active 列表消失，仍存在于 `团队` tab 概览（disbanded 状态，带 Delete 按钮）。

- [ ] **Step 6: E2E — Problem 3（级联硬删除）**

在 `团队` tab 概览对一个 active 团队点「Disband」→ 刷新后出现「Delete」→ 点「Delete」确认。验证：
- 团队从概览列表消失（不再「点了没反应」）；若后端报错，界面显示具体错误（Task C1）。
- 用 sqlite 抽查各库：该 `team_id` 在 `teams`/`team_members`/`team_messages`/`coord_tasks`/`team_events`/`task_artifacts`/`coord_team_snapshots` 中均无残留行。
- **若删除仍失败且错误指向 device-tier/authz**：说明 `teams.delete` 被 `method_authz` 当 operator-only 拦截而当前连接是 Chat tier——这是单独的授权问题，记录真实错误串后作为后续一轮处理（本计划已让该错误可见，达成「可诊断」目标）。

- [ ] **Step 7: 最终提交（如有部署期修补）**

```bash
git add -A
git commit -m "teams: post-E2E fixes for task wiring + cascade delete"
```

---

## Self-Review（plan 对照 spec）

- **Spec §1 P1 写侧**：Task B1（prompt）✓ **读侧**：Task B2（per-chat 刷新）✓；全局看板已连通仅验证（D-Step4）✓ **子决策 A 流式**：随 fanout 自动成立，D-Step4 验证 ✓
- **Spec §2 P2 改名**：Task A1 ✓
- **Spec §3 P3 错误可见**：Task C1 ✓ **级联硬删除**：Task C2–C6 五方法 + C7 编排/注册 ✓ **顺序**：delete_team 把关在先、从属 best-effort 在后 ✓ **E2E 抓 authz**：D-Step6 contingency ✓
- **Placeholder scan**：无 TBD；测试 setup 引用「本文件既有构造」是因 fixture 为文件内私有 helper，已给出断言主体与被测调用的真实签名。
- **Type consistency**：5 个新方法签名在 Phase C 头部统一声明，C7 消费处逐一匹配；`CoordTaskFilter { team_id, status, owner }` 与 `handle_list_tasks` 一致。

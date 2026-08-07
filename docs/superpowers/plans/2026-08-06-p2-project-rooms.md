# P2 Project Rooms Implementation Plan / P2「项目房间」实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 两个用户在同一个项目房间里用同一个会话与 agent 协作，项目记忆共享、个人记忆不泄露；非成员既看不见这个项目，也无法区分「不存在」和「不属于我」。

**Architecture:** P1 已经把 `ScopeId::Project(p)` 这个词写进了词汇表但没有任何生产者——P2 是给它接上生产者和消费者。三件事：(1) `src/projects/` 从 JSON「最近目录名册」升格为 SQLite 一等实体（`projects` + `project_members` 两张表，owner 可空=收养语义），并把名册投影成一份进程内快照供同步谓词读取；(2) `gateway::visibility` 增加 `project_visible`，`session_visible` / `partition_visible` / `SessionFilter` 各长出 project 分支——**成员制即授权**，判据只有一条「caller 在不在这个 project 的名册里」；(3) 会话的 scope 在创建时由请求的 `project_id` 决定（此后不可变），记忆分区跟着 scope 走 `base__p-<id>`，召回并集是 `[org, project]` 而**不含**个人。项目群聊就是一个普通会话，只是多个人类可以往里发言——**不复用 `group_chat`（遗留人格圆桌）也不复用 `teams` 广播**。

**Tech Stack:** Rust（rusqlite / tokio task-local / serde）+ Leptos WASM Panel。无新依赖。

**Spec:** `docs/superpowers/specs/2026-08-04-multi-user-org-project-design.md` §5.2 / §5.3 / §5.4 / §6 / §8 P2 行 / §9 / §10。
**前置:** P0（`011715841` 之前）+ P1（`913fe0b4f..011715841`）+ teams 收紧（`011715841..baa6860b4`），全部在 main 未推送。

---

## Global Constraints

1. **验收（spec §8 P2）:** 两用户在同一项目群聊协作，项目记忆共享。外加 P1 的不变量继续成立：非成员看不到项目会话/记忆，单用户体验逐字节不变。
2. **收养即缺席 (adoption-by-absence):** `projects.owner_user_id` 为空 ⇒ 归 `OWNER_USER_ID`（`"u-owner"`）。零回填。判据只有一份：`gateway::visibility::owner_or_legacy`。**不要再写第二个 `unwrap_or(OWNER_USER_ID)`。**
3. **Fail-closed:** 授权决策处的 store `Err` / 无法解析的状态 ⇒ 拒绝，绝不放行。`.ok().flatten()` 在闸上是禁止形状。
4. **无存在性预言机:** 「不是成员」与「项目不存在」必须产生**逐字节相同**的响应。同理，唯一索引不得跨用户可见（P1 教训：`idx_teams_name_active` 曾告诉 bob「alice 的团队名被占了」）。SQLite 里 NULL 互不相等 ⇒ **owner 键控的唯一索引必须 `COALESCE(owner_user_id, 'u-owner')`，不能裸列**。
5. **不受限调用者优先:** 每个谓词的第一臂永远是 `None => true`（cron / 后台清扫 / A2A / 进程内测试）。这是单用户零变化保证的载体，新增 project 分支必须排在它**之后**。
6. **会话 scope 不可变（spec §10）:** 会话一旦创建，scope 不迁移。请求里的 `project_id` 只在**创建**时有话语权；会话已存在时，运行的归属一律从持久行（`SessionMetadata::scope_id` → `ScopeAttribution::from_persisted`）派生。
7. **项目会话不召回个人记忆（spec §5.2 / §13）:** 召回并集是 `[base, base__p-<id>]`。**不得**把 `base__u-<caller>` 并进去——项目群聊全员可见，个人记忆经 agent 引用即向全项目泄露。
8. **两套环境机制都要覆盖:** `CALLER_USER` 在 spawn 出去的 run 里是死的，`scope::current_scope()` 在裸 RPC 里是死的。网关侧谓词用 `visible_owner_filter()`，run 内可达的谓词用 `scope::ambient_owner()`。写新谓词前先问「这个东西从几个面可达」。
9. **测试断言效果，不断言调用:** 「alice 建数据 / bob 看到空或 NOT_FOUND / alice 的数据完好」。永远不要断言「过滤函数被调用了」。
10. **R7 / R10:** 不用确定性代码替代 LLM 推理；`src/harness/` 一行不动（本计划任何一步都不该碰它——如果实现者认为需要，**停下来上报**）。
11. **验证集（Windows，`cargo check -p alephcore` 单独一条证明不了任何事）:** 每个任务的门是 `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib <module>`（前台、不接管道、timeout 600000）。最后一个任务跑四条全量：`cargo test -p alephcore --lib` + `cargo check -p aleph-panel` + `cargo check -p aleph-desktop-windows` + `cargo clippy --all-targets --workspace --exclude aleph-desktop-macos --exclude aleph-desktop-linux`。
12. **rustfmt 只按叶子文件跑:** `rustfmt --edition 2021 --check <改过的文件>`。**永远不要** `cargo fmt -p alephcore`（本仓不是全局 fmt-clean 的）。
13. **提交:** `<scope>: <description>`，英文，每任务至少一个提交。单分支 main。**不要 push。**
14. **注意 CRLF:** 本仓 CRLF/LF 混排。用 `perl -0pi` 之类的批量替换前先确认目标文件的行尾，否则会静默 no-op（P1 踩过三次）。

---

## File Structure (new / modified)

| File | Responsibility |
|---|---|
| **Rewrite** `src/projects/store.rs` | `ProjectStore` 改 SQLite：`projects` + `project_members`、名册 CRUD、`projects.json` 一次性迁移、写后发布名册投影 |
| **Create** `src/projects/roster.rs` | 进程内名册投影（`RwLock<RosterSnapshot>`）+ 同步谓词 `is_member` / `projects_of`；真源仍是 store |
| Modify `src/projects/mod.rs` | 导出 `roster`、`ProjectStore::shared()`、`Project` 新字段 |
| Modify `src/gateway/visibility.rs` | `project_visible` / `ambient_project_visible`；`session_visible` 长出 project 臂；`partition_visible` 长出 `p-` 臂 |
| Modify `src/gateway/session_store/types.rs` + 两个 backend | `SessionFilter::visible_scope_ids`，两个后端 OR 进查询 |
| Modify `src/gateway/handlers/projects.rs` | 6 个既有 handler 加闸 + 6 个新名册 handler |
| Modify `src/gateway/method_visibility.rs` | `projects.*` 条目 + pin 测试 + 刻意缺席的理由 |
| Modify `src/gateway/handlers/agent.rs`、`src/gateway/execution_engine/*` | `project_id` 参数 → 成员校验 → `ScopeId::Project` 归属；已存在会话从持久行派生 |
| Modify `src/gateway/busy_queue/` | 项目会话里非发起者的消息强制 `Queue`（spec §10） |
| Modify `src/memory/project_scope.rs` | `session_write_id` / `session_read_ids` 的 `Project` 臂 |
| Modify `src/thinker/memory_context_provider/*` | USER.md floor 判据收成 `ScopeId::Personal`（不是「有 scope」） |
| Modify `src/session/events.rs` + 发射点 | `UserMessage.author_user_id` |
| Modify `interfaces/webchat/` | 侧栏「项目」+ 项目页（群聊 tab / 设置 tab） |
| Modify `docs/reference/SECURITY.md`、`src/gateway/CLAUDE.md`、`CLAUDE.md` 判据清单 | P2 信任边界 + 新地雷 |

---

### Task 1: Project 实体升格为 SQLite + 名册投影

**Files:**
- Rewrite: `src/projects/store.rs`
- Create: `src/projects/roster.rs`
- Modify: `src/projects/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/mod.rs:1621`、`src/extension/mod.rs:821,880`、`src/gateway/execution_engine/run_loop/inner.rs:97`、`src/gateway/handlers/mod.rs:511`（4 个临时 `ProjectStore::new()` 调用点）

**Interfaces (produced — 后续任务逐字消费):**
```rust
// src/projects/store.rs
pub struct Project {
    pub id: String,                        // "p-<uuid simple>"
    pub name: String,
    pub owner_user_id: Option<String>,     // None ⇒ 收养为 u-owner
    pub workspace_path: Option<PathBuf>,
    pub status: ProjectStatus,             // Active | Archived
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used_at: i64,
}
pub enum ProjectStatus { Active, Archived }

impl ProjectStore {
    pub fn shared() -> Arc<ProjectStore>;                 // 进程内单例（替代旧的 ::new()）
    pub fn new(conn: Connection) -> Self;                 // 测试用
    pub fn migrate(&self) -> Result<(), ProjectError>;    // 建表 + projects.json 一次性迁移 + 发布名册
    pub fn create(&self, name: &str, owner: Option<&str>, workspace: Option<&Path>) -> Result<Project, ProjectError>;
    pub fn get(&self, id: &str) -> Result<Option<Project>, ProjectError>;
    pub fn list(&self) -> Result<Vec<Project>, ProjectError>;             // 全量，未过滤；过滤在 handler
    pub fn rename(&self, id: &str, name: &str) -> Result<Project, ProjectError>;
    pub fn archive(&self, id: &str) -> Result<(), ProjectError>;
    pub fn touch(&self, id: &str) -> Result<(), ProjectError>;
    pub fn remove(&self, id: &str) -> Result<(), ProjectError>;
    pub fn bind_workspace(&self, id: &str, path: Option<&Path>) -> Result<Project, ProjectError>;
    pub fn add_member(&self, id: &str, user_id: &str) -> Result<(), ProjectError>;
    pub fn remove_member(&self, id: &str, user_id: &str) -> Result<(), ProjectError>;
    pub fn members(&self, id: &str) -> Result<Vec<String>, ProjectError>;
    pub fn add(&self, path: &Path, name: Option<String>) -> Result<Project, ProjectError>;       // 兼容：最近目录登记
    pub fn create_blank(&self, parent: &Path, name: &str) -> Result<Project, ProjectError>;      // 兼容
    pub fn find_by_path(&self, path: &Path) -> Result<Option<Project>, ProjectError>;            // 兼容
}

// src/projects/roster.rs
pub fn is_member(project_id: &str, user_id: &str) -> bool;
pub fn projects_of(user_id: &str) -> Vec<String>;      // 排序后返回，供 SQL IN(...) 用
pub fn publish(snapshot: RosterSnapshot);
pub struct RosterSnapshot { /* project_id -> members，只由 store 构造 */ }
```

**架构说明（实现者必读，别自己改判据）：**
- **真源是表，名册是投影。** `roster.rs` 是一份进程内快照，**只由 `ProjectStore` 在它自己的写锁里重新发布**。这是本仓 `MessageProjector` 的同款形状（表是投影、`session_events` 是真源），倒过来就是制造第二个真源。
- **为什么需要投影:** `visibility::session_visible` 是**同步**谓词，且在 `sessions.list` 里按会话逐条调用。让它每次去查 SQLite 等于把 N 次磁盘往返塞进列表路径；让它变 async 会病毒式传染到 P1 建好的全部谓词。投影让读是纯内存的。
- **代价（明确接受）:** 第二个进程写 `projects.db` 时本进程的投影不会更新。名册变更只经 RPC（即本进程），所以不成立；**若将来加了 CLI 的名册子命令，那条路径必须走 IPC，不能直连 DB**——把这句写进 `roster.rs` 的模块 doc。
- **未发布 = 空名册。** 测试 / CLI 里没调 `migrate()` 时 `is_member` 恒 false。因为每个谓词的第一臂是「不受限调用者 ⇒ true」，这不影响任何既有行为。

**Schema:**
```sql
CREATE TABLE IF NOT EXISTS projects (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    owner_user_id  TEXT,
    workspace_path TEXT,
    status         TEXT NOT NULL DEFAULT 'active',
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    last_used_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS project_members (
    project_id TEXT NOT NULL,
    user_id    TEXT NOT NULL,
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_project_members_user ON project_members(user_id);
-- Owner-keyed, NOT global: a global unique index on workspace_path would tell
-- bob that alice already bound this folder. COALESCE is mandatory — SQLite
-- treats NULLs as distinct, so keying on the raw column silently drops the
-- constraint for every legacy (unstamped) row.
CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_owner_path_active
    ON projects(COALESCE(owner_user_id, 'u-owner'), workspace_path)
    WHERE workspace_path IS NOT NULL AND status = 'active';
```

- [ ] **Step 1: 写失败测试 — 迁移不变量**

`src/projects/store.rs` 的 `#[cfg(test)] mod tests`：

```rust
#[test]
fn legacy_json_catalogue_migrates_into_the_table_once() {
    let dir = tempdir().unwrap();
    let json = dir.path().join("projects.json");
    std::fs::write(
        &json,
        r#"{"version":1,"projects":[{"id":"deadbeefdeadbeef","name":"alpha",
            "path":"/tmp/alpha","created_at":100,"last_used_at":200}]}"#,
    )
    .unwrap();

    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate_from_json(&json).unwrap();

    let all = store.list().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "alpha");
    assert_eq!(all[0].created_at, 100, "timestamps are preserved verbatim");
    assert!(all[0].id.starts_with("p-"), "ids are re-minted into the p- family");
    assert_eq!(all[0].owner_user_id.as_deref(), Some(OWNER_USER_ID));
    assert_eq!(store.members(&all[0].id).unwrap(), vec![OWNER_USER_ID.to_string()]);

    // Idempotent: running it again must not duplicate (crash between insert
    // and the rename marker is a real state).
    store.migrate_from_json(&json).unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn two_users_may_bind_the_same_folder() {
    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    let a = store.create("repo", Some("u-alice"), Some(Path::new("/tmp/repo"))).unwrap();
    let b = store.create("repo", Some("u-bob"), Some(Path::new("/tmp/repo")));
    assert!(b.is_ok(), "path uniqueness is per-owner, never global (no oracle)");
    assert_ne!(a.id, store.get(&b.unwrap().id).unwrap().unwrap().id);
}

#[test]
fn the_same_owner_rebinding_a_folder_collapses_onto_one_row() {
    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    let first = store.add(Path::new("/tmp/x"), None).unwrap();
    let again = store.add(Path::new("/tmp/x"), None).unwrap();
    assert_eq!(first.id, again.id, "the recent-directory picker must not duplicate");
}

#[test]
fn creating_a_project_publishes_its_roster() {
    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    let p = store.create("room", Some("u-alice"), None).unwrap();
    assert!(roster::is_member(&p.id, "u-alice"));
    assert!(!roster::is_member(&p.id, "u-bob"));
    store.add_member(&p.id, "u-bob").unwrap();
    assert!(roster::is_member(&p.id, "u-bob"), "the projection follows the write");
    store.remove_member(&p.id, "u-bob").unwrap();
    assert!(!roster::is_member(&p.id, "u-bob"), "removal is immediate (spec §10)");
}
```

> ⚠️ `roster` 是进程全局的，测试之间会串。用一个 `static ROSTER_TEST_GUARD: Mutex<()>` 串行化触碰名册的测试（照抄 `utils::paths::ALEPH_HOME_TEST_GUARD` 的形状），或让每个测试用自己独立的 project id 前缀。**两者选其一并在 doc 里写明**。

- [ ] **Step 2: 跑测试确认失败**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib projects:: -- --nocapture
```
Expected: FAIL —— `ProjectStore::new` 签名不匹配 / `migrate_from_json` 不存在 / `roster` 模块不存在。

- [ ] **Step 3: 实现 `roster.rs`**

```rust
//! In-process projection of the project roster.
//!
//! The SSOT is the `project_members` table; this is a read-optimised snapshot
//! that [`crate::projects::ProjectStore`] republishes inside its own write
//! lock, exactly like `MessageProjector` republishes the `messages` table from
//! `session_events`. Never write to it from anywhere else — a second writer is
//! a second source of truth.
//!
//! It exists because `gateway::visibility::session_visible` is a SYNCHRONOUS
//! predicate called once per session while filtering a list. Querying SQLite
//! there would put N round-trips on the list path; making it async would spread
//! virally through every P1 predicate.
//!
//! Cross-process caveat: a second process writing `projects.db` will not be
//! seen here. Roster mutation is RPC-only (this process) today. **If a CLI
//! roster subcommand is ever added it MUST go through IPC, not straight to the
//! database.**

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Default, Clone)]
pub struct RosterSnapshot {
    members: HashMap<String, HashSet<String>>,
}

impl RosterSnapshot {
    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut members: HashMap<String, HashSet<String>> = HashMap::new();
        for (project_id, user_id) in pairs {
            members.entry(project_id).or_default().insert(user_id);
        }
        Self { members }
    }
}

fn cell() -> &'static RwLock<RosterSnapshot> {
    static ROSTER: OnceLock<RwLock<RosterSnapshot>> = OnceLock::new();
    ROSTER.get_or_init(|| RwLock::new(RosterSnapshot::default()))
}

/// Replace the projection. Called by `ProjectStore` after every mutation and
/// once at `migrate()`.
pub fn publish(snapshot: RosterSnapshot) {
    let mut guard = cell().write().unwrap_or_else(|e| e.into_inner());
    *guard = snapshot;
}

/// Whether `user_id` is on `project_id`'s roster. `false` for an unknown
/// project and for a never-published projection (tests, CLI) — fail closed.
/// Every caller checks the unrestricted-caller arm BEFORE reaching this.
#[must_use]
pub fn is_member(project_id: &str, user_id: &str) -> bool {
    let guard = cell().read().unwrap_or_else(|e| e.into_inner());
    guard.members.get(project_id).is_some_and(|m| m.contains(user_id))
}

/// Every project `user_id` belongs to, sorted for deterministic SQL.
#[must_use]
pub fn projects_of(user_id: &str) -> Vec<String> {
    let guard = cell().read().unwrap_or_else(|e| e.into_inner());
    let mut ids: Vec<String> = guard
        .members
        .iter()
        .filter(|(_, m)| m.contains(user_id))
        .map(|(p, _)| p.clone())
        .collect();
    ids.sort();
    ids
}
```

- [ ] **Step 4: 重写 `store.rs` 为 SQLite**

要点（逐条都是判据，不是风格建议）：
- `ProjectStore { conn: Arc<Mutex<Connection>> }`，照抄 `src/teams/store.rs:153-160` 的形状与 `db_err` helper。
- `shared()` 用 `OnceLock<Arc<ProjectStore>>`，路径 `get_data_dir()?.join("projects.db")`，开库走 `utils::sqlite_open::open_sqlite_safe`。**这一步同时消灭 4 个临时 `ProjectStore::new()` 调用点**——它们现在每次都开一个新连接会踩 SQLite locked。
- `migrate()` = 建表建索引 → `migrate_from_json(default_json_path())` → 重新发布名册。
- `migrate_from_json`：文件不存在就直接返回 `Ok`。存在则逐行 `INSERT OR IGNORE`（唯一索引负责幂等），成员表插入 owner 自己，最后把文件 rename 成 `projects.json.migrated`。**rename 失败不算迁移失败**（幂等由索引保证，不由 rename 保证）。
- 每个写方法结尾调用 `self.republish_roster()`，它在**同一把写锁内**读回 `project_members` 全表并 `roster::publish`。
- `add()` / `create_blank()` / `find_by_path()` 保留旧语义（最近目录选择器），内部落到 `create()`，owner 取 `crate::scope::ambient_owner()`。

- [ ] **Step 5: 跑测试确认通过**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib projects::
```
Expected: PASS（含既有的 `list_does_not_create_file_when_missing` 等 JSON 时代测试——**这些测试的意图（纯读不写盘）在 SQLite 下失去意义，删掉并在提交信息里说明，不要留着改成恒真**）。

- [ ] **Step 6: 修 4 个调用点 + `cargo check`**

`ProjectStore::new()` → `ProjectStore::shared()`。boot 处（`start/mod.rs:1621`）改成 `let project_store = alephcore::projects::ProjectStore::shared();` 并在其后调用 `project_store.migrate()`，失败只 warn 不 panic（照 `coord_stores.rs` 的降级形状）。

```
cargo check -p alephcore && cargo check --bin aleph-server
```

- [ ] **Step 7: Commit**

```bash
git add src/projects/ src/bin/aleph-server/commands/start/mod.rs src/extension/mod.rs \
        src/gateway/execution_engine/run_loop/inner.rs src/gateway/handlers/mod.rs
git commit -m "projects: promote the catalogue to a SQLite entity with an owner and a roster"
```

---

### Task 2: 可见性谓词长出 project 分支

**Files:**
- Modify: `src/gateway/visibility.rs`
- Modify: `src/gateway/session_store/types.rs`（`SessionFilter`）
- Modify: `src/gateway/session_store/file_backend/mod.rs`、`src/gateway/session_store/sqlite_backend/*`（列表过滤）

**Interfaces (consumed):** Task 1 的 `projects::roster::{is_member, projects_of}`。
**Interfaces (produced):**
```rust
pub fn project_visible(project_id: &str) -> bool;          // 网关侧（CALLER_USER）
pub fn ambient_project_visible(project_id: &str) -> bool;  // run 内也可达时用（scope::ambient_owner）
// SessionFilter 新字段：
pub visible_scope_ids: Option<Vec<String>>,   // None = 不限制；Some(vec) = 额外准入这些 scope_id
```

**这个任务最容易漏的一件事:** `sessions.list` 的过滤是**下推到 SQL** 的 `owner_visible_to`。项目会话的 `owner_user_id` 是**创建者**，所以 bob 用现有过滤器**看不到 alice 建的项目会话**——而他本该看到。所以 `SessionFilter` 必须变成 `owner = me OR scope_id IN (我的项目...)`，两个后端都要改。只改谓词不改 filter，症状是「群聊里能发言但列表里没有这个会话」，且没有任何报错。

- [ ] **Step 1: 写失败测试**

`src/gateway/visibility.rs` 的 tests：

```rust
#[tokio::test]
async fn a_project_session_is_visible_to_every_member_not_just_its_creator() {
    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    let p = store.create("room", Some("u-alice"), None).unwrap();
    store.add_member(&p.id, "u-bob").unwrap();

    let meta = SessionMetadata {
        owner_user_id: Some("u-alice".to_string()),
        scope_id: Some(format!("project:{}", p.id)),
        ..Default::default()
    };

    assert!(
        CALLER_USER.scope(Some("u-bob".to_string()), async { session_visible(&meta) }).await,
        "membership, not ownership, is the predicate for a project session"
    );
    assert!(
        !CALLER_USER.scope(Some("u-carol".to_string()), async { session_visible(&meta) }).await,
        "a non-member sees nothing"
    );
    assert!(session_visible(&meta), "unrestricted internal callers are unchanged");
}

#[tokio::test]
async fn removing_a_member_revokes_visibility_immediately() {
    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    let p = store.create("room", Some("u-alice"), None).unwrap();
    store.add_member(&p.id, "u-bob").unwrap();
    let meta = SessionMetadata {
        owner_user_id: Some("u-alice".to_string()),
        scope_id: Some(format!("project:{}", p.id)),
        ..Default::default()
    };
    assert!(CALLER_USER.scope(Some("u-bob".to_string()), async { session_visible(&meta) }).await);
    store.remove_member(&p.id, "u-bob").unwrap();
    assert!(
        !CALLER_USER.scope(Some("u-bob".to_string()), async { session_visible(&meta) }).await,
        "spec §10: 移出项目成员立即失去可见性"
    );
}

#[tokio::test]
async fn a_project_partition_follows_the_roster() {
    let store = ProjectStore::new(Connection::open_in_memory().unwrap());
    store.migrate().unwrap();
    let p = store.create("room", Some("u-alice"), None).unwrap();
    let partition = format!("main__{}", p.id);          // p.id already carries the `p-` prefix

    assert!(CALLER_USER.scope(Some("u-alice".to_string()), async { partition_visible(&partition) }).await);
    assert!(!CALLER_USER.scope(Some("u-bob".to_string()), async { partition_visible(&partition) }).await);
    // The legacy project-directory family must not be affected by the new arm.
    assert!(CALLER_USER.scope(Some("u-bob".to_string()), async { partition_visible("main__proj-deadbeef") }).await);
}
```

`src/gateway/session_store/` 的后端测试（两个后端各一条）：

```rust
#[tokio::test]
async fn list_shows_a_member_a_project_session_they_did_not_create() {
    // alice creates a project-scoped session; bob (a member) lists.
    // Expect: bob sees exactly that session and none of alice's personal ones.
}
```

- [ ] **Step 2: 跑测试确认失败**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::visibility
```
Expected: FAIL —— `session_visible` 现在只比 owner，bob 得到 false。

- [ ] **Step 3: 实现谓词**

```rust
/// Whether the current gateway caller may see records scoped to `project_id`.
///
/// Membership IS the authorization (spec §6.1 — v1 has no per-resource grants).
/// The unrestricted arm comes first, exactly as in every other predicate here,
/// so cron / A2A / in-process tests are unchanged.
#[must_use]
pub fn project_visible(project_id: &str) -> bool {
    match visible_owner_filter() {
        None => true,
        Some(caller) => crate::projects::roster::is_member(project_id, &caller),
    }
}

/// [`project_visible`]'s sibling for records reachable from BOTH the gateway
/// and an agent run's tools — same resolver split as
/// [`stamped_owner_visible`] vs [`ambient_owner_visible`].
#[must_use]
pub fn ambient_project_visible(project_id: &str) -> bool {
    match crate::scope::ambient_owner() {
        None => true,
        Some(actor) => crate::projects::roster::is_member(project_id, &actor),
    }
}
```

`session_visible` 改成：
```rust
#[must_use]
pub fn session_visible(meta: &SessionMetadata) -> bool {
    // A project-scoped session is a shared room: its `owner_user_id` records
    // WHO CREATED IT, which is not the visibility question. Ask the roster.
    if let Some(crate::scope::ScopeId::Project(p)) =
        meta.scope_id.as_deref().and_then(crate::scope::ScopeId::parse)
    {
        return project_visible(&p);
    }
    stamped_owner_visible(meta.owner_user_id.as_deref())
}
```

`partition_visible` 在 `proj-` 臂之后、caller 比较之前插入：
```rust
    // `p-*` (project scope): the roster decides. Checked AFTER `proj-` so the
    // legacy directory family keeps its org-tier ruling — note `"proj-"` does
    // not start with `"p-"`, so the two families cannot collide.
    if suffix.starts_with("p-") {
        return project_visible(suffix);
    }
```
并把模块 doc 里「a `p-*` partition is invisible to every member until P2 adds the membership check」那句改写成现状。

- [ ] **Step 4: 实现 `SessionFilter::visible_scope_ids` + 两个后端**

- `types.rs`：加字段并写清楚语义——「`owner_visible_to` 与 `visible_scope_ids` 是 **OR**，不是 AND」。
- SQLite backend：`WHERE (owner_user_id = ?1 OR COALESCE(owner_user_id,'u-owner') = ?1 OR scope_id IN (...))`，注意收养语义那一半必须保留。
- file backend：同样的布尔式，用 `visibility::session_visible` 逐行判断即可（file backend 本来就是内存过滤）。
- 设置点：`visibility` 增加
  ```rust
  /// The scope ids a `sessions.list`-shaped query must ALSO admit beyond the
  /// caller's own rows: every project they are on the roster of. `None` for an
  /// unrestricted caller (they already see everything).
  #[must_use]
  pub fn visible_scope_filter() -> Option<Vec<String>> {
      let caller = visible_owner_filter()?;
      Some(
          crate::projects::roster::projects_of(&caller)
              .into_iter()
              .map(|p| crate::scope::ScopeId::Project(p).render())
              .collect(),
      )
  }
  ```
  **每个已经在设 `owner_visible_to` 的调用点必须同批设 `visible_scope_ids`**——只设一个的症状是「群聊存在但列表里没有」。grep `owner_visible_to` 找齐所有调用点。

- [ ] **Step 5: 跑测试确认通过**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::visibility
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib session_store
```

- [ ] **Step 6: Commit**

```bash
git commit -am "gateway: membership decides visibility for project-scoped sessions and partitions"
```

---

### Task 3: 名册 RPC + 既有 `projects.*` 加闸

**Files:**
- Modify: `src/gateway/handlers/projects.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/settings.rs:92-147`
- Modify: `src/gateway/method_visibility.rs`

**Interfaces (consumed):** Task 1 的 store、Task 2 的 `project_visible`。

**裁决形态（spec §6.3）与 NOT_FOUND 纪律的交界，定死：**

| 情况 | 响应 |
|---|---|
| 项目不存在 | `RESOURCE_NOT_FOUND` "project not found" |
| 项目存在但 caller 不是成员 | **同上，逐字节相同** |
| caller 是成员但不是 owner/admin，做 owner 级操作（加/移成员、改名、归档、删除、绑定工作区） | `FORBIDDEN` "not the project owner" |
| 加的成员 user_id 不在 `users` 表 | `INVALID_PARAMS` "unknown user" |

> `forbidden` 只泄露「这个项目存在」——而 caller 已经是成员，本来就知道。这不是预言机。

**新方法（6 个）:** `projects.create`、`projects.rename`、`projects.archive`、`projects.member.add`、`projects.member.remove`、`projects.member.list`。
**既有 6 个加闸:** `list`（过滤到我的项目）、`get`/`remove`/`touch`（gate）、`add`/`create_blank`（创建，stamp owner + 自己入册，不 gate）。

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn a_foreign_project_reads_exactly_like_a_missing_one() {
    let (store, p) = alice_project().await;                  // owner = u-alice
    let missing = request("projects.get", json!({ "id": "p-does-not-exist" }));
    let foreign = request("projects.get", json!({ "id": p.id }));

    let as_bob = |req| CALLER_USER.scope(Some("u-bob".to_string()), handle_get(req, store.clone()));
    let a = serde_json::to_string(&as_bob(missing).await).unwrap();
    let b = serde_json::to_string(&as_bob(foreign).await).unwrap();
    assert_eq!(a.replace("\"id\":1", ""), b.replace("\"id\":1", ""),
        "no existence oracle: the two must be byte-identical modulo the JSON-RPC id");
}

#[tokio::test]
async fn list_shows_only_the_projects_i_am_on() { /* alice sees hers, bob sees empty */ }

#[tokio::test]
async fn a_member_cannot_add_another_member() {
    // bob is a member, not the owner → FORBIDDEN, and the roster is unchanged.
}

#[tokio::test]
async fn adding_an_unknown_user_is_rejected_and_changes_nothing() {
    // spec §6.3 `invalid_member`
}
```

- [ ] **Step 2: 跑测试确认失败**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib handlers::projects
```

- [ ] **Step 3: 实现 handler 闸**

在 `src/gateway/handlers/projects.rs` 顶部加一个本文件唯一的闸（照 `handlers/teams/visibility.rs` 的形状）：

```rust
/// The single admission point for every addressed `projects.*` handler.
///
/// Fail-closed on a store error (Global Constraint 3) and byte-identical to a
/// genuinely missing id on a visibility failure (Global Constraint 4).
fn gate_project(
    store: &ProjectStore,
    id: &str,
) -> Result<Project, JsonRpcResponse> {
    let not_found = || JsonRpcResponse::error(None, RESOURCE_NOT_FOUND, format!("project not found: {id}"));
    match store.get(id) {
        Ok(Some(p)) if crate::gateway::visibility::project_visible(&p.id) => Ok(p),
        Ok(_) => Err(not_found()),
        Err(_) => Err(not_found()),
    }
}

/// Owner-level operations. Assumes `gate_project` already ran — this only
/// separates "member" from "owner", it is not a visibility check.
fn require_owner(p: &Project) -> Result<(), JsonRpcResponse> { /* owner or admin role */ }
```

`ProjectView` 增加 `owner_user_id` / `status` / `member_ids` / `workspace_path`（Panel 要用）。

- [ ] **Step 4: 注册 6 个新方法 + 帮助文本**

`settings.rs` 的 `register_*` 表 + 那段 `println!` 帮助文本**同批更新**（同一事实的两份表述，只改一份就是静默说谎）。

- [ ] **Step 5: 登记 `method_visibility.rs`**

```rust
    // --- projects.* (P2 project rooms) ---
    ("projects.list", Treatment::ListFiltered),
    ("projects.get", Treatment::KeyChecked),
    ("projects.remove", Treatment::KeyChecked),
    ("projects.touch", Treatment::KeyChecked),
    ("projects.rename", Treatment::KeyChecked),
    ("projects.archive", Treatment::KeyChecked),
    ("projects.member.add", Treatment::KeyChecked),
    ("projects.member.remove", Treatment::KeyChecked),
    ("projects.member.list", Treatment::KeyChecked),
```
模块 doc 增加一节 `## projects.*`，写清楚：**`projects.create` / `projects.add` / `projects.create_blank` 刻意不登记**（创建型，没有可寻址的既有记录，新行被 stamp），并把这三条加进那条「刻意缺席」的 pin 测试——否则「未登记」和「有人忘了」在 `treatment_of` 里长得一样。

- [ ] **Step 6: 跑测试 + Commit**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib handlers::projects
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib method_visibility
git commit -am "gateway: roster RPCs for project rooms, gated behind membership"
```

---

### Task 4: 项目作用域会话（`project_id` → `ScopeId::Project`）

**Files:**
- Modify: `src/gateway/handlers/agent.rs`（`chat.send` / `agent.run` 参数 + 归属分支，锚点 `:488-497`）
- Modify: `src/gateway/execution_engine/run_loop/mod.rs`（已在消费 `scope_from_metadata`，确认 `Project` 能透传）
- Modify: `src/gateway/busy_queue/`（spec §10 的 Queue 规则）

**Interfaces (consumed):** Task 2 的 `project_visible`；`scope::{ScopeId, ScopeAttribution}`；`SessionMetadata::{scope_id, owner_user_id}`。

**归属解析的两条路，别只写一条:**

```rust
// A session that ALREADY EXISTS owns its scope (spec §10: scope is immutable).
// The request's `project_id` only has a say at creation. Getting this backwards
// means a run's memory writes land in a different partition than the session's
// stored scope — silently, with no error, and only visible as "the agent forgot".
let attribution = match sessions.get_metadata(&key).await {
    Ok(Some(meta)) => crate::scope::ScopeAttribution::from_persisted(
        meta.owner_user_id.as_deref(),
        meta.scope_id.as_deref(),
    )
    // A legacy/unstamped existing session keeps legacy semantics.
    .or_else(|| user.as_deref().map(crate::scope::ScopeAttribution::personal)),

    Ok(None) => match params.project_id.as_deref() {
        Some(pid) if crate::gateway::visibility::project_visible(pid) => {
            user.as_deref().map(|u| crate::scope::ScopeAttribution {
                owner_user_id: u.to_string(),
                scope: crate::scope::ScopeId::Project(pid.to_string()),
            })
        }
        // Absent OR foreign project: the same refusal, no oracle.
        Some(_) => return project_not_found_response(request.id),
        None => user.as_deref().map(crate::scope::ScopeAttribution::personal),
    },

    // Global Constraint 3.
    Err(_) => return project_not_found_response(request.id),
};
```

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn a_project_session_is_stamped_with_the_project_scope_not_the_creator() {
    // alice sends with project_id=p-x → the created session's scope_id is
    // "project:p-x" and its owner_user_id is "u-alice".
}

#[tokio::test]
async fn a_non_member_cannot_open_a_session_in_a_project() {
    // bob sends with project_id=p-x → RESOURCE_NOT_FOUND, byte-identical to
    // sending with a project id that was never created, AND no session row
    // exists afterwards.
}

#[tokio::test]
async fn an_existing_sessions_scope_wins_over_the_request() {
    // alice creates a personal session S, then re-sends to S with
    // project_id=p-x → the run's attribution stays Personal(u-alice) and S's
    // scope_id on disk is unchanged (spec §10).
}

#[tokio::test]
async fn a_second_member_may_speak_in_the_room() {
    // bob (member) sends to alice's project session → accepted, and the run's
    // attribution is Project(p-x) with owner_user_id = "u-bob".
}
```

- [ ] **Step 2: 跑测试确认失败** — `project_id` 参数不存在，编译失败。

- [ ] **Step 3: 实现参数 + 归属分支 + 拒绝响应**

`ChatSendParams` / `AgentRunParams` 加 `#[serde(default)] pub project_id: Option<String>`。拒绝响应复用 `visibility::not_found_response` 的形状但消息是 `"project not found"`——**并且在同一个 `fn` 里生成，两条拒绝路径共用它**。

- [ ] **Step 4: busy lane（spec §10）**

项目会话里，当会话已有 run 在跑且**新消息的作者 ≠ 该 run 的 owner** 时，忙碌输入强制降为 `Queue`（无论会话旋钮是 `Steer` 还是 `Interrupt`）。理由：steer/interrupt 是对**自己那一轮**的控制权，不是对别人那一轮的。

> ⚠️ 车道相关的仓内地雷（FEATURE_LOCATOR §4.8）：取槽成功时必须 `busy_queue::mark_admitted`，否则 `Steer`/`Interrupt` 会**静默退化成 `Queue`**。这次改的是"何时**故意**降级为 Queue"，别把这两件事搅在一起——加一条断言「同一个人连发两条仍然是 Steer」把它们分开钉住。

- [ ] **Step 5: 跑测试**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib handlers::agent
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib busy_queue
```

- [ ] **Step 6: Commit**

```bash
git commit -am "gateway: sessions can be opened in a project scope, immutably"
```

---

### Task 5: 项目记忆分区

**Files:**
- Modify: `src/memory/project_scope.rs`（`session_write_id` / `session_read_ids`）
- Modify: `src/thinker/memory_context_provider/*`（USER.md floor 判据）

**Interfaces (consumed):** `scope::ScopeId::Project`（Task 4 现在会产生它）。

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn a_project_session_writes_to_the_project_partition() {
    let attr = ScopeAttribution { owner_user_id: "u-alice".into(), scope: ScopeId::Project("p-x7f2".into()) };
    let (write, read) = with_scope(Some(attr), async {
        (session_write_id("main", false, None), session_read_ids("main", false, None))
    }).await;
    assert_eq!(write, "main__p-x7f2");
    assert_eq!(read, vec!["main".to_string(), "main__p-x7f2".to_string()]);
    assert!(
        !read.iter().any(|r| r.contains("__u-")),
        "spec §5.2/§13: a project room must NOT recall anyone's personal memory"
    );
}

#[tokio::test]
async fn the_user_profile_floor_is_personal_only() {
    // In a Project scope, the USER.md floor must not be injected at all —
    // "whose USER.md" has no good answer in a multi-person room, and injecting
    // the creator's is a leak.
}
```

- [ ] **Step 2: 跑测试确认失败**（`Project` 目前落到 `_` 臂，写成 `"main"`）

- [ ] **Step 3: 实现两个臂**

```rust
// in session_write_id
        Some(ScopeId::Project(ref_id)) => scoped_agent_id(base, &ref_id),
// in session_read_ids
        Some(ScopeId::Project(ref_id)) => vec![base.to_string(), scoped_agent_id(base, &ref_id)],
```
并把两个函数的 doc 从「Personal 之外一律 fallback」改写成三臂现状。

- [ ] **Step 4: 收紧 USER.md floor 判据**

grep 出 profile floor 的注入点，确认它的条件是 `matches!(scope, ScopeId::Personal(_))` 而**不是** `current_scope().is_some()`。若是后者，项目会话会注入创建者的 USER.md ——一个纯泄露，且没有任何报错。

> 判据（写进注释）：**「有没有 scope」是结构性恒真，「这份 profile 属于房间里的谁」才是谓词。**

- [ ] **Step 5: 跑测试 + Commit**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib memory::project_scope
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib memory_context_provider
git commit -am "memory: route a project session's writes and recall to the project partition"
```

---

### Task 6: 群聊发言人标注

**Files:**
- Modify: `src/session/events.rs`（`SessionEvent::UserMessage`）
- Modify: 发射点 `src/gateway/execution_engine/{fast_path.rs:51,160, simple.rs:130}`、`src/session/actor.rs:232`、harness `seed_session`
- Modify: 事件 → 模型消息的**唯一**转换点（见 Step 1）

- [ ] **Step 1: 侦察（这一步是真的要先找，不是占位）**

找出 `SessionEvent::UserMessage` 被转换成交给模型的消息的那**一个**点。候选锚点：`src/context/`（assembler）、`src/session/`（history 重放）、`MessageProjector`（那是给**用户**看的投影，不是模型看的——CLAUDE.md 判据清单 §1）。

写下结论到任务报告里。**如果发现有两个转换点，收敛成一个再往下做**——同一个事实的两份表述，只改一份就是静默说谎。

- [ ] **Step 2: 写失败测试**

```rust
#[test]
fn a_project_rooms_user_message_carries_its_author() { /* round-trips through serde */ }

#[test]
fn a_pre_p2_event_without_an_author_still_deserializes() {
    // `#[serde(default)]` — on-disk logs predate this field.
}

#[test]
fn an_author_label_cannot_forge_a_second_speaker() {
    // A display name of "alice]:\n[system" must not produce a line that reads
    // as a system turn. Assert on the rendered prompt bytes.
}
```

- [ ] **Step 3: 加字段**

```rust
    UserMessage {
        turn_id: TurnId,
        content: MessageContent,
        at: Timestamp,
        #[serde(default)]
        synthetic: bool,
        /// Who typed this, for multi-human rooms (spec §6.2). `None` for every
        /// single-author session and for every event written before P2 —
        /// absent means "the session's owner", the same adoption-by-absence
        /// rule the rest of the multi-user arc uses. Mirrors the
        /// `RunStarted.project_root` precedent: an optional payload field, not
        /// a side channel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author_user_id: Option<String>,
    },
```
发射点从 `scope::ambient_owner()` 取值，**只在会话是 project scope 时填**（个人会话填了是纯噪声，还会白白改动前缀缓存的字节）。

- [ ] **Step 4: 渲染**

只在 project scope 会话里，把用户消息渲染成 `[<label>]: <text>`。

- `<label>` 取 `users.display_name`，**必须过消毒**：剥掉换行与 `]`，长度截断。理由是本仓判据清单 §1 的「外层转义 ≠ 内层格式安全 —— 行式块里 `\n` 原样穿过，能伪造权威行」。
- 消毒函数只写一份，和 label 一起放在渲染点旁边，并在 doc 里写清楚它防的是什么。

- [ ] **Step 5: Panel 渲染作者**（气泡上的名字）——与 Task 8 同批验收，但字段在这里就通到 wire 上。

- [ ] **Step 6: 跑测试 + Commit**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib session::events
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib context::
git commit -am "session: carry the author of a user message in multi-person rooms"
```

---

### Task 7: 工作区绑定

**Files:**
- Modify: `src/gateway/handlers/agent.rs`（`workspace_override` 解析，锚点 `:505-520`）
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:97`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn a_project_session_defaults_its_cwd_to_the_bound_workspace() { }

#[tokio::test]
async fn a_member_does_not_need_the_loopback_config_gate_to_use_the_bound_workspace() {
    // The binding was set by the owner through a gated RPC; a remote member
    // running in it is not "choosing an arbitrary project_root".
}

#[tokio::test]
async fn a_caller_supplied_project_root_still_goes_through_the_existing_gate() {
    // Regression: the new path must not become a way to bypass the config-tier
    // / loopback check for an arbitrary directory.
}
```

- [ ] **Step 2: 跑测试确认失败**
- [ ] **Step 3: 实现** —— 解析顺序：`params.project_root`（走既有闸）> 项目绑定的 `workspace_path`（不走该闸，理由如上，写进注释）> agent 默认工作区。
- [ ] **Step 4: 跑测试 + Commit**

```bash
git commit -am "gateway: a project session runs in its bound workspace"
```

---

### Task 8: Panel 项目 UI

**Files:**
- Create: `interfaces/webchat/src/components/sidebar/projects.rs`、`interfaces/webchat/src/components/project_page.rs`
- Modify: `interfaces/webchat/src/api/`（新 RPC 绑定）、`state/`（项目列表 + 当前项目）、侧栏与路由

**范围（spec §6.4，P2 只做三个）:** 项目列表（我参与的）、群聊 tab（默认）、设置 tab（名册 / 工作区绑定 / 归档）。**看板 / 工作区浏览 / 记忆浏览三个 tab 是 P3，本任务不做，但 tab 骨架可以留位。**

- [ ] **Step 1: API 绑定 + 状态**

11 个 RPC（6 既有 + 6 新，`member.list` 可由 `get` 覆盖则省掉）绑进 `api/`。项目列表进 `state/`。

> ⚠️ 仓内地雷（FEATURE_LOCATOR §4.7）：**「按会话的状态」住进单例组件就会切页签串味。** 判据是「这份状态在用户切到另一个对话后还成立吗」——当前项目 id 属于**全局**（不随会话切换），项目群聊的草稿属于**会话**，必须进 `SessionSnapshot`。别放反了。

- [ ] **Step 2: 侧栏「项目」区块**

列出我参与的项目；点击进入项目页。**不要**和现有「最近工作目录」选择器混在一个列表里（spec §6.1「两者不混同」）。

- [ ] **Step 3: 项目页 — 群聊 tab**

复用现有 chat 组件；发消息时带上 `project_id`。气泡显示 `author_user_id` 对应的 display name（Task 6 的字段）。

- [ ] **Step 4: 项目页 — 设置 tab**

名册（列出成员、加、移；非 owner 只读）、工作区绑定（复用 `directory_browser`）、归档。

- [ ] **Step 5: 构建验证**

```
cargo check -p aleph-panel
just dev   # 或 just build，确认 WASM 真的重编了
```

> ⚠️ Panel UI 是**编译期嵌进二进制**的（`rust_embed`）。改完看不到效果 = 漏了重编 server。

- [ ] **Step 6: Commit**

```bash
git commit -am "panel: project rooms — list, group chat, settings"
```

---

### Task 9: 文档、记忆、全量验证

**Files:**
- Modify: `docs/reference/SECURITY.md`、`src/gateway/CLAUDE.md`、`CLAUDE.md`（判据清单）
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Create: `~/.claude/projects/D--Workspace-Aleph/memory/p2-project-rooms-executed.md` + MEMORY.md 指针

- [ ] **Step 1: SECURITY.md**

新增「Project rooms」小节：成员制即授权、无逐资源 Grant、`forbidden` vs `not_found` 的分界线、以及**诚实声明**——项目隔离与 P1 一样是隐私级不是安全级（spec §11 的三条硬边界对项目同样成立：同进程同 OS 账户、vault 组织级共享、org 记忆共享）。

- [ ] **Step 2: 判据清单新增条目**（`CLAUDE.md` §4 网关组 / §5 记忆组）

至少这三条（每条都是本轮真实踩到或差点踩到的）：
- **「谁拥有」回答不了「谁能看」** —— 共享房间的 `owner_user_id` 记的是**创建者**；可见性判据是名册。凡加了共享语义的表，`owner` 列的含义要重新问一遍。
- **谓词改了、下推的 SQL 过滤器没改，症状是「能进去但列表里没有」** —— `owner_visible_to` 与 `visible_scope_ids` 必须同批设置；grep 前者找齐所有调用点。
- **投影必须由真源在自己的写锁里发布** —— `projects::roster` 是 `project_members` 的投影；第二个写入者就是第二个真源。CLI 若要改名册必须走 IPC。

- [ ] **Step 3: 全量验证（四条，缺一条等于没验）**

```
CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib
cargo check -p aleph-panel
cargo check -p aleph-desktop-windows
cargo clippy --all-targets --workspace --exclude aleph-desktop-macos --exclude aleph-desktop-linux
```
基线：lib 14925 passed / 0 failed / 21 ignored（teams 收紧后）；clippy 37 warning 行、exit 0。**新增的 warning 一条都不接受。**

- [ ] **Step 4: rustfmt（按叶子文件）**

```
rustfmt --edition 2021 --check <本轮改过的每个 .rs>
```

- [ ] **Step 5: 端到端验收（人工，spec §8 P2 那一行）**

1. 起 server，Panel 以 owner 登录，建项目 `demo`，绑定一个工作区。
2. admin 发一次性配对票，第二台设备配对成 `u-bob`（member）。
3. owner 把 bob 加进 `demo`。
4. 两边都进 `demo` 群聊，各发一条消息 → 双方都看到对方的气泡且带名字；agent 的回复两边都收到。
5. bob 让 agent 记一件事 → alice 的会话里 agent 能想起来（项目记忆共享）。
6. alice 在**个人**会话里让 agent 记一件私事 → 在 `demo` 群聊里问，agent **想不起来**（个人记忆不泄露）。
7. owner 把 bob 移出 → bob 的项目立即从侧栏消失，直接调 `projects.get` 拿到 `not found`。

- [ ] **Step 6: Commit + 记忆**

```bash
git commit -am "docs: project rooms trust boundary and the three new landmines"
```

---

## Self-Review（写完计划后的自查结论）

**规格覆盖:** §6.1 实体 → T1；§6.2-1 共享会话 → T4+T6；§6.2-2 项目记忆 → T5；§6.2-3 工作区 → T7；§6.3 名册操作 → T3；§6.4 UI → T8；§5.2 分区/召回 → T5；§5.3 会话归属 → T4；§5.4 唯一强制点 → T2；§9 三类测试 → 每个任务的 Step 1 + T9 Step 5；§10 三条边界语义 → 会话 scope 不可变（T4 Step 3）/ 移出立即失效（T2 测试）/ 多作者 Queue（T4 Step 4）。

**已知不覆盖（明确留给 P3+，不是遗漏）:**
- **§6.2-4 看板 / goals / loops 进项目 scope** —— spec §8 就是 P3。本轮 `ScopeId::Project` 不进 `goal` / `looping` / `cron` 的归属列。
- **进展推送路由到项目成员** —— P3。
- **记忆浏览面 / 工作区浏览面两个 tab** —— P3。
- **移出成员后其发起的后台工作继续跑（§10 第二条）** —— 后台工作还没进项目 scope，这条在 P3 才有实体可验。
- **渠道入站消息进项目** —— spec §11-3 明确留 P4（渠道群绑定项目）。
- **`RECENT_PROJECTS_CAP` 的淘汰语义** —— 升格成实体后「自动淘汰最老的项目」不再可接受（会删掉别人在用的房间）。本轮**移除自动淘汰**，列表改为按 `last_used_at` 排序 + 前端分页；如果这与你的预期不符，在 Task 1 之前提出。

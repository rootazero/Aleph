# P0 Identity Foundation Implementation Plan
# P0「身份地基」实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 Aleph 加上 User 实体 + 设备/渠道身份链接 + CallerIdentity 用户管道 + 隐式 owner 零迁移 + admin/member RPC 角色闸——多用户多设备各自登录，member 调不到 admin RPC，单用户体验零变化。

**Architecture:** Spec = `docs/superpowers/specs/2026-08-04-multi-user-org-project-design.md`（方案 B）。P0 只做身份层：复活三条休眠接缝（`devices.role` 列语义、`CALLER_ROLE` task-local、bootstrap 票据流），不动记忆/会话作用域（那是 P1）。所有改动集中在 `src/gateway/`，零协议（shared/protocol）改动，零 Panel 改动。

**Tech Stack:** Rust (tokio + rusqlite)，存储沿用 `security.db`（schema v13 → v14）与 `pairing.db`。

## Global Constraints（全局约束）

- **单分支 main 直接开发**；提交格式 `<scope>: <description>`（英文），每个 Task 一次提交
- **最小可信验证集**（每个 Task 的收尾步骤都要过，仓库纪律，缺一不可）：
  `cargo test -p alephcore --lib --no-run`（编译含测试）→ 跑本 Task 新增测试 → Task 7 终局跑全量四件套（含 `cargo check -p aleph-panel` + `cargo clippy --all-targets`）
- **Windows 基线**：`cargo test -p alephcore --lib` 在本机有 **15 个既存失败**（见 memory `context-mode-approval-tier`）——新失败以此基线对比，不追既存
- **R4**：gateway handler 纯 I/O，副作用（踢 socket）归 `start/mod.rs` 接线处
- **R10**：不碰 `src/harness/`
- **gateway 红线**：改认证/授权逻辑必须同步更新测试（`src/gateway/CLAUDE.md`）
- **锁纪律**：`.lock().unwrap_or_else(|e| e.into_inner())`（store 全仓惯例）
- **不做**：密码、OIDC、guest 启用、Panel UI、会话/记忆隔离（P1）、渠道 role 语义变更（inbound_router 的 chat/config tier 不动）
- **命名常量**（跨任务共享，Task 1 定义）：`OWNER_USER_ID = "u-owner"`、role 字符串 `"admin"` / `"member"`、caller_role 字符串 `"operator"` / `"member"` / `"guest"`

## 关键现状锚点（写代码前先读这些行）

| 事实 | 位置 |
|---|---|
| `SCHEMA_VERSION: i32 = 13`，迁移模式 `if version < N { … set_schema_version(N) }` | `src/gateway/security/store/mod.rs:39` 及 `migrate()`（v13 块在 ~L242-249） |
| `devices` 表（`role TEXT DEFAULT 'operator'`, `scopes`, `device_type`；panel 行 `device_type='panel'`，cluster 节点行 `role='node'` 且 `device_type` NULL） | `store/mod.rs:299-310` `SCHEMA_V2` |
| `bootstrap_tickets(code PK, created_at, expires_at, consumed_at, consumed_by_device_id)` | `store/mod.rs:476-485` |
| `upsert_device` ON CONFLICT 有意不改写 `device_type`、必须清 `revoked_at`（地雷 4） | `store/devices.rs:16` 起 |
| `exchange_bootstrap_ticket(ticket, device_id, device_name, _public_key)`，非 panel 行冲突在**消费票之前**拒（地雷 3） | `security/device_token_manager.rs:113-127` |
| `create_bootstrap_ticket(&self, ttl_ms)` → store `create_bootstrap_ticket(code, ttl)` | `device_token_manager.rs:87-94`、`store/bootstrap_tickets.rs:47-55` |
| `CALLER_ROLE` / `CALLER_IS_LOOPBACK` task-locals + `current_caller_role()` | `src/gateway/caller_identity.rs` |
| dispatch 读 `caller_role`（连接表 → 网络位置 fallback：loopback=operator / remote=guest） | `src/gateway/server/handler.rs:665-676` |
| 两个 task-local scoping 站点：`do_lane_dispatch` 闭包 + idempotency `Proceed` 臂 | `handler.rs:788-801`、`handler.rs:841-843` |
| connect 授权后盖章 `state.caller_role = panel_role`（同锁内写 device 绑定） | `handler.rs:1012` 附近 |
| `ConnectionState`（`caller_role: String` 等字段） | `src/gateway/server/mod.rs:39-64` |
| 工具闸谓词：`TurnContext::caller_is_operator()` → `role_is_operator(as_deref)`；「非 operator 一律 gated」 | `src/tools/turn_context.rs:72-78`、`src/tools/scoped/dispatch.rs:330-337` |
| RPC 注册模式 | `src/gateway/handlers/mod.rs:499-522`（projects 块）；handler 形状 `src/gateway/handlers/projects.rs:47-55` |
| `pairing_requests` / `approved_senders(channel, sender_id, approved_at)` | `src/gateway/pairing_store.rs:130-151` |

## 文件结构总览

```
src/gateway/security/store/users.rs        [新] users 表 CRUD + owner bootstrap
src/gateway/security/store/mod.rs          [改] v14 迁移 + mod users
src/gateway/security/store/devices.rs      [改] DeviceUpsertData.user_id + COALESCE
src/gateway/security/store/bootstrap_tickets.rs [改] 票据携带 user_id
src/gateway/security/device_token_manager.rs    [改] create/exchange 穿 user_id
src/gateway/caller_identity.rs             [改] CALLER_USER task-local
src/gateway/server/mod.rs                  [改] ConnectionState.caller_user
src/gateway/server/handler.rs              [改] 读取/scoping caller_user + 连接盖章
src/gateway/handlers/connect.rs            [改] 授权结果 → (user, role) 解析
src/gateway/method_admin.rs                [新] admin 方法分类器
src/gateway/dispatcher.rs 或 process_request 所在文件 [改] admin 闸（单强制点）
src/gateway/handlers/users.rs              [新] users.* RPC
src/gateway/handlers/mod.rs                [改] 注册 users.*
src/gateway/pairing_store.rs               [改] approved_senders.user_id
```

---

### Task 1: users 表 + schema v14 + owner bootstrap（隐式收养）

**Files:**
- Create: `src/gateway/security/store/users.rs`
- Modify: `src/gateway/security/store/mod.rs`（SCHEMA_VERSION、迁移块、`mod users; pub use`）
- Test: `users.rs` 内 `#[cfg(test)]`（store 测试惯例见 `store/tests.rs` 用 `SecurityStore::in_memory()`）

**Interfaces:**
- Consumes: `SecurityStore { conn: Mutex<Connection> }`、`current_timestamp_ms()`
- Produces（后续任务依赖，签名固定）:
  - `pub const OWNER_USER_ID: &str = "u-owner";`
  - `pub struct UserRecord { pub user_id: String, pub display_name: String, pub role: UserRole, pub status: UserStatus, pub created_at: i64 }`
  - `pub enum UserRole { Admin, Member }`（`as_str() -> "admin"|"member"`、`from_str`）
  - `pub enum UserStatus { Active, Deactivated }`（`as_str() -> "active"|"deactivated"`）
  - `SecurityStore::{create_user, get_user, list_users, update_user, count_users, ensure_bootstrap_owner, list_device_ids_for_user}`

- [ ] **Step 1: 写失败测试**（`store/users.rs` 尾部）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::SecurityStore;

    #[test]
    fn fresh_store_bootstraps_owner_admin() {
        let store = SecurityStore::in_memory().unwrap();
        // migrate() runs in in_memory(); ensure_bootstrap_owner is called at its end.
        let owner = store.get_user(OWNER_USER_ID).unwrap().expect("owner exists");
        assert_eq!(owner.role, UserRole::Admin);
        assert_eq!(owner.status, UserStatus::Active);
        assert_eq!(store.count_users().unwrap(), 1);
    }

    #[test]
    fn bootstrap_adopts_panel_devices_but_not_nodes() {
        let store = SecurityStore::in_memory().unwrap();
        // Panel device with no user (simulates pre-v14 row).
        store.upsert_device(&device_fixture("dev-panel", Some("panel"), "operator")).unwrap();
        store.clear_device_user("dev-panel").unwrap(); // test helper, see Step 3
        // Cluster node row: role='node', device_type NULL — must stay untouched.
        store.upsert_device(&device_fixture("node-1", None, "node")).unwrap();
        store.clear_device_user("node-1").unwrap();

        store.ensure_bootstrap_owner().unwrap();

        assert_eq!(store.device_user("dev-panel").unwrap().as_deref(), Some(OWNER_USER_ID));
        assert_eq!(store.device_user("node-1").unwrap(), None);
    }

    #[test]
    fn ensure_bootstrap_owner_is_idempotent_and_respects_existing_users() {
        let store = SecurityStore::in_memory().unwrap();
        store.ensure_bootstrap_owner().unwrap();
        store.ensure_bootstrap_owner().unwrap();
        assert_eq!(store.count_users().unwrap(), 1);
        // Once a second user exists, re-running must not create anything.
        store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        store.ensure_bootstrap_owner().unwrap();
        assert_eq!(store.count_users().unwrap(), 2);
    }

    #[test]
    fn update_user_changes_role_and_status() {
        let store = SecurityStore::in_memory().unwrap();
        store.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        store.update_user("u-bob", None, Some(UserRole::Admin), Some(UserStatus::Deactivated)).unwrap();
        let bob = store.get_user("u-bob").unwrap().unwrap();
        assert_eq!(bob.role, UserRole::Admin);
        assert_eq!(bob.status, UserStatus::Deactivated);
    }
}
```

`device_fixture` 按 `store/tests.rs::test_device_crud` 里 `DeviceUpsertData` 的真实字段抄一个最小构造 helper（`device_type` 与 `role` 参数化）。`clear_device_user` / `device_user` 是本任务新增的小读写口（见 Step 3）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib security::store::users -- --nocapture`
Expected: 编译错误（`users` 模块不存在）——即 RED。

- [ ] **Step 3: 实现**

`store/mod.rs`：

```rust
const SCHEMA_VERSION: i32 = 14;
```

迁移块（紧跟 v13 块之后，同一模式）：

```rust
if version < 14 {
    info!("Migrating security store to v14 (users + identity linking)");
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    conn.execute_batch(users::USERS_SCHEMA)?;
    // ALTER TABLE ADD COLUMN is not idempotent in SQLite — the version gate
    // guarantees single execution.
    conn.execute("ALTER TABLE devices ADD COLUMN user_id TEXT", [])?;
    conn.execute("ALTER TABLE bootstrap_tickets ADD COLUMN user_id TEXT", [])?;
    drop(conn);
    self.set_schema_version(14)?;
}
// After all versioned migrations (runs on every open, idempotent):
self.ensure_bootstrap_owner()?;
```

注意：`ensure_bootstrap_owner()` 调用放在 `migrate()` 里「Final safety: ensure version is at latest」那行**之前**、所有版本块之后——它必须每次 open 都跑（新库直接建到 v14 也要产 owner），且幂等。

`store/users.rs`：

```rust
//! Users table — the principal registry for the one-server-one-org model.
//! See docs/superpowers/specs/2026-08-04-multi-user-org-project-design.md §4.

use rusqlite::{params, OptionalExtension, Result as SqliteResult};

use super::{current_timestamp_ms, SecurityStore};

/// The implicit owner minted on first boot; adopts all pre-existing
/// single-user data so the single-machine experience is byte-identical.
pub const OWNER_USER_ID: &str = "u-owner";

pub(crate) const USERS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    user_id      TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    role         TEXT NOT NULL DEFAULT 'member',
    status       TEXT NOT NULL DEFAULT 'active',
    created_at   INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRole { Admin, Member }

impl UserRole {
    #[must_use] pub const fn as_str(self) -> &'static str {
        match self { Self::Admin => "admin", Self::Member => "member" }
    }
    #[must_use] pub fn from_str(s: &str) -> Option<Self> {
        match s { "admin" => Some(Self::Admin), "member" => Some(Self::Member), _ => None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus { Active, Deactivated }

impl UserStatus {
    #[must_use] pub const fn as_str(self) -> &'static str {
        match self { Self::Active => "active", Self::Deactivated => "deactivated" }
    }
    #[must_use] pub fn from_str(s: &str) -> Option<Self> {
        match s { "active" => Some(Self::Active), "deactivated" => Some(Self::Deactivated), _ => None }
    }
}

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub user_id: String,
    pub display_name: String,
    pub role: UserRole,
    pub status: UserStatus,
    pub created_at: i64,
}

impl SecurityStore {
    pub fn create_user(&self, user_id: &str, display_name: &str, role: UserRole) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO users (user_id, display_name, role, status, created_at)
             VALUES (?1, ?2, ?3, 'active', ?4)",
            params![user_id, display_name, role.as_str(), current_timestamp_ms()],
        )?;
        Ok(())
    }

    pub fn get_user(&self, user_id: &str) -> SqliteResult<Option<UserRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT user_id, display_name, role, status, created_at FROM users WHERE user_id = ?1",
            params![user_id],
            row_to_user,
        )
        .optional()
    }

    pub fn list_users(&self) -> SqliteResult<Vec<UserRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT user_id, display_name, role, status, created_at FROM users ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_user)?;
        rows.collect()
    }

    pub fn count_users(&self) -> SqliteResult<i64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
    }

    /// Partial update; `None` fields are left unchanged.
    pub fn update_user(
        &self,
        user_id: &str,
        display_name: Option<&str>,
        role: Option<UserRole>,
        status: Option<UserStatus>,
    ) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE users SET
               display_name = COALESCE(?2, display_name),
               role         = COALESCE(?3, role),
               status       = COALESCE(?4, status)
             WHERE user_id = ?1",
            params![user_id, display_name, role.map(UserRole::as_str), status.map(UserStatus::as_str)],
        )
    }

    /// Idempotent first-boot bootstrap: if no users exist, mint the implicit
    /// owner (admin) and adopt every un-owned panel device. Cluster node rows
    /// (shared `devices` table, mine #3 in gateway/CLAUDE.md) are machines,
    /// not people — never adopted.
    pub fn ensure_bootstrap_owner(&self) -> SqliteResult<()> {
        if self.count_users()? == 0 {
            self.create_user(OWNER_USER_ID, "Owner", UserRole::Admin)?;
        }
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "UPDATE devices SET user_id = ?1
             WHERE user_id IS NULL AND device_type = 'panel'",
            params![OWNER_USER_ID],
        )?;
        Ok(())
    }

    /// The linked user of a device row, if any.
    pub fn device_user(&self, device_id: &str) -> SqliteResult<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT user_id FROM devices WHERE device_id = ?1",
            params![device_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map(Option::flatten)
    }

    /// Live (un-revoked) device ids linked to a user — deactivation revokes these.
    pub fn list_device_ids_for_user(&self, user_id: &str) -> SqliteResult<Vec<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id FROM devices WHERE user_id = ?1 AND revoked_at IS NULL",
        )?;
        let rows = stmt.query_map(params![user_id], |r| r.get(0))?;
        rows.collect()
    }

    #[cfg(test)]
    pub fn clear_device_user(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("UPDATE devices SET user_id = NULL WHERE device_id = ?1", params![device_id])?;
        Ok(())
    }
}

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    let role_s: String = row.get(2)?;
    let status_s: String = row.get(3)?;
    Ok(UserRecord {
        user_id: row.get(0)?,
        display_name: row.get(1)?,
        // Fail-soft to Member on unknown text: a downgraded binary must never
        // promote an unknown role to admin (fail-closed on privilege).
        role: UserRole::from_str(&role_s).unwrap_or(UserRole::Member),
        status: UserStatus::from_str(&status_s).unwrap_or(UserStatus::Deactivated),
        created_at: row.get(4)?,
    })
}
```

`store/mod.rs` 加 `mod users;` + `pub use users::{UserRecord, UserRole, UserStatus, OWNER_USER_ID};`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib security::store::users`
Expected: 4 个测试 PASS；`store::tests::test_schema_migration` 仍 PASS（它断言 `SCHEMA_VERSION`，改常量后自动跟随）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/security/store/
git commit -m "gateway: add users table (schema v14) with implicit owner bootstrap"
```

---

### Task 2: CALLER_USER 管道——connect 解析 (user, role) + dispatch scoping

**Files:**
- Modify: `src/gateway/caller_identity.rs`
- Modify: `src/gateway/server/mod.rs`（`ConnectionState` +1 字段）
- Modify: `src/gateway/handlers/connect.rs`（授权后解析 user/role 的纯函数）
- Modify: `src/gateway/server/handler.rs`（3 处：读取 L665-676、`do_lane_dispatch` L788-801、`Proceed` 臂 L841-843；connect 盖章 ~L1012）
- Test: `caller_identity.rs` / `connect.rs` 内嵌测试

**Interfaces:**
- Consumes: Task 1 的 `SecurityStore::{device_user, get_user}`、`OWNER_USER_ID`、`UserRole`、`UserStatus`
- Produces:
  - `caller_identity::CALLER_USER: Option<String>` + `pub fn current_caller_user() -> Option<String>`
  - `connect::resolve_connection_identity(is_loopback, device_id, store) -> (Option<String>, &'static str)`（返回 `(user_id, caller_role)`）
  - `ConnectionState.caller_user: Option<String>`

- [ ] **Step 1: 写失败测试**

`caller_identity.rs` tests 模块追加：

```rust
#[tokio::test]
async fn caller_user_scope_round_trips() {
    let seen = CALLER_USER
        .scope(Some("u-alice".to_string()), async { current_caller_user() })
        .await;
    assert_eq!(seen.as_deref(), Some("u-alice"));
    assert_eq!(current_caller_user(), None); // unset outside a scope
}
```

`handlers/connect.rs` tests 追加（用 `SecurityStore::in_memory()`；如该文件测试无 store 先例，构造方式抄 `store/tests.rs`）：

```rust
#[test]
fn loopback_resolves_to_owner_operator() {
    let store = seeded_store(); // in_memory + ensure_bootstrap_owner (ran in migrate)
    let (user, role) = resolve_connection_identity(true, None, &store);
    assert_eq!(user.as_deref(), Some(crate::gateway::security::store::OWNER_USER_ID));
    assert_eq!(role, "operator");
}

#[test]
fn device_of_member_user_resolves_member_role() {
    let store = seeded_store();
    store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
    upsert_panel_device(&store, "dev-a", "u-alice"); // fixture: upsert + set user_id
    let (user, role) = resolve_connection_identity(false, Some("dev-a"), &store);
    assert_eq!(user.as_deref(), Some("u-alice"));
    assert_eq!(role, "member");
}

#[test]
fn device_of_admin_user_resolves_operator_role() {
    let store = seeded_store();
    store.create_user("u-root", "Root", UserRole::Admin).unwrap();
    upsert_panel_device(&store, "dev-r", "u-root");
    let (user, role) = resolve_connection_identity(false, Some("dev-r"), &store);
    assert_eq!(user.as_deref(), Some("u-root"));
    assert_eq!(role, "operator");
}

#[test]
fn deactivated_user_resolves_guest() {
    let store = seeded_store();
    store.create_user("u-gone", "Gone", UserRole::Member).unwrap();
    store.update_user("u-gone", None, None, Some(UserStatus::Deactivated)).unwrap();
    upsert_panel_device(&store, "dev-g", "u-gone");
    let (user, role) = resolve_connection_identity(false, Some("dev-g"), &store);
    assert_eq!(user, None);
    assert_eq!(role, "guest"); // walled — deactivation takes effect at next connect
}

#[test]
fn unlinked_device_and_shared_token_fall_back_to_owner() {
    // Legacy shared-token / unlinked-device connections keep today's behavior:
    // full operator as the implicit owner (zero-change guarantee).
    let store = seeded_store();
    let (user, role) = resolve_connection_identity(false, None, &store);
    assert_eq!(user.as_deref(), Some(crate::gateway::security::store::OWNER_USER_ID));
    assert_eq!(role, "operator");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib "connect::" -- --nocapture` 与 `cargo test -p alephcore --lib caller_identity`
Expected: 编译错误（`CALLER_USER` / `resolve_connection_identity` 不存在）。

- [ ] **Step 3: 实现**

`caller_identity.rs`——task_local! 块内追加：

```rust
    /// The authenticated user behind the originating connection (`users.user_id`),
    /// resolved once at `connect` and scoped alongside [`CALLER_ROLE`] in the WS
    /// dispatch loop. `None` outside a scope (cron, internal) and for walled
    /// connections. Loopback resolves to the implicit owner.
    pub static CALLER_USER: Option<String>;
```

并加同款 accessor：

```rust
/// The authenticated user id for the current task, or `None` outside a scope.
#[must_use]
pub fn current_caller_user() -> Option<String> {
    CALLER_USER.try_with(|u| u.clone()).ok().flatten()
}
```

`handlers/connect.rs`——纯函数（放在 `resolve_connect_auth` 旁边）：

```rust
/// Resolve the (user, caller_role) pair for an authorized connection.
///
/// Rules (spec §4):
/// - loopback ⇒ implicit owner, operator (zero-config, unchanged)
/// - device token bound to a user ⇒ that user; role admin⇒"operator",
///   member⇒"member"; deactivated ⇒ walled ("guest", no user)
/// - authorized but unbound (legacy shared token, pre-v14 device) ⇒ implicit
///   owner, operator — the zero-change guarantee for existing deployments
#[must_use]
pub fn resolve_connection_identity(
    is_loopback: bool,
    device_id: Option<&str>,
    store: &crate::gateway::security::store::SecurityStore,
) -> (Option<String>, &'static str) {
    use crate::gateway::security::store::{UserRole, UserStatus, OWNER_USER_ID};

    if is_loopback {
        return (Some(OWNER_USER_ID.to_string()), "operator");
    }
    let linked_user = device_id
        .and_then(|d| store.device_user(d).ok().flatten())
        .and_then(|uid| store.get_user(&uid).ok().flatten());
    match linked_user {
        Some(u) if u.status == UserStatus::Deactivated => (None, "guest"),
        Some(u) => {
            let role = match u.role {
                UserRole::Admin => "operator",
                UserRole::Member => "member",
            };
            (Some(u.user_id), role)
        }
        // Authorized without a user binding: legacy paths keep full authority.
        None => (Some(OWNER_USER_ID.to_string()), "operator"),
    }
}
```

`server/mod.rs`——`ConnectionState` 加字段（跟在 `caller_role` 旁）：

```rust
    /// Authenticated user behind this connection (`users.user_id`), resolved at
    /// `connect` together with `caller_role`. `None` for walled connections.
    pub caller_user: Option<String>,
```

（该 struct 的所有构造点会编译报错——逐个补 `caller_user: None` 默认；connect 盖章处写真值。）

`server/handler.rs` 四处：

1. **connect 盖章**（~L1012，`state.caller_role = panel_role.to_string();` 同一锁块内）：调 `resolve_connection_identity(ctx.client_ip.is_loopback(), device_id_opt.as_deref(), &store)`，把返回写进 `state.caller_role` / `state.caller_user`。`store` 的获取方式与该函数当前拿 device manager 的方式一致（同一个 `ctx`/shared state 上已有 security store 句柄——connect 授权本身就在查它）。**注意**：现有代码此处 `panel_role` 恒 `"operator"`；替换为 resolve 的结果即可，loopback 语义不变。
2. **per-request 读取**（L665-676 的 `caller_role` 块旁）：

```rust
let caller_user: Option<String> = {
    let conns = ctx.connections.read().await;
    conns.get(&conn_id).and_then(|s| s.caller_user.clone())
}
.or_else(|| {
    ctx.client_ip
        .is_loopback()
        .then(|| crate::gateway::security::store::OWNER_USER_ID.to_string())
});
```

3. **`do_lane_dispatch` 闭包**（L788-801）：签名追加 `caller_user: Option<String>`，scoping 变三层嵌套：

```rust
Ok(_permit) => crate::gateway::caller_identity::CALLER_USER
    .scope(caller_user, crate::gateway::caller_identity::CALLER_ROLE
        .scope(caller_role, crate::gateway::caller_identity::CALLER_IS_LOOPBACK
            .scope(caller_is_loopback, process_request(&text, &mc))))
    .await,
```

4. **idempotency `Proceed` 臂**（L841-843）：同款三层嵌套（这两处是 CALLER 系 task-local 仅有的 scoping 站点——漏一处，那条路径上的请求对 admin 闸和后续 P1 可见性咽喉就是无身份旁路）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib caller_identity` && `cargo test -p alephcore --lib "connect::"`
Expected: 全 PASS。再跑 `cargo test -p alephcore --lib --no-run` 确认全库含测试可编译（ConnectionState 构造点全部补齐）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/caller_identity.rs src/gateway/server/ src/gateway/handlers/connect.rs
git commit -m "gateway: resolve connection user identity and scope CALLER_USER through dispatch"
```

---

### Task 3: 配对票绑定用户（邀请流）

**Files:**
- Modify: `src/gateway/security/store/bootstrap_tickets.rs`（create 写 user_id；consume 读回）
- Modify: `src/gateway/security/store/devices.rs`（`DeviceUpsertData.user_id` + COALESCE）
- Modify: `src/gateway/security/device_token_manager.rs`（`create_bootstrap_ticket` / `exchange_bootstrap_ticket` 穿参）
- Modify: `gateway.ticket.create` 的 handler（grep `ticket.create` 定位注册点）加可选 `user_id` 参数
- Test: `device_token_manager.rs` 既有测试模块追加

**Interfaces:**
- Consumes: Task 1 的 users CRUD；既有 `ConsumedBootstrapTicket`、`BootstrapExchangeResult`
- Produces:
  - `DeviceTokenManager::create_bootstrap_ticket(&self, ttl_ms: Option<i64>, user_id: Option<&str>) -> Result<String, DeviceTokenError>`
  - `ConsumedBootstrapTicket.user_id: Option<String>`
  - `DeviceUpsertData.user_id: Option<&'a str>`

- [ ] **Step 1: 写失败测试**（`device_token_manager.rs` 测试模块，抄它既有 exchange 测试的 fixture）

```rust
#[test]
fn ticket_bound_to_user_stamps_device_user() {
    let (mgr, store) = manager_fixture(); // 既有测试的构造方式
    store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
    let code = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
    let result = mgr.exchange_bootstrap_ticket(&code, Some("dev-a".into()), None, None).unwrap();
    assert_eq!(store.device_user("dev-a").unwrap().as_deref(), Some("u-alice"));
    let _ = result; // token issuance shape unchanged
}

#[test]
fn unbound_ticket_defaults_device_to_owner() {
    let (mgr, store) = manager_fixture();
    let code = mgr.create_bootstrap_ticket(None, None).unwrap();
    mgr.exchange_bootstrap_ticket(&code, Some("dev-b".into()), None, None).unwrap();
    assert_eq!(store.device_user("dev-b").unwrap().as_deref(), Some(OWNER_USER_ID));
}

#[test]
fn repairing_does_not_silently_reassign_device_user() {
    // Mine-4 sibling: ON CONFLICT must COALESCE, not overwrite with NULL.
    let (mgr, store) = manager_fixture();
    store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
    let t1 = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
    mgr.exchange_bootstrap_ticket(&t1, Some("dev-a".into()), None, None).unwrap();
    let t2 = mgr.create_bootstrap_ticket(None, None).unwrap(); // unbound re-pair
    mgr.exchange_bootstrap_ticket(&t2, Some("dev-a".into()), None, None).unwrap();
    assert_eq!(store.device_user("dev-a").unwrap().as_deref(), Some("u-alice"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib device_token_manager`
Expected: 编译错误（签名不匹配）。

- [ ] **Step 3: 实现**

- `store/bootstrap_tickets.rs::create_bootstrap_ticket(code, ttl_ms, user_id: Option<&str>)`：INSERT 加 `user_id` 列。consume 的 SELECT/RETURNING 加读 `user_id`，`ConsumedBootstrapTicket` 加 `pub user_id: Option<String>`（该 struct 在 `bootstrap_tickets.rs` 顶部，`pub use` 于 mod.rs L35）。
- `store/devices.rs`：`DeviceUpsertData<'_>` 加 `pub user_id: Option<&'a str>`；INSERT 列表加 `user_id`；ON CONFLICT 子句加 `user_id = COALESCE(excluded.user_id, devices.user_id)`（**保留既有的「不改写 device_type、清 revoked_at」两条语义原样**——地雷 3/4）。所有既有 `DeviceUpsertData` 构造点补 `user_id: None`。
- `device_token_manager.rs`：
  - `create_bootstrap_ticket(ttl_ms, user_id)` 传给 store（保持 `aleph-bt-` 前缀与 TTL clamp 不动）。
  - `exchange_bootstrap_ticket`：消费票后取 `consumed.user_id`，`unwrap_or(OWNER_USER_ID)` 填进 `DeviceUpsertData.user_id`（`Some(...)`——COALESCE 语义下 unbound 重配对不会清掉已有归属；首配 unbound 落 owner）。**注意测试 3 的语义**：unbound 重配对时应传 `None` 而非 `Some(OWNER)`，否则会把 alice 的设备改回 owner——正确实现是 `user_id: consumed.user_id.as_deref()`，然后**只在设备行插入后仍为 NULL 时**补 owner（一条 `UPDATE devices SET user_id = ?1 WHERE device_id = ?2 AND user_id IS NULL`）。两步合起来满足三条测试。
- `gateway.ticket.create` handler：params struct 加 `user_id: Option<String>`（serde default），传入 manager。回包形状不变。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib device_token_manager` && `cargo test -p alephcore --lib security::store`
Expected: 新 3 条 + 既有 device/ticket 测试全 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/security/
git commit -m "gateway: bind bootstrap tickets and paired devices to users"
```

---

### Task 4: admin 方法闸（单强制点，在 process_request 内）

**Files:**
- Create: `src/gateway/method_admin.rs`
- Modify: `src/gateway/mod.rs`（`pub mod method_admin;`——照 `method_authz` 的声明方式）
- Modify: `process_request` 所在文件（grep `fn process_request` 定位；它解析 `JsonRpcRequest` 后、查 registry 派发前插闸）
- Test: `method_admin.rs` 内嵌 + process_request 层集成测试

**Interfaces:**
- Consumes: `caller_identity::current_caller_role()`（Task 2 已 scoped 在 process_request 外层）
- Produces: `pub fn method_requires_admin(method: &str) -> bool`

**设计要点**：闸放在 `process_request` **内部**而非两个 dispatch 站点——CALLER_ROLE 的 scope 恰好包住 process_request，一个检查点覆盖两条派发路径（single-chokepoint 纪律；spec §5.4 的先导）。判据：`method_requires_admin(method) && current_caller_role().as_deref() == Some("member")` ⇒ 拒。`None`（internal/cron）与 `"operator"` 放行；`"guest"` 连接根本过不了登录墙（仅 `connect` 放行），无需在此重复判。

- [ ] **Step 1: 写失败测试**（`method_admin.rs`）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Tripwire subset — mirrors method_authz::MUST_STAY_GATED's philosophy:
    /// a curated pin, not a second source of truth.
    #[test]
    fn credential_and_config_methods_require_admin() {
        for m in [
            "gateway.token.rotate",
            "gateway.ticket.create",
            "gateway.devices.revoke",
            "gateway.devices.list",
            "users.create",
            "users.update",
        ] {
            assert!(method_requires_admin(m), "{m} must require admin");
        }
    }

    #[test]
    fn member_daily_methods_stay_open() {
        for m in [
            "connect",
            "chat.send",
            "sessions.list",
            "users.me",
            "users.list",
            "projects.list",
        ] {
            assert!(!method_requires_admin(m), "{m} must stay open to members");
        }
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib method_admin`
Expected: 编译错误（模块不存在）。

- [ ] **Step 3: 实现分类器**

```rust
//! Method-level admin authorization for the multi-user role gate (spec §4.6).
//!
//! Sibling of `method_authz` (the per-TOOL channel-tier gate). This one
//! classifies RPC METHODS: server-global configuration, credentials, fleet,
//! and user management are admin-only; everything scoped to the caller's own
//! data stays open to members. Enforced at ONE chokepoint inside
//! `process_request` — both WS dispatch paths run through it.

/// Method prefixes that mutate or expose server-global state. A prefix match
/// gates the whole family so newly-registered siblings are gated by default
/// (fail-closed for privilege); carve-outs below re-open member-safe reads.
const ADMIN_PREFIXES: &[&str] = &[
    "gateway.",   // tokens, tickets, devices, origin config
    "users.",     // principal management (carve-outs: me / list)
    "providers.", // LLM provider config
    "channels.",  // channel config
    "cluster.",   // fleet
    "config.",    // server config
    "hub.",       // extension install surface
];

/// Member-safe reads inside otherwise-admin families.
const MEMBER_CARVE_OUTS: &[&str] = &["users.me", "users.list", "gateway.status"];

#[must_use]
pub fn method_requires_admin(method: &str) -> bool {
    if MEMBER_CARVE_OUTS.contains(&method) {
        return false;
    }
    ADMIN_PREFIXES.iter().any(|p| method.starts_with(p))
}
```

**实现时必做的一次枚举**（不是可选项）：`grep -o 'registry.register("[^"]*"' src/gateway/handlers/mod.rs | sort` 列出全部已注册方法，逐个按规则「服务器全局配置/凭据/舰队/用户管理 ⇒ admin；调用者自身数据 ⇒ open」核对上面两张表——把真实存在的 admin 家族补进 `ADMIN_PREFIXES`（如枚举发现 `mcp.`、`skills.` 安装类），把误伤的 member 日常读操作补进 `MEMBER_CARVE_OUTS`，并同步扩两条测试的用例表。`connect` / `chat.*` / `sessions.*` / `memory.*` / `projects.*` / `artifacts.*` 是 member 日常面，**不进前缀表**（会话/记忆的按用户过滤是 P1 的可见性咽喉，不是本闸的事）。

- [ ] **Step 4: 在 process_request 插闸**

定位 `fn process_request`（`handler.rs` 两个站点都调它）。在解析出 `JsonRpcRequest` 之后、registry 派发之前：

```rust
// Multi-user role gate (spec §4.6): members cannot reach server-global
// config/credential methods. One chokepoint covers both dispatch paths —
// CALLER_ROLE is scoped around process_request at both call sites.
if crate::gateway::method_admin::method_requires_admin(&request.method)
    && crate::gateway::caller_identity::current_caller_role().as_deref() == Some("member")
{
    return serde_json::to_string(&JsonRpcResponse::error(
        request.id.clone(),
        AUTHORIZATION_ERROR, // ← use the same error code the login wall uses for
                             //   non-connect methods on walled connections; grep
                             //   the wall arm in handler.rs and reuse that const
    ))
    .unwrap_or_default();
}
```

（错误码常量名以 `protocol.rs` 实际定义为准——登录墙拒非 connect 方法的那条臂用什么，这里就用什么，保持客户端可识别。）

- [ ] **Step 5: 写闸的集成测试**（process_request 层，放在其所在文件的测试模块；构造方式抄该文件既有 process_request 测试，若无则用 registry 最小 fixture）

```rust
#[tokio::test]
async fn member_is_refused_admin_methods_at_the_chokepoint() {
    let mc = test_middleware_chain(); // 该文件既有测试的构造方式
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"gateway.token.rotate","params":{}}"#;
    let resp = crate::gateway::caller_identity::CALLER_ROLE
        .scope(Some("member".to_string()), process_request(req, &mc))
        .await;
    assert!(resp.contains("error"), "member must be refused: {resp}");

    let resp_ok = crate::gateway::caller_identity::CALLER_ROLE
        .scope(Some("operator".to_string()), process_request(req, &mc))
        .await;
    assert!(!resp_ok.contains(r#""code":"#) || !resp_ok.contains("Unauthorized"),
        "operator must pass the gate (may fail later for other reasons): {resp_ok}");
}
```

（第二个断言只验证「不是被本闸拒的」——rotate 在测试 fixture 里可能因缺 device manager 报别的错，那不属于本闸职责。）

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p alephcore --lib method_admin` && 集成测试所在模块
Expected: 全 PASS。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/method_admin.rs src/gateway/mod.rs src/gateway/server/
git commit -m "gateway: add member/admin method gate at the process_request chokepoint"
```

---

### Task 5: users.* RPC（me / list / create / update+deactivation kick）

**Files:**
- Create: `src/gateway/handlers/users.rs`
- Modify: `src/gateway/handlers/mod.rs`（注册块，照 projects 模式 L499-522）
- Modify: `src/gateway/start/mod.rs`（deactivation 的 socket 踢——**复用** DeviceRevoked 既有接线）
- Test: `handlers/users.rs` 内嵌

**Interfaces:**
- Consumes: Task 1 store CRUD、Task 2 `current_caller_user()`、既有 `JsonRpcRequest/JsonRpcResponse/parse_params`、devices.revoke 的既有 revoke+kick 管线
- Produces: RPC `users.me` / `users.list` / `users.create` / `users.update`

**RPC 形状**（admin 闸由 Task 4 统一强制，handler 不二次判权——单强制点纪律）：

- `users.me` → `{ user: { user_id, display_name, role, status } | null }`（读 `current_caller_user()` + store）
- `users.list` → `{ users: [UserView] }`（member 可见——项目名册选人要用）
- `users.create { display_name, role? ("member") }` → `{ user: UserView }`；`user_id` 服务端生成 `format!("u-{}", Uuid::new_v4())`
- `users.update { user_id, display_name?, role?, status? }` → `{ user: UserView }`；`status="deactivated"` 时**同时**吊销该用户全部设备（下述接线）
- **Owner 不可停用/降级守卫**：`user_id == OWNER_USER_ID` 时拒绝 `status="deactivated"` 与 `role="member"`（`JsonRpcResponse::error`，invalid params 语义）。理由：Task 2 的 loopback 臂**不查 user status**、恒解析 (u-owner, operator)——那是恢复路径（等价 root console），所以「停用 owner」只会产生半生效状态（远程设备被踢、本机不受影响），语义不自洽；直接禁止。这也顺带保证系统永远至少有一个 admin。

**Deactivation 语义**（spec §10：设备 token 即时拒绝）：handler 层只做两件纯事——store 置 status + `list_device_ids_for_user` 逐个走**与 `gateway.devices.revoke` 完全相同的路径**（store revoke + 发同一个 `DeviceRevoked` 事件）。这复用 start/mod.rs 既有的「先 `invalidate_device_sessions` 降权、再关 socket」顺序与客户端已登记的 `device_revoked` 关闭原因（gateway/CLAUDE.md 地雷 2/2b——**零新增关闭原因，零 Panel 改动**）。实现方式：把 devices.revoke handler 里「吊销一台设备」的那段提为 `pub(crate) fn revoke_device_and_kick(...)` 供两处调用，**不复制第二份**。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // fixtures: in-memory SecurityStore via seeded_store(); JsonRpcRequest built
    // the same way handlers/projects.rs tests build theirs (grep that file's
    // test module for the request constructor).

    #[tokio::test]
    async fn me_reflects_caller_user() {
        let store = seeded_store();
        let req = rpc_request("users.me", serde_json::json!({}));
        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-owner".to_string()), handle_me(req, store.clone()))
            .await;
        let v = response_json(&resp);
        assert_eq!(v["user"]["user_id"], "u-owner");
        assert_eq!(v["user"]["role"], "admin");
    }

    #[tokio::test]
    async fn create_then_list_shows_member() {
        let store = seeded_store();
        let req = rpc_request("users.create", serde_json::json!({"display_name": "Alice"}));
        let resp = handle_create(req, store.clone()).await;
        let created = response_json(&resp);
        assert_eq!(created["user"]["role"], "member");

        let listed = response_json(&handle_list(rpc_request("users.list", serde_json::json!({})), store).await);
        assert_eq!(listed["users"].as_array().unwrap().len(), 2); // owner + alice
    }

    #[tokio::test]
    async fn deactivate_revokes_all_user_devices() {
        let store = seeded_store();
        store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        upsert_panel_device(&store, "dev-a1", "u-alice");
        upsert_panel_device(&store, "dev-a2", "u-alice");

        let req = rpc_request("users.update",
            serde_json::json!({"user_id": "u-alice", "status": "deactivated"}));
        handle_update(req, store.clone(), test_kick_sink()).await;

        assert!(store.list_device_ids_for_user("u-alice").unwrap().is_empty(),
            "live (un-revoked) device list must be empty after deactivation");
    }

    #[tokio::test]
    async fn owner_cannot_be_deactivated_or_demoted() {
        let store = seeded_store();
        for body in [
            serde_json::json!({"user_id": OWNER_USER_ID, "status": "deactivated"}),
            serde_json::json!({"user_id": OWNER_USER_ID, "role": "member"}),
        ] {
            let resp = handle_update(rpc_request("users.update", body), store.clone(), test_kick_sink()).await;
            assert!(response_is_error(&resp), "owner must stay an active admin");
        }
        let owner = store.get_user(OWNER_USER_ID).unwrap().expect("owner exists");
        assert_eq!(owner.status, UserStatus::Active);
        assert_eq!(owner.role, UserRole::Admin);
    }
}
```

（`test_kick_sink()`：handler 依赖注入的事件发射口的测试替身——按 devices.revoke handler 现有的事件发射依赖形状同款注入；若它直接拿 event bus，则本测试注入测试 bus。以现有 devices.revoke 的测试写法为准，不发明新形状。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib handlers::users`
Expected: 编译错误。

- [ ] **Step 3: 实现 handler + 注册**（照 `handlers/projects.rs` 的形状：`UserView` Serialize 结构、`parse_params`、`JsonRpcResponse::success/error`；注册块照 mod.rs L499-522，store 取 gateway 已持有的 SecurityStore Arc——connect 授权用的同一个）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib handlers::users`
Expected: 4 个 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/handlers/ src/gateway/start/
git commit -m "gateway: add users.* rpc surface with deactivation device revocation"
```

---

### Task 6: 渠道配对链接用户

**Files:**
- Modify: `src/gateway/pairing_store.rs`
- Test: 该文件既有测试模块追加

**Interfaces:**
- Consumes: Task 1 `OWNER_USER_ID`
- Produces: `approved_senders.user_id` 列；`approve_*`（该文件实际的批准函数名，grep `approved_senders` 的 INSERT 定位）gains `user_id: Option<&str>`；`pub fn sender_user(&self, channel: &str, sender_id: &str) -> Option<String>`（P1 的 inbound 会话归属将消费它；P0 先落数据）

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn approve_links_owner_by_default_and_honors_explicit_user() {
    let store = test_pairing_store(); // 该文件既有测试的构造方式
    approve_fixture(&store, "telegram", "12345", None);
    assert_eq!(store.sender_user("telegram", "12345").as_deref(), Some("u-owner"));

    approve_fixture(&store, "telegram", "67890", Some("u-alice"));
    assert_eq!(store.sender_user("telegram", "67890").as_deref(), Some("u-alice"));
}

#[test]
fn migration_adopts_existing_approved_senders_to_owner() {
    let store = test_pairing_store();
    // Simulate a pre-migration row (raw SQL insert without user_id).
    store.raw_insert_approved("telegram", "old-peer"); // #[cfg(test)] helper
    store.run_migrations().unwrap();
    assert_eq!(store.sender_user("telegram", "old-peer").as_deref(), Some("u-owner"));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib pairing_store`
Expected: 编译错误。

- [ ] **Step 3: 实现**

该文件的建表是 `CREATE TABLE IF NOT EXISTS` batch（L130-151），没有版本化迁移——加列用防重跑写法：

```rust
// ALTER TABLE has no IF NOT EXISTS in SQLite; probe the schema first.
let has_user_col: bool = {
    let mut stmt = conn.prepare("SELECT 1 FROM pragma_table_info('approved_senders') WHERE name = 'user_id'")?;
    stmt.exists([])?
};
if !has_user_col {
    conn.execute("ALTER TABLE approved_senders ADD COLUMN user_id TEXT", [])?;
    // Adoption: every pre-existing approved sender belonged to the single user.
    conn.execute(
        "UPDATE approved_senders SET user_id = ?1 WHERE user_id IS NULL",
        rusqlite::params![crate::gateway::security::store::OWNER_USER_ID],
    )?;
}
```

批准函数：INSERT 加 `user_id` 列，`user_id.unwrap_or(OWNER_USER_ID)`。`sender_user` 一条 SELECT。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib pairing_store`
Expected: 新 2 条 + 既有配对测试全 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/pairing_store.rs
git commit -m "gateway: link approved channel senders to users with owner adoption"
```

---

### Task 7: 验收守卫 + 全量验证 + 文档

**Files:**
- Test: `src/gateway/` 各测试模块补两条端到端守卫
- Modify: `docs/reference/SECURITY.md`（auth-ux 节补 users/role 段）、`src/gateway/CLAUDE.md`（信任模型一段 + 新地雷）
- Modify: `src/gateway/caller_identity.rs`（改掉「every connection is an implicit operator」的过时 doc——它是全仓最硬的单用户断言，P0 之后为假）

**验收对照（spec §8 P0 行）：**

| 验收 | 由哪条测试证明 |
|---|---|
| 多用户多设备各自登录 | Task 3 `ticket_bound_to_user_stamps_device_user` + Task 2 `device_of_member_user_resolves_member_role` |
| member 调不到 admin RPC | Task 4 `member_is_refused_admin_methods_at_the_chokepoint` |
| 单用户体验零变化 | 本任务新增的两条（下） |

- [ ] **Step 1: 写零变化守卫测试**（放 `handlers/connect.rs` 测试模块）

```rust
#[test]
fn zero_change_loopback_is_owner_operator_on_fresh_store() {
    // The single-user guarantee: a fresh (or migrated) deployment touching
    // nothing new behaves exactly as before — loopback is a full operator.
    let store = seeded_store();
    let (user, role) = resolve_connection_identity(true, None, &store);
    assert_eq!(role, "operator");
    assert_eq!(user.as_deref(), Some(OWNER_USER_ID));
}

#[test]
fn zero_change_admin_gate_is_inert_for_operator_and_internal() {
    // operator (single-user default) and None (cron/internal) pass every method.
    for m in ["gateway.token.rotate", "users.create", "providers.update"] {
        assert!(crate::gateway::method_admin::method_requires_admin(m));
    }
    // The gate predicate refuses ONLY Some("member") — asserted at the
    // chokepoint test in Task 4; here we pin the classifier side.
    assert!(!crate::gateway::method_admin::method_requires_admin("chat.send"));
}
```

- [ ] **Step 2: 跑 gateway 全量测试**

Run: `cargo test -p alephcore --lib gateway`
Expected: 全 PASS（对照 Windows 基线 15 个既存失败，不新增）。

- [ ] **Step 3: 最小可信验证集（仓库纪律四件套）**

```bash
cargo test -p alephcore --lib --no-run
cargo check -p aleph-panel
cargo check -p aleph-desktop-windows
cargo clippy --all-targets
```

Expected: 全绿（macos/linux 限肢 crate 在 Windows 上按 memory `windows-full-verify-scope` 排除）。

- [ ] **Step 4: 文档同步**

- `SECURITY.md#auth-ux`：在单层信任模型段后加「多用户角色层（P0）」小节：users 表、设备/配对链接、admin/member 分界表（抄 spec §4.6）、隐式 owner 零迁移、`method_admin` 单强制点。
- `src/gateway/CLAUDE.md`：信任模型段补一句「授权后连接携带 (user, role)；member 由 `method_admin.rs` 闸在 `process_request` 单点强制」；新增地雷条目：「**新 dispatch 路径必须过 process_request**——admin 闸和 CALLER_USER 都住在它周围，绕开它的新派发路径 = 无身份旁路」。
- `caller_identity.rs` 模块 doc：删除/改写 L12-15 的 LAN-trust 单 operator 断言。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/ docs/reference/SECURITY.md
git commit -m "gateway: p0 identity acceptance guards and security docs"
```

---

## Self-Review 记录

- **Spec 覆盖**：spec §8 P0 行的五项——users 表(T1)、devices/pairing 链接(T3/T6)、CallerIdentity(T2)、隐式 owner 迁移(T1)、method_authz 角色闸(T4，实现为新 `method_admin` 而非改 `method_authz`——后者是按**工具名**的 channel 闸，语义不同，spec §4.6 的「扩展」落地为兄弟模块 + 单强制点)。users.* RPC(T5) 是验收「多用户各自登录/管理」的必要操作面。
- **类型一致性**：`OWNER_USER_ID`/`UserRole`/`UserStatus`/`resolve_connection_identity`/`method_requires_admin` 各任务间签名一致（Interfaces 块为准）。
- **明确不在 P0**：Panel UI、会话/记忆按 user 过滤（P1 咽喉）、inbound_router 渠道 role 变更、`users.delete`（P0 只停用不删除——记忆/会话归属还没按用户分区，删除语义未定义）。

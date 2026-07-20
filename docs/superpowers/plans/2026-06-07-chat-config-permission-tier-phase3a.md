# Chat/Config 权限分层 Phase 3a 实现计划（device 永久提升/降级 RPC）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 operator-only 网关 RPC `devices.set_level`，永久切换已配对设备的权限档位（chat ↔ config），并原地刷新该设备的活连接使变更立即生效。

**Architecture:** handler 落在现有 `handlers/auth/devices.rs`（与 list/revoke 同址）。持久化复用 `DeviceStore::update_permissions`；档位 SSOT 复用 `tier.rs`（`Tier::from_level` / `Tier::permissions` / `role_for_permissions`）。立即生效靠把 `GatewayServer.connections` 注册表（`Arc<RwLock<HashMap<String, ConnectionState>>>`）经 boot 路径穿进 `AuthContext`，handler 写锁遍历匹配 `device_id` 的活连接、原地改 `role`/`permissions`——Phase 2 的 caller_role 与 operator 门控每请求读活 `ConnectionState`，故下一请求即咬合。**不碰 security_store**（其 device role/scopes 不被实时 authz 读取，且 `upsert_device` 的 ON CONFLICT 只更新 name+last_seen，无法更新 role/scopes）。

**Tech Stack:** Rust，gateway JSON-RPC，tokio `RwLock`，rusqlite（device_store），`#[tokio::test]` 单测。

**关键事实（已逐一核对源码）：**
- `AuthContext` 定义 `src/gateway/handlers/auth/mod.rs:167`，无 connections 字段；生产构造 `src/bin/aleph-server/commands/start/builder/subsystems.rs:183-208`（在 `initialize_auth` 内）。
- `initialize_auth` 定义 `subsystems.rs:55`，调用点 `src/bin/aleph-server/commands/start/mod.rs:393`，已传 `server.presence.clone()` 等（`server` 创建于 `mod.rs:166`，其 `connections` 字段 `src/gateway/server/mod.rs:132`）。
- `ConnectionState` 定义 `src/gateway/server/mod.rs:43`（pub struct，pub 字段 `role`/`permissions`/`device_id`），构造器 `fn new(client_ip)` 在 `mod.rs:86`（**当前私有**），`authenticate()` 在 `mod.rs:107`（pub）。
- 路由注册 `register_auth_handlers` 在 `src/bin/aleph-server/commands/start/builder/handlers/auth.rs:5`（`register_handler!` 宏）。
- 门控 `OPERATOR_METHODS` 在 `src/gateway/method_authz.rs:58`（`devices.revoke` 在 line 67 附近）。
- `DeviceStore`：`get_device(&str)->Option<ApprovedDevice>`、`update_permissions(&str,&[String])->SqliteResult<bool>`、`approve_device(&ApprovedDevice)`；`ApprovedDevice::new(id,name,Option<type>)`，`permissions` 为 pub 字段。

---

### Task 1: 把 connections 注册表穿进 AuthContext

将 `GatewayServer.connections` 共享到 `AuthContext`，使后续 handler 能原地刷新活连接。本任务为结构性接线：门控是「编译通过 + 既有测试全绿」（无新行为）。

**Files:**
- Modify: `src/gateway/server/mod.rs:86`（`ConnectionState::new` 改 `pub(crate)`）
- Modify: `src/gateway/handlers/auth/mod.rs`（imports + `AuthContext` 字段 + 2 个测试构造器）
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs`（imports + `initialize_auth` 加参 + 构造块加字段）
- Modify: `src/bin/aleph-server/commands/start/mod.rs:393`（调用点加实参）

- [ ] **Step 1: 放开 `ConnectionState::new` 可见性**

`src/gateway/server/mod.rs:86`，把：

```rust
    fn new(client_ip: std::net::IpAddr) -> Self {
```

改为：

```rust
    pub(crate) fn new(client_ip: std::net::IpAddr) -> Self {
```

- [ ] **Step 2: 在 `AuthContext` 模块加 imports**

`src/gateway/handlers/auth/mod.rs`，在现有 `use crate::sync_primitives::Arc;`（约 line 12）下方加：

```rust
use crate::gateway::server::ConnectionState;
use std::collections::HashMap;
use tokio::sync::RwLock;
```

- [ ] **Step 3: 给 `AuthContext` 加 connections 字段**

`src/gateway/handlers/auth/mod.rs`，在 `AuthContext` 结构体里（`pub event_bus: ...` 字段附近，约 line 174）加：

```rust
    /// Live connection registry, shared with `GatewayServer`. Lets
    /// `devices.set_level` refresh the role/permissions of a device's active
    /// connection(s) in place so a tier change takes effect on the next
    /// request without waiting for reconnect.
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
```

- [ ] **Step 4: 两个测试构造器补字段**

`src/gateway/handlers/auth/mod.rs`，在 `create_test_auth_context_for_role_test()`（约 line 318 起的 `AuthContext { ... }`）和 `create_test_context()`（约 line 381 起的 `Arc::new(AuthContext { ... })`）两处构造块里各加一行：

```rust
            connections: Arc::new(RwLock::new(HashMap::new())),
```

（两个测试模块均 `use super::*;`，故 `RwLock`/`HashMap`/`Arc` 已在作用域。）

- [ ] **Step 5: `subsystems.rs` 加 imports**

`src/bin/aleph-server/commands/start/builder/subsystems.rs` 顶部 `use` 区加：

```rust
use alephcore::gateway::server::ConnectionState;
use std::collections::HashMap;
use tokio::sync::RwLock;
```

（`Arc` 已在该文件作用域——文件内大量 `Arc::new(...)`。）

- [ ] **Step 6: `initialize_auth` 加参数**

`src/bin/aleph-server/commands/start/builder/subsystems.rs`，`initialize_auth` 参数列表里，在 `presence: Arc<alephcore::gateway::presence::PresenceTracker>,` 之后插入：

```rust
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
```

- [ ] **Step 7: 生产构造块写入字段**

`src/bin/aleph-server/commands/start/builder/subsystems.rs:183-208` 的 `auth_handlers::AuthContext { ... }` 里，在 `presence,`（约 line 199）之后加一行（字段名与参数名一致，用简写）：

```rust
        connections,
```

- [ ] **Step 8: 调用点传实参**

`src/bin/aleph-server/commands/start/mod.rs:393` 的 `initialize_auth(...)` 调用里，在 `server.presence.clone(),`（约 line 403）之后插入：

```rust
        server.connections.clone(),
```

- [ ] **Step 9: 编译 + 既有测试**

Run: `cargo check -p alephcore && cargo check --bin aleph-server`
Expected: 编译通过（无 missing-field / 类型不匹配错误）。

Run: `cargo test -p alephcore --lib gateway::handlers::auth`
Expected: 既有 auth 测试全绿（构造器补字段后不回归）。

- [ ] **Step 10: Commit**

```bash
git add src/gateway/server/mod.rs src/gateway/handlers/auth/mod.rs src/bin/aleph-server/commands/start/builder/subsystems.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "gateway: thread connections registry into AuthContext for live role refresh"
```

---

### Task 2: method_authz 把 `devices.set_level` 标为 operator-only

**Files:**
- Modify: `src/gateway/method_authz.rs:58`（`OPERATOR_METHODS`）
- Test: `src/gateway/method_authz.rs`（`#[cfg(test)] mod tests`）

- [ ] **Step 1: 写失败测试**

`src/gateway/method_authz.rs` 的 `#[cfg(test)] mod tests` 里新增：

```rust
    #[test]
    fn devices_set_level_requires_operator() {
        assert_eq!(
            required_privilege("devices.set_level"),
            MethodPrivilege::Operator
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib method_authz::tests::devices_set_level_requires_operator`
Expected: FAIL（`devices.set_level` 尚未登记，返回 `Authenticated` 而非 `Operator`）。

- [ ] **Step 3: 加入 OPERATOR_METHODS**

`src/gateway/method_authz.rs`，在 `OPERATOR_METHODS` 的 “Device & pairing management” 段、`"devices.revoke",` 之后加：

```rust
    "devices.set_level",
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib method_authz`
Expected: PASS（新测试 + 既有 `admin_methods_require_operator` 全绿）。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/method_authz.rs
git commit -m "method_authz: gate devices.set_level as operator-only"
```

---

### Task 3: 实现 `handle_devices_set_level` + 单元测试

核心任务。先写测试，再实现。

**Files:**
- Modify: `src/gateway/handlers/auth/devices.rs`（新 handler + 测试）

- [ ] **Step 1: 写失败测试（升级 / 降级活连接刷新 / 未知设备 / 非法 level / 非匹配连接不动）**

`src/gateway/handlers/auth/devices.rs` 的 `#[cfg(test)] mod tests` 里新增（`use super::*;` 已在模块顶部；`ApprovedDevice` 已 import）：

```rust
    #[tokio::test]
    async fn set_level_upgrade_persists_and_refreshes_live_connection() {
        use crate::gateway::server::ConnectionState;
        let ctx = super::super::tests::create_test_context();

        // A chat-tier device is paired.
        let mut dev = ApprovedDevice::new("dev-up".to_string(), "Up".to_string(), None);
        dev.permissions = vec!["chat".to_string(), "read".to_string()];
        ctx.device_store.approve_device(&dev).unwrap();

        // It has a live (guest) connection.
        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        cs.authenticate(
            "dev-up".to_string(),
            vec!["chat".to_string(), "read".to_string()],
            Some("guest".to_string()),
        );
        ctx.connections.write().await.insert("c-up".to_string(), cs);

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-up", "level": "config"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(resp.is_success(), "{:?}", resp);

        let stored = ctx.device_store.get_device("dev-up").unwrap();
        assert_eq!(stored.permissions, vec!["*".to_string()]);

        let conns = ctx.connections.read().await;
        let live = conns.get("c-up").unwrap();
        assert_eq!(live.role.as_deref(), Some("operator"));
        assert_eq!(live.permissions, vec!["*".to_string()]);
    }

    #[tokio::test]
    async fn set_level_downgrade_persists_and_refreshes_live_connection() {
        use crate::gateway::server::ConnectionState;
        let ctx = super::super::tests::create_test_context();

        let mut dev = ApprovedDevice::new("dev-dn".to_string(), "Dn".to_string(), None);
        dev.permissions = vec!["*".to_string()];
        ctx.device_store.approve_device(&dev).unwrap();

        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        cs.authenticate(
            "dev-dn".to_string(),
            vec!["*".to_string()],
            Some("operator".to_string()),
        );
        ctx.connections.write().await.insert("c-dn".to_string(), cs);

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-dn", "level": "chat"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(resp.is_success(), "{:?}", resp);

        let stored = ctx.device_store.get_device("dev-dn").unwrap();
        assert_eq!(
            stored.permissions,
            vec!["chat".to_string(), "read".to_string()]
        );

        let conns = ctx.connections.read().await;
        let live = conns.get("c-dn").unwrap();
        assert_eq!(live.role.as_deref(), Some("guest"));
        assert_eq!(
            live.permissions,
            vec!["chat".to_string(), "read".to_string()]
        );
    }

    #[tokio::test]
    async fn set_level_unknown_device_errors() {
        let ctx = super::super::tests::create_test_context();
        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "nope", "level": "config"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx).await;
        assert!(!resp.is_success());
    }

    #[tokio::test]
    async fn set_level_invalid_level_errors_without_mutating() {
        let ctx = super::super::tests::create_test_context();
        let mut dev = ApprovedDevice::new("dev-bad".to_string(), "Bad".to_string(), None);
        dev.permissions = vec!["*".to_string()];
        ctx.device_store.approve_device(&dev).unwrap();

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-bad", "level": "admin"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(!resp.is_success());

        // Must NOT silently downgrade on a bad level.
        let stored = ctx.device_store.get_device("dev-bad").unwrap();
        assert_eq!(stored.permissions, vec!["*".to_string()]);
    }

    #[tokio::test]
    async fn set_level_leaves_other_devices_connections_untouched() {
        use crate::gateway::server::ConnectionState;
        let ctx = super::super::tests::create_test_context();

        let mut target = ApprovedDevice::new("dev-t".to_string(), "T".to_string(), None);
        target.permissions = vec!["*".to_string()];
        ctx.device_store.approve_device(&target).unwrap();

        // An unrelated device's live connection.
        let mut other = ConnectionState::new("127.0.0.1".parse().unwrap());
        other.authenticate(
            "dev-other".to_string(),
            vec!["*".to_string()],
            Some("operator".to_string()),
        );
        ctx.connections.write().await.insert("c-other".to_string(), other);

        let req = JsonRpcRequest::with_id(
            "devices.set_level",
            Some(json!({"device_id": "dev-t", "level": "chat"})),
            json!(1),
        );
        let resp = handle_devices_set_level(req, ctx.clone()).await;
        assert!(resp.is_success(), "{:?}", resp);

        let conns = ctx.connections.read().await;
        let other = conns.get("c-other").unwrap();
        assert_eq!(other.role.as_deref(), Some("operator"));
        assert_eq!(other.permissions, vec!["*".to_string()]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::devices`
Expected: FAIL（`handle_devices_set_level` 未定义 / 无法解析）。

- [ ] **Step 3: 实现 handler**

`src/gateway/handlers/auth/devices.rs`，在 `handle_devices_revoke` 之后、`#[cfg(test)]` 之前加：

```rust
/// Handle "devices.set_level" request — permanently change an approved
/// device's permission tier (chat <-> config) and refresh any live
/// connection(s) of that device in place so the change takes effect on the
/// device's next request (no reconnect required). Operator-only (gated in
/// method_authz). device_store.permissions is the SSOT; security_store is
/// deliberately not touched (its device role/scopes are not consulted by the
/// live authz gate).
pub async fn handle_devices_set_level(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct SetLevelParams {
        device_id: String,
        /// "config" => operator/config tier, "chat" => chat tier.
        level: String,
    }

    let params: SetLevelParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Reject any level that isn't exactly chat/config (case-insensitive).
    // We do NOT fall back to Tier::from_level's silent default-to-Chat here:
    // a typo must not silently downgrade an operator device.
    let level_lc = params.level.to_ascii_lowercase();
    if level_lc != "chat" && level_lc != "config" {
        return JsonRpcResponse::error(
            request.id,
            -32602,
            format!(
                "invalid level '{}': expected 'chat' or 'config'",
                params.level
            ),
        );
    }

    // Device must already be paired.
    if ctx.device_store.get_device(&params.device_id).is_none() {
        return JsonRpcResponse::error(request.id, -32004, "Device not found");
    }

    let tier = super::tier::Tier::from_level(Some(level_lc.as_str()));
    let permissions = tier.permissions();

    // Persist the new tier. device_store.permissions is what the connect path
    // re-derives the connection role from, so this alone fixes future reconnects.
    match ctx
        .device_store
        .update_permissions(&params.device_id, &permissions)
    {
        Ok(true) => {}
        Ok(false) => return JsonRpcResponse::error(request.id, -32004, "Device not found"),
        Err(e) => {
            warn!(error = %e, "Failed to update device permissions");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to update permissions: {}", e),
            );
        }
    }

    // Refresh any LIVE connection(s) for this device in place. The method-authz
    // gate and Phase-2 caller_role both read the live ConnectionState per
    // request, so a downgrade bites on the next request without reconnect.
    let new_role = super::tier::role_for_permissions(&permissions).to_string();
    {
        let mut conns = ctx.connections.write().await;
        for state in conns.values_mut() {
            if state.device_id.as_deref() == Some(params.device_id.as_str()) {
                state.role = Some(new_role.clone());
                state.permissions = permissions.clone();
            }
        }
    }

    info!(device_id = %params.device_id, level = %level_lc, "Device level set");
    JsonRpcResponse::success(
        request.id,
        json!({
            "device_id": params.device_id,
            "level": level_lc,
            "permissions": permissions,
        }),
    )
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::devices`
Expected: PASS（5 个新测试 + 既有 `test_devices_list` 全绿）。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/auth/devices.rs
git commit -m "gateway: add devices.set_level handler with live connection role refresh"
```

---

### Task 4: 注册路由 + 导出 handler

让 `devices.set_level` 方法名路由到新 handler。

**Files:**
- Modify: `src/gateway/handlers/auth/mod.rs:42`（`pub use devices::{...}` 导出）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs`（`register_handler!` 一行）

- [ ] **Step 1: 导出新 handler**

`src/gateway/handlers/auth/mod.rs:42`，把：

```rust
pub use devices::{handle_devices_list, handle_devices_revoke};
```

改为：

```rust
pub use devices::{handle_devices_list, handle_devices_revoke, handle_devices_set_level};
```

- [ ] **Step 2: 注册路由**

`src/bin/aleph-server/commands/start/builder/handlers/auth.rs`，在 `"devices.revoke"` 的 `register_handler!` 块（约 line 57-62）之后加：

```rust
    register_handler!(
        server,
        "devices.set_level",
        auth_handlers::handle_devices_set_level,
        auth_ctx
    );
```

- [ ] **Step 3: 编译确认**

Run: `cargo check --bin aleph-server`
Expected: 编译通过（`handle_devices_set_level` 可解析、宏展开正确）。

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/auth/mod.rs src/bin/aleph-server/commands/start/builder/handlers/auth.rs
git commit -m "gateway: route devices.set_level to its handler"
```

---

### Task 5: 全量验证（fmt / clippy / 测试）

**Files:** 无（仅验证 + 可能的 fmt 提交）

- [ ] **Step 1: 格式化**

Run: `cargo fmt`
Expected: 改动文件被规范化，无其它文件被卷入。

- [ ] **Step 2: clippy（改动文件零警告）**

Run: `cargo clippy -p alephcore --lib 2>&1 | grep -A3 -iE "warning|error" | head -40`
Expected: 本任务改动的文件无新增警告。

- [ ] **Step 3: 相关测试全绿**

Run: `cargo test -p alephcore --lib gateway::handlers::auth && cargo test -p alephcore --lib method_authz`
Expected: 全 PASS。

- [ ] **Step 4: 全编译**

Run: `cargo check --all-targets`
Expected: 绿。

- [ ] **Step 5: Commit（若 fmt 有改动）**

```bash
git add -p
git commit -m "chore: rustfmt for devices.set_level"
```

（仅当 fmt 产生改动时执行；用 `git add -p` 或显式路径，避免卷入并发会话的 WIP。）

---

## 验证（对照 spec）

- **operator-only**：Task 2（method_authz）+ 既有 handler.rs 门控（非 operator 调 Operator 方法被硬拒）。
- **持久化**：Task 3 `update_permissions`，升/降级测试断言 `device_store` permissions。
- **立即生效（原地刷新活连接）**：Task 1 穿 connections + Task 3 写锁刷新；升/降级测试断言活连接 `role`/`permissions` 翻转，且非匹配连接不动。
- **非法 level / 未知设备 fail-loud**：Task 3 两个错误测试，且非法 level 测试断言不静默改库。
- **不碰 security_store**：plan 全程无 security_store 调用（spec 决策）。
- **不做「最后一个 operator」守卫**：本机 shared-token 恒为 operator，YAGNI（spec 决策）。

## 非本期（与 spec 一致）

- Phase 3b（Panel/Leptos）：Devices「授权配置/降级」钮消费 `devices.set_level`；配对卡双钮；sudo 等待态消费 Phase 2b `approval.requested`。
- 真 WS-level e2e（operator WS 连接 → `devices.set_level` → 断言 chat 设备活连接下一请求被门控）——与前序阶段一致，列为 follow-up；本期由单测覆盖 handler 逻辑。

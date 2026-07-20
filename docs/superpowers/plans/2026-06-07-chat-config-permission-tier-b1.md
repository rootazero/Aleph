# chat/config 权限分层 — Phase 1 (B1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让配对的设备分 chat / config 两档——chat 档（远程默认）以非 operator 身份接入，被现有 method-authz 门控硬拒于一切 config RPC 之外；config 档 = 今天的 operator 全权。

**Architecture:** 复用现有三件套——(1) `method_authz` 已把 config RPC 标为 Operator-only；(2) 派发循环 `handler.rs:649-664` 已从 connect 响应里的 `"role"` 字符串设 `ConnectionState.role`，`is_operator()` 据此判定；(3) `handler.rs:835-862` 已对非 operator 硬拒 Operator 方法。B1 只需：配对时按 level 给设备不同 `permissions`，并让 connect 响应的 `role` 字段由设备 `permissions` 派生（含 `"*"` → operator，否则 guest）。**不改 DeviceRole 枚举，不做 DB 迁移**——设备 `permissions` 即 chat/config 的事实源。

**Tech Stack:** Rust, `serde`, gateway WS JSON-RPC, `cargo test -p alephcore --lib`。

**Spec:** `docs/superpowers/specs/2026-06-07-chat-config-permission-tier-design.md`（本 plan 覆盖 §3 缺口 1 + §3 缺口 2 的 RPC 路径；缺口 2 的工具路径与 sudo 审批见 Phase 2；Panel UI 见 Phase 3）。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `src/gateway/handlers/auth/tier.rs` | chat/config 档 ↔ permissions/role 的纯映射 SSOT | Create |
| `src/gateway/handlers/auth/mod.rs` | `ConnectResult` 增 `role` 字段；挂 `mod tier` | Modify |
| `src/gateway/handlers/auth/pairing.rs` | `pairing.approve` 读 `level`，按档写设备 permissions | Modify |
| `src/gateway/handlers/auth/connect.rs` | 各 connect 分支输出派生的 `role` | Modify |

> 约束：单分支 main、并发提交者 —— 只追加式提交、显式路径暂存，禁 `git add -A`/reset/amend（见项目记忆）。每个 Task 末尾 `cargo test -p alephcore --lib` 通过后即提交。

---

## Task 1: tier 映射 SSOT（纯函数）

**Files:**
- Create: `src/gateway/handlers/auth/tier.rs`
- Modify: `src/gateway/handlers/auth/mod.rs`（加 `pub(crate) mod tier;`）

- [ ] **Step 1: 写失败测试**

新建 `src/gateway/handlers/auth/tier.rs`：

```rust
//! Pairing tier ↔ permissions/role mapping (single source of truth).
//!
//! A paired device is either the default **chat** tier (chat + read-only
//! dashboards, no Aleph-config rights) or the explicit **config** tier
//! (operator, full control plane). The tier is persisted purely as the
//! device's `permissions`: a `"*"` wildcard means config/operator, anything
//! else is chat. `role_for_permissions` derives the connect-response role
//! string that the dispatch loop feeds into the method-authz gate.

/// Wildcard permission that marks a full-access (operator/config) device.
pub const WILDCARD: &str = "*";

/// Permissions granted to a chat-tier device: converse + read.
pub const CHAT_PERMISSIONS: &[&str] = &["chat", "read"];

/// Pairing approval tier requested by the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Default: chat + read-only dashboards, no config rights.
    Chat,
    /// Operator: full control plane.
    Config,
}

impl Tier {
    /// Parse the `level` pairing param. Unknown / missing → `Chat` (safe default).
    pub fn from_level(level: Option<&str>) -> Self {
        match level {
            Some(s) if s.eq_ignore_ascii_case("config") => Tier::Config,
            _ => Tier::Chat,
        }
    }

    /// The permission set persisted for a device approved at this tier.
    pub fn permissions(self) -> Vec<String> {
        match self {
            Tier::Config => vec![WILDCARD.to_string()],
            Tier::Chat => CHAT_PERMISSIONS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Connect-response role string for a connection holding `permissions`.
/// `"operator"` iff the wildcard is present, else `"guest"` (chat tier).
/// This is the string the dispatch loop stores in `ConnectionState.role`
/// and `is_operator()` checks.
pub fn role_for_permissions(permissions: &[String]) -> &'static str {
    if permissions.iter().any(|p| p == WILDCARD) {
        "operator"
    } else {
        "guest"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_defaults_to_chat() {
        assert_eq!(Tier::from_level(None), Tier::Chat);
        assert_eq!(Tier::from_level(Some("")), Tier::Chat);
        assert_eq!(Tier::from_level(Some("bogus")), Tier::Chat);
        assert_eq!(Tier::from_level(Some("chat")), Tier::Chat);
    }

    #[test]
    fn level_config_is_explicit_and_case_insensitive() {
        assert_eq!(Tier::from_level(Some("config")), Tier::Config);
        assert_eq!(Tier::from_level(Some("CONFIG")), Tier::Config);
    }

    #[test]
    fn config_tier_is_wildcard_operator() {
        let perms = Tier::Config.permissions();
        assert_eq!(perms, vec!["*".to_string()]);
        assert_eq!(role_for_permissions(&perms), "operator");
    }

    #[test]
    fn chat_tier_is_non_operator() {
        let perms = Tier::Chat.permissions();
        assert_eq!(perms, vec!["chat".to_string(), "read".to_string()]);
        assert_eq!(role_for_permissions(&perms), "guest");
    }

    #[test]
    fn empty_permissions_are_non_operator() {
        assert_eq!(role_for_permissions(&[]), "guest");
    }
}
```

在 `src/gateway/handlers/auth/mod.rs` 顶部模块声明区加一行（紧邻现有 `mod` 声明）：

```rust
pub(crate) mod tier;
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::tier`
Expected: 编译失败（`tier` 模块刚建）或测试未被发现 → 加 `mod tier;` 后应能编译并 PASS。若 PASS 直接进 Step 4。

- [ ] **Step 3: （仅当 Step 2 因模块未挂而失败）确认 `mod tier;` 已加**

确认 `mod.rs` 含 `pub(crate) mod tier;`。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::tier`
Expected: 5 passed。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/handlers/auth/tier.rs src/gateway/handlers/auth/mod.rs
git commit -m "gateway: add chat/config pairing tier mapping (SSOT)"
```

---

## Task 2: `ConnectResult` 增 `role` 字段

派发循环已读 `resp.result["role"]`（`handler.rs:657`），但 `ConnectResult` 结构当前**没有** `role` 字段，故设备路径目前永远落到默认 `"operator"`。本任务加该字段并在所有构造点显式赋值。

**Files:**
- Modify: `src/gateway/handlers/auth/mod.rs`（`struct ConnectResult`，约 123-145 行）
- Modify: `src/gateway/handlers/auth/connect.rs`（多处 `ConnectResult { ... }` 构造）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/auth/mod.rs` 的 `#[cfg(test)] mod tests`（若无则在文件末尾新建）加：

```rust
#[cfg(test)]
mod connect_result_role_tests {
    use super::*;

    #[test]
    fn connect_result_serializes_role() {
        let r = ConnectResult {
            token: "t".into(),
            device_id: "d".into(),
            permissions: vec!["*".into()],
            role: "operator".into(),
            expires_at: "2026-01-01T00:00:00Z".into(),
            state_version: Default::default(),
            transport: Default::default(),
            hello: Default::default(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v.get("role").and_then(|x| x.as_str()), Some("operator"));
    }
}
```

> 注：若 `StateVersion` / `TransportPolicy` / `HelloSnapshot` 未实现 `Default`，改用各自现有的构造器（在同文件 `use` 区可见其类型）；测试目的只是断言 `role` 进入 JSON。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::mod::connect_result_role_tests`
Expected: 编译失败 —— `ConnectResult` 无 `role` 字段。

- [ ] **Step 3: 加字段并修所有构造点**

在 `struct ConnectResult` 的 `device_id` 之后插入：

```rust
    /// Connection role this device authenticated as: `"operator"` (config
    /// tier, full control plane) or `"guest"` (chat tier). The dispatch loop
    /// copies this into `ConnectionState.role`, which `is_operator()` and the
    /// method-authz gate consult. Derived from `permissions` via
    /// `tier::role_for_permissions`.
    pub role: String,
```

修 `connect.rs` 每个 `ConnectResult { ... }` 构造（grep 定位：`rg -n "ConnectResult \{" src/gateway/handlers/auth/connect.rs`）：

- **共享 token / bootstrap / 全权 operator 分支**（permissions 恒 `["*"]` 的那些，如约 219/297 附近的成功返回）：加 `role: "operator".to_string(),`。
- **Case 1 token 路径**（约 380，`permissions: validation.scopes`）：先取 `let role = crate::gateway::handlers::auth::tier::role_for_permissions(&validation.scopes).to_string();`（在构造前），再在结构体里 `role,`。注意 `validation.scopes` 被 move 进 `permissions`，故先算 role 或对 permissions 取引用计算后再 move。推荐：

```rust
let permissions = validation.scopes;
let role = crate::gateway::handlers::auth::tier::role_for_permissions(&permissions).to_string();
// ... ConnectResult { permissions, role, ... }
```

- **Case 2 已批准设备**（约 414-445，`permissions = device...permissions`）：在 `permissions` 算出后加
  `let role = super::tier::role_for_permissions(&permissions).to_string();`，结构体里 `role,`。

- [ ] **Step 4: 运行测试 + 全量编译**

Run: `cargo test -p alephcore --lib gateway::handlers::auth`
Expected: 全 PASS，无 “missing field `role`” 编译错误。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/handlers/auth/mod.rs src/gateway/handlers/auth/connect.rs
git commit -m "gateway: emit connection role in connect result, derived from permissions"
```

---

## Task 3: `pairing.approve` 读 `level`，按档写设备

**Files:**
- Modify: `src/gateway/handlers/auth/pairing.rs`（`ApproveParams`、Device 分支约 145-205）

- [ ] **Step 1: 写失败测试**

在 `pairing.rs` 的测试模块加（若无测试模块则在文件末尾建 `#[cfg(test)] mod tests`）：

```rust
#[cfg(test)]
mod tier_param_tests {
    use super::*;

    #[test]
    fn approve_params_defaults_level_to_none() {
        let req = serde_json::json!({ "code": "ABCD1234" });
        let p: ApproveParams = serde_json::from_value(req).unwrap();
        assert_eq!(p.code, "ABCD1234");
        assert_eq!(p.level, None);
    }

    #[test]
    fn approve_params_parses_config_level() {
        let req = serde_json::json!({ "code": "X", "level": "config" });
        let p: ApproveParams = serde_json::from_value(req).unwrap();
        assert_eq!(p.level.as_deref(), Some("config"));
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::pairing::tier_param_tests`
Expected: 编译失败 —— `ApproveParams` 无 `level` 字段。

- [ ] **Step 3: 加 `level` 字段并按档落库**

`ApproveParams` 改为：

```rust
    #[derive(Debug, Deserialize)]
    struct ApproveParams {
        code: String,
        /// `"config"` → operator (full control). Anything else / missing →
        /// chat tier (safe default): chat + read, no Aleph-config rights.
        #[serde(default)]
        level: Option<String>,
    }
```

在 Device 分支拿到 `params` 后、生成 device 前，算出该档的 permissions：

```rust
    let tier = super::tier::Tier::from_level(params.level.as_deref());
    let tier_permissions = tier.permissions();
```

把 Device 分支里三处硬编码全权改为按档：

1. `ApprovedDevice::new(...)` 之后，设其 permissions = `tier_permissions.clone()`（若 `ApprovedDevice::new` 默认 `["*"]`，新增一行覆盖：`device.permissions = tier_permissions.clone();` —— 确认 `permissions` 字段可写；`device_store::Device.permissions` 为 `pub`）。
2. `upsert_device` 的 `role` / `scopes`：

```rust
        role: super::tier::role_for_permissions(&tier_permissions),
        scopes: &tier_permissions,
```

3. `issue_token`：B1 保持 `DeviceRole::Operator`（token 角色仅影响 memory namespace，门控由 connect 响应的 `role` 派生；namespace 分层留待 Phase 2），但 scopes 传按档值：

```rust
    let signed_token = match ctx.token_manager.issue_token(
        &device_id,
        DeviceRole::Operator,
        tier_permissions.clone(),
    ) {
```

> 校验点：`role_for_permissions` 返回 `&'static str`，`DeviceUpsertData.role` 形参为 `&str` —— 直接传。`scopes: &tier_permissions` 形参为 `&[String]` —— `&Vec<String>` 自动解引用匹配。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::handlers::auth::pairing`
Expected: 全 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/handlers/auth/pairing.rs
git commit -m "gateway: pairing.approve honors chat/config level (default chat)"
```

---

## Task 4: 端到端门控测试（chat 档被拒、config 档放行）

复用现有 `src/gateway/auth_probe_tests.rs` 的测试夹具（已有 `PairingManager`、`enable_pairing`、`pairing_required` 等用例）。

**Files:**
- Modify: `src/gateway/auth_probe_tests.rs`（在文件内既有夹具旁加用例）

- [ ] **Step 1: 写测试**

在 `auth_probe_tests.rs` 末尾（沿用文件顶部已有的 imports / 夹具构造器）加：

```rust
#[test]
fn chat_tier_permissions_yield_non_operator_role() {
    use crate::gateway::handlers::auth::tier::{role_for_permissions, Tier};
    let chat = Tier::Chat.permissions();
    assert_eq!(role_for_permissions(&chat), "guest");
    let cfg = Tier::Config.permissions();
    assert_eq!(role_for_permissions(&cfg), "operator");
}

#[test]
fn operator_only_method_blocks_chat_role() {
    use crate::gateway::method_authz::{required_privilege, MethodPrivilege};
    // The gate predicate the dispatch loop applies: an Operator method + a
    // non-operator connection ⇒ denied. (handler.rs:835-862)
    let method = "config.apply";
    let is_operator = role_is_operator("guest");
    let denied = required_privilege(method) == MethodPrivilege::Operator && !is_operator;
    assert!(denied, "chat-tier connection must be denied config.apply");

    // Same method, operator connection ⇒ allowed.
    let denied_for_op =
        required_privilege(method) == MethodPrivilege::Operator && !role_is_operator("operator");
    assert!(!denied_for_op, "config-tier connection must pass config.apply");
}

/// Mirror of `ConnectionState::is_operator` for the unit check above.
fn role_is_operator(role: &str) -> bool {
    role == "operator"
}
```

> 这是对"分类器 × 角色 ⇒ 放行/拒绝"判定的纯逻辑锁定，不需起真实 WS。完整 WS 级 e2e（真连真拒）留给 `/e2e-verify` 在 Phase 2 落地后跑（那时 sudo 审批路径才完整）。

- [ ] **Step 2: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib gateway::auth_probe_tests`
Expected: 新增 2 用例 PASS，原有用例不回归。

- [ ] **Step 3: 全量校验**

Run: `cargo test -p alephcore --lib gateway::handlers::auth gateway::method_authz gateway::auth_probe_tests`
Expected: 全 PASS。

Run: `cargo clippy -p alephcore --lib 2>&1 | rg "auth/tier|auth/pairing|auth/connect|auth/mod" || echo "no clippy warnings in touched files"`
Expected: 无新警告。

- [ ] **Step 4: 提交**

```bash
git add src/gateway/auth_probe_tests.rs
git commit -m "gateway: lock chat-tier denial / config-tier pass of operator methods"
```

---

## Self-Review（已执行）

- **Spec 覆盖**：缺口 1（配对授级）= Task 3；缺口 2 的 **RPC 路径**（chat 档被现有 method-authz 硬拒）= Task 2+4 的角色派生使现有门控对 chat 生效；安全默认 chat = Task 1/3。缺口 2 的**工具路径** + **sudo 审批** = **Phase 2（另立 plan）**；Panel UI = **Phase 3（另立 plan）**。
- **占位符扫描**：无 TBD/TODO；每个代码步给出真实代码。Task 2 Step 3 含按既有构造点定位指引（`rg`）而非伪代码。
- **类型一致**：`Tier`/`role_for_permissions`/`permissions()`/`CHAT_PERMISSIONS` 在 Task 1 定义，Task 2/3/4 引用名一致；`ConnectResult.role: String` 全程一致。

---

## Phase 2 & 3 — 后续独立 plan（本 plan 不含实现，待 B1 落地后立计划）

> 拆分理由（writing-plans 范围检查）：二者各自是可独立测试的增量，且其集成接缝（工具运行期角色传播、RPC 层审批 requester、Panel Leptos 组件）需在 B1 落地、相关代码被实读后才能写成无占位符的精确步骤。

### Phase 2 — 工具路径门控 + sudo 现场审批（核心）
- **入口**：`src/tools/scoped/dispatch.rs`（`request_approval` 已存在）、`src/tools/scoped/mod.rs`（`execute_with_cancel` 已 scope `TURN_CONTEXT`/`SESSION_ID` —— 在此注入发起连接的 role/permissions）、`src/gateway/server/handler.rs:835-862`（把 RPC 硬拒改为：非 operator 命中 Operator 方法 → 触发审批 requester 而非直接 `PERMISSION_DENIED`）。
- **SSOT 扩展**：把 `method_authz` 的 OPERATOR 集合抽成同时认 RPC 名与 config 工具名的分类器（spec §3 缺口 2）。
- **产出**：chat 档对话改配置（LLM 调 config 工具）被拦截 → operator 端 sudo 审批 → 批准放行/可记住提升、拒绝回 `permission_required`。

### Phase 3 — Panel UI（Leptos）
- **入口**：配对审批卡组件（`interfaces/webchat/` 内 pairing/notification 相关组件）、Devices 管理页。
- **产出**：配对卡"批准为 chat / 批准为 config"双钮；Devices"授权配置/降级"钮；sudo"等待 server 授权…"态。

---

## Execution Handoff

Plan complete（Phase 1）。两种执行方式：

1. **Subagent-Driven（推荐）** — 每 Task 派新 subagent，Task 间双阶段审查，快速迭代。
2. **Inline Execution** — 本会话内按 executing-plans 分批执行 + 检查点。

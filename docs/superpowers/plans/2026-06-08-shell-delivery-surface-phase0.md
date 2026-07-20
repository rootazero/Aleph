# 壳投递面 Phase 0：命名身份 + 权限源头 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给每个壳连接一个具名 `channel_kind` 身份，并把今天隐式埋在 loopback 操作员授权里的默认档位规则抽成具名 SSOT 函数 `default_tier(channel_kind, is_loopback)`，本机零回归。

**Architecture:** 纯增量。新增 `SurfaceKind` 枚举（identity，与既有 `ChannelClass` lane-priority 概念**不同**，勿混淆）落进 `ConnectionState` 新具名字段；客户端经 `connect` 握手声明 `channel_kind`（缺省按 loopback 推断）。新增 `default_tier` 落在 Spec B 的 tier SSOT（`tier.rs`），并把它接到两条 loopback 操作员授权分支（shared-token / no-auth）作为真实消费者——`default_tier(_, true) == Tier::Config == vec!["*"]`，与今天字节等价，由现有 connect 测试守护。不重建 device+tier 权限引擎。

**Tech Stack:** Rust, `cargo test -p alephcore`, serde（连接参数反序列化）。

**Spec:** `docs/superpowers/specs/2026-06-08-shell-delivery-surface-design.md`（Phase 0）。

**关键事实（实现前必读）：**
- `ConnectionState`（`src/gateway/server/mod.rs:43`）只经 `ConnectionState::new(client_ip)`（同文件 `:88`）构造——4 处调用全走 `new()`，**无字面量构造**。加字段只需改 struct + `new()` 默认值。
- `ConnectParams`（`src/gateway/handlers/auth/mod.rs:64`）有 `device_type`，但它在 connect.rs **被丢弃**（`let _device_type` at `:480`）。`channel_kind` 是**新概念**，与 `device_type` 无关。
- tier SSOT = `src/gateway/handlers/auth/tier.rs`（`Tier`/`from_level`/`permissions`/`role_for_permissions`）。
- 两条 loopback 操作员授权分支：shared-token（`connect.rs:204`，授 `vec!["*"]`+`"operator"`）和 no-auth（`connect.rs:287` 起，同样授 `vec!["*"]`+`"operator"`）。
- 连接 role 应用点：`src/gateway/server/handler.rs:655-672`（`state.authenticate(...)`）。这里也设 `channel_kind`。
- 并发 main 纪律：单分支 main，**只追加提交**（禁 reset/amend/rebase），**显式路径暂存**（勿 `git add -A`，别的会话有 WIP）。每个 Task 前先 `git status`。

---

## File Structure

- **Create** `src/gateway/surface/mod.rs` — `SurfaceKind` 枚举 + 解析（identity SSOT，Phase 1 的 `DeliverySurface` trait 将来同住此模块）。
- **Modify** `src/gateway/mod.rs` — 注册 `pub mod surface;`。
- **Modify** `src/gateway/handlers/auth/tier.rs` — 新增 `default_tier(channel_kind, is_loopback)`。
- **Modify** `src/gateway/server/mod.rs` — `ConnectionState` 加 `channel_kind` 字段 + `new()` 默认。
- **Modify** `src/gateway/handlers/auth/mod.rs` — `ConnectParams` 加 `channel_kind: Option<String>`。
- **Modify** `src/gateway/server/handler.rs` — 在 role 应用点设 `state.channel_kind`。
- **Modify** `src/gateway/handlers/auth/connect.rs` — shared-token + no-auth 两分支经 `default_tier` 授权。

---

## Task 1: `SurfaceKind` 身份枚举

**Files:**
- Create: `src/gateway/surface/mod.rs`
- Modify: `src/gateway/mod.rs`（在 `pub mod server;` 一带加 `pub mod surface;`）

- [ ] **Step 1: 写新模块（含失败测试）**

Create `src/gateway/surface/mod.rs`:

```rust
//! Surface identity — what *kind* of I/O surface a connection is.
//!
//! This is distinct from `ChannelClass` (lane priority, see
//! `server::handler`): `SurfaceKind` names the attachment for tiering and
//! delivery routing, not scheduling. Phase 1's `DeliverySurface` trait will
//! live alongside this enum.

/// The kind of I/O surface a gateway connection presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Native desktop shell (Tauri).
    Desktop,
    /// Web Panel running in a browser.
    Browser,
    /// Command-line client.
    Cli,
    /// Unspecified or legacy client that declared no kind.
    Unknown,
}

impl SurfaceKind {
    /// Parse a client-declared `channel_kind` string (case-insensitive,
    /// trimmed). Anything unrecognised or absent → `Unknown` (fail-safe: an
    /// unknown surface gets no special treatment).
    pub fn from_opt_str(s: Option<&str>) -> Self {
        match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("desktop") => SurfaceKind::Desktop,
            Some("browser") => SurfaceKind::Browser,
            Some("cli") => SurfaceKind::Cli,
            _ => SurfaceKind::Unknown,
        }
    }

    /// Stable wire/log string for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            SurfaceKind::Desktop => "desktop",
            SurfaceKind::Browser => "browser",
            SurfaceKind::Cli => "cli",
            SurfaceKind::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_kinds_case_insensitively() {
        assert_eq!(SurfaceKind::from_opt_str(Some("desktop")), SurfaceKind::Desktop);
        assert_eq!(SurfaceKind::from_opt_str(Some("  Browser ")), SurfaceKind::Browser);
        assert_eq!(SurfaceKind::from_opt_str(Some("CLI")), SurfaceKind::Cli);
    }

    #[test]
    fn unknown_and_absent_map_to_unknown() {
        assert_eq!(SurfaceKind::from_opt_str(None), SurfaceKind::Unknown);
        assert_eq!(SurfaceKind::from_opt_str(Some("")), SurfaceKind::Unknown);
        assert_eq!(SurfaceKind::from_opt_str(Some("telegram")), SurfaceKind::Unknown);
    }

    #[test]
    fn as_str_round_trips() {
        for k in [SurfaceKind::Desktop, SurfaceKind::Browser, SurfaceKind::Cli, SurfaceKind::Unknown] {
            assert_eq!(SurfaceKind::from_opt_str(Some(k.as_str())), k);
        }
    }
}
```

Then register the module — in `src/gateway/mod.rs`, add next to the other `pub mod` lines (e.g. right after `pub mod server;` at line 48):

```rust
pub mod surface;
```

- [ ] **Step 2: 运行测试，确认通过**

Run: `cargo test -p alephcore gateway::surface -- --nocapture`
Expected: 3 tests PASS。若编译失败先修 `mod` 注册。

- [ ] **Step 3: 提交**

```bash
git add src/gateway/surface/mod.rs src/gateway/mod.rs
git commit -m "gateway: add SurfaceKind connection identity enum"
```

---

## Task 2: `default_tier` 具名 SSOT 函数

**Files:**
- Modify: `src/gateway/handlers/auth/tier.rs`（在 `tier_for_permissions` 之后、`#[cfg(test)]` 之前加函数；测试加进既有 `mod tests`）

- [ ] **Step 1: 写失败测试**

在 `tier.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn loopback_attach_is_config_operator_for_every_kind() {
        use crate::gateway::surface::SurfaceKind;
        for k in [SurfaceKind::Desktop, SurfaceKind::Browser, SurfaceKind::Cli, SurfaceKind::Unknown] {
            assert_eq!(default_tier(k, true), Tier::Config);
            // Byte-equivalent to the legacy `vec!["*"]` loopback operator grant.
            assert_eq!(default_tier(k, true).permissions(), vec!["*".to_string()]);
            assert_eq!(role_for_permissions(&default_tier(k, true).permissions()), "operator");
        }
    }

    #[test]
    fn remote_attach_defaults_to_chat_for_every_kind() {
        use crate::gateway::surface::SurfaceKind;
        for k in [SurfaceKind::Desktop, SurfaceKind::Browser, SurfaceKind::Cli, SurfaceKind::Unknown] {
            assert_eq!(default_tier(k, false), Tier::Chat);
            assert_eq!(role_for_permissions(&default_tier(k, false).permissions()), "guest");
        }
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p alephcore gateway::handlers::auth::tier -- --nocapture`
Expected: FAIL — `cannot find function default_tier in this scope`。

- [ ] **Step 3: 写实现**

在 `tier.rs` 中 `tier_for_permissions(...)` 函数之后插入：

```rust
/// Default permission tier for a freshly-attached connection, as a function of
/// its surface identity and whether it attaches over loopback (same machine).
///
/// SSOT for the rule that used to live implicitly in the shared-token / no-auth
/// operator grants: a same-machine attach is operator (the OS boundary is the
/// trust boundary); a remote attach defaults to chat and must be raised
/// explicitly (Spec B `devices.set_level`).
///
/// `channel_kind` is carried for identity and future per-kind refinement; today
/// the tier is determined solely by the attach boundary, so every kind maps the
/// same way. Keeping it in the signature makes the rule's shape explicit and
/// gives later phases one place to specialise.
pub fn default_tier(channel_kind: crate::gateway::surface::SurfaceKind, is_loopback: bool) -> Tier {
    let _ = channel_kind; // carried for identity; not yet a tier input (see doc).
    match is_loopback {
        // Same-machine attach: operator. Byte-equivalent to the legacy
        // `vec!["*"]` grant on the shared-token / no-auth loopback paths.
        true => Tier::Config,
        // Remote attach: chat by default (matches pairing's `Tier::from_level`
        // default; an operator raises it via `devices.set_level`).
        false => Tier::Chat,
    }
}
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore gateway::handlers::auth::tier -- --nocapture`
Expected: 全部 PASS（含既有 tier 测试 + 2 个新测试）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/handlers/auth/tier.rs
git commit -m "gateway: add default_tier(channel_kind, is_loopback) SSOT"
```

---

## Task 3: `channel_kind` 落进连接身份

**Files:**
- Modify: `src/gateway/server/mod.rs`（`ConnectionState` struct + `new()`）
- Modify: `src/gateway/handlers/auth/mod.rs`（`ConnectParams`）
- Modify: `src/gateway/server/handler.rs`（role 应用点设 `channel_kind`）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/server/mod.rs` 末尾的测试模块（若无则新建 `#[cfg(test)] mod channel_kind_tests`）追加：

```rust
#[cfg(test)]
mod channel_kind_tests {
    use super::*;
    use crate::gateway::surface::SurfaceKind;

    #[test]
    fn new_connection_has_no_channel_kind() {
        let cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        assert_eq!(cs.channel_kind, None);
    }

    #[test]
    fn channel_kind_is_settable() {
        let mut cs = ConnectionState::new("127.0.0.1".parse().unwrap());
        cs.channel_kind = Some(SurfaceKind::Desktop);
        assert_eq!(cs.channel_kind, Some(SurfaceKind::Desktop));
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p alephcore gateway::server::channel_kind_tests -- --nocapture`
Expected: FAIL — `no field channel_kind on type ConnectionState`。

- [ ] **Step 3: 加字段 + 默认值**

在 `src/gateway/server/mod.rs` 的 `ConnectionState` struct（`client_ip` 字段之后、闭合 `}` 之前）加：

```rust
    /// Surface identity declared by the client on `connect` (or inferred from
    /// loopback when undeclared). Names *what kind of shell* this connection is
    /// — desktop / browser / cli — for tiering and (Phase 1+) delivery routing.
    /// Distinct from `ChannelClass` (lane priority). `None` before connect or
    /// for legacy clients that declared nothing.
    pub channel_kind: Option<crate::gateway::surface::SurfaceKind>,
```

在 `ConnectionState::new(...)` 的 `Self { ... }` 字面量（`client_ip,` 之后）加：

```rust
            channel_kind: None,
```

- [ ] **Step 4: `ConnectParams` 加字段**

在 `src/gateway/handlers/auth/mod.rs` 的 `ConnectParams` struct（`device_id` 字段之后）加：

```rust
    /// Surface identity the client declares: `"desktop"`, `"browser"`, `"cli"`.
    /// Absent/unknown → treated as `SurfaceKind::Unknown` and the tier falls
    /// back to loopback inference. Purely identity — does not grant anything.
    #[serde(default)]
    pub channel_kind: Option<String>,
```

然后修 `connect.rs` 里 `ConnectParams { ... }` 的两处**默认构造**（`:34` 的无参分支 与 `:215`/`:298` 内部构造——`grep -n "device_id: None," src/gateway/handlers/auth/connect.rs` 定位），各加一行 `channel_kind: None,`，否则编译报缺字段。

- [ ] **Step 5: 在 role 应用点设 `channel_kind`**

在 `src/gateway/server/handler.rs:655-672` 区块，`state.authenticate(device_id.clone(), permissions, role);` 之后、`state.first_message = false;` 之前插入：

```rust
                                                    // Record the surface identity: client-declared kind, else
                                                    // inferred from loopback (same-machine attach ⇒ desktop-class).
                                                    let declared = req
                                                        .params
                                                        .as_ref()
                                                        .and_then(|p| p.get("channel_kind"))
                                                        .and_then(|v| v.as_str());
                                                    let kind = match crate::gateway::surface::SurfaceKind::from_opt_str(declared) {
                                                        crate::gateway::surface::SurfaceKind::Unknown if ctx.client_ip.is_loopback() => {
                                                            crate::gateway::surface::SurfaceKind::Desktop
                                                        }
                                                        other => other,
                                                    };
                                                    state.channel_kind = Some(kind);
```

> 注：`req` 是当前 connect 分支已匹配的 `JsonRpcRequest`，在此作用域内可见；`ctx.client_ip` 是已解析的真实客户端 IP（`ConnectionContext` 字段）。若变量名在你这版略有差异，用 `grep -n "state.authenticate(" src/gateway/server/handler.rs` 锚定后就近取同作用域的 `req` / `ctx`。

- [ ] **Step 6: 运行测试 + 全量编译，确认通过**

Run: `cargo test -p alephcore gateway::server::channel_kind_tests -- --nocapture`
Expected: 2 tests PASS。
Run: `cargo check -p alephcore --all-targets`
Expected: 编译通过（确认无遗漏的 `ConnectParams` 构造点缺字段）。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/server/mod.rs src/gateway/handlers/auth/mod.rs src/gateway/handlers/auth/connect.rs src/gateway/server/handler.rs
git commit -m "gateway: thread channel_kind surface identity onto ConnectionState"
```

---

## Task 4: 两条 loopback 操作员授权分支经 `default_tier`（真实消费者，零回归）

**Files:**
- Modify: `src/gateway/handlers/auth/connect.rs`（shared-token 分支 `:204`、no-auth 分支 `:287`）

这一步把硬编码的 `vec!["*"]` + `"operator"` 换成 `default_tier(kind, true)` 派生——`Tier::Config.permissions() == vec!["*"]`、`role_for_permissions == "operator"`，**字节等价**。两分支只在 loopback bootstrap / 本地模式可达，故 `is_loopback=true` 是常量且名副其实。

- [ ] **Step 1: 确认现有 connect 测试断言 operator（守护等价）**

Run: `cargo test -p alephcore gateway::handlers::auth::connect -- --nocapture`
Expected: 既有测试（`test_connect_with_shared_token` 等）PASS。先记录绿，作为回归基线。

- [ ] **Step 2: 给 shared-token 分支加等价断言（若缺）**

在 `connect.rs` 的 `mod tests` 里找到 `test_connect_with_shared_token`，确认其断言响应 `permissions == ["*"]` 且 `role == "operator"`。若只断言 success，补：

```rust
        let result = response.result.expect("shared-token connect should succeed");
        assert_eq!(result.get("role").and_then(|v| v.as_str()), Some("operator"));
        assert_eq!(
            result.get("permissions").and_then(|v| v.as_array()).map(|a| a.len()),
            Some(1)
        );
        assert_eq!(
            result.get("permissions")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str()),
            Some("*")
        );
```

Run: `cargo test -p alephcore gateway::handlers::auth::connect::tests::test_connect_with_shared_token -- --nocapture`
Expected: PASS（断言当前硬编码行为）。

- [ ] **Step 3: 改写 shared-token 分支用 `default_tier`**

在 `connect.rs:204` 的 `if let Some(ref shared_token) = params.shared_token {` → `Ok(true) => {` 块内，`let device_name = ...;` 之后插入：

```rust
                // Tier source (SSOT): a shared-token connect is the loopback
                // bootstrap path → operator. Byte-equivalent to the previous
                // hardcoded `vec!["*"]` / `"operator"`.
                let kind = crate::gateway::surface::SurfaceKind::from_opt_str(
                    params.channel_kind.as_deref(),
                );
                let perms = super::tier::default_tier(kind, true).permissions();
                let role = super::tier::role_for_permissions(&perms).to_string();
```

然后在同块内：
- `upsert_device` 的 `scopes: &["*".to_string()],` 改为 `scopes: &perms,`
- `issue_token(&device_id, DeviceRole::Operator, vec!["*".to_string()])` 的最后一参改为 `perms.clone()`
- `ConnectResult { ... permissions: vec!["*".to_string()], role: "operator".to_string(), ... }` 改为 `permissions: perms, role,`

（`DeviceRole::Operator` 保留——它对应 `Tier::Config`。）

- [ ] **Step 4: 改写 no-auth 分支用 `default_tier`**

在 `connect.rs:287` 起的 `if !ctx.auth_mode.is_auth_required() {` 块内，`let device_name = ...;` 之后插入相同的三行：

```rust
        let kind = crate::gateway::surface::SurfaceKind::from_opt_str(
            params.channel_kind.as_deref(),
        );
        let perms = super::tier::default_tier(kind, true).permissions();
        let role = super::tier::role_for_permissions(&perms).to_string();
```

同样把该块内 `scopes: &["*".to_string()]` → `scopes: &perms`、`issue_token(..., vec!["*".to_string()])` → `perms.clone()`、`ConnectResult { permissions: vec!["*".to_string()], role: "operator".to_string(), ... }` → `permissions: perms, role,`。

> 注：`params.channel_kind` 在 no-auth 分支若已被 move（前面用过 `params.device_id` 等），用 `params.channel_kind.as_deref()` 取引用即可，不会 move `params`。若编译器报 `params` partial-move 冲突，把 `kind` 的计算上移到该块第一个使用 `params` 字段之前。

- [ ] **Step 5: 运行 connect 测试 + 全量，确认零回归**

Run: `cargo test -p alephcore gateway::handlers::auth::connect -- --nocapture`
Expected: 全部 PASS（含 Step 2 的等价断言——证明改写后 shared-token 仍授 `["*"]`/operator）。
Run: `cargo check -p alephcore --all-targets`
Expected: 编译通过。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/handlers/auth/connect.rs
git commit -m "gateway: route loopback operator grants through default_tier (zero-regression)"
```

---

## 收尾验证

- [ ] **Step 1: 全量编译 + 相关测试**

Run: `cargo check -p alephcore --all-targets`
Run: `cargo test -p alephcore gateway::surface gateway::handlers::auth::tier gateway::handlers::auth::connect gateway::server -- --nocapture`
Expected: 全绿。

- [ ] **Step 2: clippy（本任务文件零新警告）**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | grep -E "surface|tier|connect|server/mod|server/handler" | head`
Expected: 无输出（本计划改动文件无新 clippy 警告）。

- [ ] **Step 3: 本机零回归人工核对**

确认：本机 desktop shell（loopback shared-token / no-auth）连上后 role 仍为 `operator`、permissions 仍为 `["*"]`；旧客户端（不发 `channel_kind`）行为不变（`channel_kind` 经 loopback 推断为 `Desktop`，tier 仍 Config）。

---

## Self-Review（写完已核对）

- **Spec 覆盖**：Phase 0 两项验收均有任务——「连接身份带具名 kind」=Task 1+3；「默认档位由具名函数决定」=Task 2+4；「本机零回归」=Task 4 等价断言 + 收尾人工核对。Phase 1/2（DeliverySurface、R5 推送、审批回投）刻意不在本计划内。
- **占位符**：无 TBD/TODO；每个改码步骤含完整代码。
- **类型一致**：`SurfaceKind`（Task 1）→ `default_tier(SurfaceKind, bool)`（Task 2）→ `ConnectionState.channel_kind: Option<SurfaceKind>`（Task 3）→ connect.rs 经 `from_opt_str` 取 `SurfaceKind`（Task 4），签名贯穿一致。`Tier::Config.permissions() == vec!["*"]` 与既有 `tier.rs` 实现一致。
- **已知取舍（诚实记录）**：`default_tier` 的 `channel_kind` 参数本期不影响 tier 结果（tier 只由 attach boundary 决定）——保留在签名里是为 identity 显式化 + Phase 1/2 特化口子，用 `let _ = channel_kind;` 显式标记，非遗漏。生产中本期 `is_loopback` 实参恒为 `true`（两消费分支都是 loopback 授权）；`false`/Chat 臂由 pairing 既有路径覆盖、并由纯函数单测守护，Phase 1 起获得更多生产消费者。

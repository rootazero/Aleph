# LAN-Trust 架构回退 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除设备认证/配对/token 全套（~13k 行），desktop/shell 重写为单 crate 双变体（完整版内嵌 server / 纯壳版连远程），发行矩阵扩为三产物（完整 App / 纯壳 App / server 裸二进制 + install.sh）。

**Architecture:** 信任模型从"设备配对 + token"简化为"网络边界即信任边界"（默认 loopback，`host = "0.0.0.0"` 显式开 LAN）。`AuthMode::None`（免认证模式）**已存在且全链路接通**——策略是先把默认值翻成 None 拿到行为里程碑，再自上而下删除 Token 机器（方法注册 → handlers → 中间件 → 存储 → 配置 → CLI → 测试），每个 task 编译绿。唯一保留的护栏是 OriginPolicy（已有，挡跨源 WS/DNS rebinding），只加 `allow_any_origin` 逃生口。

**Tech Stack:** Rust workspace（alephcore + aleph-cli + aleph-shell + aleph-panel WASM）、Tauri 2、Leptos、GitHub Actions。

**Spec:** `docs/superpowers/specs/2026-06-12-lan-trust-architecture-revert-design.md`

---

## 全局约定

- 仓库根：`/Volumes/TBU4/Workspace/Aleph`，单分支 main 直接开发
- 每个 task 结束必须 `cargo check -p alephcore` 绿（涉及别的 crate 时检查对应 crate）
- 共享 target-dir 有 flock 串行化，编译排队是预期，**严禁设置独立 `CARGO_TARGET_DIR`**
- 提交格式 `<scope>: <description>`（英文），不加 attribution
- **删除策略**：自上而下（先删注册/挂载点，再让编译器指出死代码）。每删一层跑 `cargo check`，把编译器报的 unused import/dead code 一并清掉
- panel 改动必须验证 wasm target（native check 过 ≠ wasm 过）；shell 改动双 feature 矩阵都要 check
- ⚠️ `src/gateway/security/crypto.rs` 被 `src/secrets/vault.rs`、飞书 webhook、WhatsApp vault_store 使用，**不可删除**

## 关键现状坐标（探索已核实）

| 事实 | 位置 |
|------|------|
| `AuthMode { Token(default), None }` + `AuthConfig { mode, session_expiry_hours, token_expiry_hours, allowed_origins }` | `src/gateway/config.rs:250-291` |
| connect 的免认证分支 | `src/gateway/handlers/auth/connect.rs:291`（`if !ctx.auth_mode.is_auth_required()`） |
| auth 族方法注册（与 cluster/secrets/environments 混在同一文件） | `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` |
| auth HTTP 路由挂载（`/auth/*`、`/pair`、`/rpc`） | `src/gateway/server/mod.rs:362-367,548-549,676-680`（`auth_routes` 字段 + `set_auth_routes`） |
| WS 首消息 connect 门 + 失败计数 | `src/gateway/server/handler.rs:692-748`（`MAX_AUTH_ATTEMPTS` 定义在 `server/mod.rs:39`） |
| method_authz 调用点 | `src/gateway/server/handler.rs:1028-1047`；`caller_identity::CALLER_ROLE` 在 1125/1174 |
| ConnectionState auth 字段 | `src/gateway/server/mod.rs:44-128`（authenticated/auth_attempts/role/token_hash + authenticate()/is_operator()） |
| OriginPolicy（语义已满足 spec：无 Origin/loopback/tauri:/同源/allowlist） | `src/gateway/origin_policy.rs`（200 行，`is_allowed(origin, host)`） |
| Panel auth 符号所在文件 | `interfaces/webchat/src/{context.rs, app.rs, state/connection.rs, components/boot_check_gate.rs, views/pairing_modal.rs}` |
| mDNS 广播 `_aleph._tcp.local.`（mdns-sd crate） | `src/gateway/mdns_broadcaster.rs:57` |
| 发版 workflow（matrix 构建 server → 拷入 externalBin → tauri build → 上传 bundle） | `.github/workflows/aleph-app-release.yml`（desktop-app job :26，上传段 :96-131） |
| shell 模块清单 | `desktop/shell/src/main.rs:13-24`（connection/daemon/deeplink/external_link/hotkey/menu/notify/perm_monitor/tray/update/webview_perms） |

---

### Task 1: 行为先行——默认 AuthMode 翻成 None

**Files:**
- Modify: `src/gateway/config.rs:250-262`（AuthMode enum 默认值）
- Delete: `src/gateway/auth_probe_tests.rs`（1043 行，测的是即将整体移除的双模式矩阵）
- Modify: `src/gateway/mod.rs`（移除 `auth_probe_tests` module 声明，用 `grep -n "auth_probe_tests" src/gateway/mod.rs` 定位）

- [ ] **Step 1: 翻转默认值**

`src/gateway/config.rs` 中将 `#[default]` 从 `Token` 移到 `None`：

```rust
/// Authentication mode
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Require shared token for access
    Token,
    /// No authentication required (default)
    #[default]
    None,
}
```

- [ ] **Step 2: 删掉双模式矩阵测试**

```bash
git rm src/gateway/auth_probe_tests.rs
```

并删除 `src/gateway/mod.rs` 中对应的 `mod auth_probe_tests;`（带 `#[cfg(test)]` 属性行一起删）。

- [ ] **Step 3: 编译 + 修正受默认值影响的断言**

```bash
cargo check -p alephcore && cargo test -p alephcore --lib 2>&1 | tail -20
```

预期：可能有少量测试断言 `AuthMode::Token` 为默认（用 `grep -rn "AuthMode::Token" src/ --include="*.rs" | grep -i "default\|assert"` 找）。把这些断言改成 `AuthMode::None`。其他失败暂记录不修（后续 task 会删除其载体）。

- [ ] **Step 4: 手动 e2e 验证免认证行为**

```bash
cargo run --bin aleph-server -- start
# 另一终端：
curl -s http://127.0.0.1:18790/ -o /dev/null -w "%{http_code}\n"   # 预期 200（panel HTML，不是 /pair 重定向）
./target/debug/aleph-server stop
```

预期：浏览器/curl 无 token 直达 panel。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "gateway: default AuthMode to None (LAN-trust revert step 1)"
```

---

### Task 2: connect 极简化 + 删 method_authz/caller-role 门

**Files:**
- Create: `src/gateway/handlers/connect.rs`（新极简 handler，~80 行）
- Modify: `src/gateway/handlers/mod.rs`（`pub mod connect;`）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs`（connect 注册指向新 handler；删 `connect.challenge` 注册）
- Modify: `src/gateway/server/handler.rs`（删 1028-1047 method_authz 门、简化 1125/1174 CALLER_ROLE、692-748 删失败计数只保留"首消息必须是 connect"）
- Modify: `src/gateway/server/mod.rs`（ConnectionState 删 authenticated/auth_attempts/role/token_hash/authenticate()/is_operator()，删 `MAX_AUTH_ATTEMPTS`）
- Delete: `src/gateway/method_authz.rs`（361 行）
- Modify: `src/gateway/caller_identity.rs`（保留 task-local 但 role 恒为 operator；3 个消费文件用 `grep -rln "caller_identity" src/` 定位）
- Modify: `src/gateway/handlers/pty.rs`、`src/gateway/pty/mod.rs`（去掉 operator-only 引用注释/检查）

- [ ] **Step 1: 写新极简 connect handler**

创建 `src/gateway/handlers/connect.rs`。**字段构造从旧免认证分支移植**：打开 `src/gateway/handlers/auth/connect.rs:291` 起的 `if !ctx.auth_mode.is_auth_required()` 分支，把其中 `state_version` 快照与 keepalive 策略的构造代码原样搬来（保持返回 JSON 字段名 `state_version`、`keepalive` 与旧版一致，panel 依赖它们）。骨架：

```rust
//! Connect handler — session handshake (no authentication).
//!
//! LAN-trust model: every connection is implicitly the owner/operator.
//! The handshake only delivers server state baseline + keepalive policy.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Context for the connect handshake.
pub struct ConnectContext {
    // 两个字段的声明从 handlers/auth/mod.rs 的 `AuthContext` 整行复制：
    // 找到名为 state_version 的字段行和 keepalive/transport 策略字段行，
    // 连同类型与文档注释一起搬过来（不要自己重写类型）。
}

/// Handle "connect" — accepts and ignores any legacy params (token/
/// device_name/...) so old clients don't break mid-rollout.
pub async fn handle_connect(request: JsonRpcRequest, ctx: Arc<ConnectContext>) -> JsonRpcResponse {
    // json! 的 "state_version" / "keepalive" 两个值的构造表达式从
    // handlers/auth/connect.rs:291 的免认证分支整段复制（那里已经写好
    // 如何从 ctx 取快照），字段名保持不变——panel 依赖这两个键名。
    JsonRpcResponse::success(
        request.id,
        json!({
            "role": "operator",
            "state_version": (/* 复制自 connect.rs:291 分支 */),
            "keepalive": (/* 复制自 connect.rs:291 分支 */),
        }),
    )
}
```

唯一允许的"留白"是上面两处复制点——源头位置已精确到行，**除复制外不得新写逻辑**。注意：**不做参数校验**——`request.params` 完全忽略，老客户端带 token 也能连。

- [ ] **Step 2: 切换注册**

`builder/handlers/auth.rs` 中 `"connect"` 注册改指向 `handlers::connect::handle_connect`（构造 `ConnectContext` 替代 `AuthContext` 传参）；删除 `"connect.challenge"` 的整个 `register_handler!` 块。

- [ ] **Step 3: 拆 WS 循环的 auth 门**

`src/gateway/server/handler.rs`：
- 692-748 区域：保留"首消息必须是 `connect`"的会话初始化语义；删除 `connect.challenge` 分支、`auth_attempts` 计数、认证失败断开逻辑。connect 成功后照旧设置会话（748 行 `resp.is_success()` 分支保留）
- 1028-1047：整块删除（method_authz 门）
- 1125/1174：`CALLER_ROLE` 的 scope 调用保留但传入恒定 operator 角色（看 `caller_identity.rs` 的常量定义，3 个消费者不需要再分支）
- 1214 行附近 connect 特判按编译器指引同步简化

`src/gateway/server/mod.rs`：删 `ConnectionState` 的 authenticated/auth_attempts/role/token_hash 字段、`authenticate()`/`is_operator()` 方法、`MAX_AUTH_ATTEMPTS` 常量。编译器会指出所有读写点——一律按"恒已认证、恒 operator"语义删除分支（保 true 分支，删 false 分支）。

- [ ] **Step 4: 删 method_authz.rs 并清理消费者**

```bash
git rm src/gateway/method_authz.rs
cargo check -p alephcore 2>&1 | head -30
```

按编译错误清理：`src/gateway/mod.rs` 的 mod 声明、`handlers/mod.rs`/`handlers/pty.rs`/`pty/mod.rs` 的引用与注释。PTY 方法从此对所有连接开放（spec §4.1 明确接受）。

- [ ] **Step 5: 编译 + 既有 handler 测试**

```bash
cargo check -p alephcore && cargo test -p alephcore --lib -- server::handler 2>&1 | tail -20
```

`handler.rs:1789-1916` 的旧 connect token 测试会失败/不再编译：把 token/shared_token/invitation 变体测试删除，保留并改写"裸 connect 成功"+"首消息非 connect 被拒"两条。

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "gateway: minimal no-auth connect handshake, drop method authz gate"
```

---

### Task 3: 注销并删除 auth 族方法 handlers

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` → 重命名为 `core.rs`（保留 connect/cluster.*/environments.list/secrets.* 注册，删除 auth.*/devices.*/pairing.*/gateway.bootstrap.issue 注册）
- Delete: `src/gateway/handlers/auth/` 整目录（bootstrap.rs/connect.rs/connect_challenge.rs/devices.rs/pairing.rs/tier.rs/mod.rs，~3.4k 行）
- Delete: `src/gateway/handlers/pairing.rs`（318 行）、`src/gateway/handlers/auth_tools.rs`（243 行）
- Modify: `src/gateway/handlers/mod.rs`（删 `pub mod auth; pub mod auth_tools; pub mod pairing;`）
- Inspect→Modify/Delete: `src/gateway/handlers/gateway_credentials.rs`（引用 AuthMode；若它就是 `gateway.bootstrap.issue` 的 nonce 签发器则整删，若含 channel 凭据逻辑则只删 auth 分支——先 `sed -n 1,30p` 看头注释定性）

- [ ] **Step 1: 重命名注册文件并裁剪**

```bash
git mv src/bin/aleph-server/commands/start/builder/handlers/auth.rs \
       src/bin/aleph-server/commands/start/builder/handlers/core.rs
```

在 `core.rs` 中删除以下方法的 `register_handler!` 块及其 handler import：`auth.show_token`、`auth.reset_token`、`auth.list_sessions`、`auth.revoke_session`、`devices.list`、`devices.revoke`、`devices.set_level`、`pairing.approve`、`pairing.list`、`pairing.poll`、`pairing.reject`、`pairing.start_browser`、`pairing.start_node`、`gateway.bootstrap.issue`。**保留**：`connect`、`cluster.enroll`、`cluster.deregister`、`environments.list`、`secrets.*` 全部。同步更新 `builder/handlers/mod.rs`（或同级 mod 声明）中的模块名与函数名（`register_auth_handlers` → `register_core_handlers`，调用点用 `grep -rn "register_auth_handlers" src/bin/` 定位）。

- [ ] **Step 2: 删除 handler 实现**

```bash
git rm -r src/gateway/handlers/auth/
git rm src/gateway/handlers/pairing.rs src/gateway/handlers/auth_tools.rs
```

更新 `src/gateway/handlers/mod.rs`：删三个 mod 声明 + 文件头方法表注释中的 auth/pairing 行。

- [ ] **Step 3: 定性 gateway_credentials.rs**

```bash
sed -n 1,30p src/gateway/handlers/gateway_credentials.rs && grep -n "AuthMode\|bootstrap" src/gateway/handlers/gateway_credentials.rs
```

若文件职责 = bootstrap nonce/token 签发 → `git rm` + 清 mod 声明与注册；若混有 provider/channel 凭据逻辑 → 只删 AuthMode/bootstrap 相关函数。

- [ ] **Step 4: 编译收敛**

```bash
cargo check -p alephcore 2>&1 | head -40
```

逐个清掉报错（多为 `use` 残留与 `AuthContext` 构造点）。`ConnectContext` 构造处（builder）此时是唯一保留的 connect 装配。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "gateway: remove auth/pairing/devices method handlers"
```

---

### Task 4: 拆 HTTP auth 路由与中间件

**Files:**
- Modify: `src/gateway/server/mod.rs:362-367,433,486,542-549,676-680`（删 `auth_routes` 字段、`set_auth_routes`、挂载块）
- Delete: `src/gateway/auth_middleware.rs`（486 行）、`src/gateway/bootstrap.rs`（160 行）、`src/gateway/challenge.rs`（334 行）
- Modify: `src/gateway/mod.rs`（删 3 个 mod 声明）

> **T4 审查修正（2026-06-12）**：原文此处还删 `src/gateway/pair_loop_guard.rs`——**计划错误，不删**。它是 channel 适配器的 bot↔bot 回复风暴防护（出生提交 `0a8e40389` 即 channel 功能，按 `MessageMeta::BotAuthored` 门控），被 `inbound_router/mod.rs` 和 `channel_policy.rs`（spec §4.2 明确保留）消费，与 `/pair` 页面、设备配对、HTTP auth 零关系。与 T3 的 pairing.rs/pairing_store.rs 同属"channel vs device 混淆"。`gateway/mod.rs` 的 `mod pair_loop_guard` 声明保留。
- Modify: builder 中 `set_auth_routes(...)` 调用点（`grep -rn "set_auth_routes\|auth_routes(" src/bin/ src/gateway/`）

- [ ] **Step 1: 删字段与挂载**

`server/mod.rs`：删 `auth_routes: Option<Router>` 字段（362-367）、两处 `auth_routes: None` 初始化（433/486）、`set_auth_routes` 方法（542-549）、root 挂载块（676-680，连同"unauthenticated browsers are redirected to /pair"注释）。

- [ ] **Step 2: 删四个文件**

```bash
git rm src/gateway/auth_middleware.rs src/gateway/bootstrap.rs \
       src/gateway/challenge.rs src/gateway/pair_loop_guard.rs
```

清 `src/gateway/mod.rs` 对应 mod 声明与 re-export。

- [ ] **Step 3: 编译收敛 + 验证 panel 直达**

```bash
cargo check -p alephcore && cargo run --bin aleph-server -- start
curl -s http://127.0.0.1:18790/ | head -3        # 预期 panel HTML
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:18790/pair   # 预期 404
./target/debug/aleph-server stop
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "gateway: remove HTTP auth routes, middleware, bootstrap and challenge"
```

---

### Task 5: 删存储层 + 配置收编（AuthMode/AuthConfig 寿终）

**Files:**
- Delete: `src/gateway/security/` 下除 `crypto.rs`、`mod.rs` 外全部（token/pairing/device/brute_force/guest_session_manager/invitation_manager/policy_engine/identity_map/activity_log/activity_logger/shared_token/token_readonly + store 子目录若有）
- Modify: `src/gateway/security/mod.rs`（只剩 `pub mod crypto;`）
- Delete: `src/gateway/device_store.rs`（334）、`src/gateway/pairing_store.rs`（472）、`src/gateway/trusted_proxy.rs`（178）
- Modify: `src/gateway/config.rs`（删 `AuthMode`/`AuthConfig`，`allowed_origins` + 新 `allow_any_origin` 收编进 gateway 根）
- Modify: `src/gateway/origin_policy.rs`（加 `allow_any` 逃生口 + 测试）
- Modify: `src/gateway/credential_planner.rs`、`src/gateway/server/probe.rs`、`src/gateway/server/mod.rs:143,231`（AuthMode 引用清除）

- [ ] **Step 1: 先写 origin_policy 逃生口的失败测试**

`src/gateway/origin_policy.rs` tests 模块追加：

```rust
#[test]
fn allow_any_origin_bypasses_all_checks() {
    let policy = OriginPolicy::allow_any();
    assert!(policy.is_allowed(Some("https://evil.example.com"), Some("10.0.0.6:18790")));
}
```

```bash
cargo test -p alephcore --lib -- origin_policy 2>&1 | tail -5
```

预期：FAIL（`allow_any` 不存在）。

- [ ] **Step 2: 实现逃生口**

`OriginPolicy` 加 `allow_any: bool` 字段（现有构造器置 false）、新增：

```rust
/// Escape hatch: trust every Origin. For users who front the gateway
/// with their own reverse proxy / auth layer.
#[must_use]
pub const fn allow_any() -> Self {
    Self { allowed: Vec::new(), allow_any: true }
}
```

`is_allowed` 开头加 `if self.allow_any { return true; }`。（`allowed` 字段名以文件实际为准，保持既有命名。）

```bash
cargo test -p alephcore --lib -- origin_policy 2>&1 | tail -5   # 预期 PASS（全部）
```

- [ ] **Step 3: 配置收编**

`config.rs`：删除 `AuthMode` enum、`AuthConfig` struct 及其 Default impl；`GatewayConfig`（host/port 所在那个 struct，:224 附近）删 `auth: AuthConfig` 字段，新增：

```rust
/// Extra browser origins allowed on the `/ws` upgrade (moved from
/// [gateway.auth] allowed_origins; same semantics).
#[serde(default)]
pub allowed_origins: Vec<String>,
/// Trust every Origin on the `/ws` upgrade. Escape hatch for reverse
/// proxy deployments. SECURITY: leaves the agent drivable by any web
/// page the user's browser visits — keep false unless you know why.
#[serde(default)]
pub allow_any_origin: bool,
```

OriginPolicy 装配点（`grep -rn "OriginPolicy::new\|origin_policy" src/bin/ src/gateway/server/`）改为：`allow_any_origin` 为 true 时用 `OriginPolicy::allow_any()`，否则 `OriginPolicy::new(config.allowed_origins.clone())`。

兼容性检查：确认配置根 struct **没有** `#[serde(deny_unknown_fields)]`（`grep -n "deny_unknown_fields" src/gateway/config.rs`）——老用户配置文件里残留的 `[gateway.auth]` 表必须被静默忽略而不是解析失败。若有 deny 属性则去掉。

> **Spec §4.2 实现注（偏差说明）**：spec 写的"放行私网 IP 字面量 Origin"不需要新代码——现有 OriginPolicy 的同源规则（Origin == Host）已天然覆盖一切合法 LAN 访问（浏览器访问 `http://10.10.10.6:18790` 时 Origin 与 Host 相同），并拒绝公网域名 Origin（evil.com ≠ 10.10.10.6，DNS rebinding 同理被拒）。安全结果与 spec 等价且更严格。本 task 对 origin_policy 的唯一改动就是 `allow_any` 逃生口。

- [ ] **Step 4: 删存储与 security**

```bash
cd src/gateway/security && ls | grep -v -e "^crypto.rs$" -e "^mod.rs$" | xargs git rm -r && cd -
git rm src/gateway/device_store.rs src/gateway/trusted_proxy.rs
```

> **T3 审查修正（2026-06-12）**：原文此处还删 `src/gateway/pairing_store.rs`——那是**计划错误**。该文件是 **channel 发送者配对** store（`channel.pairing.*`，被 `inbound_router/{mod,permission,types}.rs`、`start/mod.rs`、`builder/subsystems.rs` 消费，属 spec §4.2 "session/channel/execution 全部不动"的保留范围），与设备认证配对（`security/store/pairing.rs`、`security/pairing.rs`，本步删除）是两套东西。**不要删 `pairing_store.rs` 和 `handlers/pairing.rs`**（后者 T3 已据此保留）。

> **T5 执行修正（2026-06-12，方案 B 重定范围）**：implementer BLOCKED 升级证实 `security/shared_token.rs` 是**生产密钥保险库本体**（SecretVault 宿主 + vault 主密钥经 `store/` 持久化，合计 ~50+ 保留范围消费者），照原清单删除会毁掉 providers/OAuth/channel 的全部密钥存储。裁决：**保留 `shared_token.rs`、`store/`（整个）、`token_readonly.rs`（admin IPC bearer）、`crypto.rs`**；只删纯 auth 模块（上行 Step 4 命令需排除这四者）。级联删除：`gateway/session.rs`（HTTP cookie 会话，T4 后零消费者）、`handlers/guests.rs`、`wizard/flows/pairing.rs`、前拉 T6 的 4 个 server 侧 CLI 文件。`AuthContext` 收缩为 `{shared_token_mgr, security_store, node_registry}` 三字段；`initialize_auth`→`initialize_vault`。实际落地：commits `5b37242a4` + `6fdd7810f`（47 文件，净 −7,185 行）。Vault 抽离 = 未来独立任务。

`security/mod.rs` 改写为仅 `pub mod crypto;`（保留文件头注释里 crypto 相关部分）。

- [ ] **Step 5: 编译收敛**

```bash
cargo check -p alephcore 2>&1 | head -50
```

清理顺序建议：`server/mod.rs`（auth_mode 字段 :143/:231、session_mgr/shared_token 字段 :176-189）→ `credential_planner.rs` → `server/probe.rs` → 其余编译器指出的点。规则同前：恒"无认证"语义，删 Token 分支保 None 分支，最后 None 分支也不再分支（条件整体消失）。

```bash
cargo test -p alephcore --lib 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "gateway: drop token/pairing/device stores and AuthMode config, add allow_any_origin"
```

---

### Task 6: CLI 双侧清理（aleph-server + aleph）

**Files:**
- Delete: `src/bin/aleph-server/commands/bootstrap_url.rs`（178）、`bootstrap_token.rs`（87）、`devices.rs`（90）、`pairing.rs`
- Modify: `src/bin/aleph-server/cli.rs`、`main.rs`、`commands/mod.rs`（子命令 enum/分发/`with_policy` 注册清除——`grep -n "BootstrapUrl\|BootstrapToken\|Devices\|Pairing" src/bin/aleph-server/cli.rs main.rs`）
- Delete: `interfaces/cli/src/commands/auth_cmd.rs`、`devices_cmd.rs`、`pairing_cmd.rs`、`guests.rs`
- Modify: `interfaces/cli/src/commands/open_cmd.rs`（去 nonce，直接 `open http://<host>:<port>/`）、`mod.rs`、`cli_args.rs`（子命令裁剪）
- Modify: `interfaces/cli/src/commands/connect.rs`（若向 connect 传 token 参数则去掉；新 handler 容忍旧参数，故先 grep 定性：`grep -n "token" interfaces/cli/src/commands/connect.rs`）

- [ ] **Step 1: aleph-server 侧删除**

```bash
git rm src/bin/aleph-server/commands/{bootstrap_url.rs,bootstrap_token.rs,devices.rs,pairing.rs}
```

按编译错误清 `cli.rs` 子命令 variants、`main.rs`/`commands/mod.rs` 分发臂与 `with_policy` 注册。

```bash
cargo check --bin aleph-server 2>&1 | head -20
```

- [ ] **Step 2: aleph 侧删除**

```bash
git rm interfaces/cli/src/commands/{auth_cmd.rs,devices_cmd.rs,pairing_cmd.rs,guests.rs}
```

`open_cmd.rs`：删除 nonce 签发调用，保留"读 config 拼 URL + 系统 open"路径。清 `cli_args.rs`/`mod.rs` 引用。

```bash
cargo check -p aleph-cli 2>&1 | head -20
```

- [ ] **Step 3: 双二进制冒烟**

```bash
cargo run --bin aleph-server -- start
cargo run -p aleph-cli -- health     # 预期正常输出（无 token 握手）
cargo run -p aleph-cli -- open      # 预期打开浏览器到 panel
./target/debug/aleph-server stop
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "cli: remove auth/pairing/devices/bootstrap subcommands"
```

---

### Task 7: 测试面收口 + 全量绿

**Files:**
- Modify: 全仓残余测试（`grep -rln "shared_token\|pairing\|device_token\|AuthMode\|auth_mode" src/ --include="*.rs"` 收尾）
- Modify: `scripts/spec_c_regression.sh`（若引用已删 CLI 子命令则同步裁剪——`grep -n "pairing\|devices\|secret" scripts/spec_c_regression.sh` 先看）

- [ ] **Step 1: 残余符号清零**

```bash
grep -rn "AuthMode\|auth_middleware\|pairing_store\|device_store\|method_authz\|shared_token" src/ interfaces/cli/src/ --include="*.rs" | grep -v "test" | head -20
```

预期：零命中（除注释性历史文档）。有命中则回到对应 task 的删除规则处理。

- [ ] **Step 2: 全量测试 + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo test -p aleph-cli 2>&1 | tail -5
just clippy 2>&1 | tail -5
```

预期：全绿、零警告。失败逐个修（修实现优先于改测试，除非测试断言的就是已删除的 auth 行为——那种直接删测试）。

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "gateway: finish auth removal test sweep"
```

---

### Task 8: Panel 去认证 UI

**Files:**
- Delete: `interfaces/webchat/src/views/pairing_modal.rs`
- Modify: `interfaces/webchat/src/components/boot_check_gate.rs`（删 pairing_required 分支）
- Modify: `interfaces/webchat/src/state/connection.rs`、`context.rs`、`app.rs`（token/challenge/pairing 状态机 → 直接 connect）
- Inspect→Modify: `grep -rln "pairing\|devices\.\|guest\|invitation" interfaces/webchat/src/` 其余命中（settings_sidebar 的设备区、notification_center 的配对审批卡片）

- [ ] **Step 1: 圈定全部命中**

```bash
grep -rn "pairing\|token\|devices\.\|guest\|invitation" interfaces/webchat/src/ -l
```

- [ ] **Step 2: 删除与简化**

规则：
- `views/pairing_modal.rs` 整删 + `views/mod.rs` 声明清除
- `boot_check_gate.rs`：删 `pairing_required` 分支（:80 注释所指的 gate 排除路径），gate 只剩连接成功/失败两态
- `state/connection.rs`/`context.rs`/`app.rs`：connect 请求不再携带 token/device_name 凭据参数；解析新极简结果（`role`/`state_version`/`keepalive`）；删 token 持久化（localStorage 读写）与重连时的凭据重放
- NotificationCenter 中 pairing approval 卡片、settings 中 devices 管理区：整块删除
- 凡 UI 文案引用"配对/设备/token"的死入口一并清

- [ ] **Step 3: wasm 构建验证（必须）**

```bash
cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -5
just wasm 2>&1 | tail -5
```

预期：双绿。

- [ ] **Step 4: 浏览器 e2e**

```bash
cargo build --release -p alephcore --bin aleph-server   # rust_embed 烧入新 dist
./target/release/aleph-server start
# 浏览器开 http://127.0.0.1:18790 → 无任何配对/登录拦截，直接进对话；发一条消息走通
./target/release/aleph-server stop
```

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "panel: remove pairing/devices/token UI, plain connect handshake"
```

---

### Task 9: Shell feature 拆分（embedded-core）

**Files:**
- Modify: `desktop/shell/Cargo.toml`（新增 `[features]`）
- Modify: `desktop/shell/src/main.rs`（mod 与 setup 按 feature 门控；删 token 加载 :72-77、bootstrap 握手）
- Modify: `desktop/shell/src/daemon.rs`（删 `load_shared_token` 与 bootstrap-token 调用；整个 mod 加 `#![cfg(feature = "embedded-core")]` 语义的门控——放在 main.rs 的 `mod daemon;` 声明上）
- Modify: `desktop/shell/src/deeplink.rs`（删 pairing 深链分支）
- Modify: `desktop/shell/src/connection.rs`（lite 下 `Local` 仍解析为 `http://127.0.0.1:18790` 但不触发 daemon 监督）

- [ ] **Step 1: 加 feature**

`desktop/shell/Cargo.toml`：

```toml
[features]
default = ["embedded-core"]
# Bundles + supervises the local aleph-server daemon (full app variant).
# Build the panel-only shell with --no-default-features.
embedded-core = []
```

- [ ] **Step 2: 门控与删除**

`main.rs`：
- `mod daemon;` → `#[cfg(feature = "embedded-core")] mod daemon;`，`mod perm_monitor;` 同样门控（spec §5.4：纯壳无本机 daemon 可监控）
- 删 :72-77 的 `daemon::load_shared_token()` 路径及其下游（webview URL 不再带凭据，直接导航到目标 origin）
- setup 中 daemon 拉起/健康监督调用全部包进 `#[cfg(feature = "embedded-core")]` 块；lite 路径直接 `connection` 目标导航
- splash 等待 daemon ready 的逻辑：full 保留；lite 直接加载远程 URL（连不上走 Task 10 的设置页）

`daemon.rs`：删 `load_shared_token`、bootstrap-token 子进程调用、`bootstrap-url` 握手；保留拉起 `aleph-server start`、版本接管、健康探测。

`deeplink.rs`：删 pairing 分支（`grep -n "pair" desktop/shell/src/deeplink.rs` 定位）；若删后文件无实义则整删并清 mod。

> **修正注（T9 执行时核实，2026-06-12）**：`deeplink.rs` 实际**没有 pairing 分支**——它是通用 raw-URL 转发器（`aleph://…` → focus 窗口 + `aleph:deep-link` DOM CustomEvent 交 Panel 路由），壳从不解释深链内容。spec §5.4 所指"配对深链（deeplink 中 pairing 部分）"是 Panel 侧概念，已随 T1-T8 删除。本文件零改动，保留原样。
>
> **修正注 2（"Open in Browser" 菜单项处置）**：menu.rs 的 `ID_OPEN_BROWSER` 原实现 100% nonce 耦合（spawn `aleph-server bootstrap-url` 子进程→`/auth/bootstrap?nonce=…`→系统浏览器），机制必删；但其用户面功能（在系统浏览器打开 Panel）按 spec §4.1 对 CLI 胞兄 `aleph open` 的对称处置（去 nonce 保留）应予保留——恢复为裸打开当前 `ConnectionTarget` origin（Local→`http://127.0.0.1:18790`，Remote→所配 URL），复用 `external_link::open_url`，两变体共有不门控。

- [ ] **Step 3: 双矩阵编译**

```bash
cargo check -p aleph-shell 2>&1 | tail -3
cargo check -p aleph-shell --no-default-features 2>&1 | tail -3
```

（crate 名以 `desktop/shell/Cargo.toml` 的 `name` 为准。）预期：双绿。

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "shell: gate daemon supervision behind embedded-core feature, drop auth handoff"
```

---

### Task 10: 纯壳首启连接设置页 + mDNS 发现

**Files:**
- Create: `desktop/shell/src/connect_setup.rs`（lite 首启窗口 + mDNS 浏览，~150 行）
- Modify: `desktop/shell/src/main.rs`（lite 且无 target marker 时先开设置窗）
- Modify: `desktop/shell/Cargo.toml`（`mdns-sd` 依赖，workspace 已用于 server 侧广播）

- [ ] **Step 1: 写 connect_setup**

`connect_setup.rs` 职责：
1. `discover()` —— 用 `mdns_sd::ServiceDaemon::browse("_aleph._tcp.local.")` 收集 3 秒内的 ServiceInfo（host/port），返回 `Vec<String>`（`"http://<ip>:<port>"`）
2. 一个小的 Tauri WebviewWindow，HTML 内联（`data:` URL 或 `splash/` 同款方式）：地址输入框 + 发现列表 + 连接按钮；提交走已有 `connection::set_connection_target` invoke handler（main.rs:117-119 已注册），成功后关设置窗、主窗导航目标 origin
3. 校验复用 `ConnectionTarget::parse`（connection.rs:38 起，host/port/scheme 规则现成）

```rust
//! First-run connection setup for the panel-only shell variant.
#![cfg(not(feature = "embedded-core"))]

use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::time::Duration;

const SERVICE_TYPE: &str = "_aleph._tcp.local.";

/// Browse the LAN for running aleph-server instances (3s window).
pub fn discover() -> Vec<String> {
    let Ok(daemon) = ServiceDaemon::new() else { return Vec::new() };
    let Ok(rx) = daemon.browse(SERVICE_TYPE) else { return Vec::new() };
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut found = Vec::new();
    while let Ok(event) = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
        if let ServiceEvent::ServiceResolved(info) = event {
            for addr in info.get_addresses() {
                found.push(format!("http://{}:{}", addr, info.get_port()));
            }
        }
        if std::time::Instant::now() >= deadline { break; }
    }
    found.sort();
    found.dedup();
    found
}
```

（窗口部分按 splash 现有模式实现：`grep -rn "splash" desktop/shell/src/main.rs` 找加载内联 HTML 的现成写法照抄。）

> **修正注（T10 执行时裁决，2026-06-12）**：实际实现**未新建独立 WebviewWindow**，而是复用既有 `splash/connect.html`（先于本任务存在：地址输入+Connect 按钮+双变体打包+tray/menu "Connect Remote" 既有目标），lite 下渐进增强出 mDNS 发现区（`discover_servers` invoke 失败=full 变体→静默隐藏发现区，full 行为等价不变）。spec §5.3"原生小窗"的功能要求（手填 IP[:端口]+发现列表点选）全满足；单源 DRY 避免第二份连接 UI 漂移。失败重试选型：TCP-probe-before-navigate（Tauri 2.11.2 无导航失败事件，错误回调不可行）；`connection::marker_exists()` 区分首启与 marker=local。

- [ ] **Step 2: main.rs 接线**

lite 构建（`#[cfg(not(feature = "embedded-core"))]`）启动序列：`ConnectionTarget` marker 存在 → 直接导航；不存在 → 开 connect_setup 窗。连接失败（webview 加载错误）→ 重开设置窗（错误处理 spec §8：不白屏）。

- [ ] **Step 3: 本机验证**

```bash
cargo run --bin aleph-server -- start    # 本机 server 即 mDNS 广播源
cd desktop/shell && cargo tauri dev --no-default-features
# 预期：首启出设置窗，发现列表里有本机 server；选择后进 panel
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "shell: panel-only first-run connect setup with mDNS discovery"
```

---

### Task 11: lite Tauri 配置变体 + just 配方

**Files:**
- Create: `desktop/shell/tauri.lite.conf.json`
- Modify: `justfile`（`shell-build-lite` / `shell-dev-lite` 配方）

- [ ] **Step 1: lite 配置（--config 合并覆盖）**

`tauri.lite.conf.json`（与 `tauri.conf.json` 做 JSON 合并，仅覆盖差异键）：

```json
{
  "productName": "Aleph Panel",
  "identifier": "com.aleph.panel",
  "bundle": {
    "externalBin": []
  }
}
```

（identifier 基值看 `grep -n "identifier" desktop/shell/tauri.conf.json`，lite 必须不同以便并存安装。）

- [ ] **Step 2: just 配方**

`justfile` 在 `shell-build` 后追加（模式照抄 :86-102 现有配方，差异只在 `--no-default-features` 与 `--config`）：

```make
# Build the panel-only desktop shell (no embedded server)
shell-build-lite: wasm
    cd desktop/shell && cargo tauri build --no-default-features --config tauri.lite.conf.json

# Run the panel-only shell in dev mode
shell-dev-lite:
    cd desktop/shell && cargo tauri dev --no-default-features --config tauri.lite.conf.json
```

注意：lite 不依赖 `swift-bridge`/server 构建（对比 `build:` 配方），wasm 仍要（splash/设置页若引用）；若实际无需 wasm 则去掉依赖项。

- [ ] **Step 3: 本机双构建验证**

```bash
just shell-build 2>&1 | tail -3        # full：含 externalBin
just shell-build-lite 2>&1 | tail -3   # lite：无 externalBin，体积显著小
ls target/release/bundle/dmg/           # 预期出现 Aleph 与 Aleph Panel 两个 dmg（macOS 本机）
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "shell: lite bundle variant config and just recipes"
```

---

### Task 12: 发版三产物（workflow + install.sh）

**Files:**
- Create: `scripts/install.sh`
- Modify: `.github/workflows/aleph-app-release.yml`（desktop-app job 追加 lite 构建步 + server 裸二进制上传；release job 附加 server 二进制与 install.sh）

- [ ] **Step 1: install.sh**

```bash
#!/usr/bin/env bash
# Aleph server installer — downloads the standalone aleph-server binary.
# Usage: curl -fsSL https://github.com/<owner>/<repo>/releases/latest/download/install.sh | bash
set -euo pipefail

REPO="${ALEPH_REPO:-rootazero/Aleph}"
VERSION="${ALEPH_VERSION:-latest}"

os="$(uname -s)"; arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64)  asset="aleph-server-aarch64-apple-darwin" ;;
  Linux-x86_64)  asset="aleph-server-x86_64-unknown-linux-gnu" ;;
  *) echo "Unsupported platform: $os/$arch (download manually from GitHub Releases)"; exit 1 ;;
esac

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

dest_dir="/usr/local/bin"
[ -w "$dest_dir" ] || dest_dir="$HOME/.local/bin"
mkdir -p "$dest_dir"

echo "Downloading $asset -> $dest_dir/aleph-server"
curl -fsSL "$url" -o "$dest_dir/aleph-server"
chmod +x "$dest_dir/aleph-server"

echo "Installed. Start it with:  aleph-server start"
echo "LAN access: set [gateway] host = \"0.0.0.0\" in ~/.aleph/config.toml (trusts your whole LAN)."
```

（`REPO` 默认值以 `git remote get-url origin` 实际值为准；triple 命名与 workflow :93-95 已有的 `aleph-server-$triple` 拷贝一致。）

```bash
bash -n scripts/install.sh   # 语法检查，预期无输出
```

- [ ] **Step 2: workflow 扩展**

`aleph-app-release.yml` desktop-app job，在现 full `cargo tauri build`（:114 附近）之后追加两步：

```yaml
      - name: Build panel-only shell (lite)
        shell: bash
        run: |
          version="$(tr -d '[:space:]' < VERSION)"
          cd desktop/shell && cargo tauri build --verbose --no-default-features \
            --config tauri.lite.conf.json --config "{\"version\":\"$version\"}"

      - name: Upload standalone server binary
        uses: actions/upload-artifact@v7
        with:
          name: server-${{ matrix.asset }}
          if-no-files-found: error
          path: desktop/shell/binaries/aleph-server-*
```

要点：
- lite 构建复用同一 matrix（三 OS 各出一份 lite 安装包）；lite bundle 产物（productName "Aleph Panel"）会出现在同一 `target/release/bundle/**` 通配下，与 full 的上传段共用（:116-131 的 path 通配天然覆盖，无需改）
- server 二进制 workflow :93-95 已拷到 `desktop/shell/binaries/aleph-server-$triple`，上传零额外构建
- release job（:132 起）：下载 `server-*` artifacts、`scripts/install.sh`，随安装包一并 `gh release upload`（照抄该 job 既有上传写法追加文件列表）
- updater 签名步骤只对 full 生效即可（lite 首版可不带自更新工件；若现有签名配置自动覆盖 lite 也无害）

- [ ] **Step 3: CI 验证（build-only）**

```bash
git add -A && git commit -m "release: three-artifact matrix (full app, panel-only app, standalone server)"
just verify-build
```

预期：三平台全绿。**吸取 CI 平台门控教训**：失败不要逐次 round-trip，一次性把失败 job 的日志全拉下来按平台分组修。

---

### Task 13: 文档收口

**Files:**
- Modify: `CLAUDE.md`（"分发形态"备注、"Auth UX"整节、进程管理节中 pairing 例子）
- Modify: `docs/reference/SECURITY.md`（auth-ux 节改写为 LAN-trust 模型 + Origin 护栏说明 + allow_any_origin）
- Modify: `docs/reference/GATEWAY.md`（认证段落删改，`grep -n "auth\|pair\|token" docs/reference/GATEWAY.md` 定位）
- Modify: `README.md`（安装三选一：完整 App / 纯壳 App / `curl | bash`）

- [ ] **Step 1: 改写**

CLAUDE.md 分发形态备注改为：

> **分发形态**: Aleph 发布三类产物：完整桌面 App（内嵌 `aleph-server`，单机零配置）、Aleph Panel 纯壳 App（连接局域网内任一 server）、独立 `aleph-server` 二进制（`curl | bash` 安装，服务器/NAS 部署）。信任模型 = 网络边界：默认只绑 `127.0.0.1`，`[gateway] host = "0.0.0.0"` 显式开放局域网（局域网内任何设备即获得完全控制权）。唯一保留的协议护栏是 WS Origin 校验（挡公网网页跨源驱动，`allow_any_origin` 可关）。

"Auth UX"整节删除，替换为指向 SECURITY.md 新节的一行。

- [ ] **Step 2: 全仓文档残留扫描**

```bash
grep -rn "pairing\|show-token\|bootstrap-url\|/pair" docs/ CLAUDE.md README.md --include="*.md" | head -20
```

逐个清除或改写（历史 specs/plans 目录下的文档**不改**——它们是史料）。

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "docs: LAN-trust distribution and security model"
```

> ⚠️ 实现修正：`git add -A` 不安全（worktree 有未 gitignore 的 `interfaces/webchat/node_modules` 数千文件）。改为只 add 本任务改动的 `.md` 文件。

### Amendment 6（controller，2026-06-12）— T13 scope 扩展 + 计划材料 3 处更正

执行中发现两类与计划散文不符，据此扩展/修正（均以分支已提交的真实代码为准）：

**A. SECURITY.md 残留子系统区扩入 scope。** `docs/reference/SECURITY.md` 的 `## Identity Context & Permission Enforcement` 整节（约行 22–284）连同 Overview（行 3 顶注、行 9–16 bullets）描述的是**本弧 Task 1–5 已删除的 role-based guest-invitation 权限子系统**——`policy_engine.rs` / `invitation_manager.rs` 已不存在（核实：`src/gateway/security/` 现仅 `crypto.rs / mod.rs / shared_token.rs / store/ / token_readonly.rs`），`aleph guests *` CLI 已删。计划原文仅命名"auth-ux 节"属 under-scoping。**T13 scope 扩展为同时清理此节**，与"使 SECURITY.md 符合 LAN-trust 现实"同质。清理须严格区分：
- **删/改写**：Identity-based 权限流图、Invitation Manager、Policy Engine、GuestScope、`Role::Guest/Anonymous`、Guest Invitation Flow、`aleph guests` CLI、"Security Guarantees"中 guest/invitation 条目。
- **保留**：`## Architecture`（行 286 起的 Exec Kernel：Command Parser / Risk Analyzer / Approval Manager / Allowlist / Output Masking / Audit）及其后 `## Exec Kernel` 全部——这是 `src/exec/` shell 安全子系统，**未删**。
- **据实改写为保留机制**：工具级权限现由 **ScopedToolService 通道工具权限层**（spec §4.2 明确保留，三层 merge global→agent→channel）治理，非 role-based；`IdentityContext`/`Role` 若仍存于 `shared/protocol/src/auth.rs` 则坍缩为单一 owner 身份（connect 恒返 operator），按**存活类型**据实写，勿引用已删类型。

**B. 计划材料 3 处更正（已据实落文档，记录在案）：**
1. 材料 #5「`aleph open` 保留去 nonce」→ 实为**桌面 shell 菜单项 "Open in Browser"**（`desktop/shell/src/menu.rs`），无此 CLI 命令（`cli.rs` 的 `Command` enum 无 `Open` 变体）。
2. 材料 #9「`method_authz` 已删」→ 文件仍在（`src/gateway/method_authz.rs`），但 RPC 级 gate 已 inert（caller 恒 operator 恒过），仍被 `ScopedToolService` 当 tool-dispatch 分类器消费。文档按**效果**写（无方法级门槛、含 PTY/shell），不声称"文件已删"。
3. 材料 #3「Origin 放行…私网」→ 代码**不**按 RFC1918 网段 auto-allow 私网 IP，仅放行 无 Origin / loopback / `tauri:` / allow-list / 同源。SECURITY.md 规则表据实写明。

---

## 收尾验收（全计划完成后）

- [ ] `cargo test -p alephcore --lib` + `cargo test -p aleph-cli` + `just clippy` 全绿
- [ ] `just verify-build` 三平台绿
- [ ] E2E 四场景（spec §9）：
  1. 本机浏览器无凭据直访 panel 走通对话
  2. `host = "0.0.0.0"` 后局域网另一设备直访（手动）
  3. 纯壳填 IP / mDNS 选择连远程 server
  4. 完整版零配置开箱（`just shell-build` 装 .app 验证）
- [ ] 删除量核对：`git diff --stat <计划起点>..HEAD | tail -1` 预期净删 10k+ 行
- [ ] CHANGELOG.md 写条目后按 CLAUDE.md 发版流程 `just release`（用户触发）

---

## Amendment 7（controller，2026-06-13）— Host 白名单 DNS-rebinding 硬化（最终审查后，用户选定）

**背景**：全部 13 task 完成后的最终代码审查（final-rev-static）判 ✅ 可合入、零 CRITICAL/HIGH，但记一条 **MEDIUM (M1)**：删除认证后 `src/gateway/origin_policy.rs` 的 WS Origin 校验成为浏览器面**唯一**护栏，其 **same-origin 放行路径挡不住经典 DNS-rebinding**——攻击者在 `evil.com:18790` 托管恶意页，把 `evil.com` 重绑到网关地址后，请求带 `Origin == Host == evil.com:18790` 即同源放行，驱动 agent（含 PTY）。`is_allowed` 逻辑与回退前逐字节相同（既有 gap），但回退删了门后的 auth 兜底使其更暴露。

**决策（用户经 AskUserQuestion 选定"加 Host 白名单硬化"）**：gate 住 same-origin 放行——**仅当请求 `Host` 的主机部分是 IP 字面量或 loopback 时**才 auto-allow same-origin；域名 Host 不再自动同源放行（须走 `[gateway] allowed_origins`）。依据：DNS-rebinding 必须用域名（重绑 A 记录），纯 IP/loopback 的 Host 无法被重绑。

**不破坏的已发布场景**：loopback 浏览器、LAN-IP 浏览器（`10.x:18790` Host 是 IP 字面量→放行，LAN 模式关键路径）、IPv6、tauri shell、native 无 Origin、allow-list。**有意行为变更**：零配置**域名** same-origin 现被拒，域名部署须 allow-list 其 origin——这是堵 rebinding 的必要代价。

**实现**：`is_allowed` same-origin 分支加 `&& host_is_ip_or_loopback(host)` 门 + helper（剥端口含 IPv6 括号、判 loopback/IP 字面量）+ TDD 测试（rebinding 域名 Host 被拒 / LAN-IP 同源放行 / IPv6 / 域名同源无配置被拒+加 allow-list 后放行）+ 三处文档措辞改为"已防御"（origin_policy.rs / SECURITY.md / CLAUDE.md，撤回 Amendment 阶段加的"当前未实现"限制说明）。验证 `cargo clippy -p alephcore -- -D warnings`（项目 gate，不含 --all-targets）+ `cargo test -p alephcore --lib origin_policy`。

**合并**：硬化完成后，用户选定由 controller 合 `lan-trust-revert → 本地 main`（main 已并发前进 32 commits，先 merge main 入分支 ort 解 webchat 冲突→验证→`--no-ff`）。全程 NOT pushed，推送/发版由用户触发。

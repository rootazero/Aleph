# Panel 连接韧性与错误路由 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Panel 把"为什么连不上"变成一个类型化的值,据此分流到正确的补救路径——网络/IP 问题提示检查网络并退避重连,token 失效立即弹登录墙重输,并补齐 lite-shell 换 IP 的原生恢复与网关 token 轮换主动踢下线。

**Architecture:** 在 `shared-ui-logic` 引入纯函数 `ConnectionFailure` + `classify()` 作单一真相源;webchat 的 connect/handshake/reconnect 在边界分类并据此分支;三个 Gate 与 TokenWall 读类型化原因选 i18n 文案;lite-shell 加 Remote-only 常驻 supervisor;网关 rotate 经现有 broadcast 总线发 `TokenRotated` 帧,连接循环关远程会话(留 loopback)。

**Tech Stack:** Rust · Leptos/WASM(`aleph-panel`)· `shared-ui-logic`(host-testable 纯逻辑)· `leptos_i18n`(编译期 locale 校验,en.json + zh.json 必须同步)· Tauri(`aleph-desktop-shell`)· tokio + axum WebSocket(`alephcore` gateway)。

## Global Constraints

- **提交规范**:English commit messages,格式 `<scope>: <description>`(如 `panel: classify connection failures`)。
- **i18n**:`leptos_i18n` 编译期校验——新增 key **必须同时**加进 `interfaces/webchat/locales/en.json` 与 `zh.json`,否则编译失败。代码注释英文,UI 文案中英双语。
- **cargo 节制**(系统负担重,`alephcore` 构建吃内存):纯逻辑 task 用 `cargo test -p <crate> --lib <scoped_name>`;WASM 接线 task 用 `cargo check -p aleph-panel` 验编译,运行行为靠手动 e2e;gateway test 必须 scope 到具体测试名,**不跑全量**。
- **R4(Interface 纯 I/O)**:不改 `handle_token_rotate(req)` 纯签名——发事件在注册闭包里做。
- **R10(薄 harness)**:不新增 session 注册表,复用现有 broadcast 总线 + `CloseFrame`。
- **安全红线(`src/gateway/CLAUDE.md`)**:改授权行为必须同步加测试——轮换**关远程、不关 loopback**。
- **WS close code**:token 轮换踢下线用 `code=4001, reason="token_rotated"`(4001 = 应用私有区间)。
- **节奏对齐常量**:panel 重连预算 ≈ 31s(1+2+4+8+16);lite 原生 supervisor 宣告 Down 必须 ≳ 35s。

---

## File Structure

| 文件 | 职责 | 动作 |
|------|------|------|
| `shared/ui_logic/src/connection/failure.rs` | `ConnectionFailure` / `FailureStage` / `classify()` 纯逻辑 | 新增 |
| `shared/ui_logic/src/connection/mod.rs` | 导出 failure 模块 | 改 |
| `shared/ui_logic/src/connection/reconnect.rs` | `ReconnectStrategy` 加抖动 | 改 |
| `interfaces/webchat/src/state/connection.rs` | `ConnectionPhase::Failed` 携 `ConnectionFailure` | 改 |
| `interfaces/webchat/src/context.rs` | `connection_failure` 信号、WS 超时、handshake 三态、reconnect 分支、清旧 token | 改 |
| `interfaces/webchat/locales/{en,zh}.json` | 连接错误文案 key | 改 |
| `interfaces/webchat/src/components/boot_check_gate.rs` | 分类文案 | 改 |
| `interfaces/webchat/src/components/service_blocking_gate.rs` | 分类文案 + lite-remote 去 Retry | 改 |
| `interfaces/webchat/src/components/token_wall.rs` | 清旧 token + 失效/首次双文案 | 改 |
| `desktop/shell/src/main.rs` | 放开 Supervisor cfg + lite remote 循环 | 改 |
| `src/gateway/events/frame.rs` | `GatewayEventFrame::TokenRotated` 变体 + topic | 改 |
| `src/bin/aleph-server/commands/start/mod.rs` | rotate 闭包捕获 event_bus 发帧 | 改 |
| `src/gateway/server/handler.rs` | 事件臂拦截 TokenRotated:关远程/忽略 loopback | 改 |

---

## Phase A — 失败模型(shared-ui-logic,纯函数 host-testable)

### Task 1: `ConnectionFailure` + `classify()`

**Files:**
- Create: `shared/ui_logic/src/connection/failure.rs`
- Modify: `shared/ui_logic/src/connection/mod.rs`
- Test: 内联 `#[cfg(test)]` 于 `failure.rs`

**Interfaces:**
- Produces:
  - `enum ConnectionFailure { Unreachable { detail: String }, Timeout { detail: String }, AuthRequired, Dropped { detail: String }, Unknown { detail: String } }`(derive `Clone, Debug, PartialEq, Eq`)
  - `enum FailureStage { BeforeOpen, AfterOpen, Handshake, RpcTimeout }`(derive `Clone, Copy, Debug, PartialEq, Eq`)
  - `fn classify(stage: FailureStage, close_reason: Option<&str>, needs_token: bool) -> ConnectionFailure`
  - `impl ConnectionFailure { pub const fn should_retry(&self) -> bool; pub const fn i18n_key(&self) -> &'static str; }`

- [ ] **Step 1: Write the failing test**

在 `shared/ui_logic/src/connection/failure.rs` 末尾(先建文件含下方实现骨架,再加测试——但按 TDD 先写测试块,实现留空让它编译失败):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_token_is_auth_required_regardless_of_stage() {
        assert_eq!(classify(FailureStage::Handshake, None, true), ConnectionFailure::AuthRequired);
        assert_eq!(classify(FailureStage::AfterOpen, Some("whatever"), true), ConnectionFailure::AuthRequired);
    }

    #[test]
    fn token_rotated_close_is_auth_required() {
        assert_eq!(classify(FailureStage::AfterOpen, Some("token_rotated"), false), ConnectionFailure::AuthRequired);
    }

    #[test]
    fn before_open_failure_is_unreachable() {
        assert_eq!(
            classify(FailureStage::BeforeOpen, None, false),
            ConnectionFailure::Unreachable { detail: String::new() }
        );
    }

    #[test]
    fn rpc_timeout_stage_is_timeout() {
        assert_eq!(
            classify(FailureStage::RpcTimeout, None, false),
            ConnectionFailure::Timeout { detail: String::new() }
        );
    }

    #[test]
    fn after_open_drop_is_dropped() {
        assert_eq!(
            classify(FailureStage::AfterOpen, Some("WebSocket closed: code=1006 reason="), false),
            ConnectionFailure::Dropped { detail: "WebSocket closed: code=1006 reason=".to_string() }
        );
    }

    #[test]
    fn auth_required_does_not_retry_others_do() {
        assert!(!ConnectionFailure::AuthRequired.should_retry());
        assert!(ConnectionFailure::Unreachable { detail: String::new() }.should_retry());
        assert!(ConnectionFailure::Timeout { detail: String::new() }.should_retry());
        assert!(ConnectionFailure::Dropped { detail: String::new() }.should_retry());
    }

    #[test]
    fn i18n_keys_are_stable() {
        assert_eq!(ConnectionFailure::Unreachable { detail: String::new() }.i18n_key(), "unreachable");
        assert_eq!(ConnectionFailure::Timeout { detail: String::new() }.i18n_key(), "timeout");
        assert_eq!(ConnectionFailure::AuthRequired.i18n_key(), "auth_required");
        assert_eq!(ConnectionFailure::Dropped { detail: String::new() }.i18n_key(), "dropped");
        assert_eq!(ConnectionFailure::Unknown { detail: String::new() }.i18n_key(), "unknown");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p shared-ui-logic --lib connection::failure`
Expected: FAIL,编译错误 `cannot find function classify` / `ConnectionFailure not found`。

- [ ] **Step 3: Write minimal implementation**

在 `failure.rs` 顶部(测试块之前)写:

```rust
//! Typed connection-failure classification.
//!
//! Collapses the opaque `connection_error: String` into a value that drives
//! UI copy, retry policy, and the lite-shell handoff. Pure + host-testable —
//! no wasm, no Leptos. Browsers report almost every WebSocket failure as
//! close code 1006, so classification keys off *which stage* failed plus the
//! `needs_token` verdict and known close reasons (e.g. `token_rotated`),
//! never on the close code alone.

/// Why a connection attempt or live connection ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionFailure {
    /// WS never opened / TCP unreachable / DNS failure → "check network/address".
    Unreachable { detail: String },
    /// WS opened but the server went silent / an RPC timed out.
    Timeout { detail: String },
    /// `connect` RPC reported `needs_token`, or the server closed us with
    /// `token_rotated` → re-enter the Gateway token (login wall).
    AuthRequired,
    /// A previously-healthy connection dropped → transient, auto-reconnect.
    Dropped { detail: String },
    /// Anything we can't place — surface the raw detail verbatim.
    Unknown { detail: String },
}

/// The point in the connect lifecycle a failure surfaced at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureStage {
    /// Before the WebSocket reached OPEN (connect()/open timeout).
    BeforeOpen,
    /// After OPEN — a live socket dropped.
    AfterOpen,
    /// During the `connect` handshake RPC (transport-level error).
    Handshake,
    /// An RPC exceeded its timeout without the socket closing.
    RpcTimeout,
}

/// Pure classification. `close_reason` is the WS close reason (or transport
/// error string) when available; `needs_token` is the handshake verdict.
#[must_use]
pub fn classify(stage: FailureStage, close_reason: Option<&str>, needs_token: bool) -> ConnectionFailure {
    if needs_token {
        return ConnectionFailure::AuthRequired;
    }
    if matches!(close_reason, Some(r) if r.contains("token_rotated")) {
        return ConnectionFailure::AuthRequired;
    }
    let detail = close_reason.unwrap_or_default().to_string();
    match stage {
        FailureStage::BeforeOpen | FailureStage::Handshake => ConnectionFailure::Unreachable { detail },
        FailureStage::RpcTimeout => ConnectionFailure::Timeout { detail },
        FailureStage::AfterOpen => ConnectionFailure::Dropped { detail },
    }
}

impl ConnectionFailure {
    /// Whether the reconnect loop should keep retrying this failure.
    /// `AuthRequired` is terminal-for-now: retrying the same bad token is wasted.
    #[must_use]
    pub const fn should_retry(&self) -> bool {
        !matches!(self, Self::AuthRequired)
    }

    /// Stable suffix used to build the i18n key for this failure's copy.
    #[must_use]
    pub const fn i18n_key(&self) -> &'static str {
        match self {
            Self::Unreachable { .. } => "unreachable",
            Self::Timeout { .. } => "timeout",
            Self::AuthRequired => "auth_required",
            Self::Dropped { .. } => "dropped",
            Self::Unknown { .. } => "unknown",
        }
    }
}
```

在 `mod.rs` 加:

```rust
pub mod failure;
pub use failure::{classify, ConnectionFailure, FailureStage};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p shared-ui-logic --lib connection::failure`
Expected: PASS（7 tests）。

- [ ] **Step 5: Commit**

```bash
git add shared/ui_logic/src/connection/failure.rs shared/ui_logic/src/connection/mod.rs
git commit -m "shared-ui-logic: add typed ConnectionFailure classification"
```

---

### Task 2: `ReconnectStrategy` 抖动

**Files:**
- Modify: `shared/ui_logic/src/connection/reconnect.rs`
- Test: 内联 `#[cfg(test)]` 于 `reconnect.rs`

**Interfaces:**
- Consumes: 现有 `ReconnectStrategy::new(max_attempts, base_delay_ms)` / `next_delay()`。
- Produces: `fn next_delay_jittered(&mut self, jitter_permille: u64) -> Option<u64>`（确定性抖动,入参可测;`jitter_permille` 取 0..=1000 表示 0..100% 的下偏比例）。

> 说明:WASM 无 `Math::random` 的纯函数版本,真实抖动比例由调用方(context.rs)用 `js_sys::Math::random()` 生成后以千分比传入,使本函数保持纯净可测(呼应 P8 纯逻辑可测)。

- [ ] **Step 1: Write the failing test**

在 `reconnect.rs` 的 `#[cfg(test)] mod tests`(若无则新建)加:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_zero_equals_base_delay() {
        let mut s = ReconnectStrategy::new(5, 1000);
        assert_eq!(s.next_delay_jittered(0), Some(1000));
    }

    #[test]
    fn jitter_subtracts_proportional_fraction_only_downward() {
        let mut s = ReconnectStrategy::new(5, 1000);
        // 100 permille = 10% 下偏 → 1000 - 100 = 900,绝不超过 base。
        assert_eq!(s.next_delay_jittered(100), Some(900));
    }

    #[test]
    fn jitter_respects_attempt_exhaustion() {
        let mut s = ReconnectStrategy::new(1, 1000);
        assert!(s.next_delay_jittered(50).is_some());
        assert_eq!(s.next_delay_jittered(50), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p shared-ui-logic --lib connection::reconnect`
Expected: FAIL,`no method named next_delay_jittered`。

- [ ] **Step 3: Write minimal implementation**

在 `impl ReconnectStrategy` 内 `next_delay` 之后加:

```rust
    /// Like [`next_delay`], but shaves a deterministic *downward* fraction off
    /// the delay to avoid every client re-connecting in lockstep after a server
    /// restart. `jitter_permille` is 0..=1000 (0 = no jitter, 100 = minus 10%).
    /// Only ever reduces the delay, so it can never exceed the backoff ceiling.
    pub fn next_delay_jittered(&mut self, jitter_permille: u64) -> Option<u64> {
        let base = self.next_delay()?;
        let permille = jitter_permille.min(1000);
        let cut = base.saturating_mul(permille) / 1000;
        Some(base.saturating_sub(cut))
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p shared-ui-logic --lib connection::reconnect`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add shared/ui_logic/src/connection/reconnect.rs
git commit -m "shared-ui-logic: add deterministic downward jitter to ReconnectStrategy"
```

---

## Phase B — webchat 连接接线(`aleph-panel`)

### Task 3: `ConnectionPhase::Failed` 携类型化原因

**Files:**
- Modify: `interfaces/webchat/src/state/connection.rs`
- Test: 内联 `#[cfg(test)]`（已存在,扩展之）

**Interfaces:**
- Consumes: `shared_ui_logic::connection::ConnectionFailure`（Task 1）。
- Produces: `ConnectionPhase::Failed { failure: ConnectionFailure }`(原 `{ reason: String }` 改为类型化);`derive(stage_failure: Option<ConnectionFailure>, ...)` 签名调整见下。

> 注:现有 `derive` 入参是 `connection_error: Option<&str>`。为最小改动,**保留** `derive` 现签名但内部改为构造 `Failed { failure }`——失败时用 `ConnectionFailure::Unknown { detail }` 兜底包裹该字符串;真正的类型化失败由 context.rs 在 Task 5/6 直接 set 到新信号(Task 4),Gate 读新信号优先、`ConnectionPhase` 仅作 fallback。这样 Task 3 不破坏现有大量 `derive` 调用点。

- [ ] **Step 1: Write the failing test**

修改 `connection.rs` 现有测试 `failed_after_max_attempts` 与 `explicit_error_during_boot_surfaces_immediately`,并新增:

```rust
    #[test]
    fn failed_wraps_error_string_as_unknown() {
        use shared_ui_logic::connection::ConnectionFailure;
        let p = ConnectionPhase::derive(false, false, 5, Some("WebSocket closed"), true);
        assert_eq!(
            p,
            ConnectionPhase::Failed {
                failure: ConnectionFailure::Unknown { reason_into() }
            }
        );
    }
```

> 把上面两处旧测试里的 `ConnectionPhase::Failed { reason: "...".into() }` 改成 `ConnectionPhase::Failed { failure: ConnectionFailure::Unknown { detail: "...".to_string() } }`。`reason_into()` 仅示意,实际写 `detail: "WebSocket closed".to_string()`。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-panel --lib state::connection`
Expected: FAIL,`Failed` 变体字段不匹配 / `ConnectionFailure` 未导入。

- [ ] **Step 3: Write minimal implementation**

在 `connection.rs`:
1. 顶部加 `use shared_ui_logic::connection::ConnectionFailure;`
2. 改枚举变体:`Failed { failure: ConnectionFailure },`
3. 改 `derive` 内 `connection_error` 分支:

```rust
        if let Some(reason) = connection_error {
            return Self::Failed {
                failure: ConnectionFailure::Unknown { detail: reason.to_string() },
            };
        }
```
4. `is_pre_ready` 不变(仍只匹配 Initial/Connecting)。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-panel --lib state::connection`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/state/connection.rs
git commit -m "panel: carry typed ConnectionFailure in ConnectionPhase::Failed"
```

---

### Task 4: `connection_failure` 信号 + WS 打开超时

**Files:**
- Modify: `interfaces/webchat/src/context.rs`

**Interfaces:**
- Consumes: `shared_ui_logic::connection::{classify, ConnectionFailure, FailureStage}`;`gloo_timers::future::TimeoutFuture`、`futures::future::{select, Either}`(均已 import 或同 crate 可用)。
- Produces: `DashboardState.connection_failure: RwSignal<Option<ConnectionFailure>>`;私有常量 `const WS_OPEN_TIMEOUT_MS: u32 = 8_000;`;私有方法 `fn set_failure(&self, f: ConnectionFailure)`(同时写 `connection_failure` 与派生 `connection_error` 字符串)。

- [ ] **Step 1: 加字段与常量(无独立单测,WASM 异步;靠 cargo check + 后续手动 e2e)**

在 `DashboardState` struct 加字段:

```rust
    /// Typed classification of the latest connection failure. Single source of
    /// truth; `connection_error` (String) is derived from it for legacy readers.
    pub connection_failure: RwSignal<Option<ConnectionFailure>>,
```

`DashboardState::new()` 初始化:`connection_failure: RwSignal::new(None),`
顶部 import:`use shared_ui_logic::connection::{classify, ConnectionFailure, FailureStage};`
文件级常量:`const WS_OPEN_TIMEOUT_MS: u32 = 8_000;`

加辅助方法(impl 内):

```rust
    /// Record a typed failure and derive its legacy string. Centralised so the
    /// two never drift.
    fn set_failure(&self, f: ConnectionFailure) {
        let legacy = match &f {
            ConnectionFailure::AuthRequired => "auth required".to_string(),
            ConnectionFailure::Unreachable { detail }
            | ConnectionFailure::Timeout { detail }
            | ConnectionFailure::Dropped { detail }
            | ConnectionFailure::Unknown { detail } => detail.clone(),
        };
        self.connection_failure.set(Some(f));
        self.connection_error.set(Some(legacy));
    }
```

- [ ] **Step 2: WS 打开超时——改 `connect()` 的 connector.connect 调用**

将 `connect()` 内:

```rust
        match connector.connect(&url).await {
            Ok(()) => {
```

替换为(用已 import 的 `select`/`Either`/`TimeoutFuture`):

```rust
        use futures::future::{select, Either};
        let open = select(
            Box::pin(connector.connect(&url)),
            TimeoutFuture::new(WS_OPEN_TIMEOUT_MS),
        )
        .await;
        let open_result = match open {
            Either::Left((res, _)) => res,
            Either::Right(((), _)) => {
                // TCP may be up but WS upgrade hung — fail closed instead of
                // spinning the boot gate forever.
                Err(ConnectionError::ConnectFailed("WebSocket open timed out".into()))
            }
        };
        match open_result {
            Ok(()) => {
```

> `ConnectionError` 已在 context.rs 通过 `connector` 路径可见;若未直接 import,加 `use shared_ui_logic::connection::ConnectionError;`。

- [ ] **Step 3: 失败分支改用 `set_failure` + classify**

将 `connect()` 末尾的 `Err(e) =>` 分支:

```rust
            Err(e) => {
                self.is_connected.set(false);
                let error_msg = e.to_string();
                self.connection_error.set(Some(error_msg.clone()));
                Err(error_msg)
            }
```

替换为:

```rust
            Err(e) => {
                self.is_connected.set(false);
                let detail = e.to_string();
                self.set_failure(classify(FailureStage::BeforeOpen, Some(&detail), false));
                Err(detail)
            }
```

- [ ] **Step 4: Run check**

Run: `cargo check -p aleph-panel`
Expected: 编译通过(无 unused import / 类型错误)。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/context.rs
git commit -m "panel: add connection_failure signal and bound WS-open with an 8s timeout"
```

---

### Task 5: handshake 三态 + connect() 分支(NeedsToken 不标已连接、清旧 token)

**Files:**
- Modify: `interfaces/webchat/src/context.rs`

**Interfaces:**
- Consumes: Task 4 的 `set_failure`、`connection_failure`;现有 `read_gateway_token`、`persist_gateway_token`、`scrub_token_from_url`、`needs_token`。
- Produces: `enum Handshake { Authorized, NeedsToken, Failed(ConnectionFailure) }`;`async fn handshake(&self) -> Handshake`(替换原 `-> Result<(), String>`);wasm-only `fn clear_gateway_token()`。

- [ ] **Step 1: 加 `clear_gateway_token`(wasm + 非 wasm 双实现,镜像现有 token 函数)**

紧邻 `persist_gateway_token` 加:

```rust
/// Drop the persisted Gateway token. Called when a previously-stored token is
/// rejected (rotation / mismatch) so it can't silently re-fail on the next
/// load and the login box starts empty.
#[cfg(target_arch = "wasm32")]
fn clear_gateway_token() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.remove_item(GATEWAY_TOKEN_KEY);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_gateway_token() {}
```

- [ ] **Step 2: 改 `handshake()` 返回三态**

把 `handshake` 整体替换:

```rust
    /// Handshake outcome — distinguishes "authorized", "needs a token"
    /// (login wall, NOT a transport failure), and a transport-level failure.
    async fn handshake(&self) -> Handshake {
        let (device_id, device_name) = panel_device_identity();
        let token = read_gateway_token();
        let mut params = serde_json::json!({
            "device_id": device_id,
            "device_name": device_name,
        });
        if let Some(t) = token.as_ref() {
            params["token"] = serde_json::json!(t);
        }
        let resp = match self.rpc_call("connect", params).await {
            Ok(r) => r,
            Err(e) => return Handshake::Failed(classify(FailureStage::Handshake, Some(&e), false)),
        };
        self.capture_role(&resp);
        if resp.get("authorized").and_then(serde_json::Value::as_bool) == Some(true) {
            if let Some(t) = token {
                persist_gateway_token(&t);
            }
            scrub_token_from_url();
            Handshake::Authorized
        } else {
            // Reachable but unauthorized: a stale/rotated/mismatched token (if
            // any) must not silently re-fail next load. Login wall takes over.
            clear_gateway_token();
            Handshake::NeedsToken
        }
    }
```

在文件内(`DashboardState` impl 上方或附近)加枚举:

```rust
enum Handshake {
    Authorized,
    NeedsToken,
    Failed(ConnectionFailure),
}
```

- [ ] **Step 3: 改 `connect()` 里 handshake 调用点分支**

把:

```rust
                let handshake_state = *self;
                match handshake_state.handshake().await {
                    Ok(()) => { /* ...happy path... */ }
                    Err(e) => { /* ...surface... */ }
                }
```

替换为:

```rust
                let handshake_state = *self;
                match handshake_state.handshake().await {
                    Handshake::Authorized => {
                        self.is_connected.set(true);
                        self.connection_error.set(None);
                        self.connection_failure.set(None);
                        self.reconnect_count.set(0);
                        self.is_reconnecting.set(false);
                        self.has_connected_once.set(true);

                        let state_for_subscribe = *self;
                        spawn_local(async move {
                            if let Err(e) = state_for_subscribe.subscribe_topic("config.**").await {
                                web_sys::console::error_1(
                                    &format!("Failed to subscribe to config events: {e}").into(),
                                );
                            }
                        });
                        Ok(())
                    }
                    Handshake::NeedsToken => {
                        // Reachable but walled — do NOT mark connected and do NOT
                        // spawn subscriptions (only `connect` is allowed unauthorized).
                        self.is_connected.set(false);
                        self.needs_token.set(true);
                        self.set_failure(ConnectionFailure::AuthRequired);
                        // Returning Ok keeps the boot/reconnect path from treating
                        // the wall as a transport failure; TokenWall (z-100) covers it.
                        Ok(())
                    }
                    Handshake::Failed(f) => {
                        self.is_connected.set(false);
                        let msg = match &f {
                            ConnectionFailure::Unreachable { detail }
                            | ConnectionFailure::Timeout { detail }
                            | ConnectionFailure::Dropped { detail }
                            | ConnectionFailure::Unknown { detail } => detail.clone(),
                            ConnectionFailure::AuthRequired => "auth required".to_string(),
                        };
                        self.set_failure(f);
                        Err(msg)
                    }
                }
```

- [ ] **Step 4: Run check**

Run: `cargo check -p aleph-panel`
Expected: 编译通过。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/context.rs
git commit -m "panel: make handshake three-state and clear rejected tokens"
```

---

### Task 6: reconnect() 差异化(复用 ReconnectStrategy + AuthRequired 立即停)

**Files:**
- Modify: `interfaces/webchat/src/context.rs`

**Interfaces:**
- Consumes: `shared_ui_logic::connection::ReconnectStrategy`(Task 2 的 `next_delay_jittered`);`MAX_RECONNECT_ATTEMPTS`(来自 `crate::state::connection`)。
- Produces: 重写后的 `pub async fn reconnect(&self) -> Result<(), String>`。

- [ ] **Step 1: 重写 `reconnect()`**

把整个 `reconnect()` 替换为:

```rust
    /// Attempt to reconnect. Differentiated by failure type: an `AuthRequired`
    /// failure breaks out immediately to the login wall (retrying the same bad
    /// token is wasted); every other class uses exponential backoff with
    /// downward jitter, reusing the shared `ReconnectStrategy`.
    pub async fn reconnect(&self) -> Result<(), String> {
        use crate::state::connection::MAX_RECONNECT_ATTEMPTS;
        use shared_ui_logic::connection::{ConnectionFailure, ReconnectStrategy};

        if matches!(self.connection_failure.get_untracked(), Some(ConnectionFailure::AuthRequired)) {
            self.needs_token.set(true);
            self.is_reconnecting.set(false);
            return Ok(());
        }

        self.is_reconnecting.set(true);
        let mut strategy = ReconnectStrategy::new(MAX_RECONNECT_ATTEMPTS, 1000);
        let mut attempt: u32 = 0;
        while let Some(delay) = {
            // ~±10% downward jitter; Math::random is wasm-only.
            #[cfg(target_arch = "wasm32")]
            let permille = (js_sys::Math::random() * 100.0) as u64;
            #[cfg(not(target_arch = "wasm32"))]
            let permille = 0u64;
            strategy.next_delay_jittered(permille)
        } {
            self.reconnect_count.set(attempt);
            TimeoutFuture::new(delay as u32).await;
            match self.connect().await {
                Ok(()) => {
                    // connect() returns Ok for both Authorized and NeedsToken;
                    // if it walled, stop here (TokenWall covers it).
                    self.is_reconnecting.set(false);
                    return Ok(());
                }
                Err(_) => {
                    attempt += 1;
                }
            }
        }

        // Budget exhausted — leave the classified failure in place (connect()
        // already set it) so the gate shows the right copy.
        self.reconnect_count.set(MAX_RECONNECT_ATTEMPTS);
        self.is_reconnecting.set(false);
        Err("Reconnection failed".to_string())
    }
```

> 删除原 `reconnect()` 里手写的 `let delay_ms = (1000 * 2_u32.pow(attempt)).min(16000);` 退避块——单一真相源移到 `ReconnectStrategy`。

- [ ] **Step 2: Run check**

Run: `cargo check -p aleph-panel`
Expected: 编译通过(确认 `js_sys` 在 wasm 下可用——context.rs 已用 `js_sys::Date`)。

- [ ] **Step 3: Run existing host tests(确保未回归 ConnectionPhase)**

Run: `cargo test -p aleph-panel --lib state::connection`
Expected: PASS。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/context.rs
git commit -m "panel: differentiate reconnect by failure type, reuse ReconnectStrategy"
```

---

## Phase C — Gate 文案与补救 + i18n

### Task 7: i18n key(en.json + zh.json 同步)

**Files:**
- Modify: `interfaces/webchat/locales/en.json`, `interfaces/webchat/locales/zh.json`

**Interfaces:**
- Produces: 新 namespace `conn_error` 与 `boot_gate`/`service_gate`/`common` 增补 key,供 Task 8/9/10 引用。

- [ ] **Step 1: 在 en.json 顶层加 `conn_error` 对象,并给 token_wall 增补 key**

`en.json` 顶层加(放在 `boot_gate` 同级):

```json
  "conn_error": {
    "unreachable_title": "Server not found",
    "unreachable_body": "Couldn't reach the server. Check your network connection, and confirm the server address and port are correct.",
    "timeout_title": "Server not responding",
    "timeout_body": "Connected to the server but it isn't responding — it may be restarting. Please retry shortly.",
    "dropped_body": "Connection interrupted — reconnecting…",
    "lite_relocating": "Server unreachable — reconnecting you to the server…"
  },
```

`common` 内增补(供 TokenWall 失效场景):

```json
    "token_wall_instruction_rejected": "The server's access token was rotated or is no longer valid. Enter the new Gateway token to reconnect.",
```

- [ ] **Step 2: 在 zh.json 加完全对应的 key(顺序/层级一致)**

`zh.json` 顶层加:

```json
  "conn_error": {
    "unreachable_title": "找不到服务器",
    "unreachable_body": "无法连接到服务器。请检查网络连接,并确认服务器地址和端口是否正确。",
    "timeout_title": "服务器无响应",
    "timeout_body": "已连上服务器但它没有回应,可能正在重启。请稍候重试。",
    "dropped_body": "连接中断,正在重连…",
    "lite_relocating": "服务器不可达,正在为你重新连接服务器…"
  },
```

`common` 内增补:

```json
    "token_wall_instruction_rejected": "服务器的访问令牌已更新或失效,请输入新的 Gateway token 重新连接。",
```

- [ ] **Step 3: Run check（leptos_i18n 编译期校验两 locale 对齐)**

Run: `cargo check -p aleph-panel`
Expected: 编译通过。若报 "missing key in locale" → 两文件 key 不一致,补齐。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: add i18n keys for classified connection errors"
```

---

### Task 8: BootCheckGate 分类文案

**Files:**
- Modify: `interfaces/webchat/src/components/boot_check_gate.rs`

**Interfaces:**
- Consumes: `DashboardState.connection_failure`（Task 4）;Task 7 的 `conn_error.*` key;`shared_ui_logic::connection::ConnectionFailure`。

- [ ] **Step 1: 改 Failed 分支按 failure 选标题/正文**

在 `BootCheckGate` 内,把现有 `ConnectionPhase::Failed { reason }` 分支改为读 `connection_failure` 信号决定标题/正文。先在组件顶部取信号:

```rust
    let failure = state.connection_failure;
```

把 `match phase.get() { ConnectionPhase::Failed { .. } => { ... } }` 的 Failed 臂正文部分改为根据 `failure.get()` 选 key(其余结构/Retry 按钮不变):

```rust
                        ConnectionPhase::Failed { .. } => {
                            use shared_ui_logic::connection::ConnectionFailure;
                            let (title_key, body_key) = match failure.get() {
                                Some(ConnectionFailure::Timeout { .. }) =>
                                    ("conn_error.timeout_title", "conn_error.timeout_body"),
                                _ =>
                                    ("conn_error.unreachable_title", "conn_error.unreachable_body"),
                            };
                            // 用 t_string! 取对应文案渲染标题与正文;Retry 按钮保留原逻辑。
                            view! {
                                <h2 class="text-xl font-semibold text-text-primary">
                                    { move || match failure.get() {
                                        Some(ConnectionFailure::Timeout { .. }) => t_string!(i18n, conn_error.timeout_title).to_string(),
                                        _ => t_string!(i18n, conn_error.unreachable_title).to_string(),
                                    }}
                                </h2>
                                <p class="mt-2 text-sm text-text-secondary">
                                    { move || match failure.get() {
                                        Some(ConnectionFailure::Timeout { .. }) => t_string!(i18n, conn_error.timeout_body).to_string(),
                                        _ => t_string!(i18n, conn_error.unreachable_body).to_string(),
                                    }}
                                </p>
                                // 保留原 trouble_hint + Retry 按钮(原代码原样)
                                // ...（复制原 Failed 臂中的 hint <p> 与 <button> Retry 块）
                            }.into_any()
                        }
```

> 实现注意:把原 Failed 臂里的 `trouble_hint` 提示 `<p>` 与 Retry `<button>`（含 `is_retrying`/`reconnect()` 逻辑）原样保留在新 view 末尾——只替换标题与正文两段文案来源,不动重试按钮。`title_key`/`body_key` 两个绑定可省略(上面直接内联 match),避免未使用变量告警。

- [ ] **Step 2: Run check**

Run: `cargo check -p aleph-panel`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/boot_check_gate.rs
git commit -m "panel: BootCheckGate shows classified network/timeout copy"
```

---

### Task 9: ServiceBlockingGate 分类文案 + lite-remote 去 Retry

**Files:**
- Modify: `interfaces/webchat/src/components/service_blocking_gate.rs`

**Interfaces:**
- Consumes: `DashboardState.connection_failure`;`crate::components::connection_status` 的 `resolve_target_label`/loopback 判断逻辑——复用 `is_loopback_host`(若为私有,改用同样的 `window().location().host()` 判定本地)。

- [ ] **Step 1: 判定是否 lite-remote(非 loopback origin)**

在 `ServiceBlockingGate` 顶部加:

```rust
    let failure = state.connection_failure;
    // Remote origin (lite shell / browser hitting a LAN IP) — for an
    // Unreachable failure the native supervisor will relocate to connect.html,
    // so a Retry against the dead origin is useless here.
    let is_remote_origin = {
        let host = web_sys::window()
            .and_then(|w| w.location().host().ok())
            .unwrap_or_default();
        !(host.is_empty()
            || host.starts_with("127.0.0.1")
            || host.starts_with("localhost")
            || host.starts_with("[::1]"))
    };
```

- [ ] **Step 2: Unreachable + remote → 换文案、隐藏 Retry**

在 body `<p>` 处,按 `failure.get()` + `is_remote_origin` 选文案;并把 Retry `<button>` 包进 `<Show when=...>` 只在"非(remote 且 Unreachable)"时显示:

```rust
                    // 正文:remote + Unreachable 显示 relocating 文案,否则现有 service_gate 文案
                    {move || {
                        use shared_ui_logic::connection::ConnectionFailure;
                        if is_remote_origin && matches!(failure.get(), Some(ConnectionFailure::Unreachable { .. })) {
                            t_string!(i18n, conn_error.lite_relocating).to_string()
                        } else {
                            format!(
                                "{}{}{}",
                                t_string!(i18n, service_gate.body_prefix),
                                state.reconnect_count.get(),
                                t_string!(i18n, service_gate.body_suffix),
                            )
                        }
                    }}
```

Retry 按钮外包:

```rust
                    <Show
                        when=move || {
                            use shared_ui_logic::connection::ConnectionFailure;
                            !(is_remote_origin && matches!(failure.get(), Some(ConnectionFailure::Unreachable { .. })))
                        }
                        fallback=|| ()
                    >
                        // ...原 Retry <button> 整块...
                    </Show>
```

> "打开日志"按钮保持始终可见(原逻辑不动)。

- [ ] **Step 3: Run check**

Run: `cargo check -p aleph-panel`
Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/service_blocking_gate.rs
git commit -m "panel: ServiceBlockingGate hides dead-origin Retry for lite-remote unreachable"
```

---

### Task 10: TokenWall 清旧 token + 失效/首次双文案

**Files:**
- Modify: `interfaces/webchat/src/components/token_wall.rs`

**Interfaces:**
- Consumes: `DashboardState.connection_failure`(AuthRequired);Task 7 的 `common.token_wall_instruction_rejected`。
- 说明:`handshake`(Task 5)已在被拒时 `clear_gateway_token()`,故墙弹出时 localStorage 已无失效 token,输入框天然从空开始——本 task 只处理**文案区分**。

- [ ] **Step 1: 失效场景用 rejected 文案**

`TokenWall` 内,把固定的 `token_wall_instruction` 改为按"是否曾被拒"选。判据:`connection_failure == AuthRequired` 且页面带过持久 token 痕迹较难取——简化为**只要 needs_token 因 AuthRequired 触发就用 rejected 文案,首连(无 connection_failure)用原 instruction**:

```rust
    let failure = state.connection_failure;
    // ...
                    <p class="text-sm text-text-secondary mb-6">
                        {move || {
                            use shared_ui_logic::connection::ConnectionFailure;
                            if matches!(failure.get(), Some(ConnectionFailure::AuthRequired)) {
                                t!(i18n, common.token_wall_instruction_rejected)
                            } else {
                                t!(i18n, common.token_wall_instruction)
                            }
                        }}
                    </p>
```

> 取舍:首连远程时 handshake 也会走 NeedsToken→set_failure(AuthRequired),因此首连也会显示 rejected 文案。为区分"首次"与"失效",改判据为:**有 persisted token 痕迹**。简单做法——在 `read_gateway_token().is_some()` 时为失效、否则首次。但 token_wall.rs 在 wasm 下可调用 context 的判定。**最小可行**:导出一个 `pub(crate) fn had_stored_token() -> bool`(wasm:`read_gateway_token().is_some()`;非 wasm:false)于 context.rs,TokenWall 用它选文案。

补充:在 `context.rs` 加:

```rust
/// Whether a Gateway token was present at load (URL or localStorage). Used by
/// the login wall to phrase "enter token" (first time) vs "token rejected".
#[cfg(target_arch = "wasm32")]
pub(crate) fn had_stored_token() -> bool {
    read_gateway_token().is_some()
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn had_stored_token() -> bool { false }
```

> 注意:`clear_gateway_token()`(Task 5)在被拒时清掉 token,会让 `had_stored_token()` 在墙弹出后返回 false。因此判据改为在 **handshake 决定 NeedsToken 的那一刻** 记录一个信号更可靠:在 `DashboardState` 加 `pub token_was_rejected: RwSignal<bool>`,handshake 的 NeedsToken 分支里 `self.token_was_rejected.set(had_stored_token_before_clear)`。**实现顺序**:在 Task 5 的 handshake NeedsToken 分支,`clear_gateway_token()` 之前先 `let had = read_gateway_token().is_some();` 再 set 信号。TokenWall 读 `state.token_was_rejected`。

**修订 Task 5 NeedsToken 分支**(回填):在 `clear_gateway_token()` 调用处改为:

```rust
        } else {
            let had = read_gateway_token().is_some();
            clear_gateway_token();
            // 记录供 TokenWall 选文案
            // (token_was_rejected 信号在 Task 10 加入 DashboardState)
            Handshake::NeedsToken
        }
```

并在 connect() 的 `Handshake::NeedsToken =>` 分支 set:`self.token_was_rejected.set(/* had */);`——为传递 `had`,把 `Handshake::NeedsToken` 改带字段:`NeedsToken { was_rejected: bool }`。

- [ ] **Step 2: DashboardState 加 `token_was_rejected` 信号,Handshake 带字段**

- `DashboardState` 加 `pub token_was_rejected: RwSignal<bool>,`,`new()` 初始化 `RwSignal::new(false)`。
- `enum Handshake` 改 `NeedsToken { was_rejected: bool }`。
- handshake 的 else 分支返回 `Handshake::NeedsToken { was_rejected: read_gateway_token().is_some() }`(在 `clear_gateway_token()` 之前求值)。
- connect() 分支:`Handshake::NeedsToken { was_rejected } => { ...; self.token_was_rejected.set(was_rejected); ... }`。
- TokenWall 文案判据改用 `state.token_was_rejected.get()` 而非 connection_failure。

- [ ] **Step 3: Run check**

Run: `cargo check -p aleph-panel`
Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/token_wall.rs interfaces/webchat/src/context.rs
git commit -m "panel: TokenWall distinguishes first-time vs rejected-token copy"
```

---

## Phase D — Lite-shell 原生 supervisor(`aleph-desktop-shell`)

### Task 11: 放开 Supervisor 状态机 cfg + lite remote 循环

**Files:**
- Modify: `desktop/shell/src/main.rs`
- Test: 内联 `#[cfg(test)]`（扩展现有 Supervisor 测试)

**Interfaces:**
- Consumes: 现有 `Supervisor::new_remote`、`tick`、`SupervisorAction::ShowConnectionError`、`connect_setup::{target_reachable, show_lite_connect_page}`、`connection::load_target`、`HEALTH_POLL_INTERVAL`、`FAILURES_TO_DECLARE_DOWN`。
- Produces: 始终编译的 `Supervisor`/`DaemonHealth`/`SupervisorAction` 纯状态机;`#[cfg(not(feature = "embedded-core"))] async fn supervise_remote_lite(handle)`;在 lite setup 处 spawn 它。

- [ ] **Step 1: Write the failing test（纯状态机,remote 腿)**

在 main.rs 的 Supervisor 测试模块加(若测试模块当前 `#[cfg(feature="embedded-core")]`,Step 3 会一并放开):

```rust
    #[test]
    fn remote_supervisor_declares_down_and_shows_connection_page() {
        let mut sup = Supervisor::new_remote(true);
        // 健康期间 idle
        assert_eq!(sup.tick(true), SupervisorAction::Idle);
        // 连续失败累积到阈值后,Remote 腿要求 ShowConnectionError(不 Relaunch)
        let mut last = SupervisorAction::Idle;
        for _ in 0..FAILURES_TO_DECLARE_DOWN {
            last = sup.tick(false);
        }
        assert_eq!(last, SupervisorAction::ShowConnectionError);
    }

    #[test]
    fn remote_supervisor_recovers_without_relaunch() {
        let mut sup = Supervisor::new_remote(false);
        // Down→Up 回升应是 ReloadPanel,绝不 Relaunch(远程不归我们管)
        assert_eq!(sup.tick(true), SupervisorAction::ReloadPanel);
    }
```

- [ ] **Step 2: Run test to verify it fails (or fails to compile in lite build)**

Run: `cargo test -p aleph-desktop-shell --lib supervisor` （默认含 embedded-core——若 Supervisor 仍门控于 embedded-core,此步在默认特性下应能跑现有测试;新测试若引用未放开符号则编译失败)
Expected: 新测试通过/失败取决于符号可见性;若 `Supervisor` 仅 embedded-core 可见,默认特性下可编译,真正要验证的是 Step 3 放开后 **lite 构建** 也能见到状态机。

- [ ] **Step 3: 放开状态机 cfg,只门控 I/O 循环**

- 移除 `Supervisor`、`DaemonHealth`、`SupervisorAction`、`impl Supervisor`(`new`/`new_remote`/`for_target`/`down_action`/`tick`)上的 `#[cfg(feature = "embedded-core")]`——让纯状态机**始终编译**。
- 保留 `#[cfg(feature = "embedded-core")]` 于 `supervise_daemon`、`reveal_panel`、`show_daemon_error`、`show_connection_page`、`DaemonHealth` 的 daemon I/O 调用处(这些用 `daemon::*`,lite 编译不出)。
- `show_connection_page` 当前 embedded-core-only 且 lite 用 `connect_setup::show_lite_connect_page` 替代——无需复制。

新增 lite 循环:

```rust
/// Resident health loop for the panel-only (lite) shell. The full app uses
/// `supervise_daemon`; the lite shell hosts no daemon, so it only watches the
/// remote Gateway's reachability and, when it stays down past the budget,
/// relocates the webview to the bundled connect page (mDNS re-discovery +
/// manual address). Deliberately later than the panel's own ~31s reconnect
/// budget so a transient blip is recovered in-panel without yanking the user.
#[cfg(not(feature = "embedded-core"))]
async fn supervise_remote_lite(handle: tauri::AppHandle) {
    // Seed from a live probe so a reachable target isn't mistaken for recovery.
    let reachable = connect_setup::target_reachable(&connection::load_target()).await;
    let mut supervisor = Supervisor::new_remote(reachable);
    loop {
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
        let ready = connect_setup::target_reachable(&connection::load_target()).await;
        match supervisor.tick(ready) {
            SupervisorAction::ShowConnectionError => {
                tracing::warn!("remote Gateway unreachable — relocating to connect page");
                connect_setup::show_lite_connect_page(&handle);
            }
            // Remote recovered while we were on the connect page → re-point at it.
            SupervisorAction::ReloadPanel => {
                if let connection::ConnectionTarget::Remote(url) = connection::load_target() {
                    if let Some(window) = tauri::Manager::get_webview_window(&handle, "main") {
                        let _ = window.navigate(url);
                    }
                }
            }
            SupervisorAction::Idle | SupervisorAction::Relaunch => {}
        }
    }
}
```

- [ ] **Step 4: 校准节奏常量 + spawn lite 循环**

确认 `HEALTH_POLL_INTERVAL` 与 `FAILURES_TO_DECLARE_DOWN` 使 `HEALTH_POLL_INTERVAL × FAILURES_TO_DECLARE_DOWN ≳ 35s`(panel 重连预算 ≈ 31s)。若现值不足(例如 poll 3s × 阈值 3 = 9s),为 lite 路径单独定义:

```rust
#[cfg(not(feature = "embedded-core"))]
const LITE_REMOTE_POLL: std::time::Duration = std::time::Duration::from_secs(5);
```
并把 lite 循环里的 `HEALTH_POLL_INTERVAL` 换成 `LITE_REMOTE_POLL`,配合 `Supervisor::new_remote` 的 `FAILURES_TO_DECLARE_DOWN`(需 ≥7;若现值更小,在 lite 循环里改用本地阈值常量并据此 tick——保持状态机纯净,只在调用处计数)。

> 决策:若 `FAILURES_TO_DECLARE_DOWN` 现值 <7,**不改全局常量**(会影响 full-app relaunch 灵敏度)。改为 lite 循环本地累计:连续失败计到 `LITE_FAILURES_TO_RELOCATE = 7` 才调 `show_lite_connect_page`,`Supervisor` 只用于 ReloadPanel 回升判定。择其一实现并在 commit message 注明。

在 lite shell 的 setup(`#[cfg(not(feature = "embedded-core"))]` 的启动分支,靠近 `bring_target_online`/首启导航处)spawn:

```rust
        #[cfg(not(feature = "embedded-core"))]
        {
            let h = app.handle().clone();
            tauri::async_runtime::spawn(supervise_remote_lite(h));
        }
```

- [ ] **Step 5: Run test + lite check**

Run:
```
cargo test -p aleph-desktop-shell --lib supervisor
cargo check -p aleph-desktop-shell --no-default-features
```
Expected: 测试 PASS;lite 构建(`--no-default-features`)编译通过(状态机可见、`supervise_remote_lite` 编入)。

- [ ] **Step 6: Commit**

```bash
git add desktop/shell/src/main.rs
git commit -m "shell: add resident remote-health supervisor to the lite panel shell"
```

---

## Phase E — 网关 token 轮换主动踢下线(`alephcore`,安全红线)

### Task 12: `GatewayEventFrame::TokenRotated` 变体

**Files:**
- Modify: `src/gateway/events/frame.rs`
- Test: 内联 `#[cfg(test)]`（若无序列化测试,新增一个最小用例)

**Interfaces:**
- Produces: `GatewayEventFrame::TokenRotated`(payload-free,序列化为 `{"type":"token_rotated"}`);topic 字符串 `"gateway.token.rotated"`。

- [ ] **Step 1: Write the failing test**

在 frame.rs 测试模块加:

```rust
    #[test]
    fn token_rotated_serializes_with_snake_case_tag() {
        let f = GatewayEventFrame::TokenRotated;
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(json, r#"{"type":"token_rotated"}"#);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::events::frame::tests::token_rotated_serializes_with_snake_case_tag`
Expected: FAIL,`no variant TokenRotated`。

- [ ] **Step 3: Add variant + topic mapping**

在 `GatewayEventFrame` 枚举内(`AcpSessionsChanged` 附近)加:

```rust
    /// Emitted when the shared Gateway token is rotated. The connection loop
    /// intercepts it to close *remote* (token-authorized) sessions with
    /// 4001/`token_rotated`; loopback sessions ignore it. Payload-free.
    TokenRotated,
```

在 topic 字符串映射处(`Self::AcpSessionsChanged => "acp.sessions.changed",` 附近)加:

```rust
            Self::TokenRotated => "gateway.token.rotated",
```

> 若该 `match` 非穷尽会编译报错——补上即可。检查是否还有其它对 `GatewayEventFrame` 穷尽 match 的地方(如有,加 `Self::TokenRotated` 臂)。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::events::frame`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/events/frame.rs
git commit -m "gateway: add TokenRotated event frame variant"
```

---

### Task 13: rotate 闭包捕获 event_bus 发帧

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs`

**Interfaces:**
- Consumes: 作用域内 `event_bus`(L327 `let event_bus = server.event_bus().clone();`);`GatewayEventFrame::TokenRotated`(Task 12)。
- Produces: rotate 注册闭包在 handler 成功后 publish `TokenRotated`。纯 `handle_token_rotate(req)` 签名不变(R4)。

- [ ] **Step 1: 改 rotate 注册闭包**

把:

```rust
    server
        .handlers_mut()
        .register("gateway.token.rotate", |req| async move {
            alephcore::gateway::handlers::gateway_token::handle_token_rotate(req).await
        });
```

替换为:

```rust
    {
        let rotate_bus = event_bus.clone();
        server
            .handlers_mut()
            .register("gateway.token.rotate", move |req| {
                let bus = rotate_bus.clone();
                async move {
                    let resp = alephcore::gateway::handlers::gateway_token::handle_token_rotate(req).await;
                    // Only kick sessions when the rotation actually succeeded
                    // (success responses carry the new token in `result`).
                    if resp.error.is_none() {
                        bus.publish(alephcore::gateway::events::GatewayEventFrame::TokenRotated);
                    }
                    resp
                }
            });
    }
```

> 校验点:
> - `event_bus.clone()` 的类型支持 `.publish(frame)`——确认 `GatewayEventBus`/总线类型的发布方法名(可能是 `publish` / `send` / `broadcast`)。先 grep:`grep -n "fn publish\|fn send\|fn broadcast" src/gateway/event_emitter* src/gateway/events*`,用真实方法名替换 `.publish(...)`。
> - `JsonRpcResponse` 判成功的字段名(`error` / `is_error()`)——确认后替换 `resp.error.is_none()`。
> - `register` 闭包签名是否要求 `Fn`(不可 `move` 捕获后多次调用?)——`handlers_mut().register` 多为 `Fn + Clone`;`rotate_bus.clone()` 每次调用克隆,满足。若签名要求返回特定 Future 类型,按相邻 handler 写法对齐。

- [ ] **Step 2: Run check（scoped,避免全量构建)**

Run: `cargo check -p aleph-server`
Expected: 编译通过。若 `.publish`/`.error` 名不符,按 grep 结果修正。

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs
git commit -m "aleph-server: broadcast TokenRotated when the shared token is rotated"
```

---

### Task 14: 连接循环拦截 TokenRotated(关远程 / 忽略 loopback)+ 授权测试

**Files:**
- Modify: `src/gateway/server/handler.rs`
- Test: 内联 `#[cfg(test)]`（纯判定函数)

**Interfaces:**
- Consumes: 事件转发臂的 `event_json: String`;连接的 `ctx.client_ip.is_loopback()`。
- Produces: 纯判定 `fn rotated_should_close_remote(event_json: &str, is_loopback: bool) -> bool`(host-testable);事件臂据此 `Close(4001,"token_rotated")` + break。

- [ ] **Step 1: Write the failing test（纯判定函数,这是红线必测项)**

在 handler.rs 测试模块(文件末尾 `#[cfg(test)] mod tests`,若无则新建)加:

```rust
#[cfg(test)]
mod token_rotation_tests {
    use super::rotated_should_close_remote;

    const ROTATED: &str = r#"{"type":"token_rotated"}"#;

    #[test]
    fn remote_session_closes_on_token_rotated() {
        assert!(rotated_should_close_remote(ROTATED, /* is_loopback = */ false));
    }

    #[test]
    fn loopback_session_ignores_token_rotated() {
        assert!(!rotated_should_close_remote(ROTATED, /* is_loopback = */ true));
    }

    #[test]
    fn other_events_never_trigger_close() {
        assert!(!rotated_should_close_remote(r#"{"type":"acp_sessions_changed"}"#, false));
        assert!(!rotated_should_close_remote(r#"{"topic":"alerts.system"}"#, false));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::server::handler::token_rotation_tests`
Expected: FAIL,`cannot find function rotated_should_close_remote`。

- [ ] **Step 3: Add the pure predicate + wire the event arm**

在 handler.rs(模块级,函数区)加纯判定:

```rust
/// Whether this connection must be torn down because the shared token was
/// rotated. True only for a `token_rotated` event on a *remote* (non-loopback)
/// connection — loopback is always operator and never token-gated, so it is
/// unaffected. Pure for host testing.
fn rotated_should_close_remote(event_json: &str, is_loopback: bool) -> bool {
    if is_loopback {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(event_json)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|s| s == "token_rotated"))
        .unwrap_or(false)
}
```

在事件转发臂(`event = client_event_rx.recv() => { Ok(event_json) => {` 之后,**在现有 `should_forward` 计算之前**)插入拦截:

```rust
                    Ok(event_json) => {
                        // Token rotation kick: close remote (token-authorized)
                        // sessions so they re-authenticate; never forward this
                        // frame to clients verbatim, and never close loopback.
                        if rotated_should_close_remote(&event_json, ctx.client_ip.is_loopback()) {
                            info!("token rotated — closing remote session {}", conn_id);
                            let _ = write
                                .send(WsMessage::Close(Some(CloseFrame {
                                    code: 4001,
                                    reason: "token_rotated".into(),
                                })))
                                .await;
                            break;
                        }
                        // loopback 收到 token_rotated:静默吞掉,不转发给客户端。
                        if serde_json::from_str::<serde_json::Value>(&event_json)
                            .ok()
                            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|s| s == "token_rotated"))
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        // ...原有 should_forward 逻辑...
```

> 校验点:确认 `ctx.client_ip` 在该 `select!` 臂作用域可见(同文件别处用 `ctx.client_ip.is_loopback()`,应可见)。`CloseFrame`/`WsMessage` 已在文件顶 import(L9)。`continue` 用于 `loop`——确认事件臂在 `loop { futures::select! { ... } }` 内(是)。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::server::handler::token_rotation_tests`
Expected: PASS（3 tests）。

- [ ] **Step 5: Run check（确保事件臂改动编译)**

Run: `cargo check -p alephcore --lib`
Expected: 编译通过。

- [ ] **Step 6: Commit**

```bash
git add src/gateway/server/handler.rs
git commit -m "gateway: close remote sessions on token rotation, leave loopback intact"
```

---

## 手动 e2e 验证(实现完成后,沿用现有 TBD smoke 惯例)

这些跨 WASM reactive 接线 + Tauri 导航 + 真实 WS,无法纯单测,实现后手动跑一遍:

1. **网络不通**:启动完整 App,停 `aleph-server`(或拔网)→ BootCheckGate 显示"找不到服务器 / 检查网络" + Retry;恢复后自动重连进入应用。
2. **WS 半开超时**:(可选,难复现)确认 8s 内从 spinner 转为 Failed,不再无限转。
3. **远程换 IP(lite shell)**:lite shell 连一台 LAN server → 改 server 绑定地址重启 → panel 重连贩尽(~31s)→ lite supervisor 在 ~35s 后跳 connect.html → mDNS 重新发现新地址 → 连上。瞬时抖动(<31s 恢复)不应跳转。
4. **token 失效/不匹配**:远程 panel 用错误 token → 弹 TokenWall 显示首次文案;在 server 端 `gateway.token.rotate` → 在线远程 panel **立即**收 4001 弹墙、显示"令牌已更新"文案;本机完整 App **无感**(不断连)。

---

## Self-Review

- **Spec 覆盖**:§3.1 失败模型→Task 1;§3.1 重试策略→Task 2+6;§3.2 WS 超时→Task 4;§3.3a handshake 三态→Task 5;§3.3b 差异化重连→Task 6;§3.4 Boot/Service/TokenWall 文案→Task 7/8/9/10;§3.5 lite supervisor→Task 11;§3.6 网关轮换→Task 12/13/14;§4 测试→各 task 内 + 手动 e2e 段。**全覆盖**。
- **Placeholder 扫描**:无 TBD/TODO;每个代码步给出完整代码;Task 13/14 含 3 处"校验点"(总线方法名、JsonRpcResponse 字段、闭包签名、ctx.client_ip 可见性)——这些是**实现期必须用 grep 核实的真实符号名**,已给出 grep 命令,非占位。
- **类型一致**:`ConnectionFailure`/`FailureStage`/`classify` 跨 Task 1→3→4→5→6 一致;`Handshake` 在 Task 5 定义、Task 10 修订为带字段 `NeedsToken { was_rejected }`(Task 10 显式回填 Task 5);`rotated_should_close_remote` 在 Task 14 定义并自用;`GatewayEventFrame::TokenRotated` Task 12 定义、Task 13 发布、Task 14 按 `"type":"token_rotated"` 字符串消费(序列化形态由 Task 12 测试锁定)。
- **已知耦合**:Task 10 回填修改 Task 5 的 NeedsToken 分支——执行 Task 10 时需同时编辑 context.rs 的 handshake 与 connect 分支(已在 Task 10 Step 2 列明)。

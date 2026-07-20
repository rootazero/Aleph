# Owner DM → Main 会话同步 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 打通断线的 `dm_scope` 配置，让单用户 owner 在各 channel 与同一 agent 的 DM 共享同一段 `Main` 会话（一脑多端连续上下文）。

**Architecture:** 不新建任何子系统。给 `Config` 补一个顶层 `[session]` 配置块（复用现成 `routing::config::SessionConfig`），把它的 `dm_scope` 同时接到 inbound router 的两条路由路径：`resolve_route`（configured-bindings 路径，读 `SessionConfig.dm_scope`）与 `resolve_session_key_with_agent`（zero-config fallback 路径，读 `RoutingConfig.dm_scope`）。两条路径同一来源。代码默认仍为 `PerPeer`，单用户经 `[session] dm_scope = "main"` 显式 opt-in。

**Tech Stack:** Rust，serde/toml 配置，schemars JsonSchema，`#[tokio::test]`/`#[test]` 单测。

**Spec:** `docs/superpowers/specs/2026-06-08-owner-dm-main-session-sync-design.md`

**Worktree:** 执行时经 superpowers:using-git-worktrees 在分支 `feat/owner-dm-main-session-sync` 隔离开发。

---

## File Structure

| 文件 | 职责 | 改动 |
|---|---|---|
| `src/routing/session_key.rs` | `DmScope`（SessionConfig 用） | 加 `JsonSchema` derive |
| `src/routing/config.rs` | `SessionConfig`（dm_scope + identity_links） | 加 `JsonSchema` derive |
| `src/config/structs.rs` | `Config`（AppConfig 根） | 加 `pub session: SessionConfig` 字段 + 测试 |
| `src/gateway/routing_config.rs` | `RoutingConfig`/`DmScope`（fallback 路径用） | 加 `From<session_key::DmScope>` 转换 + 测试 |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` | inbound router 启动接线 | 两处把硬编码 `default()` 换成 `cfg.session` 来源 |
| `src/gateway/inbound_router/mod.rs` | inbound router + 单测 | 加 fallback 路径 dm_scope=Main 测试 |
| `docs/reference/SESSION_SERVICE.md` | 会话语义文档 | 加单用户 Main / 迁移 / 多用户警示一节 |

**关键事实（已核实）**
- `resolve_route`（`src/routing/resolve.rs:117` 内 `build_session_key`）已正确处理 dm_scope；`src/routing/resolve.rs` 已有测试 `test_dm_scope_main_collapses` 证明 `SessionConfig{dm_scope:Main}` + 空 bindings → `agent:main:main`。**bug 纯在 subsystems 传了 `SessionConfig::default()`。**
- fallback 路径方法 `InboundMessageRouter::resolve_session_key_with_agent(&self, msg, agent_id)`（`src/gateway/inbound_router/agent_resolver.rs:147`）用 `self.config.dm_scope`（`RoutingConfig` 的 `DmScope`，与 `SessionConfig` 的 `DmScope` 是**两个不同类型**，故需 Task 2 的转换）。
- 两个 `DmScope` 都有 `Main`/`PerPeer`/`PerChannelPeer` 三个等价变体，均 `#[serde(rename_all = "kebab-case")]`。

---

## Task 1: 给 Config 补 `[session]` 配置块

**Files:**
- Modify: `src/routing/session_key.rs:10`（DmScope derive）
- Modify: `src/routing/config.rs:22`（SessionConfig derive）
- Modify: `src/config/structs.rs:187`（在 `bindings` 字段后加 `session` 字段）
- Test: `src/config/structs.rs`（文件末尾新增 `#[cfg(test)] mod session_block_tests`）

- [ ] **Step 1: 写失败测试**

在 `src/config/structs.rs` 文件**末尾**追加：

```rust
#[cfg(test)]
mod session_block_tests {
    use super::Config;
    use crate::routing::session_key::DmScope;

    #[test]
    fn session_block_parses_main() {
        let toml_str = r#"
            [session]
            dm_scope = "main"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.session.dm_scope, DmScope::Main);
    }

    #[test]
    fn session_block_defaults_to_per_peer_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.session.dm_scope, DmScope::PerPeer);
    }
}
```

> 注：`use crate::gateway::routing_config::DmScope as _;` 仅用于确保该路径在本 crate 可达；若 clippy 报未使用可删除该行，测试本体只依赖 `routing::session_key::DmScope`。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore session_block_ 2>&1 | tail -20`
Expected: 编译失败 —— `Config` 无 `session` 字段（`no field 'session' on type 'Config'`）。

- [ ] **Step 3: 加 JsonSchema derive + session 字段**

`src/routing/session_key.rs` 第 10 行，把：
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
```
改为：
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
```

`src/routing/config.rs` 第 22 行，把：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
```
改为（该文件第 3 行已 `use schemars::JsonSchema;`）：
```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfig {
```

`src/config/structs.rs`，在 `bindings` 字段（第 187 行）之后插入：
```rust
    /// 会话隔离策略（DM scope）。单用户 owner 设 `dm_scope = "main"`
    /// 可让各 channel 的 DM 坍缩到同一 agent 的 Main 会话（一脑多端连续上下文）。
    /// 默认 `per-peer`（多用户安全）。
    #[serde(default)]
    pub session: crate::routing::config::SessionConfig,
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore session_block_ 2>&1 | tail -20`
Expected: PASS（2 个测试通过）。

- [ ] **Step 5: 提交**

```bash
git add src/routing/session_key.rs src/routing/config.rs src/config/structs.rs
git commit -m "config: add top-level [session] block backing dm_scope"
```

---

## Task 2: `DmScope` 跨类型转换（SessionConfig → RoutingConfig）

**Files:**
- Modify: `src/gateway/routing_config.rs`（在 `impl RoutingConfig` 块后、`#[cfg(test)]` 前加 From impl；并在 tests 模块加测试）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/routing_config.rs` 的 `#[cfg(test)] mod tests`（第 80 行起）内追加：

```rust
    #[test]
    fn dm_scope_from_session_config_variant() {
        use crate::routing::session_key::DmScope as Sk;
        assert_eq!(DmScope::from(Sk::Main), DmScope::Main);
        assert_eq!(DmScope::from(Sk::PerPeer), DmScope::PerPeer);
        assert_eq!(DmScope::from(Sk::PerChannelPeer), DmScope::PerChannelPeer);
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore dm_scope_from_session_config_variant 2>&1 | tail -20`
Expected: 编译失败 —— `From<session_key::DmScope>` 未实现（`the trait 'From<...>' is not implemented for 'DmScope'`）。

- [ ] **Step 3: 加 From 实现**

在 `src/gateway/routing_config.rs` 第 77 行（`impl RoutingConfig { ... }` 闭合 `}`）之后、第 79 行 `#[cfg(test)]` 之前插入：

```rust
/// Bridge the routing-layer `SessionConfig` DM scope (`routing::session_key::DmScope`)
/// into the gateway `RoutingConfig` DM scope used by the zero-config fallback path.
/// The two enums are structurally identical but distinct types; this keeps a single
/// user-facing `[session] dm_scope` value driving both routing paths.
impl From<crate::routing::session_key::DmScope> for DmScope {
    fn from(scope: crate::routing::session_key::DmScope) -> Self {
        use crate::routing::session_key::DmScope as S;
        match scope {
            S::Main => DmScope::Main,
            S::PerPeer => DmScope::PerPeer,
            S::PerChannelPeer => DmScope::PerChannelPeer,
        }
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore dm_scope_from_session_config_variant 2>&1 | tail -20`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/routing_config.rs
git commit -m "gateway: map SessionConfig DmScope into RoutingConfig DmScope"
```

---

## Task 3: 把 dm_scope 接到 inbound router 两条路径

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs:553`（routing_config 来源）
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs:~756`（with_route_bindings 的 SessionConfig）

> 这是 bin 启动接线，无独立单测；编译 + Task 4 单测 + Step 4 手动冒烟共同验证。

- [ ] **Step 1: routing_config 改为从 `cfg.session` 取 dm_scope**

`src/bin/aleph-server/commands/start/builder/subsystems.rs` 第 553 行，把：
```rust
    let routing_config = RoutingConfig::default();
```
替换为：
```rust
    // Single source of truth for dm_scope: the user's [session] block.
    // Captured once and fed to BOTH routing paths — the zero-config fallback
    // (RoutingConfig.dm_scope, below) and the configured-bindings path
    // (with_route_bindings' SessionConfig, further down).
    let session_cfg = if let Some(ref cfg_arc) = app_config {
        cfg_arc.read().await.session.clone()
    } else {
        alephcore::routing::config::SessionConfig::default()
    };
    let routing_config = RoutingConfig::default().with_dm_scope(session_cfg.dm_scope.into());
```

- [ ] **Step 2: with_route_bindings 用同一 `session_cfg`**

同文件，找到第 ~756 行的 `with_route_bindings` 调用：
```rust
            inbound_router = inbound_router.with_route_bindings(
                bindings,
                alephcore::routing::config::SessionConfig::default(),
                alephcore::routing::DEFAULT_AGENT_ID,
            );
```
把第二个实参 `alephcore::routing::config::SessionConfig::default()` 替换为 `session_cfg.clone()`：
```rust
            inbound_router = inbound_router.with_route_bindings(
                bindings,
                session_cfg.clone(),
                alephcore::routing::DEFAULT_AGENT_ID,
            );
```

- [ ] **Step 3: 编译确认无误**

Run: `cargo check -p alephcore --bin aleph-server 2>&1 | tail -20`
Expected: 编译通过（无 `session_cfg` 未定义/类型错配；`.into()` 命中 Task 2 的 From）。

- [ ] **Step 4: 提交**

```bash
git add src/bin/aleph-server/commands/start/builder/subsystems.rs
git commit -m "server: wire [session] dm_scope into both inbound routing paths"
```

---

## Task 4: fallback 路径 dm_scope=Main 验证（防回归）

**Files:**
- Modify: `src/gateway/inbound_router/mod.rs`（在 `group_message_resolves_to_group_session_key` 测试附近，约第 1180 行后追加）

> resolve_route（bindings）路径已由 `src/routing/resolve.rs::test_dm_scope_main_collapses` 覆盖。本任务补 fallback（zero-config 单用户）路径，照搬已存在的 `group_message_resolves_to_group_session_key` 范式。

- [ ] **Step 1: 写失败测试**

在 `src/gateway/inbound_router/mod.rs` 第 1180 行（`group_message_resolves_to_group_session_key` 测试的闭合 `}`）之后追加：

```rust
    /// 单用户 owner：dm_scope=Main 时，零配置 fallback 路径的 DM 必须坍缩到
    /// `agent:<id>:main`，使同一 agent 在不同 channel 的 DM 共享同一 Main 会话。
    #[test]
    fn dm_main_scope_collapses_to_main_session_key() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default().with_dm_scope(DmScope::Main),
        );
        let make = |channel: &str| InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new(channel),
            conversation_id: ConversationId::new("dm-conv"),
            sender_id: UserId::new("owner"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        // 同一 agent、不同 channel → 同一 Main key（跨 channel 共享）。
        let k_tg = router.resolve_session_key_with_agent(&make("telegram"), "main");
        let k_sl = router.resolve_session_key_with_agent(&make("slack"), "main");
        assert_eq!(k_tg.to_key_string(), "agent:main:main");
        assert_eq!(k_sl.to_key_string(), "agent:main:main");
        assert_eq!(k_tg.to_key_string(), k_sl.to_key_string());
    }

    /// 反向保护：默认 PerPeer 下 DM 仍按 peer 隔离（零回归）。
    #[test]
    fn dm_per_peer_scope_stays_isolated() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(), // PerPeer
        );
        let msg = InboundMessage {
            id: MessageId::new("m2"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("dm-conv"),
            sender_id: UserId::new("owner"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        let key = router.resolve_session_key_with_agent(&msg, "main");
        assert_eq!(key.to_key_string(), "agent:main:dm:owner");
    }

    /// 不同 agent 各自 Main，互不串味。
    #[test]
    fn dm_main_scope_different_agents_isolated() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default().with_dm_scope(DmScope::Main),
        );
        let msg = InboundMessage {
            id: MessageId::new("m3"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("dm-conv"),
            sender_id: UserId::new("owner"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        let work = router.resolve_session_key_with_agent(&msg, "work");
        let personal = router.resolve_session_key_with_agent(&msg, "personal");
        assert_eq!(work.to_key_string(), "agent:work:main");
        assert_eq!(personal.to_key_string(), "agent:personal:main");
        assert_ne!(work.to_key_string(), personal.to_key_string());
    }
```

> 若该测试模块的 `use` 未涵盖 `DmScope`，在模块顶部 `use` 区加 `use crate::gateway::routing_config::DmScope;`（其余 `MessageId`/`ChannelId`/`ConversationId`/`UserId`/`InboundMessage`/`RoutingConfig`/`SqlitePairingStore`/`ChannelRegistry` 已被同模块既有测试使用，无需新增）。

- [ ] **Step 2: 运行测试确认失败（先于 Task 3 时）/通过（Task 3 后）**

Run: `cargo test -p alephcore -- dm_main_scope dm_per_peer_scope 2>&1 | tail -30`
Expected: 3 个新测试 + 既有测试全 PASS（逻辑已由 `resolve_session_key_with_agent` + Task 2/3 支撑）。若先于 Task 3 单独跑本任务，断言逻辑仍应通过，因为它直接构造 `RoutingConfig.with_dm_scope(Main)`，不依赖 subsystems 接线。

- [ ] **Step 3: 全量回归**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: 全绿（特别确认 `routing::resolve::tests::test_dm_scope_main_collapses` 仍 PASS）。

- [ ] **Step 4: clippy + fmt**

Run: `cargo fmt && cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -20`
Expected: 无警告。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/inbound_router/mod.rs
git commit -m "test: verify dm_scope=Main collapses DMs to shared Main session"
```

---

## Task 5: 文档

**Files:**
- Modify: `docs/reference/SESSION_SERVICE.md`（追加一节）

- [ ] **Step 1: 追加文档节**

在 `docs/reference/SESSION_SERVICE.md` 末尾追加：

```markdown
## DM Scope —— 单用户「一脑多端」连续上下文

`[session] dm_scope` 控制 DM 如何映射到会话：

| 值 | 语义 |
|---|---|
| `per-peer`（默认） | 每个发送者独立会话（跨 channel 按 peer） |
| `per-channel-peer` | 每个 channel × 发送者独立会话（多用户推荐） |
| `main` | 所有 DM 坍缩到该 agent 的 `Main` 会话 |

**单用户 owner**（只有你本人会 DM 这个 bot，由 allowlist/pairing 保证）建议设：

​```toml
[session]
dm_scope = "main"
​```

效果：你在 Telegram / Slack / WebChat Panel 等各 channel 与**同一 agent** 的 DM 共享同一段
`agent:<id>:main` 上下文——agent 记得你在任意 channel 说过的话；打开 Panel 即见完整历史。
绑定到**不同 agent** 的 channel 各自 `agent:<id>:main` 隔离（工作 / 个人不串味）。回复仍只回到
你发问的那个 channel（不向其他 channel 推送）。

**注意事项**
- **多用户警示**：若 owner 之外还有人被 allowlist 也能 DM，`main` 会把所有人并进同一会话。
  多用户请用 `per-channel-peer`（owner 专属 Main 的 Tier 2 判定本期未实现）。
- **迁移断点**：从 `per-peer` 切到 `main` 后，旧的 `agent:<id>:dm:<peer>` 会话停在原地（不迁移），
  新消息走 `agent:<id>:main`，会有一次性上下文断点。
- 群组消息不受影响，始终按 `Group` 会话隔离。
```

- [ ] **Step 2: 提交**

```bash
git add docs/reference/SESSION_SERVICE.md
git commit -m "docs: document [session] dm_scope main for single-user sync"
```

---

## 收尾验证（完成所有任务后）

- [ ] 全量：`cargo test -p alephcore --lib 2>&1 | tail -20` → 全绿
- [ ] 集成测试编译：`cargo test -p alephcore --all-targets --no-run 2>&1 | tail -10` → 通过
- [ ] clippy：`cargo clippy -p alephcore --all-targets -- -D warnings` → 净
- [ ] （可选手动冒烟）配置 `[session] dm_scope = "main"`，把 Telegram 与 WebChat 绑到同一 agent，从手机 DM 一句、再开 Panel → 历史互见。

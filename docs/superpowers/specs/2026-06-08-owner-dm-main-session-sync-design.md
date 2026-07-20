# Owner DM → Main 会话同步（一脑多端连续上下文）

**日期**: 2026-06-08
**状态**: Design / 待实现（worktree 隔离开发）
**范围**: 仅同步核心 + 验证 + 文档（绑定 UI 推迟另起一期）

---

## 1. 背景与目标

### 起点
原始诉求是「把 channel 与 agent 从一对一改成多对多」：首次配对可选多个 agent，
agent tab 可重新勾选，配对后通信保持同步——任一 channel 收发的消息同步到该 agent
绑定的其他 channel。

### 经评估后的重定标
深入分析 + 对标 openclaw（`/Volumes/TBU4/Github/openclaw`，成熟自托管个人 AI）后，
判定**完整多对多 + 实时镜像 fan-out 成本远大于收益**，应砍。把诉求拆开看其实是三件事：

| 子关系 | 现状 | 处置 |
|---|---|---|
| 一个 agent → 多 channel | 今天已支持（`channel_active_agent` 不阻止多 channel 指向同一 agent_id） | 保留，补「共享上下文」 |
| 多个 agent → 一个 channel（活跃切换） | 不支持 | **砍**（认知负担/复杂度的真正来源） |
| 不同 channel → 不同 agent | 今天已支持（每 channel 独立映射 agent_id） | 已有，无需改 |

openclaw 的关键实证：
- 它的 agent↔channel 多对多靠**静态 binding 规则**、**每条入消息只解析唯一一个 agent**，
  没有「一 channel 多 agent + 活跃切换」。
- 它**刻意不做跨 channel 实时镜像/fan-out**。「连续性」用 `dmScope: "main"`（同一 agent
  的所有 DM 坍缩到同一 main session，单用户默认）实现——读写同一段历史，回复只回原 channel。
- 权限是 per-channel（谁能说话）× per-agent（能用什么工具/sandbox）两个**正交**维度，
  不做 (agent×channel) 矩阵。

### 本设计的目标
**owner 在自己各 channel 上与同一 agent 的 DM 共享同一段 `Main` 上下文**；打开 Panel
即见完整历史（含手机上刚聊的）。这就是「一脑多端」里真正有价值的部分，且**不需要任何
fan-out 子系统**——只需让 owner 的 DM 路由到已存在的 `Main { agent_id }` 会话。

适用前提：**单用户（Tier 1）**——只有 owner 本人会 DM 这个 bot（由 allowlist/pairing 保证）。

---

## 2. 非目标（明确砍掉）

- ❌ channel ↔ agent 多对多（schema / 绑定模型不改）
- ❌ 一个 channel 挂多个 agent + 活跃切换
- ❌ 实时 fan-out / 跨 channel 消息镜像（messaging→messaging）
- ❌ Panel 实时蹦字（channel 发起的 run 投影到 event_bus）——本期只要「打开即见完整历史」
- ❌ 群组 / 多人会话同步（维持现有 `Group` / `PerPeer` 隔离）
- ❌ (agent × channel) 权限矩阵
- ❌ owner vs 非-owner 判定（Tier 2）——单用户下不需要
- ❌ 配对选 agent / agent tab 重新绑定的 UI（绑定 UI，另起一期）

---

## 3. 现状架构与已确认的缺口

### 会话模型
`SessionKey`（`src/routing/session_key.rs`）已有 `Main { agent_id, main_key, epoch }` 跨
channel 共享会话变体。`main_key` 为常量 `"main"`。`SessionKey::main(agent_id)` 现成；
`SessionKey::dm(agent_id, channel, peer, DmScope::Main)` 也会坍缩为 `Main`。

### Panel 已坐在 Main 上
`src/gateway/router.rs`：Panel 传 `agent_id` 而不带 `peer_id` 时直接 `SessionKey::main(agent_id)`。
**所以只要 owner 的 messaging DM 也落到 `Main`，Panel 与手机就是同一段会话**，连续性
（含 Panel↔Telegram）自动成立，无需额外工作。

### 两条入站路由路径
1. **`resolve_route` 路径**（`src/routing/resolve.rs:65,91`）——走 configured bindings，
   读 `SessionConfig.dm_scope`（`src/routing/config.rs`，TOML 可配，含 identity_links）。
2. **`agent_resolver` fallback 路径**（`src/gateway/inbound_router/agent_resolver.rs:168`）——
   零配置回退，读**另一个** `RoutingConfig.dm_scope`（`src/gateway/routing_config.rs`）。

两者对 DM 的判定（agent_resolver.rs:167-179）：
```rust
match self.config.dm_scope {
    DmScope::Main          => SessionKey::main(agent_id),
    DmScope::PerPeer       => SessionKey::peer(agent_id, format!("dm:{}", sender_id)),
    DmScope::PerChannelPeer=> SessionKey::peer(agent_id, format!("{}:dm:{}", channel, sender_id)),
}
```

### ⚠️ 已确认的缺口（核心待修）
可设的 `dm_scope` **到不了 inbound router**，两条路径都吃 `SessionConfig::default()`（= `PerPeer`）：

- `src/bin/aleph-server/commands/start/builder/subsystems.rs:~756`：
  ```rust
  inbound_router = inbound_router.with_route_bindings(
      bindings,
      alephcore::routing::config::SessionConfig::default(),  // ← 硬编码 default，无视用户配置
      alephcore::routing::DEFAULT_AGENT_ID,
  );
  ```
- `src/gateway/inbound_router/mod.rs:158`：`route_session_config: SessionConfig::default()`，
  且 `config: RoutingConfig`（fallback 用）构造时 dm_scope 也未从用户配置注入。
- AppConfig（`src/config/structs.rs`）**没有顶层 `[session]` 字段**——
  `config/ui_hints/definitions.rs:443` 的 `"session.dm_scope"` ui_hint **是孤立的、无后端字段**。
- dm_scope 概念还散落在 per-agent `agents.defaults.dm_scope: Option<String>`
  （`src/config/types/agents_def.rs:107`，甚至有枚举外的 `"workspace"` 值），与 routing 的
  `SessionConfig.dm_scope` enum 互不贯通。

**净结论**：`session.dm_scope = "main"` 今天设了也不生效。本期核心 = 打通它。

---

## 4. 设计

### 机制
单用户下，唯一会 DM 的就是 owner。令 `dm_scope = Main`，则每条 DM →
`SessionKey::main(agent_id)`，与 Panel 同坐一席。

- 回复：仍经 `ReplyEmitter`（持有入站 `ReplyRoute`）回到**原 channel**——**不变**。
- 多 agent：各自 `agent:<id>:main` 天然隔离（Telegram→工作 agent、Slack→个人 agent 不串味）。
- 群组：`is_group` 分支不受影响，仍走 `SessionKey::group(...)`。

### 单一真相源：补齐 `[session]` 配置块
让孤立的 `session.dm_scope` ui_hint 有真实后端，并贯通到两条路径：

1. **AppConfig 增加顶层 `session: SessionConfig` 字段**（`src/config/structs.rs`），
   `#[serde(default)]`，复用现成的 `routing::config::SessionConfig`（已有 `dm_scope` +
   `identity_links` + 校验）。对应 TOML：
   ```toml
   [session]
   dm_scope = "main"
   ```
2. **subsystems.rs 接线**：把 `with_route_bindings(...)` 的硬编码 `SessionConfig::default()`
   换成 `cfg.session.clone()`；并让 fallback 用的 `RoutingConfig.dm_scope` 也从
   `cfg.session.dm_scope` 注入（经 `RoutingConfig::with_dm_scope(...)` 或在
   `InboundRouter` 构造处统一来源）。两条路径**同一来源**。
3. **`bindings` 为空时也要生效**：当前 subsystems 仅在 `!cfg.bindings.is_empty()` 时才调
   `with_route_bindings`。单用户通常没有 bindings → 走 fallback 路径。必须保证**即使没有
   bindings，`cfg.session.dm_scope` 也注入到 fallback 的 `RoutingConfig`**（否则 dm_scope=main
   对零配置单用户无效——这是最常见场景，绝不能漏）。

> 注：per-agent `agents.defaults.dm_scope`（String，含 `"workspace"`）是另一套更丰富的语义，
> **本期不动**，避免范围蔓延。单一真相源用顶层 `[session]`。若未来需要 per-agent 覆盖，
> 另行设计映射。

### 默认值决策
- **保持代码默认 `PerPeer`**（`SessionConfig::default()` 不变）——对测试/未来多用户安全。
- 单用户 owner 通过 `[session] dm_scope = "main"` **显式 opt-in**。
- 文档明确推荐；可选经 R8 config 工具/RPC 让 LLM 对话式设置（ui_hint 已存在）。
- 不改全局默认，避免对既有部署静默改变行为。

---

## 5. 边界与迁移

- **已有 PerPeer 历史**：切到 Main 后，旧 `agent:<id>:dm:<peer>` 会话停在原地（不删、不迁移），
  新消息走 `agent:<id>:main`。会有一次性「上下文断点」。文档说明；不做自动迁移（YAGNI）。
- **多用户风险**：若 owner 之外有人被 allowlist 也能 DM，`dm_scope=main` 会把所有人并进同一
  main 会话。文档明确警示：`dm_scope=main` 仅适用于单用户；多用户请用 `per-channel-peer`
  并走 Tier 2（owner 判定，本期不做）。
- **identity_links**：本期不依赖，但补齐 `[session]` 后顺带可被 `resolve_route` 路径读到
  （现状 `agent_resolver:173` 用原始 sender_id，未走 canonical——不在本期范围，仅记录）。

---

## 6. 验证

集成测试（`tests/` 或相应 inbound_router 单测）：

1. `dm_scope=main` + 两个 channel（如 telegram、slack）绑**同一** agent，分别投 DM →
   断言两者解析出的 `SessionKey` 均为 `agent:<id>:main`（同一 key）。
2. 跨 channel 上下文互见：channel A 说一句 → channel B 的后续 run 能在历史里看到（同一 session
   store 记录）。
3. 绑**不同** agent 的两 channel → 各自 `agent:<idA>:main` / `agent:<idB>:main`，**不**互见。
4. **零 bindings 单用户路径**：不配置任何 `bindings`，仅 `[session] dm_scope = "main"` →
   fallback 路径确实产出 `Main`（防止「只在 bindings 路径生效」的回归）。
5. 群组消息不受影响：`is_group` → 仍 `SessionKey::group(...)`。
6. 默认（无 `[session]` 配置）仍为 `PerPeer`——保证零回归。

`cargo test -p alephcore`、`cargo clippy -- -D warnings`、`cargo fmt` 全绿。

---

## 7. 构建顺序（worktree 隔离）

worktree 分支（如 `feat/owner-dm-main-session-sync`）：

1. **配置层**：AppConfig 增 `session: SessionConfig`；确认 `[session]` TOML 反序列化 + ui_hint
   贯通。单测：解析 `dm_scope = "main"`。
2. **接线层**：subsystems.rs 用 `cfg.session` 替换两处硬编码 `default()`；保证 bindings
   为空时 fallback 的 `RoutingConfig.dm_scope` 也注入。
3. **验证层**：第 6 节集成测试（TDD：先写断言两 channel 同 key，红 → 接线 → 绿）。
4. **文档**：`docs/reference/` 相关（会话/路由）说明单用户 Main 语义、迁移断点、多用户警示；
   CLAUDE.md/配置文档补 `[session] dm_scope`。

落地后 worktree 合并（注意：`EnterWorktree` 会话内只合并不删除 worktree；清理用新会话）。

---

## 8. 关联

- 现状会话/路由：`src/routing/session_key.rs`、`src/routing/resolve.rs`、
  `src/gateway/inbound_router/agent_resolver.rs`、`src/gateway/routing_config.rs`、
  `src/routing/config.rs`、`src/bin/aleph-server/commands/start/builder/subsystems.rs`
- 对标：openclaw `docs/concepts/session.md`（dmScope）、`docs/concepts/channel-docking.md`
- 红线契合：R3 核心轻量化、R10 笨循环、P6 KISS/YAGNI（不建 fan-out 子系统）

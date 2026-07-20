# Discord Channel Refactor — 熵减 + 连线真实路径

**Status**: Draft (pending user review)
**Created**: 2026-07-20
**Supersedes**: `2026-04-15-discord-channel-redesign-design.md`（该重设计建了一套 nested-config + resolver + account_pool + handler 组件，**但从未接入真正的 serenity 事件循环**，成为孤立死代码）
**Reference**: openclaw `extensions/discord/` (~61k LOC TS) + `src/channels/` 共享逻辑
**Redlines**: R4（Interface 纯 I/O）、R10（YAGNI 撤回 / 熵减）、P6（简洁）

---

## 1. 背景：为什么是"熵减 + 连线"而不是"再加功能"

### 1.1 生产路径的真相

Discord 频道生产链路只有一条：

```
DiscordChannel::new(id, DiscordConfig)   // flat 配置
  └─ start() → Handler { config: DiscordConfig, thread_bindings: HashMap<..> }
       └─ serenity EventHandler：message() / interaction_create() / thread_*()
```

`Handler` **只读 flat `DiscordConfig`**，内联实现 allowlist、mention/prefix、slash-command、thread 追踪。

### 1.2 平行宇宙（孤立死代码）

2026-04-15 重设计建的这套结构**从未被 `Handler` 消费**，仅被单测引用：

| 孤立单元 | 文件 | 行数 | 真实消费者 |
|---------|------|------|-----------|
| `DiscordAccountPool` | `account_pool.rs` | 94 | ❌ 0 |
| `AccountResolver` | `resolver/account.rs` | 39 | ❌ 0 |
| `ChannelSettingsResolver` | `resolver/channel.rs` | 269 | ❌ 仅测试 |
| `StreamingHandler`（presence 追踪） | `handlers/streaming.rs` | 128 | ❌ 0 |
| `ApprovalQueue` | `handlers/approval.rs` | 176 | ❌ 0 |
| `InteractionHandler` | `handlers/interaction.rs` | 62 | ❌ 0 |
| `ThreadBindingHandler` | `handlers/thread.rs` | 192 | ❌ 0（Handler 用本地重复实现） |
| 嵌套配置全家桶 | `config.rs` 部分 | ~250 | ❌ 仅 mod.rs + 孤立 resolver |
| `resolve_settings()` + 三字段 + `with_config()` | `mod.rs` | ~60 | ❌ 仅测试 |

**共 ~1000+ 行孤立/半连线代码。** 这直接命中目标三要点：功能连线（组件没连）、熵减（平行结构）、错误修复（见下）。

### 1.3 一个 real bug：`StreamingHandler` 是对 openclaw 的误读

openclaw `streaming.ts` = **回复流式渐进编辑**（agent 生成时不断编辑消息 + 工具进度）。
Aleph 孤立的 `StreamingHandler` 实现的却是 **presence 追踪**（用户在 Twitch 直播），与 openclaw 意图完全无关——即便接线也是错的特性。**正确的 openclaw 特性 Aleph 已有基础设施可复用**（见 §3.1）。

### 1.4 关键基础设施发现（决定"连线"可行性）

| Aleph 已有基础设施 | 锚点 | 用途 |
|-------------------|------|------|
| `StreamProtocol::EditBased` + `Channel::edit()` | `channel.rs:424,772` | 通用 `ReplyEmitter` streaming 路径调 `channel.edit()`（`reply_emitter/emitter/streaming.rs`）。声明 `EditBased` + 实现 `edit()` 即得回复流式，**零新增流式逻辑** |
| `ApprovalCallbackSink` + `channel_approval` + `exec.callback.handle` | `inbound_router/approval_callback.rs`、`channel_approval.rs`、`handlers/exec_approvals.rs` | 跨 channel 按钮审批的**真实路径**。Discord 按钮只需转发 custom_id+clicker 给 sink，**孤立 `ApprovalQueue` 冗余可删** |
| `DiscordResolver`（channel 名→id） | `resolver/mod.rs` + `input/strategy/error` | 服务 outbound 频道解析（2026-04-08 独立特性，**是活的，保留**） |

---

## 2. 目标与非目标

### 2.1 目标
- **熵减**：删除 §1.2 全部孤立/误读代码（保留 `DiscordResolver` 家族）。
- **连线真实路径**：把两项高价值 openclaw 特性接进真实 `Handler`，全部复用已有基础设施：
  - 回复流式（EditBased）
  - 交互按钮 → 真实 `ApprovalCallbackSink` 审批
- **打磨/增强**：typing 生命周期从一次性 → 处理期持续。
- **thread 连线去重**：`Handler` 内联 `thread_bindings` map 与本地 `ThreadBinding` struct 与 `handlers/thread.rs::ThreadBindingHandler` 二选一，消除重复。

### 2.2 非目标（YAGNI）
- ❌ 多账号 / 多 bot 实例（单用户助手无真实需求，account_pool 删除）。
- ❌ per-guild/per-channel 嵌套配置覆盖链（零消费者，删除）。
- ❌ 安全审计子系统（openclaw 有，但 Aleph 无消费者且 R4 要求 Interface 纯 I/O——审计属 Core 关切，不在 Interface 层新建）。
- ❌ presence/streaming 追踪（误读特性，删除）。
- ❌ status-reactions（需 run-lifecycle hook，留作独立 follow-up，本次不做以免越界）。

---

## 3. 设计

### 3.1 回复流式（连线 `EditBased`）

**改动面**：`mod.rs`（**实测仅一行**）
1. `capabilities()`：`stream_protocol = StreamProtocol::EditBased`（`max_message_length = 2000` 已设）。
2. `Channel::edit()` **Discord 已实现**（serenity `edit_message`）——无需新增。
3. 复用：`inbound_router/executor.rs:134` 见 `EditBased` 即 `stream_enabled = true`，`ReplyEmitter` + `StreamingController`（debounce/flood 控制）自动驱动 send→edit。**Discord 侧零流式状态机**。

**收益**：Discord 回复从"憋完一次性发"→"实时渐进编辑"，对齐 Telegram 体验。

### 3.2 交互按钮 → 真实审批（连线 `ApprovalCallbackSink`）

> **实测修正**：审批按钮是**双向**的。router 用 InboundMessage 的 **`cb_` id 前缀**识别回调（`inbound_router/mod.rs:546`），把 `text`（=custom_id）+ `sender_id` 交给注入的 sink。故 Discord **不需持有 sink**——只发一条 `cb_`-前缀 InboundMessage 走现有 `inbound_tx` 即可（R4 纯 I/O）。但按钮要先出现，还需 outbound 渲染。

**改动面**：`mod.rs`（双向）
1. **Outbound**（`send()`）：把 `OutboundMessage.inline_keyboard` 渲染成 Discord 按钮组件（`CreateActionRow::Buttons` + `CreateButton`，5×5 上限）。`custom_id` = `callback_data` 逐字透传。`approve*`→绿 / `deny*`→红 / 其余中性（`button_style_for`）。
2. **Inbound**（`interaction_create`）：新增 `Interaction::Component` 分支 → `handle_component()`：施加 guild/channel allowlist → 构造 `id="cb_<interaction_id>"`、`text=custom_id`、`sender_id=clicker` 的 InboundMessage 发 `inbound_tx` → `CreateInteractionResponse::Acknowledge` 消解点击 spinner（结果由 router 作普通消息回渲染）。

**熵减**：`ApprovalQueue`/`InteractionHandler` 已在 Phase 1 删除——真实审批走 `ApprovalCallbackSink`。

> ✅ **决策已确认（2026-07-20）**：真实审批基础设施是 `ApprovalCallbackSink`，`ApprovalQueue` 与之重复。"连线真实路径"的正解是接 sink，故**删除 ApprovalQueue + InteractionHandler**。

### 3.3 Typing 生命周期（打磨）— **已降级/取消**

> **ROI 决策（2026-07-20）**：Phase 2 回复流式启用后，消息会实时出现并渐进增长——用户已获得处理反馈。在其上再叠"持续 typing 指示器"需 message()（start）↔ send()（stop）跨方法生命周期协调，收益边际且增复杂度，违 R10 YAGNI。既有一次性 `broadcast_typing` 作流式前指示足矣。**本次不做。**

### 3.4 Thread 绑定去重（连线 + 熵减）

> ✅ **决策已确认（2026-07-20）：方案 A**。删除 `handlers/thread.rs::ThreadBindingHandler` + `AgentId`（sub-agent participants 无消费者），保留 `Handler` 内联 `thread_bindings` + 本地 `ThreadBinding`（已连线、够用）。纯删死代码，最符 R10 YAGNI。

---

## 4. 删除清单（熵减）

| 动作 | 目标 |
|------|------|
| DELETE 文件 | `account_pool.rs`、`resolver/account.rs`、`resolver/channel.rs`、`handlers/streaming.rs`、`handlers/approval.rs`、`handlers/interaction.rs` |
| DELETE（方案 A） | `handlers/thread.rs` |
| DELETE config.rs 类型 | `DiscordChannelConfig`、`DiscordChannelSettings`、`DiscordGuildSettings`、`AccountConfig`、`DiscordSecurityConfig`、`AuditEvents`、`ContentRetention`、`DiscordFeatures`、`ThreadBindingConfig`、`ReplyConfig`、`From<DiscordConfig>` impl |
| DELETE mod.rs | `account_pool`/`account_resolver`/`settings_resolver` 字段、`resolve_settings()`、`with_config()` 孤立构造、相关 `pub use` |
| DELETE 测试 | `pair_loop_guard.rs` 中 `resolve_settings_*` 测试、各删除文件内联测试 |
| KEEP | `DiscordResolver` 家族（`resolver/mod.rs`+`input`+`strategy`+`error`）、flat `DiscordConfig`、`IntentsConfig`、`security/`（保持 stub，不新建审计） |

预计净删除 ~1000+ 行；新增（§3.1–3.3）~150 行。**净熵减。**

---

## 5. 验证

- `cargo check -p alephcore`（每阶段一次，节制原则）。
- `cargo test -p alephcore --lib -- discord`（回归现有 discord 契约测试 + `tests/discord_*`）。
- 新增最小单测：`edit()` 参数映射、`interaction_create` 按钮 custom_id 解析转发。
- clippy 本模块无新增 warning。

---

## 6. 阶段（实施记录）

1. **Phase 1 — 熵减** ✅：删除 §4 全部孤立代码 + 修引用 + 删孤立测试（净 −1456 行）。
2. **Phase 2 — 回复流式** ✅：`stream_protocol = EditBased`（一行；`edit()` 已存在）。
3. **Phase 3 — 按钮审批双向连线** ✅：`send()` 渲染按钮 + `interaction_create` Component→cb_ inbound + `button_style_for` 单测。
4. **Phase 4 — typing 生命周期**：降级取消（见 §3.3）。thread 去重＝Phase 1 方案 A（删孤立 handler，无需替换）。

每 Phase 一或多次提交，遵循 `discord: <desc>` 规范。

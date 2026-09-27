# Discord Channel R2 — Backlog（2026-09-27 起）

> **来源**：[R1 spec](../specs/2026-09-27-discord-r1-design.md) + 线2 subagent 收尾报告的「已知风险/遗留问题」+ vs openclaw Discord (175k LOC TS) Gap 分析的剩余项。
>
> **状态**：均 **未开始**。每条标注工作量、依赖、是否被 R1 脚手架已经搭好。
>
> **工作量图例**：**S** ≈ 单文件/半天 ｜ **M** ≈ 跨 3-6 文件/1-2 天 ｜ **L** ≈ 跨子系统/需设计 + 多日。

## 摘要

R1 完成度：
- 线2（深度重构）✅ 合 main (`4453a4db2`) — 5 commits + 1 audit follow-up, 1707 行新增
- 线1（连线 + 熵减）🔄 仍在跑 — T1.2 commit `0589148de`, T1.3 编译过, T1.1/T1.4/T1.5 待做

R2 任务分两类：
- **A. 闭合 R1 脚手架**（D1, D2, D3, D6）— 已经在 R1 写好代码但未实际接线，必须做
- **B. 产品增强**（D4, D5, D7, D8）— 对标 openclaw 剩余能力或产品需求，可排期

## 任务列表

### A 类 — R1 脚手架闭合（4 项）

#### D1 — `CommandRegistry` 替换 `handle_component` 的 raw `cb_<id>` 路径
- **模块**：`src/gateway/interfaces/discord/mod.rs::handle_component` + `src/gateway/interfaces/discord/commands.rs::CommandRegistry::dispatch`
- **类型**：broken-wiring | **工作量**：M
- **现状**：`commands.rs` 已实现完整 `ComponentId` codec + `CommandRegistry` 派发表（13 测试），但 `handle_component` 仍走旧路径 `self.inbound_tx.send(InboundMessage { text: component.data.custom_id.clone() })`，把整个 `cb_<message_id>` 当文本塞给 agent 循环。
- **目标**：`handle_component` 解析 `custom_id` 为 `ComponentId`，按 `id.kind` 查表派发；未知 kind 才走 fallback 文本通道（保留兼容层）。
- **改动文件**：`mod.rs`（`handle_component` 单点改写）+ `commands.rs`（增 `ComponentKind::Callback` 默认派发器）+ 至少 3 个集成测试。
- **风险**：行为回归（旧路径已写进 prod 数月）。**必须有前/后对比测试**——加一个 `channel_inbound_legacy_cb_id_keeps_compat` 守住 fallback 路径。
- **验收**：`aleph-server` 起服后用 bot token 测试一个 approval 按钮点击，新代码派发去 approval sink（旧路径也是去那里），但通过 typed enum 而非 raw string。
- **不包含**：TUI/Panel 渲染侧（不变）。

#### D2 — `audit_hooks.rs` 全路径接入 + `manager.rs::resolve_with_reason` 单行钩入
- **模块**：`src/gateway/interfaces/discord/audit_hooks.rs` (R1 spec T1.1) + `src/exec/manager.rs::resolve_with_reason`
- **类型**：new-feature | **工作量**：S
- **现状**：R1 spec 写过 `audit_approval_requested / _resolved / _blocked` 三个函数（直接构 `AuditEntry` + `SecurityAuditLog::log().await`），但**未落地**。
- **目标**：新增 `audit_hooks.rs`，三个 helper 函数；在 `manager.rs::resolve_with_reason` 末尾单行钩入 `_resolved`（最小补丁）。
- **改动文件**：1 个新增 + `manager.rs` 1 行。
- **验收**：audit 表出现 `discord.approval.resolved: decision=Allowed|Blocked cmd=...` 行；单测覆盖三个 helper 各一个反例。
- **前提**：线1 R1 完成（spec 已在 main 上由线2 合入）。

#### D3 — `startup_audit` 在 `start()` 路径钩入
- **模块**：`src/gateway/interfaces/discord/security/startup_audit.rs` (R1 spec T1.3) + `src/gateway/interfaces/discord/mod.rs::start`
- **类型**：new-feature | **工作量**：S
- **现状**：`startup_audit.rs` 已实现 `for_guild / for_config` 两个 builder（线1 在写），但未连接到 `start()`。
- **目标**：`start()` 末尾对每个 `allowed_guild` 调用 `for_guild` + `SecurityAuditLog::log`（仅启动期一次，非每次启动都重复——用 startup 戳）。
- **改动文件**：`mod.rs::start` 末尾 5 行；可能需要 `startup_audit::run_startup_audit(channel_id, log)` 包装。
- **验收**：`aleph-server --discord-startup-dry-run` 输出 audit 行；单测覆盖 `for_config` 空 allowed_guilds → 0 行；非空 → N 行。

#### D6 — `ReconnectCoordinator::mark_event` 接入生效路径
- **模块**：`src/gateway/interfaces/discord/reconnect.rs::ReconnectCoordinator` + `src/gateway/interfaces/discord/mod.rs::Handler`
- **类型**：broken-wiring | **工作量**：S
- **现状**：R1 实现了 `ReconnectCoordinator::mark_event()` 但 Handler 未在每条 serenity gateway event 触发；僵尸通道安全网**仍然不完整**——is_zombie 检测需要 last_event_at，而 mark_event 没在调用。
- **目标**：Handler 的 `message` / `interaction_create` / `ready` 入口各调一次 `self.reconnect.mark_event()`（或 serenity 的 `EventHandler::cache_update`）。
- **改动文件**：`mod.rs` Handler impl 加 3 行。
- **验收**：真机测试（mocked gateway）连续 60s 无 event → health monitor sweep 触发僵尸重启。

### B 类 — 产品增强（4 项）

#### D4 — Discord approval capability 接入
- **模块**：`src/gateway/interfaces/discord/commands.rs` + `src/gateway/handlers/approval.rs`
- **类型**：new-feature | **工作量**：M
- **现状**：`approval_card.rs` 已有 button 基建；`commands.rs::ComponentKind::{ApprovalApprove, ApprovalDeny}` 已编码；但 discord 端**按钮按了不进 approval sink**——按钮点击的 `custom_id` 仍是 `cb_<message_id>` 的 raw text。
- **目标**：D1 落地后，approval card 直接发 `ComponentId{kind: ApprovalApprove, payload: <message_id>}` 的按钮，按钮点击 → `commands::dispatch` → 调 approval sink。**不再用 text 兜底**。
- **依赖**：D1 必须先完成。
- **改动文件**：`approval_card.rs` (Panel UI) + `commands.rs`（dispatch 路径）+ `handlers/approval.rs` 接收侧（已通）。
- **验收**：Panel 渲染 Discord approval 卡片，按钮真的能 resolve 一次 approval record（不再像 R1 那样 raw text）。

#### D5 — Discord TTS outbound 接线
- **模块**：`src/gateway/interfaces/discord/mod.rs::send` + `src/gateway/voice/outbound.rs`
- **类型**：blocked | **工作量**：M
- **现状**：R1 只接了 voice inbound (STT) 侧——音频附件转写进文本。**TTS outbound 未接**，需要 voice 子系统先暴露 `voice.synthesize(text) -> AudioBytes` 接口。
- **目标**：discord `send` 接受 `OutboundMessage{voice_payload: Some(text)}` 时调 `voice::synthesize` → 生成 opus → `CreateAttachment::path` 发音频消息。
- **依赖**：voice 子系统先暴露 `voice.synthesize`（属 voice 子系统责任，不属本轮 — `D5` 等 voice R?）。
- **改动文件**：D5 落地时再议（待 voice 子系统 spec）。
- **验收**：未启动；列出待 voice 子系统 readiness。

#### D7 — PluralKit 检测 + 转发身份处理
- **模块**：`src/gateway/interfaces/discord/` 新增 `pluralkit.rs`（独立模块，不破坏 R1 边界）
- **类型**：new-feature | **工作量**：M
- **现状**：Aleph 完全不识别 PluralKit 转发——如果用户在 Discord 用 PK 系统转发消息，agent 会把转发者（system member）当真实发送者，导致 audit trail 记错人。
- **目标**：检测 inbound message 的 `MessageReference` + 系统成员元数据 → 推断真实 `sender_id`；audit log 的 `actor_user` 字段填真实 PK member id。
- **对标**：openclaw `pluralkit.ts`（429 LOC）。
- **改动文件**：新增 `pluralkit.rs` + `mod.rs` inbound 路径钩入 + `audit_hooks.rs` 配合。
- **验收**：用一个 mock PK 系统消息（fixture）验证 actor_user 解析正确。
- **风险**：PK API 速率限制——需每 guild 缓存。

#### D8 — session-key normalization + DM/group policy 深化
- **模块**：`src/gateway/interfaces/discord/mod.rs::inbound_router` + `src/gateway/channel.rs::ConversationId`
- **类型**：new-feature | **工作量**：M
- **现状**：Discord session key 拼接规则散在 inbound handler 各处（guild/channel/user 三种拼接），没有 SSOT。DM/group 政策实现是硬 if 链。
- **目标**：抽出 `session_key.rs` 单源（参照 openclaw `session-key-normalization.ts`），覆盖：DM `<@user_id>` / guild `<guild_id>#<channel_id>` / thread `<channel_id>+<thread_id>` 三种形态；`group_policy.rs` 抽 module（openclaw 有独立 224 行文件）。
- **改动文件**：新增 `session_key.rs` + `group_policy.rs`；改 `inbound_router` 调用方。
- **验收**：现有 3 个 discord integration test 仍绿 + 6 个新单测（每种 session_key 形态 + 每种 group policy 分支）。

## 建议执行顺序

```
1. 先 A 类（闭合 R1 脚手架）─── 这是债，必须还
   ├── D2 + D3 同时做（S 类，单文件，1-2 天）
   └── D6 单独做（S 类，1-3 小时），D6 是 D1 的前置
2. D1 ── broken-wiring 收敛（保留 fallback 防回归）
3. 再 B 类（产品增强）─── 可选，看优先级
   ├── D4 ── 紧跟 D1，approval capability 上线
   ├── D8 ── SSOT 重构，session-key/group-policy 拆分
   ├── D7 ── PK 检测（依赖 audit_hooks.D2 完成）
   └── D5 ── blocked on voice.synthesize
```

## 不做（明确边界）

- ❌ Plugin SDK 化（openclaw 形态，R7 mismatch，R1 已决策）
- ❌ 新建 `src/gateway/interfaces/discord/voice/`（spec 红线）
- ❌ TTS outbound 在 voice 子系统暴露 `voice.synthesize` 之前（D5 blocked）
- ❌ Discord 全量 175k 行 TS 移植（架构差异巨大）
- ❌ Panel/TUI 渲染侧的 Discord 适配（属前端工作，独立轮次）

## 验收七条（每 D 任务完成后必跑）

```bash
cd /home/zou/data/workspace/Aleph-discord-r2-<task>
CARGO_BUILD_JOBS=2 cargo check -p alephcore 2>&1 | tail -3
CARGO_BUILD_JOBS=2 cargo clippy -p alephcore -- -D warnings 2>&1 | tail -3
CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib discord 2>&1 | tail -10
```

合并点：每个 D 任务独立 commit + 独立 merge（不像 R1 是批量合并）。

## 与 R1 的关系

| R1 落地状态 | R2 任务 |
|---|---|
| ✅ 线2 commands.rs (D1 前置已就位) | D1 替换 handle_component |
| ⚠️ 线1 audit_hooks.rs 未落地 | D2 真正落地 |
| ⚠️ 线1 startup_audit.rs 编译过但未连 start() | D3 接线 |
| ✅ 线2 reconnect.rs (D6 前置已就位) | D6 接 Handler mark_event |
| ⚠️ 线1 + 线2 都未做 | D4-D8 全部新功能 |

---

**最后更新**：2026-09-27（R1 线2 合 main 后起草）。后续由各 D 任务 owner 更新。

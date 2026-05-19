# exec-approval 通道审批闭环 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `ApprovalGate` 把 Ask-tier / sandbox 升级的审批请求送达 Telegram，由用户点按钮 approve/deny，决策回灌唤醒阻塞的工具调用，取代当前一律 auto-Denied。

**Architecture:** 修复 3 个库缺陷（id 透传 / 回调格式统一 / 结构化 SessionKey 路由），新增 RPC 注册函数与回调分发链路（`ApprovalCallbackSink` 注入 router），boot 构造共享 `Arc<ExecApprovalManager>` 并经 `set_requester` 注入 `ChannelApprovalBridgeAdapter`。

**Tech Stack:** Rust, tokio（oneshot / RwLock）, async-trait, teloxide(Telegram), JSON-RPC HandlerRegistry。

**Spec:** `docs/superpowers/specs/2026-05-19-exec-approval-channel-loop-design.md`

**工作目录:** worktree `/Volumes/TBU4/Workspace/Aleph-exec-approval`，分支 `feat/exec-approval-channel-wiring`。所有命令在该目录执行。

---

## 文件结构

| 文件 | 职责 | 动作 |
|------|------|------|
| `src/approval/session_route.rs` | 从结构化 `SessionKey` 解出 `(ChannelId, ConversationId)` | 新建 |
| `src/approval/callback_sink.rs` | `ManagerCallbackSink`：实现 `ApprovalCallbackSink` | 新建 |
| `src/approval/mod.rs` | 声明上两个子模块 | 改 |
| `src/approval/adapters.rs` | 改读结构化 SessionKey；`request_for_tool` 新签名 | 改 |
| `src/gateway/channel_approval.rs` | `deliver_approval` trait 增 `approval_id` 参 | 改 |
| `src/gateway/interfaces/telegram/approval.rs` | `deliver_approval` 用传入 id 拼 3 段按钮 | 改 |
| `src/exec/approval/channel_bridge.rs` | `request_for_tool` 结构化签名 + `deliver_routed` + `send_timeout_notice` | 改 |
| `src/gateway/handlers/exec_approvals.rs` | `register_handlers`；删 `create_handlers` | 改 |
| `src/gateway/inbound_router/approval_callback.rs` | `ApprovalCallbackSink` trait + `ApprovalCallbackResult` | 新建 |
| `src/gateway/inbound_router/mod.rs` | router 字段 + builder + `handle_message` 拦截 | 改 |
| `src/bin/aleph-server/commands/start/mod.rs` | 构造 manager；接 adapter / RPC / router | 改 |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` | `initialize_inbound_router` 增 sink 参 | 改 |
| `tests/exec_approval_resolve_loop.rs` | 集成测试：回调 resolve 唤醒闭环 | 新建 |

每个 Task 结束时 `cargo build -p alephcore` 必须通过（boot 相关 Task 7 用 `cargo build`）。

---

## Task 1: `channel_route` — 结构化 SessionKey → 通道路由

**Files:**
- Create: `src/approval/session_route.rs`
- Modify: `src/approval/mod.rs`

- [ ] **Step 1: 声明模块**

在 `src/approval/mod.rs` 的 `pub mod adapters;` 下一行加：

```rust
mod session_route;
```

- [ ] **Step 2: 写失败测试 + 实现骨架**

创建 `src/approval/session_route.rs`：

```rust
//! 从结构化 `SessionKey` 解出通道投递路由 `(ChannelId, ConversationId)`。
//!
//! 替代 `ChannelApprovalBridge::parse_session_key` 对字符串 session_key 的
//! 有损扫描 —— 后者对默认 `DmScope::PerPeer`（`agent:{a}:dm:{p}`，不含通道名）
//! 静默返回 `None`。结构化 `SessionKey` 直接携带 `channel` 字段，无歧义。

use crate::gateway::channel::{ChannelId, ConversationId};
use crate::routing::session_key::SessionKey;

/// 解出审批提示应投递到的通道与会话。
///
/// `Main` / `Task` / `Ephemeral` 无通道来源 → `None`（调用方据此明确 Denied）。
/// `Subagent` 递归其 `parent_key`。
pub(crate) fn channel_route(key: &SessionKey) -> Option<(ChannelId, ConversationId)> {
    match key {
        SessionKey::DirectMessage {
            channel, peer_id, ..
        }
        | SessionKey::Group {
            channel, peer_id, ..
        } => Some((
            ChannelId::new(channel.clone()),
            ConversationId::new(peer_id.clone()),
        )),
        SessionKey::Subagent { parent_key, .. } => channel_route(parent_key),
        SessionKey::Main { .. } | SessionKey::Task { .. } | SessionKey::Ephemeral { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::{DmScope, PeerKind, SessionKey};

    #[test]
    fn dm_per_peer_routes_to_channel() {
        let key = SessionKey::dm("main", "telegram", "123456", DmScope::PerPeer);
        let (ch, conv) = channel_route(&key).expect("DM must route");
        assert_eq!(ch.as_str(), "telegram");
        assert_eq!(conv.as_str(), "123456");
    }

    #[test]
    fn dm_per_channel_peer_routes() {
        let key = SessionKey::dm("main", "telegram", "u1", DmScope::PerChannelPeer);
        assert!(channel_route(&key).is_some());
    }

    #[test]
    fn group_routes_to_channel() {
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "g1");
        let (ch, conv) = channel_route(&key).expect("Group must route");
        assert_eq!(ch.as_str(), "telegram");
        assert_eq!(conv.as_str(), "g1");
    }

    #[test]
    fn main_session_has_no_route() {
        assert!(channel_route(&SessionKey::main("main")).is_none());
    }

    #[test]
    fn ephemeral_session_has_no_route() {
        assert!(channel_route(&SessionKey::ephemeral("main")).is_none());
    }

    #[test]
    fn task_session_has_no_route() {
        assert!(channel_route(&SessionKey::task("main", "cron", "daily")).is_none());
    }

    #[test]
    fn subagent_recurses_to_parent() {
        let parent = SessionKey::dm("main", "telegram", "777", DmScope::PerPeer);
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "coding".to_string(),
        };
        let (ch, conv) = channel_route(&key).expect("Subagent must recurse");
        assert_eq!(ch.as_str(), "telegram");
        assert_eq!(conv.as_str(), "777");
    }

    #[test]
    fn subagent_of_main_has_no_route() {
        let key = SessionKey::Subagent {
            parent_key: Box::new(SessionKey::main("main")),
            subagent_id: "x".to_string(),
        };
        assert!(channel_route(&key).is_none());
    }
}
```

- [ ] **Step 3: 跑测试确认通过**

Run: `cargo test -p alephcore --lib approval::session_route -- --nocapture`
Expected: 8 tests PASS。若 `SessionKey::group` 参数数量不符，按 `session_key.rs` 内 `test_group_constructor`（`SessionKey::group("main","discord",PeerKind::Group,"guild456")`）对齐。

- [ ] **Step 4: Commit**

```bash
git add src/approval/session_route.rs src/approval/mod.rs
git commit -m "approval: structured SessionKey -> channel route helper"
```

---

## Task 2: `deliver_approval` 接收调用方 id（缺陷①②）

**Files:**
- Modify: `src/gateway/channel_approval.rs`（trait 签名 + 文档示例）
- Modify: `src/gateway/interfaces/telegram/approval.rs`（impl + `approval_callback_data`）
- Modify: `src/exec/approval/channel_bridge.rs:108`（唯一 caller）
- Test: `src/gateway/interfaces/telegram/approval.rs` 内 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败测试**

在 `src/gateway/interfaces/telegram/approval.rs` 文件末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ApprovalBridge;

    #[test]
    fn approval_callback_data_is_three_part_and_parseable() {
        let approve = TelegramChannelApprovalCapability::approval_callback_data(
            ApprovalAction::Approve,
            "rec-123",
        );
        let deny = TelegramChannelApprovalCapability::approval_callback_data(
            ApprovalAction::Deny,
            "rec-123",
        );
        assert_eq!(approve, "approve:rec-123:once");
        assert_eq!(deny, "approve:rec-123:deny");

        // 与 RPC 侧 ApprovalBridge::parse_callback 必须双向一致
        let (id, _) = ApprovalBridge::parse_callback(&approve).expect("approve parses");
        assert_eq!(id, "rec-123");
        let (id, _) = ApprovalBridge::parse_callback(&deny).expect("deny parses");
        assert_eq!(id, "rec-123");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib telegram::approval -- --nocapture`
Expected: FAIL —— `approval_callback_data` 当前返回 2 段 `approve:rec-123` / `deny:rec-123`。

- [ ] **Step 3: 改 trait 签名**

在 `src/gateway/channel_approval.rs`，`ChannelApprovalCapability::deliver_approval` 改为：

```rust
    /// Deliver an approval request to the user.
    ///
    /// `approval_id` is the caller-owned id (the `ExecApprovalManager` record
    /// id). The capability MUST embed it verbatim into the button callback
    /// data so the click resolves the correct pending approval.
    ///
    /// Returns a `PendingApproval` that can be used to resolve or cancel later.
    async fn deliver_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        approval_id: &str,
    ) -> ChannelResult<PendingApproval>;
```

同文件 trait 文档示例（约 106-118 行 `/// async fn deliver_approval(...)`）把示例签名同步成带 `approval_id` 的形式（仅文档注释，照抄上面签名）。

- [ ] **Step 4: 改 `approval_callback_data` 为 3 段**

`src/gateway/interfaces/telegram/approval.rs` 的 `approval_callback_data`：

```rust
    fn approval_callback_data(action: ApprovalAction, approval_id: &str) -> String {
        match action {
            ApprovalAction::Approve => format!("approve:{}:once", approval_id),
            ApprovalAction::Deny => format!("approve:{}:deny", approval_id),
        }
    }
```

- [ ] **Step 5: 改 Telegram `deliver_approval`**

`src/gateway/interfaces/telegram/approval.rs` 的 `deliver_approval` 全替换为（用传入 `approval_id` 自拼键盘，不再调 `render_approval`）：

```rust
    async fn deliver_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        approval_id: &str,
    ) -> ChannelResult<PendingApproval> {
        let expires_at = Utc::now() + Duration::minutes(5);
        let text = self.render_approval_text(request);

        let keyboard = InlineKeyboard::new()
            .button(
                "\u{2705} Approve",
                Self::approval_callback_data(ApprovalAction::Approve, approval_id),
            )
            .button(
                "\u{274c} Deny",
                Self::approval_callback_data(ApprovalAction::Deny, approval_id),
            );

        let mut message = OutboundMessage::text(conversation_id.as_str(), text);
        message.inline_keyboard = Some(keyboard);

        let result = self.channel.send(message).await?;

        Ok(PendingApproval::new(
            approval_id,
            request.clone(),
            self.channel.id().as_str(),
            conversation_id.clone(),
            expires_at,
        )
        .with_message_id(result.message_id.as_str()))
    }
```

- [ ] **Step 6: 改唯一 caller**

`src/exec/approval/channel_bridge.rs` 第 ~106-110 行，`capability.deliver_approval(&conversation_id, &approval_req)` 改为：

```rust
            capability.deliver_approval(&conversation_id, &approval_req, &request.id),
```

（`request` 是 `&ApprovalRequest`，其 `.id` 即记录 id。此处属旧 `request_approval` 方法，Task 3 不动它，仅补 id 参数让其继续编译。）

- [ ] **Step 7: 跑测试 + 编译**

Run: `cargo test -p alephcore --lib telegram::approval -- --nocapture`
Expected: PASS。
Run: `cargo build -p alephcore`
Expected: 编译通过（`render_approval` 仍是 trait 必需方法，保留不报错）。

- [ ] **Step 8: Commit**

```bash
git add src/gateway/channel_approval.rs src/gateway/interfaces/telegram/approval.rs src/exec/approval/channel_bridge.rs
git commit -m "approval: deliver_approval embeds caller-owned id in callback data"
```

---

## Task 3: `request_for_tool` 结构化路由 + adapter 改造（缺陷③）

**Files:**
- Modify: `src/exec/approval/channel_bridge.rs`（`request_for_tool` 签名 + `deliver_routed` + `send_timeout_notice`）
- Modify: `src/approval/adapters.rs`（`request_approval` 改读结构化 SessionKey）
- Test: `src/approval/adapters.rs` 内 `#[cfg(test)] mod tests`

- [ ] **Step 1: 改 `channel_bridge.rs` 顶部 import**

`src/exec/approval/channel_bridge.rs` 第 10 行：

```rust
use crate::gateway::channel::{ChannelId, ConversationId, OutboundMessage, UserId};
```

（在原 `{ChannelId, ConversationId, UserId}` 中加入 `OutboundMessage`。）

- [ ] **Step 2: 新签名 `request_for_tool` + 私有方法**

`src/exec/approval/channel_bridge.rs` 中 `request_for_tool` 全替换为下面三个方法（替换原 `request_for_tool` 整段，含其文档注释）：

```rust
    /// 请求工具调用审批并阻塞等待用户决策。
    ///
    /// 两阶段：(1) `manager.create` 建记录得 `record.id`；
    /// (2) `deliver_routed` 用 `record.id` 投递按钮到指定通道会话；
    /// (3) `wait_for_decision` 阻塞在 `record.id` 的 oneshot，由通道按钮回调
    /// 经 `manager.resolve(record.id, ...)` 唤醒。
    ///
    /// `channel_id` / `conversation_id` 为结构化路由参数（来自调用方解析的
    /// `SessionKey`），不再 parse 有损的字符串 session_key。
    pub async fn request_for_tool(
        &self,
        approval_manager: &crate::exec::manager::ExecApprovalManager,
        tool_name: &str,
        reason: &str,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        agent_id: &str,
        timeout_ms: u64,
    ) -> ApprovalOutcome {
        #[cfg(test)]
        if let Some(outcome) = self.test_outcome_override {
            return outcome;
        }

        let request = ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            command: tool_name.to_string(),
            cwd: None,
            analysis: crate::exec::analysis::CommandAnalysis::error(reason),
            agent_id: agent_id.to_string(),
            session_key: format!("{}:{}", channel_id.as_str(), conversation_id.as_str()),
        };

        let record = approval_manager.create(&request, timeout_ms);

        match self
            .deliver_routed(channel_id, conversation_id, tool_name, &record.id)
            .await
        {
            Some(true) => {
                tracing::info!(
                    tool = %tool_name,
                    id = %record.id,
                    channel = %channel_id.as_str(),
                    "Approval delivered via channel — waiting for user decision"
                );
            }
            Some(false) => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record.id,
                    "Approval delivery failed — denying"
                );
                return ApprovalOutcome::Denied;
            }
            None => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record.id,
                    "No channel capability for approval delivery — denying"
                );
                return ApprovalOutcome::Denied;
            }
        }

        match approval_manager.wait_for_decision(record).await {
            Some(crate::exec::socket::ApprovalDecisionType::AllowOnce)
            | Some(crate::exec::socket::ApprovalDecisionType::AllowAlways) => {
                ApprovalOutcome::Approved
            }
            Some(crate::exec::socket::ApprovalDecisionType::Deny) => ApprovalOutcome::Denied,
            None => {
                self.send_timeout_notice(channel_id, conversation_id).await;
                ApprovalOutcome::Timeout
            }
        }
    }

    /// 按结构化 `channel_id` 投递审批提示。返回 `Some(true)` 已投递、
    /// `Some(false)` 投递失败、`None` 无通道 / 无审批能力。
    async fn deliver_routed(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        tool_name: &str,
        approval_id: &str,
    ) -> Option<bool> {
        let channel = self.registry.get(channel_id).await?;
        let capability = {
            let ch = channel.read().await;
            ch.approval_capability()?
        };
        let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
            crate::exec::approval::types::CommandApprovalRequest {
                command: tool_name.to_string(),
                cwd: None,
            },
        );
        match timeout(
            Duration::from_secs(DELIVERY_TIMEOUT_SECS),
            capability.deliver_approval(conversation_id, &approval_req, approval_id),
        )
        .await
        {
            Ok(Ok(_pending)) => Some(true),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "deliver_approval returned error");
                Some(false)
            }
            Err(_) => {
                tracing::warn!("deliver_approval timed out after {}s", DELIVERY_TIMEOUT_SECS);
                Some(false)
            }
        }
    }

    /// 审批超时后向通道发一条友好提示（best-effort）。
    async fn send_timeout_notice(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
    ) {
        if let Some(channel) = self.registry.get(channel_id).await {
            let ch = channel.read().await;
            let msg = OutboundMessage::text(
                conversation_id.as_str(),
                "\u{23f1} 审批请求已超时，操作被拒绝。",
            );
            let _ = ch.send(msg).await;
        }
    }
```

- [ ] **Step 3: 改 adapter 测试 helper + 写期望（先红）**

`src/approval/adapters.rs` 的 `#[cfg(test)] mod tests`：把 `test_session_id` 改为返回 DM key，并更新 import。将测试模块顶部 `use` 与 `test_session_id` 改为：

```rust
    use super::*;
    use crate::routing::session_key::{DmScope, SessionKey};
    use crate::session::with_session_scope;

    fn test_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    /// DM session key — `channel_route` 能解出 `("telegram","123456")`。
    fn test_session_id() -> crate::session::service::SessionId {
        SessionKey::dm("main", "telegram", "123456", DmScope::PerPeer)
    }
```

其余三个测试 `adapter_forwards_approved` / `adapter_forwards_denied` / `adapter_denies_when_channel_missing` 函数体不变（它们已用 `test_session_id()`）。`adapter_denies_when_session_id_unset` 不变。

- [ ] **Step 4: 跑测试确认失败**

Run: `cargo test -p alephcore --lib approval::adapters -- --nocapture`
Expected: FAIL（`request_approval` 仍调旧签名 `request_for_tool`，类型不匹配，编译失败）。

- [ ] **Step 5: 改 adapter 实现**

`src/approval/adapters.rs`：顶部 `use` 区加：

```rust
use crate::approval::session_route::channel_route;
use crate::gateway::channel::{ChannelId, ConversationId};
```

把 `current_session_key` 方法替换为 `current_channel_route`，并改 `request_approval`：

```rust
    /// 从 `SESSION_ID` task-local 的结构化 `SessionKey` 解出通道路由。
    /// task-local 未设置、或会话无通道来源时返回 `None`。
    fn current_channel_route() -> Option<(ChannelId, ConversationId)> {
        crate::sandbox::context::SESSION_ID
            .try_with(|sid| channel_route(sid))
            .ok()
            .flatten()
    }
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        let Some((channel_id, conversation_id)) = Self::current_channel_route() else {
            tracing::warn!(
                tool = %tool_name,
                "ChannelApprovalBridgeAdapter: no channel route from SESSION_ID \
                 (unset, or Main/Task/Ephemeral session) — denying"
            );
            return ApprovalOutcome::Denied;
        };

        self.bridge
            .request_for_tool(
                &self.approval_manager,
                tool_name,
                reason,
                &channel_id,
                &conversation_id,
                "",
                self.timeout_ms,
            )
            .await
    }
}
```

同时更新文件头 doc 注释中关于 `SESSION_ID` 序列化的描述（约 9-16 行），改为「直接读结构化 `SessionKey` 经 `channel_route` 解出 `(ChannelId, ConversationId)`；无通道来源产生 `Denied`」。

- [ ] **Step 6: 跑测试 + 编译**

Run: `cargo test -p alephcore --lib approval::adapters -- --nocapture`
Expected: 4 tests PASS（forwards_approved/denied 经 `test_outcome_override`；denies_when_channel_missing 经空 registry；denies_when_session_id_unset 经 task-local 未设）。
Run: `cargo build -p alephcore`
Expected: 通过。

- [ ] **Step 7: Commit**

```bash
git add src/exec/approval/channel_bridge.rs src/approval/adapters.rs
git commit -m "approval: route approvals via structured SessionKey, not lossy string"
```

---

## Task 4: 注册 `exec.approval.*` RPC 处理器

**Files:**
- Modify: `src/gateway/handlers/exec_approvals.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/exec_approvals.rs` 的 `mod tests` 内追加：

```rust
    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let (_dir, manager) = temp_manager();
        let mut registry = super::super::HandlerRegistry::empty();
        register_handlers(&mut registry, manager);
        for m in [
            "exec.approval.request",
            "exec.approval.resolve",
            "exec.approvals.get",
            "exec.approvals.set",
            "exec.approvals.pending",
            "exec.callback.handle",
        ] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib exec_approvals::tests::register_handlers -- --nocapture`
Expected: FAIL —— `register_handlers` 未定义。

- [ ] **Step 3: 实现 `register_handlers`，删除 `create_handlers`**

`src/gateway/handlers/exec_approvals.rs`：
1. 删除 `use std::future::Future;` 与 `use std::pin::Pin;` 两行。
2. 删除 `type RpcHandler = ...;` 整段。
3. 删除 `pub fn create_handlers(...)` 整个函数。
4. 顶部 `use` 区加：`use super::HandlerRegistry;`
5. 在原 `create_handlers` 位置插入：

```rust
/// 把 exec-approval 全部方法注册进 JSON-RPC 处理器注册表。
/// 所有方法共享同一个 `Arc<ExecApprovalManager>`。
pub fn register_handlers(registry: &mut HandlerRegistry, manager: Arc<ExecApprovalManager>) {
    {
        let m = manager.clone();
        registry.register("exec.approval.request", move |req| {
            let m = m.clone();
            async move { handle_approval_request(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approval.resolve", move |req| {
            let m = m.clone();
            async move { handle_approval_resolve(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.get", move |req| {
            let m = m.clone();
            async move { handle_approvals_get(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.set", move |req| {
            let m = m.clone();
            async move { handle_approvals_set(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.pending", move |req| {
            let m = m.clone();
            async move { handle_approvals_pending(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.callback.handle", move |req| {
            let m = m.clone();
            async move { handle_callback(req, m).await }
        });
    }
}
```

- [ ] **Step 4: 跑测试 + 编译**

Run: `cargo test -p alephcore --lib exec_approvals -- --nocapture`
Expected: 全 PASS（含新测试 + 原有 handler 测试）。
Run: `cargo build -p alephcore`
Expected: 通过，且无 `create_handlers` 未用告警（已删）。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/exec_approvals.rs
git commit -m "gateway: register exec.approval.* handlers, drop dead create_handlers"
```

---

## Task 5: `ApprovalCallbackSink` trait + `ManagerCallbackSink`

**Files:**
- Create: `src/gateway/inbound_router/approval_callback.rs`
- Create: `src/approval/callback_sink.rs`
- Modify: `src/gateway/inbound_router/mod.rs`（声明子模块）
- Modify: `src/approval/mod.rs`（声明子模块）

- [ ] **Step 1: 声明模块**

`src/gateway/inbound_router/mod.rs`：在模块声明区（文件靠前的 `mod xxx;` 群）加：

```rust
pub mod approval_callback;
```

`src/approval/mod.rs`：在 `mod session_route;` 下加：

```rust
pub mod callback_sink;
```

- [ ] **Step 2: 写 trait 模块**

创建 `src/gateway/inbound_router/approval_callback.rs`：

```rust
//! Interface 层与 core 审批管理之间的窄接口。
//!
//! `InboundMessageRouter` 保持纯 I/O：把回调 callback_data 交给注入的
//! `ApprovalCallbackSink`、再把返回文案渲染回通道 —— 自身不解析、不 resolve。

use async_trait::async_trait;

/// 解析一次审批按钮回调的结果。
pub struct ApprovalCallbackResult {
    /// 是否真正投递了一个待决审批的决策（false = 过期 / 未知 id）。
    pub resolved: bool,
    /// 渲染回通道的用户可见文案。
    pub response_text: String,
}

/// router 注入的审批回调汇。具体实现包 `ExecApprovalManager`。
#[async_trait]
pub trait ApprovalCallbackSink: Send + Sync {
    /// 返回 `Some` 当且仅当 `callback_data` 是审批按钮回调；
    /// `None` 表示非审批回调，router 应放行进正常消息流。
    async fn handle_callback(
        &self,
        callback_data: &str,
        user_id: &str,
    ) -> Option<ApprovalCallbackResult>;
}
```

- [ ] **Step 3: 写 `ManagerCallbackSink` + 失败测试**

创建 `src/approval/callback_sink.rs`：

```rust
//! `ApprovalCallbackSink` 的实现 —— 把通道按钮回调投递进 `ExecApprovalManager`。

use async_trait::async_trait;

use crate::exec::bridge::ApprovalBridge;
use crate::exec::manager::ExecApprovalManager;
use crate::gateway::inbound_router::approval_callback::{
    ApprovalCallbackResult, ApprovalCallbackSink,
};
use crate::sync_primitives::Arc;

/// 包 `Arc<ExecApprovalManager>`，解析回调并 resolve 对应待决审批。
pub struct ManagerCallbackSink {
    manager: Arc<ExecApprovalManager>,
}

impl ManagerCallbackSink {
    pub fn new(manager: Arc<ExecApprovalManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ApprovalCallbackSink for ManagerCallbackSink {
    async fn handle_callback(
        &self,
        callback_data: &str,
        user_id: &str,
    ) -> Option<ApprovalCallbackResult> {
        // parse 失败 → 非审批回调 → None（router 放行）
        let (id, decision) = ApprovalBridge::parse_callback(callback_data)?;
        let resolved = self
            .manager
            .resolve(&id, decision, Some(user_id.to_string()));
        let response_text = if resolved {
            ApprovalBridge::decision_response_text(&decision).to_string()
        } else {
            "该审批已过期或已处理。".to_string()
        };
        Some(ApprovalCallbackResult {
            resolved,
            response_text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::analysis::CommandAnalysis;
    use crate::exec::decision::ApprovalRequest;
    use crate::exec::socket::ApprovalDecisionType;

    fn mock_request(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            command: "code_exec".to_string(),
            cwd: None,
            analysis: CommandAnalysis::error("danger"),
            agent_id: "main".to_string(),
            session_key: "telegram:123".to_string(),
        }
    }

    #[tokio::test]
    async fn non_callback_data_returns_none() {
        let sink = ManagerCallbackSink::new(Arc::new(ExecApprovalManager::new()));
        assert!(sink.handle_callback("hello world", "u1").await.is_none());
    }

    #[tokio::test]
    async fn unknown_id_reports_not_resolved() {
        let sink = ManagerCallbackSink::new(Arc::new(ExecApprovalManager::new()));
        let out = sink
            .handle_callback("approve:no-such-id:once", "u1")
            .await
            .expect("is an approval callback");
        assert!(!out.resolved);
        assert!(out.response_text.contains("过期"));
    }

    #[tokio::test]
    async fn pending_approval_gets_resolved() {
        let manager = Arc::new(ExecApprovalManager::new());
        let record = manager.create(&mock_request("rec-1"), 5_000);
        let id = record.id.clone();

        // 在后台任务里 wait（wait_for_decision 负责把 record 注册进 pending）
        let m2 = manager.clone();
        let waiter = tokio::spawn(async move { m2.wait_for_decision(record).await });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let sink = ManagerCallbackSink::new(manager.clone());
        let out = sink
            .handle_callback(&format!("approve:{}:once", id), "u1")
            .await
            .expect("is an approval callback");
        assert!(out.resolved);

        let decision = waiter.await.unwrap();
        assert_eq!(decision, Some(ApprovalDecisionType::AllowOnce));
    }
}
```

- [ ] **Step 4: 跑测试**

Run: `cargo test -p alephcore --lib callback_sink -- --nocapture`
Expected: 3 tests PASS。若 `ApprovalRequest` 字段名/路径不符，照 `src/exec/manager.rs` 内 `mock_request()`（test 模块）对齐。

- [ ] **Step 5: 编译检查**

Run: `cargo build -p alephcore`
Expected: 通过。

- [ ] **Step 6: Commit**

```bash
git add src/gateway/inbound_router/approval_callback.rs src/approval/callback_sink.rs src/gateway/inbound_router/mod.rs src/approval/mod.rs
git commit -m "approval: ApprovalCallbackSink trait + ManagerCallbackSink impl"
```

---

## Task 6: router 拦截 `cb_` 回调

**Files:**
- Modify: `src/gateway/inbound_router/mod.rs`
- Test: 同文件 `#[cfg(test)] mod tests`（若无则新建）

- [ ] **Step 1: 加字段**

`src/gateway/inbound_router/mod.rs` 的 `struct InboundMessageRouter` 末尾字段（`coalescer` 之后）加：

```rust
    /// 审批按钮回调汇 —— 注入则拦截 `cb_` 回调，否则按普通消息处理。
    pub(super) approval_callback_sink:
        Option<Arc<dyn approval_callback::ApprovalCallbackSink>>,
```

`new()` 构造体里（`coalescer: None,` 之后）加：

```rust
            approval_callback_sink: None,
```

- [ ] **Step 2: 加 builder**

在 `with_coalescer` 方法之后加：

```rust
    /// 注入审批回调汇，启用通道按钮 approve/deny 分发。
    pub fn with_approval_callback_sink(
        mut self,
        sink: Arc<dyn approval_callback::ApprovalCallbackSink>,
    ) -> Self {
        self.approval_callback_sink = Some(sink);
        self
    }
```

- [ ] **Step 3: `handle_message` 顶部拦截**

`handle_message` 的开头 `info!("[Router] Handling message ...")` 之后、`// Resolve agent ID` 之前插入：

```rust
        // 审批按钮回调短路：在正常路由之前拦截。
        // callback query 入站消息 id 以 "cb_" 前缀（webhook / 轮询两路一致）。
        if msg.id.as_str().starts_with("cb_") {
            if let Some(ref sink) = self.approval_callback_sink {
                if let Some(result) = sink
                    .handle_callback(&msg.text, msg.sender_id.as_str())
                    .await
                {
                    let reply = OutboundMessage::text(
                        msg.conversation_id.as_str(),
                        result.response_text,
                    );
                    let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                    return Ok(());
                }
                // sink 返回 None → 非审批回调 → 落入正常消息流
            }
        }
```

- [ ] **Step 4: 写测试**

在 `src/gateway/inbound_router/mod.rs` 的 `#[cfg(test)] mod tests` 内追加（若文件无 tests 模块，在文件末尾新建 `#[cfg(test)] mod tests { use super::*; ... }`）：

```rust
    #[tokio::test]
    async fn cb_message_with_approval_sink_is_intercepted() {
        use crate::gateway::inbound_router::approval_callback::{
            ApprovalCallbackResult, ApprovalCallbackSink,
        };

        struct AlwaysIntercept;
        #[async_trait::async_trait]
        impl ApprovalCallbackSink for AlwaysIntercept {
            async fn handle_callback(&self, _d: &str, _u: &str) -> Option<ApprovalCallbackResult> {
                Some(ApprovalCallbackResult {
                    resolved: true,
                    response_text: "ok".to_string(),
                })
            }
        }

        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            crate::gateway::pairing_store::in_memory_pairing_store(),
            RoutingConfig::default(),
        )
        .with_approval_callback_sink(Arc::new(AlwaysIntercept));

        let msg = InboundMessage {
            id: MessageId::new("cb_1"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("123"),
            sender_id: UserId::new("u1"),
            sender_name: None,
            text: "approve:rec-1:once".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        // 拦截 → 早返回 Ok，不进入 agent 解析。
        assert!(router.handle_message(msg).await.is_ok());
    }
```

> 实现注意：`in_memory_pairing_store()` / `PairingStore` 测试构造函数名以代码库实际为准 —— 执行前 `grep -rn "PairingStore" src/gateway/ | grep -i test\|memory\|new` 确认；若无现成内存实现，复用 `inbound_router` 其它测试已有的 pairing store 构造方式（搜 `InboundMessageRouter::new(` 的现有测试）。`MessageId` / `ChannelId` / `ConversationId` / `UserId` / `InboundMessage` 均来自 `crate::gateway::channel`。

- [ ] **Step 5: 跑测试 + 编译**

Run: `cargo test -p alephcore --lib inbound_router -- --nocapture`
Expected: 新测试 PASS，原有测试不回归。
Run: `cargo build -p alephcore`
Expected: 通过。

- [ ] **Step 6: Commit**

```bash
git add src/gateway/inbound_router/mod.rs
git commit -m "gateway: router intercepts approval-button callbacks before routing"
```

---

## Task 7: boot 接线

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs`

- [ ] **Step 1: 构造共享 `ExecApprovalManager`**

`src/bin/aleph-server/commands/start/mod.rs`：在 `approval_gate` 构造块（约第 552 行 `let approval_gate = {`）**之前**插入：

```rust
    // exec-approval 闭环共享实例：boot 构造一个 ExecApprovalManager，
    // 供 (a) ChannelApprovalBridgeAdapter (ApprovalGate requester)、
    // (b) exec.approval.* RPC 处理器、(c) router 回调汇 共用。
    let exec_approval_manager =
        alephcore::sync_primitives::Arc::new(alephcore::exec::ExecApprovalManager::new());
```

> `Arc` 路径以该文件既有写法为准（文件内已大量用 `Arc::new` —— 直接 `Arc::new(...)`，`Arc` 已在 use 域内）。若顶部已 `use ...Arc`，写 `Arc::new(alephcore::exec::ExecApprovalManager::new())`。

- [ ] **Step 2: 改 `initialize_inbound_router` 签名（subsystems.rs）**

`src/bin/aleph-server/commands/start/builder/subsystems.rs` 的 `initialize_inbound_router`：在参数表末尾 `daemon: bool,` 之前加一参：

```rust
    approval_callback_sink: Option<
        Arc<dyn alephcore::gateway::inbound_router::approval_callback::ApprovalCallbackSink>,
    >,
```

并在该函数内构造 `inbound_router` 后、`.with_*` 链合适处加（紧跟 `let mut inbound_router = ...` 之后即可）：

```rust
    if let Some(sink) = approval_callback_sink {
        inbound_router = inbound_router.with_approval_callback_sink(sink);
    }
```

> `with_approval_callback_sink` 接受 `self` 返回 `Self`，而此处 `inbound_router` 是 `mut` 变量：写成 `inbound_router = inbound_router.with_approval_callback_sink(sink);`。

- [ ] **Step 3: boot 注册 RPC + 注入 adapter + 传 sink**

`src/bin/aleph-server/commands/start/mod.rs`：在 `initialize_channels(...)` 调用返回（约第 1705 行 `.await;` 之后）、`initialize_inbound_router(...)` 之前插入：

```rust
    // exec-approval 闭环接线（channel_registry 已就绪）。
    {
        use alephcore::approval::adapters::ChannelApprovalBridgeAdapter;
        use alephcore::approval::callback_sink::ManagerCallbackSink;
        use alephcore::exec::approval::channel_bridge::ChannelApprovalBridge;

        // (2) 注册 exec.approval.* RPC 处理器（server handlers 此刻引用计数为 1）。
        alephcore::gateway::handlers::exec_approvals::register_handlers(
            server.handlers_mut(),
            exec_approval_manager.clone(),
        );

        // (5) 构造 bridge + adapter，注入 ApprovalGate。
        let bridge = Arc::new(ChannelApprovalBridge::new(channel_registry.clone()));
        let adapter = Arc::new(ChannelApprovalBridgeAdapter::new(
            bridge,
            exec_approval_manager.clone(),
        ));
        approval_gate.set_requester(adapter);

        if !args.daemon {
            println!("exec-approval: ApprovalGate requester wired (Telegram channel approvals enabled)");
        }
    }
```

把 `initialize_inbound_router(...)` 调用的实参表末尾、`args.daemon,` 之前加：

```rust
        Some(Arc::new(alephcore::approval::callback_sink::ManagerCallbackSink::new(
            exec_approval_manager.clone(),
        ))),
```

- [ ] **Step 4: 删除过时告警**

`src/bin/aleph-server/commands/start/mod.rs` 中 `approval_gate` 构造块之后那段 `if !args.daemon { tracing::warn!("ApprovalGate has no ApprovalRequester wired ...") }`（约 557-565 行）整段删除 —— requester 现已接线。

- [ ] **Step 5: 验证导出可见性**

执行前确认（`grep`）：
- `src/exec/mod.rs` 是否 `pub use ...ExecApprovalManager`（exec_approvals.rs 用 `crate::exec::ExecApprovalManager` 已证可行）。
- `src/exec/approval/mod.rs` 是否 `pub mod channel_bridge;`；`ChannelApprovalBridge` 是否 `pub`（是）。
- `src/exec/approval/mod.rs` 与 `src/exec/mod.rs`、`src/gateway/handlers/mod.rs` 是否令 `alephcore::exec::approval::channel_bridge`、`alephcore::gateway::handlers::exec_approvals` 路径对 bin 可达。若某路径非 `pub`，加最小 `pub` 使其可达（不改其它语义）。

- [ ] **Step 6: 编译**

Run: `cargo build`
Expected: `aleph-server` 与 `alephcore` 均通过。报错按提示修（多为路径可见性 / `Arc` import）。

- [ ] **Step 7: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs src/bin/aleph-server/commands/start/builder/subsystems.rs
git commit -m "boot: wire exec-approval loop — manager, RPC handlers, adapter, router sink"
```

---

## Task 8: 集成测试 — 回调 resolve 唤醒闭环

**Files:**
- Create: `tests/exec_approval_resolve_loop.rs`

- [ ] **Step 1: 写集成测试**

创建 `tests/exec_approval_resolve_loop.rs`：

```rust
//! 集成测试：审批决策侧闭环 —— ManagerCallbackSink 收到按钮回调 →
//! ExecApprovalManager.resolve → 唤醒 wait_for_decision 的阻塞侧。
//!
//! 投递侧（adapter → bridge → Telegram capability）依赖真实通道，
//! 由 `/e2e-verify` 手动验证；此处覆盖与通道无关的 resolve 半环。

use std::time::Duration;

use alephcore::approval::callback_sink::ManagerCallbackSink;
use alephcore::exec::analysis::CommandAnalysis;
use alephcore::exec::decision::ApprovalRequest;
use alephcore::exec::manager::ExecApprovalManager;
use alephcore::exec::socket::ApprovalDecisionType;
use alephcore::gateway::inbound_router::approval_callback::ApprovalCallbackSink;
use alephcore::sync_primitives::Arc;

fn request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        id: id.to_string(),
        command: "code_exec".to_string(),
        cwd: None,
        analysis: CommandAnalysis::error("danger-tier"),
        agent_id: "main".to_string(),
        session_key: "telegram:123456".to_string(),
    }
}

#[tokio::test]
async fn approve_callback_wakes_blocked_waiter() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-approve"), 5_000);
    let id = record.id.clone();

    let m2 = manager.clone();
    let waiter = tokio::spawn(async move { m2.wait_for_decision(record).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sink = ManagerCallbackSink::new(manager.clone());
    let out = sink
        .handle_callback(&format!("approve:{}:once", id), "user-1")
        .await
        .expect("approval callback");
    assert!(out.resolved, "pending approval must resolve");

    let decision = waiter.await.unwrap();
    assert_eq!(decision, Some(ApprovalDecisionType::AllowOnce));
}

#[tokio::test]
async fn deny_callback_wakes_blocked_waiter() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-deny"), 5_000);
    let id = record.id.clone();

    let m2 = manager.clone();
    let waiter = tokio::spawn(async move { m2.wait_for_decision(record).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let sink = ManagerCallbackSink::new(manager.clone());
    sink.handle_callback(&format!("approve:{}:deny", id), "user-1")
        .await
        .expect("approval callback");

    assert_eq!(waiter.await.unwrap(), Some(ApprovalDecisionType::Deny));
}

#[tokio::test]
async fn timeout_when_no_callback_arrives() {
    let manager = Arc::new(ExecApprovalManager::new());
    let record = manager.create(&request("rec-timeout"), 100); // 100ms 超时
    // 不发回调 → wait_for_decision 应在超时后返回 None。
    assert_eq!(manager.wait_for_decision(record).await, None);
}

#[tokio::test]
async fn unknown_callback_does_not_resolve() {
    let manager = Arc::new(ExecApprovalManager::new());
    let sink = ManagerCallbackSink::new(manager);
    let out = sink
        .handle_callback("approve:ghost-id:once", "user-1")
        .await
        .expect("is an approval callback");
    assert!(!out.resolved);
}
```

- [ ] **Step 2: 跑集成测试**

Run: `cargo test -p alephcore --test exec_approval_resolve_loop -- --nocapture`
Expected: 4 tests PASS。若 import 路径报错，按 `cargo build` 提示对齐（`ApprovalRequest` / `CommandAnalysis` 的真实公开路径）。

- [ ] **Step 3: Commit**

```bash
git add tests/exec_approval_resolve_loop.rs
git commit -m "test: integration test for exec-approval resolve loop"
```

---

## Task 9: 全量验证

**Files:** 无（仅验证）

- [ ] **Step 1: 全量编译**

Run: `cargo build`
Expected: 全 workspace 通过。

- [ ] **Step 2: 跑相关测试**

Run: `cargo test -p alephcore --lib approval:: exec_approvals:: callback_sink inbound_router::tests session_route telegram::approval`
Run: `cargo test -p alephcore --test exec_approval_resolve_loop`
Expected: 全 PASS。

- [ ] **Step 3: clippy**

Run: `cargo clippy -p alephcore 2>&1 | grep -A3 "approval\|exec_approvals\|channel_bridge\|inbound_router" | head -40`
Expected: 不引入新告警（基线既有告警除外 —— 见 memory `fmt_clippy_baseline_drift`，不做全局 `cargo fmt`）。

- [ ] **Step 4: 仅对改动文件 fmt**

Run: `cargo fmt -p alephcore -- src/approval/session_route.rs src/approval/callback_sink.rs src/approval/adapters.rs src/exec/approval/channel_bridge.rs src/gateway/channel_approval.rs src/gateway/interfaces/telegram/approval.rs src/gateway/handlers/exec_approvals.rs src/gateway/inbound_router/approval_callback.rs src/gateway/inbound_router/mod.rs`
Expected: 仅格式化本次改动文件（勿全局 fmt）。

- [ ] **Step 5: 启动自检**

Run: `cargo run --bin aleph-server -- start --daemon &` 后查日志（或读 stdout）
Expected: 出现 `exec-approval: ApprovalGate requester wired`；**不再**出现 `ApprovalGate has no ApprovalRequester wired`。随后停服。

- [ ] **Step 6: Commit（如有 fmt 改动）**

```bash
git add -A && git commit -m "chore: fmt exec-approval wiring files" || true
```

- [ ] **Step 7: E2E（手动，记录在交付说明）**

真实 Telegram bot：触发一次 Ask-tier 工具调用 → 收到带 `✅ Approve` / `❌ Deny` 按钮的消息 → 点 Approve → 工具放行、通道收到 `✅ Allowed (once)` → 另一轮点 Deny 对称验证 → 放任 2 分钟验证收到超时提示。此步由用户在真实环境执行（`/e2e-verify` 或手动）。

---

## 验收对照（spec §11）

1. `cargo build` 全通过 — Task 9 Step 1。
2. 单测 + 集成测试全绿 — Task 1-8 各自 + Task 9 Step 2。
3. boot 不再有 "no ApprovalRequester wired" 告警 — Task 7 Step 4 + Task 9 Step 5。
4. resolve 闭环（Approved/Denied/Timeout）— Task 8。
5. 无新 clippy 告警 — Task 9 Step 3。
6. main 不受影响 — 全程在 `feat/exec-approval-channel-wiring`。

## 范围边界提醒

- 不接 `ExecSecurityGate`、不动 `parse_session_key` / `request_approval` / `authorize_and_deliver` / `resolve_approval` / `render_approval`（spec §8/§9）。
- Telegram only。Allow-Always 按钮不出（格式保留段位）。

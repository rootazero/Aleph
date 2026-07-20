use std::time::Duration;

use crate::sandbox::exec_approval::gate::ApprovalOutcome;
use crate::sync_primitives::Arc;
use tokio::time::timeout;

use crate::exec::decision::ApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::channel::{ChannelId, ConversationId, OutboundMessage};
use crate::gateway::channel_approval::ApprovalAction;
use crate::gateway::channel_registry::ChannelRegistry;

const DELIVERY_TIMEOUT_SECS: u64 = 30;

pub struct ChannelApprovalBridge {
    registry: Arc<ChannelRegistry>,
    /// Test-only override that short-circuits `request_for_tool` with a fixed
    /// outcome, bypassing the real channel lookup and pending-approval wait.
    /// Field exists only under `cfg(test)` so production has zero surface.
    #[cfg(test)]
    test_outcome_override: Option<ApprovalOutcome>,
}

impl ChannelApprovalBridge {
    pub fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self {
            registry,
            #[cfg(test)]
            test_outcome_override: None,
        }
    }

    /// 请求工具调用审批并阻塞等待用户决策。
    ///
    /// 两阶段：(1) `manager.create` 建记录得 `record.id`，随即
    /// `register_pending` 先注册（投递前注册，快手 resolver 不会扑空）；
    /// (2) `deliver_routed` 用 `record.id` 投递按钮到指定通道会话；
    /// (3) `await_registered` 阻塞在 `record.id` 的 oneshot，由通道按钮回调
    /// 经 `manager.resolve(record.id, ...)` 唤醒。
    ///
    /// `channel_id` / `conversation_id` 为结构化路由参数（来自调用方解析的
    /// `SessionKey`），不再 parse 有损的字符串 `session_key`。
    ///
    /// `session_key` 是发起会话的结构化 key 字符串（router 侧
    /// `ctx.session_key.to_string()` 的同一形态）：record 必须携带它，
    /// `/approve`/`/deny` 文本回复才能经 `resolve_for_session` 命中这条
    /// 审批（仅此会话恰有一张活卡时直接命中；多卡并发时会拒绝裸回复、
    /// 回编号清单，须 `/approve <n>` 指定——见 `SessionResolveOutcome`）。
    /// 传空则回退为 `channel:conversation` 合成 key（仅按钮回调可达）。
    pub async fn request_for_tool(
        &self,
        approval_manager: &crate::exec::manager::ExecApprovalManager,
        action: &crate::sandbox::exec_approval::ApprovalAction,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        session_key: &str,
        originator: Option<&str>,
        timeout_ms: u64,
    ) -> crate::sandbox::exec_approval::ApprovalResponse {
        #[cfg(test)]
        if let Some(outcome) = self.test_outcome_override {
            return outcome.into();
        }

        let tool_name = action.tool_name.as_str();
        let reason = action.reason.as_str();

        let record_session_key = if session_key.is_empty() {
            format!("{}:{}", channel_id.as_str(), conversation_id.as_str())
        } else {
            session_key.to_string()
        };

        let request = ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            // The redacted ACTION, not the bare tool name — this is the string
            // the user reads before deciding, on every surface.
            command: action.summary.clone(),
            cwd: action.cwd.clone(),
            analysis: action.analysis_for_record(),
            // Single source for the issuing agent (`audit_identity`); the
            // context string it also builds is redundant here — the approval
            // card renders `command` + `reason`.
            agent_id: crate::approval::audit_identity("tool", tool_name, reason).0,
            session_key: record_session_key,
            reason: Some(reason.to_string()),
            // The human who triggered this tool call. Stamped onto the record so
            // the channel button-callback gate refuses a resolution from anyone
            // but them (group-chat approval-bypass fix). `None` when the run has
            // no channel originator — the gate then no-ops.
            originator_user_id: originator.map(str::to_string),
        };

        let record = approval_manager.create(&request, timeout_ms);

        // Register the pending entry BEFORE delivering the prompt so a fast
        // resolver (instant button tap / "/approve" reply) cannot race ahead
        // of registration (resolve-before-register → spurious timeout); see
        // `ExecApprovalManager::register_pending`.
        let (record_id, rx, wait_timeout) = approval_manager.register_pending(record);

        match self
            .deliver_routed(channel_id, conversation_id, action, &record_id)
            .await
        {
            Some(true) => {
                tracing::info!(
                    tool = %tool_name,
                    id = %record_id,
                    channel = %channel_id.as_str(),
                    "Approval delivered via channel — waiting for user decision"
                );
            }
            Some(false) => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record_id,
                    "Approval delivery failed — denying"
                );
                // Retire the just-registered entry so a later session-FIFO
                // "/approve" cannot consume it.
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
                return ApprovalOutcome::Denied.into();
            }
            None => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record_id,
                    "No channel capability for approval delivery — denying"
                );
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
                return ApprovalOutcome::Denied.into();
            }
        }

        let resolved = approval_manager
            .await_registered(record_id, rx, wait_timeout)
            .await;
        let outcome = match resolved.decision {
            // "Allow once" approves this single invocation. Both "allow session"
            // and "allow always" carry the session-scoped grant so the dispatch
            // gate remembers it and stops re-prompting the same tool this
            // session: this confirm-gated tool path has no on-disk allowlist of
            // its own (persistent allowlisting lives in the shell-exec / gateway
            // path), so the strongest grant it can honor is session scope.
            // `AllowSession` is the explicit decision for that; `AllowAlways`
            // degrades to it here rather than being silently dropped.
            Some(ApprovalDecisionType::AllowOnce) => ApprovalOutcome::Approved,
            Some(ApprovalDecisionType::AllowSession) => ApprovalOutcome::ApprovedForSession,
            Some(ApprovalDecisionType::AllowAlways) => ApprovalOutcome::ApprovedForSession,
            Some(ApprovalDecisionType::Deny) => ApprovalOutcome::Denied,
            None => {
                self.send_timeout_notice(channel_id, conversation_id).await;
                ApprovalOutcome::Timeout
            }
        };
        // A `/deny <reason>` text reply rides the record; relay it so the
        // dispatch gate can put the human's own words in front of the model.
        crate::sandbox::exec_approval::ApprovalResponse {
            outcome,
            deny_reason: resolved.deny_reason,
        }
    }

    /// 该通道当前是否可投递审批（已在 `ChannelRegistry` 注册）。
    ///
    /// Panel 回合携带 `gui:chat` —— 一个从不注册为外部通道的伪 channel id，
    /// 所以 `deliver_routed` 恒返回 `None`。调用方据此在通道不可达时改走
    /// operator 事件总线，而不是直接拒绝。
    pub async fn can_deliver(&self, channel_id: &ChannelId) -> bool {
        #[cfg(test)]
        if self.test_outcome_override.is_some() {
            return true;
        }
        self.registry.get(channel_id).await.is_some()
    }

    /// 按结构化 `channel_id` 投递审批提示。返回 `Some(true)` 已投递、
    /// `Some(false)` 投递失败、`None` 无通道。
    ///
    /// 无原生审批能力的通道走纯文本回退：发一条带 `/approve` / `/deny`
    /// 指引的消息，由 inbound router 的文本拦截按 session FIFO 解析。
    /// 此前这些通道会被直接 `Denied`，确认门工具在非 Telegram 通道上
    /// 等于全部静默拒绝。
    ///
    /// 授权语义：回退路径没有 capability 路径的 `authorize_actor` 逐人
    /// 校验，信任边界与既有 `/approve` 文本命令一致 —— 依赖通道入站层
    /// 的 allowlist / pairing 把关（能与 bot 对话的人即被信任）。提示只
    /// 投递到发起会话本身，不广播。
    async fn deliver_routed(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        action: &crate::sandbox::exec_approval::ApprovalAction,
        approval_id: &str,
    ) -> Option<bool> {
        let tool_name = action.tool_name.as_str();
        let reason = action.reason.as_str();
        let channel = self.registry.get(channel_id).await?;
        let capability = {
            let ch = channel.read().await;
            ch.approval_capability()
        };

        let Some(capability) = capability else {
            // The action summary is the point of the prompt: `/approve` on a
            // bare tool name approves whatever the model happened to pass.
            let text = format!(
                "⚠️ 工具 `{tool_name}` 需要你的授权。\n```\n{}\n```\n{reason}\n\n\
                 回复 /approve 批准本次、/approve session 本会话内不再询问、\
                 /deny 拒绝（可附原因：/deny 原因…，会转告给 agent）。",
                action.summary
            );
            let ch = channel.read().await;
            return match ch
                .send(OutboundMessage::text(conversation_id.as_str(), text))
                .await
            {
                Ok(_) => Some(true),
                Err(e) => {
                    tracing::warn!(error = %e, "plain-text approval fallback send failed");
                    Some(false)
                }
            };
        };

        // Confirm-gated tools can honor at most a session-scoped grant (no
        // on-disk allowlist on this path), so the rendered decision set stops
        // at the session tier — offering "always" here would be a lie.
        let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
            crate::exec::approval::types::CommandApprovalRequest {
                command: action.summary.clone(),
                cwd: action.cwd.clone(),
                reason: Some(reason.to_string()),
                allowed_decisions: vec![
                    ApprovalDecisionType::AllowOnce,
                    ApprovalDecisionType::AllowSession,
                    ApprovalDecisionType::Deny,
                ],
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
                tracing::warn!(
                    "deliver_approval timed out after {}s",
                    DELIVERY_TIMEOUT_SECS
                );
                Some(false)
            }
        }
    }

    /// 审批超时后向通道发一条友好提示（best-effort）。
    async fn send_timeout_notice(&self, channel_id: &ChannelId, conversation_id: &ConversationId) {
        if let Some(channel) = self.registry.get(channel_id).await {
            let ch = channel.read().await;
            let msg = OutboundMessage::text(
                conversation_id.as_str(),
                "\u{23f1} 审批请求已超时，操作被拒绝。",
            );
            let _ = ch.send(msg).await;
        }
    }

    /// Test helper: a bridge that always returns `ApprovalOutcome::Approved`.
    #[cfg(test)]
    pub fn for_test_always_approved() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
            test_outcome_override: Some(ApprovalOutcome::Approved),
        }
    }

    /// Test helper: a bridge that always returns `ApprovalOutcome::Denied`.
    #[cfg(test)]
    pub fn for_test_always_denied() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
            test_outcome_override: Some(ApprovalOutcome::Denied),
        }
    }
}

impl From<ApprovalAction> for ApprovalDecisionType {
    fn from(action: ApprovalAction) -> Self {
        match action {
            ApprovalAction::Approve => Self::AllowOnce,
            ApprovalAction::Deny => Self::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_action_to_decision_type() {
        assert!(matches!(
            ApprovalDecisionType::AllowOnce,
            ApprovalDecisionType::AllowOnce
        ));
        assert!(matches!(
            ApprovalAction::Approve.into(),
            ApprovalDecisionType::AllowOnce
        ));
        assert!(matches!(
            ApprovalAction::Deny.into(),
            ApprovalDecisionType::Deny
        ));
    }
}

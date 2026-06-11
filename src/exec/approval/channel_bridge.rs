use std::time::Duration;

use crate::sandbox::exec_approval::gate::ApprovalOutcome;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::exec::decision::ApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::channel::{ChannelId, ConversationId, OutboundMessage, UserId};
use crate::gateway::channel_approval::{
    ApprovalAction, AuthorizationResult, PendingApproval as ChannelPendingApproval,
};
use crate::gateway::channel_registry::ChannelRegistry;
use chrono::{DateTime, Utc};

const DELIVERY_TIMEOUT_SECS: u64 = 30;

fn sanitize_channel_error(e: &crate::gateway::channel::ChannelError) -> String {
    use crate::gateway::channel::ChannelError;
    match e {
        ChannelError::SendFailed(_) | ChannelError::ReceiveFailed(_) => {
            "Failed to communicate with channel".to_string()
        }
        ChannelError::NotConnected(_) => "Channel not connected".to_string(),
        ChannelError::AuthFailed(_) => "Authentication failed".to_string(),
        ChannelError::RateLimited { .. } => "Rate limited".to_string(),
        ChannelError::MessageTooLarge { .. } => "Message too large".to_string(),
        ChannelError::UnsupportedFeature(_) => "Feature not supported".to_string(),
        ChannelError::ConfigError(_) => "Configuration error".to_string(),
        ChannelError::Internal(_) => "Internal error".to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct ChannelApprovalResult {
    pub approval_id: String,
    pub channel_id: String,
    pub conversation_id: ConversationId,
    pub delivered: bool,
    pub reason: Option<String>,
}

pub struct ChannelApprovalBridge {
    registry: Arc<ChannelRegistry>,
    pending_approvals: Arc<RwLock<Vec<PendingApprovalState>>>,
    /// Test-only override that short-circuits `request_for_tool` with a fixed
    /// outcome, bypassing the real channel lookup and pending-approval wait.
    /// Field exists only under `cfg(test)` so production has zero surface.
    #[cfg(test)]
    test_outcome_override: Option<ApprovalOutcome>,
}

#[derive(Debug, Clone)]
pub struct PendingApprovalState {
    approval_id: String,
    channel_id: String,
    conversation_id: ConversationId,
    expires_at: DateTime<Utc>,
}

impl ChannelApprovalBridge {
    pub fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self {
            registry,
            pending_approvals: Arc::new(RwLock::new(Vec::new())),
            #[cfg(test)]
            test_outcome_override: None,
        }
    }

    pub async fn request_approval(
        &self,
        request: &ApprovalRequest,
    ) -> Option<ChannelApprovalResult> {
        let (channel_id, conversation_id) = self.parse_session_key(&request.session_key)?;

        let channel = {
            match self.registry.get(&ChannelId::new(&channel_id)).await {
                Some(c) => c,
                None => {
                    tracing::warn!(channel_id = %channel_id, "Channel not found");
                    return None;
                }
            }
        };

        let capability = {
            let channel = channel.read().await;
            match channel.approval_capability() {
                Some(c) => c,
                None => {
                    tracing::debug!(channel_id = %channel_id, "Channel has no approval capability");
                    return None;
                }
            }
        };

        let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
            crate::exec::approval::types::CommandApprovalRequest {
                command: request.command.clone(),
                cwd: request.cwd.clone(),
                reason: request.reason.clone(),
                allowed_decisions: crate::exec::allowed_decisions::assess_command_decisions(
                    &request.command,
                ),
            },
        );

        let pending = match timeout(
            Duration::from_secs(DELIVERY_TIMEOUT_SECS),
            capability.deliver_approval(&conversation_id, &approval_req, &request.id),
        )
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                tracing::warn!(
                    channel_id = %channel_id,
                    approval_id = %request.id,
                    error = %e,
                    "Failed to deliver approval via channel capability"
                );
                return Some(ChannelApprovalResult {
                    approval_id: request.id.clone(),
                    channel_id,
                    conversation_id,
                    delivered: false,
                    reason: Some(sanitize_channel_error(&e)),
                });
            }
            Err(_) => {
                tracing::warn!(
                    channel_id = %channel_id,
                    approval_id = %request.id,
                    "Approval delivery timed out after {}s",
                    DELIVERY_TIMEOUT_SECS
                );
                return Some(ChannelApprovalResult {
                    approval_id: request.id.clone(),
                    channel_id,
                    conversation_id,
                    delivered: false,
                    reason: Some("Delivery timed out".to_string()),
                });
            }
        };

        {
            let mut approvals = self.pending_approvals.write().await;
            let now = Utc::now();
            approvals.retain(|p| p.expires_at > now);
            approvals.push(PendingApprovalState {
                approval_id: pending.approval_id.clone(),
                channel_id: channel_id.clone(),
                conversation_id: conversation_id.clone(),
                expires_at: pending.expires_at,
            });
        }

        Some(ChannelApprovalResult {
            approval_id: pending.approval_id,
            channel_id,
            conversation_id,
            delivered: true,
            reason: None,
        })
    }

    pub async fn authorize_and_deliver(
        &self,
        request: &ApprovalRequest,
        actor_user_id: &UserId,
    ) -> Option<ChannelApprovalResult> {
        let (channel_id, conversation_id) = self.parse_session_key(&request.session_key)?;

        let channel = {
            match self.registry.get(&ChannelId::new(&channel_id)).await {
                Some(c) => c,
                None => {
                    tracing::warn!(channel_id = %channel_id, "Channel not found");
                    return None;
                }
            }
        };

        let capability = {
            let channel = channel.read().await;
            match channel.approval_capability() {
                Some(c) => c,
                None => {
                    tracing::debug!(channel_id = %channel_id, "Channel has no approval capability");
                    return None;
                }
            }
        };

        let auth_result = capability
            .authorize_actor(actor_user_id, ApprovalAction::Approve)
            .await;

        match auth_result {
            AuthorizationResult::Authorized => {
                let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
                    crate::exec::approval::types::CommandApprovalRequest {
                        command: request.command.clone(),
                        cwd: request.cwd.clone(),
                        reason: request.reason.clone(),
                        allowed_decisions: crate::exec::allowed_decisions::assess_command_decisions(
                            &request.command,
                        ),
                    },
                );

                let pending_approval_id = format!("tg-{}", uuid::Uuid::new_v4());

                let rendered = capability
                    .render_approval_for_actor(
                        &conversation_id,
                        &approval_req,
                        actor_user_id,
                        &pending_approval_id,
                    )
                    .await;

                match rendered {
                    Ok(rendered) => {
                        let channel_ref = channel.read().await;
                        match channel_ref.send(rendered.message).await {
                            Ok(_result) => {
                                let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);

                                {
                                    let mut approvals = self.pending_approvals.write().await;
                                    let now = Utc::now();
                                    approvals.retain(|p| p.expires_at > now);
                                    approvals.push(PendingApprovalState {
                                        approval_id: pending_approval_id.clone(),
                                        channel_id: channel_id.clone(),
                                        conversation_id: conversation_id.clone(),
                                        expires_at,
                                    });
                                }

                                Some(ChannelApprovalResult {
                                    approval_id: pending_approval_id,
                                    channel_id,
                                    conversation_id,
                                    delivered: true,
                                    reason: None,
                                })
                            }
                            Err(e) => {
                                tracing::warn!(
                                    channel_id = %channel_id,
                                    approval_id = %request.id,
                                    error = %e,
                                    "Failed to send approval message"
                                );
                                Some(ChannelApprovalResult {
                                    approval_id: request.id.clone(),
                                    channel_id,
                                    conversation_id,
                                    delivered: false,
                                    reason: Some(sanitize_channel_error(&e)),
                                })
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            channel_id = %channel_id,
                            approval_id = %request.id,
                            error = %e,
                            "Failed to render approval"
                        );
                        Some(ChannelApprovalResult {
                            approval_id: request.id.clone(),
                            channel_id,
                            conversation_id,
                            delivered: false,
                            reason: Some(sanitize_channel_error(&e)),
                        })
                    }
                }
            }
            AuthorizationResult::Denied => {
                tracing::info!(
                    channel_id = %channel_id,
                    user_id = %actor_user_id.0,
                    "Actor denied authorization for approval"
                );
                Some(ChannelApprovalResult {
                    approval_id: request.id.clone(),
                    channel_id,
                    conversation_id,
                    delivered: false,
                    reason: Some("Not authorized".to_string()),
                })
            }
            AuthorizationResult::NotAuthenticated => {
                tracing::warn!(
                    channel_id = %channel_id,
                    user_id = %actor_user_id.0,
                    "Actor not authenticated for approval"
                );
                Some(ChannelApprovalResult {
                    approval_id: request.id.clone(),
                    channel_id,
                    conversation_id,
                    delivered: false,
                    reason: Some("Not authenticated".to_string()),
                })
            }
        }
    }

    pub async fn resolve_approval(&self, approval_id: &str, action: ApprovalAction) -> Option<()> {
        let pending = {
            let mut approvals = self.pending_approvals.write().await;
            match approvals
                .iter()
                .find(|p| p.approval_id == approval_id)
                .cloned()
            {
                Some(p) => {
                    if p.expires_at < Utc::now() {
                        tracing::warn!(approval_id = %approval_id, "Approval expired - removing");
                        approvals.retain(|p| p.approval_id != approval_id);
                        return None;
                    }
                    p
                }
                None => return None,
            }
        };

        let channel = {
            match self
                .registry
                .get(&ChannelId::new(&pending.channel_id))
                .await
            {
                Some(c) => c,
                None => return None,
            }
        };

        let capability = {
            let channel = channel.read().await;
            match channel.approval_capability() {
                Some(c) => c,
                None => return None,
            }
        };

        let channel_pending = ChannelPendingApproval::new(
            approval_id,
            crate::exec::approval::types::ApprovalRequest::Command(
                crate::exec::approval::types::CommandApprovalRequest {
                    command: String::new(),
                    cwd: None,
                    reason: None,
                    allowed_decisions: crate::exec::allowed_decisions::full_set(),
                },
            ),
            &pending.channel_id,
            pending.conversation_id.clone(),
            pending.expires_at,
        );

        let result = capability.resolve_approval(&channel_pending, action).await;

        if result.is_ok() {
            let mut approvals = self.pending_approvals.write().await;
            approvals.retain(|p| p.approval_id != approval_id);
        }

        result.ok()
    }

    fn parse_session_key(&self, session_key: &str) -> Option<(String, ConversationId)> {
        let parts: Vec<&str> = session_key.split(':').collect();

        for (i, part) in parts.iter().enumerate() {
            match *part {
                "telegram" | "discord" | "imessage" | "slack" | "webchat" => {
                    if i + 2 < parts.len() {
                        let channel = part.to_string();
                        let target = parts[i + 2].to_string();
                        return Some((channel, ConversationId::new(target)));
                    }
                }
                _ => continue,
            }
        }

        None
    }

    pub async fn get_pending_approvals(&self) -> Vec<PendingApprovalState> {
        let mut approvals = self.pending_approvals.write().await;
        let now = Utc::now();
        approvals.retain(|p| p.expires_at > now);
        approvals.clone()
    }

    pub async fn remove_pending(&self, approval_id: &str) {
        self.pending_approvals
            .write()
            .await
            .retain(|p| p.approval_id != approval_id);
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
    /// `SessionKey`），不再 parse 有损的字符串 session_key。
    ///
    /// `session_key` 是发起会话的结构化 key 字符串（router 侧
    /// `ctx.session_key.to_string()` 的同一形态）：record 必须携带它，
    /// `/approve`/`/deny` 文本回复才能经 `resolve_for_session` 命中这条
    /// 审批（FIFO）。传空则回退为 `channel:conversation` 合成 key（仅按钮
    /// 回调可达）。
    #[allow(clippy::too_many_arguments)]
    pub async fn request_for_tool(
        &self,
        approval_manager: &crate::exec::manager::ExecApprovalManager,
        tool_name: &str,
        reason: &str,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        session_key: &str,
        timeout_ms: u64,
    ) -> ApprovalOutcome {
        #[cfg(test)]
        if let Some(outcome) = self.test_outcome_override {
            return outcome;
        }

        let record_session_key = if session_key.is_empty() {
            format!("{}:{}", channel_id.as_str(), conversation_id.as_str())
        } else {
            session_key.to_string()
        };

        let request = ApprovalRequest {
            id: uuid::Uuid::new_v4().to_string(),
            command: tool_name.to_string(),
            cwd: None,
            analysis: crate::exec::analysis::CommandAnalysis {
                ok: true,
                reason: None,
                segments: vec![],
                chains: None,
            },
            agent_id: String::new(),
            session_key: record_session_key,
            reason: Some(reason.to_string()),
        };

        let record = approval_manager.create(&request, timeout_ms);

        // Register the pending entry BEFORE delivering the prompt so a fast
        // resolver (instant button tap / "/approve" reply) cannot race ahead
        // of registration (resolve-before-register → spurious timeout); see
        // `ExecApprovalManager::register_pending`.
        let (record_id, rx, wait_timeout) = approval_manager.register_pending(record);

        match self
            .deliver_routed(channel_id, conversation_id, tool_name, reason, &record_id)
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
                return ApprovalOutcome::Denied;
            }
            None => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record_id,
                    "No channel capability for approval delivery — denying"
                );
                approval_manager.resolve(&record_id, ApprovalDecisionType::Deny, None);
                return ApprovalOutcome::Denied;
            }
        }

        match approval_manager
            .await_registered(record_id, rx, wait_timeout)
            .await
        {
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
        }
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
        tool_name: &str,
        reason: &str,
        approval_id: &str,
    ) -> Option<bool> {
        let channel = self.registry.get(channel_id).await?;
        let capability = {
            let ch = channel.read().await;
            ch.approval_capability()
        };

        let Some(capability) = capability else {
            let text = format!(
                "⚠️ 工具 `{tool_name}` 需要你的授权。\n{reason}\n\n\
                 回复 /approve 批准本次、/approve session 本会话内不再询问、\
                 /deny 拒绝。"
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
                command: tool_name.to_string(),
                cwd: None,
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
            pending_approvals: Arc::new(RwLock::new(Vec::new())),
            test_outcome_override: Some(ApprovalOutcome::Approved),
        }
    }

    /// Test helper: a bridge that always returns `ApprovalOutcome::Denied`.
    #[cfg(test)]
    pub fn for_test_always_denied() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
            pending_approvals: Arc::new(RwLock::new(Vec::new())),
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

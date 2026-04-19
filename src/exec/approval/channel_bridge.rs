use std::time::Duration;

use crate::agent_loop::exec_approval::gate::ApprovalOutcome;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::exec::decision::ApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::channel::{ChannelId, ConversationId, UserId};
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
            },
        );

        let pending = match timeout(
            Duration::from_secs(DELIVERY_TIMEOUT_SECS),
            capability.deliver_approval(&conversation_id, &approval_req),
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

        self.pending_approvals
            .write()
            .await
            .push(PendingApprovalState {
                approval_id: pending.approval_id.clone(),
                channel_id: channel_id.clone(),
                conversation_id: conversation_id.clone(),
                expires_at: pending.expires_at,
            });

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

                                self.pending_approvals
                                    .write()
                                    .await
                                    .push(PendingApprovalState {
                                        approval_id: pending_approval_id.clone(),
                                        channel_id: channel_id.clone(),
                                        conversation_id: conversation_id.clone(),
                                        expires_at,
                                    });

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
        self.pending_approvals.read().await.clone()
    }

    pub async fn remove_pending(&self, approval_id: &str) {
        self.pending_approvals
            .write()
            .await
            .retain(|p| p.approval_id != approval_id);
    }

    /// Request approval for a tool invocation and await the real user decision.
    ///
    /// Two-stage flow mirroring `ExecSecurityGate::request_approval`:
    /// 1. `approval_manager.create(...)` builds the record with a unique id.
    /// 2. `self.request_approval(...)` delivers the prompt to the channel (inline
    ///    keyboard on Telegram/Discord, etc). Delivery success is NOT an approval
    ///    — it only means the UI was shown.
    /// 3. `approval_manager.wait_for_decision(record)` blocks on a oneshot that
    ///    the channel's button-click callback handler resolves via
    ///    `approval_manager.resolve(id, decision, ...)`.
    ///
    /// `session_key` must be a valid channel session key in the format
    /// `{platform}:{...}:{conversation_id}:{...}` so the bridge can route the
    /// prompt to the correct channel. An empty or unparseable session_key yields
    /// `Denied` (delivery is skipped and no approver can click the button).
    pub async fn request_for_tool(
        &self,
        approval_manager: &crate::exec::manager::ExecApprovalManager,
        tool_name: &str,
        reason: &str,
        session_key: &str,
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
            session_key: session_key.to_string(),
        };

        let record = approval_manager.create(&request, timeout_ms);

        // Deliver the prompt via the channel capability. If delivery fails
        // (no channel capability, registry miss, send error), we cannot await
        // a user decision — deny. This matches the existing
        // `ExecSecurityGate::request_approval` contract.
        match self.request_approval(&request).await {
            Some(result) if result.delivered => {
                tracing::info!(
                    tool = %tool_name,
                    id = %record.id,
                    channel_id = %result.channel_id,
                    "Approval delivered via channel — waiting for user decision"
                );
            }
            Some(result) => {
                tracing::warn!(
                    tool = %tool_name,
                    id = %record.id,
                    reason = ?result.reason,
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

        // Block on the oneshot until the button-click callback resolves it,
        // or the record's timeout fires.
        match approval_manager.wait_for_decision(record).await {
            Some(crate::exec::socket::ApprovalDecisionType::AllowOnce)
            | Some(crate::exec::socket::ApprovalDecisionType::AllowAlways) => {
                ApprovalOutcome::Approved
            }
            Some(crate::exec::socket::ApprovalDecisionType::Deny) => ApprovalOutcome::Denied,
            None => ApprovalOutcome::Timeout,
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
            ApprovalAction::Approve => ApprovalDecisionType::AllowOnce,
            ApprovalAction::Deny => ApprovalDecisionType::Deny,
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

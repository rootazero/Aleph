use std::sync::Arc;

use tokio::sync::RwLock;

use crate::gateway::channel::{ChannelId, ConversationId, UserId};
use crate::gateway::channel_approval::{
    ApprovalAction, AuthorizationResult, ChannelApprovalCapability, PendingApproval as ChannelPendingApproval,
};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::exec::decision::ApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use chrono::{DateTime, Utc};

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
}

#[derive(Debug, Clone)]
struct PendingApprovalState {
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

        let pending = match capability.deliver_approval(&conversation_id, &approval_req).await {
            Ok(p) => p,
            Err(e) => {
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
                    reason: Some(e.to_string()),
                });
            }
        };

        self.pending_approvals.write().await.push(PendingApprovalState {
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

        let auth_result = capability.authorize_actor(actor_user_id, ApprovalAction::Approve).await;

        match auth_result {
            AuthorizationResult::Authorized => {
                let approval_req = crate::exec::approval::types::ApprovalRequest::Command(
                    crate::exec::approval::types::CommandApprovalRequest {
                        command: request.command.clone(),
                        cwd: request.cwd.clone(),
                    },
                );

                match capability.deliver_approval(&conversation_id, &approval_req).await {
                    Ok(pending) => {
                        self.pending_approvals.write().await.push(PendingApprovalState {
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
                    Err(e) => {
                        tracing::warn!(
                            channel_id = %channel_id,
                            approval_id = %request.id,
                            error = %e,
                            "Failed to deliver approval"
                        );
                        Some(ChannelApprovalResult {
                            approval_id: request.id.clone(),
                            channel_id,
                            conversation_id,
                            delivered: false,
                            reason: Some(e.to_string()),
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

    pub async fn resolve_approval(
        &self,
        approval_id: &str,
        action: ApprovalAction,
    ) -> Option<()> {
        let pending = {
            let approvals = self.pending_approvals.read().await;
            match approvals.iter().find(|p| p.approval_id == approval_id).cloned() {
                Some(p) => p,
                None => return None,
            }
        };

        if pending.expires_at < Utc::now() {
            tracing::warn!(approval_id = %approval_id, "Approval expired");
            return None;
        }

        let channel = {
            match self.registry.get(&ChannelId::new(&pending.channel_id)).await {
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

        capability.resolve_approval(&channel_pending, action).await.ok()
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
        self.pending_approvals.write().await.retain(|p| p.approval_id != approval_id);
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

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};

use crate::exec::approval::types::ApprovalRequest;
use crate::gateway::channel::{
    Channel, ChannelResult, ConversationId, InlineKeyboard, OutboundMessage, UserId,
};
use crate::gateway::channel_approval::{
    ApprovalAction, AuthorizationResult, ChannelApprovalCapability, PendingApproval,
    RenderedApproval,
};
use crate::gateway::interfaces::telegram::{AccessController, TelegramChannel};

pub struct TelegramChannelApprovalCapability {
    channel: Arc<TelegramChannel>,
    access: Arc<AccessController>,
}

impl TelegramChannelApprovalCapability {
    pub fn new(channel: Arc<TelegramChannel>, access: Arc<AccessController>) -> Self {
        Self { channel, access }
    }

    fn render_approval_text(&self, request: &ApprovalRequest) -> String {
        match request {
            ApprovalRequest::Command(cmd) => {
                format!(
                    "⚠️ *Command Approval Required*\n\n\
                     A command requires your approval:\n\n\
                     `{}`\n\n\
                     CWD: {}",
                    cmd.command,
                    cmd.cwd.as_deref().unwrap_or("default")
                )
            }
            ApprovalRequest::Capability(cap) => {
                let stage_emoji = match cap.trust_stage {
                    crate::exec::approval::types::TrustStage::Draft => "📝",
                    crate::exec::approval::types::TrustStage::Trial => "🔧",
                    crate::exec::approval::types::TrustStage::Verified => "✅",
                };
                format!(
                    "{} *Tool Approval Required*\n\n\
                     *Tool:* `{}`\n\n\
                     _{}_\n\n\
                     _Stage: {:?}_",
                    stage_emoji, cap.tool_name, cap.tool_description, cap.trust_stage
                )
            }
        }
    }

    fn approval_callback_data(action: ApprovalAction, approval_id: &str) -> String {
        match action {
            ApprovalAction::Approve => format!("approve:{}", approval_id),
            ApprovalAction::Deny => format!("deny:{}", approval_id),
        }
    }
}

#[async_trait]
impl ChannelApprovalCapability for TelegramChannelApprovalCapability {
    async fn deliver_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
    ) -> ChannelResult<PendingApproval> {
        let approval_id = format!("tg-{}", uuid::Uuid::new_v4());
        let expires_at = Utc::now() + Duration::minutes(5);

        let rendered = self.render_approval(conversation_id, request).await?;

        let result = self.channel.send(rendered.message).await?;

        Ok(PendingApproval::new(
            approval_id,
            request.clone(),
            self.channel.id().as_str(),
            conversation_id.clone(),
            expires_at,
        )
        .with_message_id(result.message_id.as_str()))
    }

    async fn authorize_actor(
        &self,
        actor_user_id: &UserId,
        _action: ApprovalAction,
    ) -> AuthorizationResult {
        let user_id = actor_user_id.0.parse::<i64>().ok();
        match user_id {
            Some(uid) => {
                let is_paired = {
                    let runtime_users = self.access.runtime_users();
                    let users = runtime_users.read().await;
                    users.contains(&uid)
                };
                if is_paired || self.access.config().allowed_users.contains(&uid) {
                    AuthorizationResult::Authorized
                } else {
                    AuthorizationResult::Denied
                }
            }
            None => AuthorizationResult::NotAuthenticated,
        }
    }

    async fn render_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
    ) -> ChannelResult<RenderedApproval> {
        let approval_id = format!("tg-{}", uuid::Uuid::new_v4());
        let text = self.render_approval_text(request);

        let keyboard = InlineKeyboard::new()
            .button(
                "✅ Approve",
                Self::approval_callback_data(ApprovalAction::Approve, &approval_id),
            )
            .button(
                "❌ Deny",
                Self::approval_callback_data(ApprovalAction::Deny, &approval_id),
            );

        let mut message = OutboundMessage::text(conversation_id.as_str(), text);
        message.inline_keyboard = Some(keyboard);

        Ok(RenderedApproval {
            message,
            callback_prefix: "approval".to_string(),
        })
    }

    async fn resolve_approval(
        &self,
        pending: &PendingApproval,
        _action: ApprovalAction,
    ) -> ChannelResult<()> {
        if let Some(msg_id) = &pending.message_id {
            let text = match _action {
                ApprovalAction::Approve => "✅ Approval granted.",
                ApprovalAction::Deny => "❌ Approval denied.",
            };
            let update = OutboundMessage::text(pending.conversation_id.as_str(), text);
            let _ = self.channel.send(update).await;
            let _ = msg_id;
        }
        Ok(())
    }

    async fn render_approval_for_actor(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        actor_user_id: &UserId,
        approval_id: &str,
    ) -> ChannelResult<RenderedApproval> {
        let auth_result = self
            .authorize_actor(actor_user_id, ApprovalAction::Approve)
            .await;

        let text = self.render_approval_text(request);

        match auth_result {
            AuthorizationResult::Authorized => {
                let keyboard = InlineKeyboard::new()
                    .button(
                        "✅ Approve",
                        Self::approval_callback_data(ApprovalAction::Approve, approval_id),
                    )
                    .button(
                        "❌ Deny",
                        Self::approval_callback_data(ApprovalAction::Deny, approval_id),
                    );

                let mut message = OutboundMessage::text(conversation_id.as_str(), text);
                message.inline_keyboard = Some(keyboard);

                Ok(RenderedApproval {
                    message,
                    callback_prefix: "approval".to_string(),
                })
            }
            AuthorizationResult::Denied | AuthorizationResult::NotAuthenticated => {
                let message = OutboundMessage::text(conversation_id.as_str(), text);

                Ok(RenderedApproval {
                    message,
                    callback_prefix: "approval".to_string(),
                })
            }
        }
    }
}

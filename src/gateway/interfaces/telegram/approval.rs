use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};

use crate::exec::approval::types::ApprovalRequest;
use crate::exec::socket::ApprovalDecisionType;
use crate::exec::ApprovalBridge;
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
    #[must_use]
    pub const fn new(channel: Arc<TelegramChannel>, access: Arc<AccessController>) -> Self {
        Self { channel, access }
    }

    fn render_approval_text(request: &ApprovalRequest) -> String {
        match request {
            ApprovalRequest::Command(cmd) => {
                let mut text = format!(
                    "⚠️ *Command Approval Required*\n\n\
                     A command requires your approval:\n\n\
                     `{}`\n\n\
                     CWD: {}",
                    cmd.command,
                    cmd.cwd.as_deref().unwrap_or("default")
                );
                if let Some(reason) = cmd.reason.as_deref().filter(|r| !r.is_empty()) {
                    text.push_str(&format!("\n\n*Why:* {reason}"));
                }
                text
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

    /// Build the inline keyboard for `request` via the shared risk-aware
    /// builder ([`ApprovalBridge::build_approval_keyboard`]) instead of a
    /// hand-rolled Approve/Deny pair, so the rendered decision tiers follow
    /// the request's [`allowed_decisions`] set (the session tier appears only
    /// when permitted). Every grant is at most session-scoped, so capability
    /// (tool) approvals declare that set directly.
    fn approval_keyboard(request: &ApprovalRequest, approval_id: &str) -> InlineKeyboard {
        let allowed: Vec<ApprovalDecisionType> = match request {
            ApprovalRequest::Command(cmd) => cmd.allowed_decisions.clone(),
            ApprovalRequest::Capability(_) => vec![
                ApprovalDecisionType::AllowOnce,
                ApprovalDecisionType::AllowSession,
                ApprovalDecisionType::Deny,
            ],
        };
        ApprovalBridge::build_approval_keyboard(approval_id, &allowed)
    }
}

#[async_trait]
impl ChannelApprovalCapability for TelegramChannelApprovalCapability {
    async fn deliver_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        approval_id: &str,
    ) -> ChannelResult<PendingApproval> {
        let expires_at = Utc::now() + Duration::minutes(5);
        let text = Self::render_approval_text(request);

        let keyboard = Self::approval_keyboard(request, approval_id);

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

    async fn authorize_actor(
        &self,
        actor_user_id: &UserId,
        _action: ApprovalAction,
    ) -> AuthorizationResult {
        let user_id = actor_user_id.0.parse::<i64>().ok();
        match user_id {
            // Approval authority stays with the statically-configured allowlist.
            // Runtime pairing is owned by the router's `pairing_store`; a paired
            // chat user is not implicitly granted tool-approval authority.
            Some(uid) => {
                if self.access.config().allowed_users.contains(&uid) {
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
        let text = Self::render_approval_text(request);

        let keyboard = Self::approval_keyboard(request, &approval_id);

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

        let text = Self::render_approval_text(request);

        match auth_result {
            AuthorizationResult::Authorized => {
                let keyboard = Self::approval_keyboard(request, approval_id);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::approval::types::CommandApprovalRequest;

    fn command_request(command: &str, allowed: Vec<ApprovalDecisionType>) -> ApprovalRequest {
        ApprovalRequest::Command(CommandApprovalRequest {
            command: command.to_string(),
            cwd: None,
            reason: Some("escalation: first execution".to_string()),
            allowed_decisions: allowed,
        })
    }

    #[test]
    fn keyboard_renders_allowed_tiers_and_parses_back() {
        let request = command_request(
            "ls -la",
            vec![
                ApprovalDecisionType::AllowOnce,
                ApprovalDecisionType::AllowSession,
                ApprovalDecisionType::AllowAlways,
                ApprovalDecisionType::Deny,
            ],
        );
        let kb = TelegramChannelApprovalCapability::approval_keyboard(&request, "rec-123");
        let json = serde_json::to_string(&kb).unwrap();
        for decision in ["once", "session", "deny"] {
            let data = format!("approve:rec-123:{decision}");
            assert!(json.contains(&data), "missing button {data}: {json}");
            // 与 RPC 侧 ApprovalBridge::parse_callback 必须双向一致
            let (id, _) = ApprovalBridge::parse_callback(&data).expect("parses");
            assert_eq!(id, "rec-123");
        }
        // No tier promises permanence — nothing persists an allow-always grant.
        assert!(
            !json.contains("approve:rec-123:always"),
            "keyboard must not offer allow-always: {json}"
        );
    }

    #[test]
    fn keyboard_for_danger_set_omits_always() {
        let request = command_request(
            "rm -rf ./build",
            vec![
                ApprovalDecisionType::AllowOnce,
                ApprovalDecisionType::AllowSession,
                ApprovalDecisionType::Deny,
            ],
        );
        let kb = TelegramChannelApprovalCapability::approval_keyboard(&request, "danger-1");
        let json = serde_json::to_string(&kb).unwrap();
        assert!(!json.contains("approve:danger-1:always"));
        assert!(json.contains("approve:danger-1:session"));
    }

    #[test]
    fn capability_request_keyboard_stops_at_session_tier() {
        use crate::exec::approval::parameter_binding::RequiredCapabilities;
        use crate::exec::approval::types::{CapabilityApprovalRequest, TrustStage};

        let request = ApprovalRequest::Capability(Box::new(CapabilityApprovalRequest {
            tool_name: "vault_store".to_string(),
            tool_description: "store a secret".to_string(),
            required_capabilities: RequiredCapabilities {
                base_preset: "default".to_string(),
                description: String::new(),
                overrides: Default::default(),
                parameter_bindings: Default::default(),
            },
            resolved_capabilities: Default::default(),
            trust_stage: TrustStage::Draft,
        }));
        let kb = TelegramChannelApprovalCapability::approval_keyboard(&request, "cap-1");
        let json = serde_json::to_string(&kb).unwrap();
        assert!(json.contains("approve:cap-1:once"));
        assert!(json.contains("approve:cap-1:session"));
        assert!(!json.contains("approve:cap-1:always"));
    }

    #[test]
    fn reason_is_rendered_in_command_approval_text() {
        let request = command_request("git push", vec![ApprovalDecisionType::AllowOnce]);
        let text = TelegramChannelApprovalCapability::render_approval_text(&request);
        assert!(text.contains("git push"));
        assert!(
            text.contains("escalation: first execution"),
            "reason must reach the user-facing message: {text}"
        );

        // No reason → no dangling "Why:" label.
        let bare = ApprovalRequest::Command(CommandApprovalRequest {
            command: "git push".to_string(),
            cwd: None,
            reason: None,
            allowed_decisions: vec![ApprovalDecisionType::AllowOnce],
        });
        let text = TelegramChannelApprovalCapability::render_approval_text(&bare);
        assert!(!text.contains("Why:"));
    }
}

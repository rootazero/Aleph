//! Channel Approval Capability
//!
//! Provides per-channel approval delivery and authorization for the approval system.
//! Each channel can optionally expose a capability to deliver approval UI to users
//! and authorize approval actions based on channel-specific rules (e.g., paired users).

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::channel::{ChannelResult, ConversationId, OutboundMessage, UserId};
use crate::exec::approval::types::ApprovalRequest;

/// Action that can be taken on an approval request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAction {
    /// Approve the request
    Approve,
    /// Deny the request
    Deny,
}

/// Result of an authorization check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationResult {
    /// Actor is authorized to take this action
    Authorized,
    /// Actor is explicitly denied
    Denied,
    /// Actor is not authenticated
    NotAuthenticated,
}

/// Rendered approval UI ready for delivery
#[derive(Debug, Clone)]
pub struct RenderedApproval {
    /// The outbound message with approval UI
    pub message: OutboundMessage,
    /// Callback data prefix for button clicks
    pub callback_prefix: String,
}

/// Pending approval tracked for callback resolution
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Unique approval ID for tracking
    pub approval_id: String,
    /// The original approval request
    pub request: ApprovalRequest,
    /// Channel ID where approval was delivered
    pub channel_id: String,
    /// Conversation ID where approval was sent
    pub conversation_id: ConversationId,
    /// Message ID of the approval message (for edits/cancellation)
    pub message_id: Option<String>,
    /// When the approval expires
    pub expires_at: DateTime<Utc>,
}

impl PendingApproval {
    /// Create a new pending approval
    pub fn new(
        approval_id: impl Into<String>,
        request: ApprovalRequest,
        channel_id: impl Into<String>,
        conversation_id: ConversationId,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            approval_id: approval_id.into(),
            request,
            channel_id: channel_id.into(),
            conversation_id,
            message_id: None,
            expires_at,
        }
    }

    /// Set the message ID after delivery
    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    /// Check if the approval has expired
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at < Utc::now()
    }
}

/// Capability trait for channel-specific approval delivery and authorization.
///
/// Implement this trait to enable a channel to deliver approval requests to users
/// via its native UI (e.g., inline keyboards, interactive components) and authorize
/// actors based on channel-specific rules.
///
/// # Example
///
/// ```ignore
/// struct TelegramApprovalCapability {
///     channel: Arc<TelegramChannel>,
/// }
///
/// #[async_trait]
/// impl ChannelApprovalCapability for TelegramApprovalCapability {
///     async fn deliver_approval(&self, conv: &ConversationId, req: &ApprovalRequest, approval_id: &str) -> ChannelResult<PendingApproval> {
///         // Send message with inline keyboard
///         let keyboard = InlineKeyboard::new()
///             .button("✅ Approve", format!("approve:{}", req.id))
///             .button("❌ Deny", format!("deny:{}", req.id));
///
///         let message = OutboundMessage::text(chat_id, render_approval_text(req))
///             .with_inline_keyboard(keyboard);
///
///         let result = self.channel.send(message).await?;
///         // ...
///     }
///
///     async fn authorize_actor(&self, actor: &Actor, action: ApprovalAction) -> AuthorizationResult {
///         // Check if actor matches paired user
///     }
/// }
/// ```
#[async_trait]
pub trait ChannelApprovalCapability: Send + Sync {
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

    /// Authorize an actor for an approval action.
    ///
    /// This is called before showing approval controls to verify the actor
    /// has permission to approve/deny. Return `AuthorizationResult::Authorized`
    /// to proceed, or `AuthorizationResult::Denied` to hide controls.
    async fn authorize_actor(
        &self,
        actor_user_id: &UserId,
        action: ApprovalAction,
    ) -> AuthorizationResult;

    /// Render the approval UI for a request.
    ///
    /// Returns the text and inline keyboard markup for displaying the approval.
    /// This is used both for initial delivery and for re-rendering after state changes.
    async fn render_approval(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
    ) -> ChannelResult<RenderedApproval>;

    /// Render the approval UI for a specific actor, checking authorization first.
    ///
    /// If the actor is not authorized, returns `RenderedApproval` with no interactive buttons.
    /// If authorized, returns the full approval UI with approve/deny buttons.
    ///
    /// The `approval_id` is used in callback data to identify which approval is being resolved.
    async fn render_approval_for_actor(
        &self,
        conversation_id: &ConversationId,
        request: &ApprovalRequest,
        actor_user_id: &UserId,
        _approval_id: &str,
    ) -> ChannelResult<RenderedApproval> {
        // Default implementation: render normally and check authorization
        let rendered = self.render_approval(conversation_id, request).await?;
        let auth_result = self
            .authorize_actor(actor_user_id, ApprovalAction::Approve)
            .await;

        match auth_result {
            AuthorizationResult::Authorized => Ok(rendered),
            AuthorizationResult::Denied | AuthorizationResult::NotAuthenticated => {
                // Return rendered message without inline keyboard buttons
                let mut message = rendered.message.clone();
                message.inline_keyboard = None;
                Ok(RenderedApproval {
                    message,
                    callback_prefix: rendered.callback_prefix,
                })
            }
        }
    }

    /// Resolve a pending approval with the given action.
    ///
    /// Called when the user clicks an approve/deny button or when the
    /// approval times out. The channel should clean up any message state.
    async fn resolve_approval(
        &self,
        pending: &PendingApproval,
        action: ApprovalAction,
    ) -> ChannelResult<()>;
}

/// Extension trait for `Channel` to optionally expose approval capability.
pub trait ChannelApprovalExt {
    /// Get the approval capability for this channel, if supported.
    ///
    /// Returns `None` if the channel doesn't support approval delivery.
    /// Default implementation returns `None`.
    fn approval_capability(&self) -> Option<Arc<dyn ChannelApprovalCapability>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::approval::types::CommandApprovalRequest;
    use crate::exec::socket::ApprovalDecisionType;

    fn make_test_request() -> ApprovalRequest {
        ApprovalRequest::Command(CommandApprovalRequest {
            command: "test_tool".to_string(),
            cwd: None,
            reason: None,
            allowed_decisions: vec![ApprovalDecisionType::AllowOnce],
        })
    }

    #[test]
    fn test_pending_approval_creation() {
        let request = make_test_request();
        let pending = PendingApproval::new(
            "approval-123",
            request,
            "telegram:bot",
            ConversationId::new("123456"),
            Utc::now() + chrono::Duration::minutes(5),
        );

        assert_eq!(pending.approval_id, "approval-123");
        assert_eq!(pending.channel_id, "telegram:bot");
        assert!(!pending.is_expired());
    }

    #[test]
    fn test_pending_approval_expired() {
        let request = make_test_request();
        let pending = PendingApproval::new(
            "approval-123",
            request,
            "telegram:bot",
            ConversationId::new("123456"),
            Utc::now() - chrono::Duration::minutes(1),
        );

        assert!(pending.is_expired());
    }

    #[test]
    fn test_pending_approval_with_message_id() {
        let request = make_test_request();
        let pending = PendingApproval::new(
            "approval-123",
            request,
            "telegram:bot",
            ConversationId::new("123456"),
            Utc::now() + chrono::Duration::minutes(5),
        )
        .with_message_id("msg-789");

        assert_eq!(pending.message_id, Some("msg-789".to_string()));
    }

    #[test]
    fn test_authorization_result_variants() {
        assert!(matches!(
            AuthorizationResult::Authorized,
            AuthorizationResult::Authorized
        ));
        assert!(matches!(
            AuthorizationResult::Denied,
            AuthorizationResult::Denied
        ));
        assert!(matches!(
            AuthorizationResult::NotAuthenticated,
            AuthorizationResult::NotAuthenticated
        ));
    }

    #[test]
    fn test_approval_action_variants() {
        assert!(matches!(ApprovalAction::Approve, ApprovalAction::Approve));
        assert!(matches!(ApprovalAction::Deny, ApprovalAction::Deny));
    }
}

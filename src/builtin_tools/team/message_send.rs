//! MessageSendTool — send a message to team members with to/cc routing.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

fn default_msg_type() -> MessageType {
    MessageType::Message
}

/// Arguments for sending a team message.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MessageSendArgs {
    /// The team this message belongs to
    pub team_id: String,
    /// Agent IDs who must process the message (To recipients)
    pub to: Vec<String>,
    /// Agent IDs to inform (Cc recipients)
    #[serde(default)]
    pub cc: Vec<String>,
    /// Message type for filtering and TTL
    #[serde(default = "default_msg_type")]
    pub msg_type: MessageType,
    /// Short subject line
    pub subject: String,
    /// Full message content (Markdown supported)
    pub content: String,
    /// Message ID to reply to (creates/continues a thread)
    pub reply_to: Option<String>,
    /// Attachment references (optional)
    #[serde(default)]
    pub attachments: Vec<String>,
    /// If true, send to all team members (except sender). The `to` field is ignored.
    #[serde(default)]
    pub broadcast: bool,
}

/// Output from message_send.
#[derive(Debug, Clone, Serialize)]
pub struct MessageSendOutput {
    pub message_id: String,
    pub thread_id: Option<String>,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that sends a message to team members via the MessageRouter.
#[derive(Clone)]
pub struct MessageSendTool {
    router: Arc<MessageRouter>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl MessageSendTool {
    pub fn new(
        router: Arc<MessageRouter>,
        team_store: Arc<dyn TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            router,
            team_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for MessageSendTool {
    const NAME: &'static str = "message_send";
    const DESCRIPTION: &'static str = "Send a message to team members with to/cc routing";

    type Args = MessageSendArgs;
    type Output = MessageSendOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "message_send(team_id='team-1', to=['agent-b'], subject='Status update', content='All tasks complete')".to_string(),
            "message_send(team_id='team-1', to=['agent-b'], cc=['agent-c'], msg_type='review_request', subject='Please review', content='...')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Resolve broadcast: query all team members and exclude the sender.
        let to = if args.broadcast {
            let members = self
                .team_store
                .get_members(&args.team_id)
                .await
                .map_err(|e| AlephError::other(format!("Failed to resolve broadcast recipients: {e}")))?;
            members
                .into_iter()
                .map(|m| m.agent_id)
                .filter(|id| id != &self.current_agent_id)
                .collect::<Vec<_>>()
        } else {
            args.to
        };

        debug!(
            team_id = %args.team_id,
            to = ?to,
            cc = ?args.cc,
            msg_type = %args.msg_type.as_str(),
            subject = %args.subject,
            from = %self.current_agent_id,
            broadcast = args.broadcast,
            "message_send: sending message"
        );

        if to.is_empty() && args.cc.is_empty() {
            return Err(AlephError::tool(
                "message_send: at least one recipient (to or cc) is required",
            ));
        }

        let msg = self
            .router
            .send(SendRequest {
                team_id: args.team_id,
                from_agent: self.current_agent_id.clone(),
                to,
                cc: args.cc,
                msg_type: args.msg_type,
                subject: args.subject.clone(),
                content: args.content,
                reply_to: args.reply_to,
                attachments: args.attachments,
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to send message: {e}")))?;

        Ok(MessageSendOutput {
            message_id: msg.id,
            thread_id: msg.thread_id,
            message: format!("Message '{}' sent successfully", args.subject),
        })
    }
}

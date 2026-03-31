//! InboxReadTool — read inbox messages or a full thread.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::messages::inbox::Inbox;
use crate::teams::messages::types::{MessageType, TeamMessage};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

fn default_inbox() -> String {
    "inbox".to_string()
}

fn default_true() -> bool {
    true
}

/// Arguments for reading inbox messages or a thread.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct InboxReadArgs {
    /// The team to read messages from
    pub team_id: String,
    /// Read mode: "inbox" (default) to read your messages, "thread" to read a conversation thread
    #[serde(default = "default_inbox")]
    pub mode: String,
    /// Thread ID — required when mode="thread"
    pub thread_id: Option<String>,
    /// Filter by message type (optional)
    pub msg_type: Option<MessageType>,
    /// Mark returned messages as read (default: true)
    #[serde(default = "default_true")]
    pub mark_read: bool,
}

/// Output from inbox_read.
#[derive(Debug, Clone, Serialize)]
pub struct InboxReadOutput {
    pub messages: Vec<TeamMessage>,
    pub count: usize,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that reads inbox messages or a full thread for the current agent.
#[derive(Clone)]
pub struct InboxReadTool {
    inbox: Arc<Inbox>,
    current_agent_id: String,
}

impl InboxReadTool {
    pub fn new(inbox: Arc<Inbox>, current_agent_id: String) -> Self {
        Self {
            inbox,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for InboxReadTool {
    const NAME: &'static str = "inbox_read";
    const DESCRIPTION: &'static str =
        "Read inbox messages or a full thread. Use mode='inbox' (default) to read \
         your messages, mode='thread' with thread_id to read a conversation thread.";

    type Args = InboxReadArgs;
    type Output = InboxReadOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "inbox_read(team_id='team-1')".to_string(),
            "inbox_read(team_id='team-1', mode='thread', thread_id='thr-abc123')".to_string(),
            "inbox_read(team_id='team-1', msg_type='review_request')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            team_id = %args.team_id,
            mode = %args.mode,
            agent_id = %self.current_agent_id,
            "inbox_read: reading messages"
        );

        let messages = match args.mode.as_str() {
            "thread" => {
                let thread_id = args.thread_id.as_deref().ok_or_else(|| {
                    AlephError::tool(
                        "inbox_read: thread_id is required when mode='thread'",
                    )
                })?;
                self.inbox.read_thread(thread_id).await?
            }
            _ => {
                self.inbox
                    .read(
                        &self.current_agent_id,
                        &args.team_id,
                        args.msg_type.as_ref(),
                        args.mark_read,
                    )
                    .await?
            }
        };

        let count = messages.len();

        Ok(InboxReadOutput { messages, count })
    }
}

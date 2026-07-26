//! `channel_message` — LLM-invoked tool for proactively sending messages,
//! reactions, edits, and typing indicators to a specific channel conversation.
//!
//! Unlike `media_send` (which only feeds the *current* reply path) and
//! `gateway_route` (which only *queries* the routing engine), this tool lets
//! the agent actively reach out on any connected channel — fulfilling R5
//! ("AI comes to you", proactive multi-channel push) and R8 ("everything is a
//! tool"). It is a thin wiring layer over the existing `ChannelRegistry`
//! outbound API (`send`/`react`/`edit`/`send_typing`); no new transport logic
//! is introduced, and the registry's extension-hook observability is inherited
//! for free.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::gateway::channel::{ChannelId, ConversationId, MessageId, OutboundMessage};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args
// =============================================================================

/// Action to perform on a channel conversation.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMessageAction {
    /// Send a new text message (optionally as a reply).
    Send,
    /// React to an existing message with an emoji.
    React,
    /// Edit a previously sent message's text.
    Edit,
    /// Send a typing indicator (no text).
    Typing,
}

/// Arguments for the `channel_message` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ChannelMessageArgs {
    /// What to do: send a message, react, edit, or show a typing indicator.
    pub action: ChannelMessageAction,

    /// Target channel ID (e.g. "telegram", "slack", "discord").
    pub channel_id: String,

    /// Target conversation ID (chat / channel / DM identifier on that platform).
    pub conversation_id: String,

    /// Message text — required for `send` and `edit`.
    #[serde(default)]
    pub text: Option<String>,

    /// Target message ID — required for `react` and `edit`.
    #[serde(default)]
    pub message_id: Option<String>,

    /// Emoji to apply — required for `react`.
    #[serde(default)]
    pub reaction: Option<String>,

    /// Message ID to reply to — optional for `send`.
    #[serde(default)]
    pub reply_to: Option<String>,
}

// =============================================================================
// Output
// =============================================================================

/// Output from the `channel_message` tool.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelMessageOutput {
    /// Human-readable status message.
    pub message: String,
    /// Whether the action was delivered to the channel.
    pub delivered: bool,
    /// ID of the sent message (only for the `send` action).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool for proactively interacting with channel conversations via natural
/// language. Wires the LLM to the existing `ChannelRegistry` outbound API.
#[derive(Clone)]
pub struct ChannelMessageTool {
    channel_registry: Arc<ChannelRegistry>,
}

impl ChannelMessageTool {
    pub const fn new(channel_registry: Arc<ChannelRegistry>) -> Self {
        Self { channel_registry }
    }

    /// Append "what to do next" to a delivery failure (A2: compress the error
    /// AND the way out into the tool result, so the model can self-heal instead
    /// of retrying the same wrong id).
    ///
    /// Substring routing over machine-generated text — `ChannelError` displays
    /// and platform error codes, never natural language — which is the same
    /// exemption `builtin_tools/desktop/recovery.rs` documents (P8 does not
    /// apply). Note `channel_not_found` (a platform code, underscored) and
    /// `Channel not found:` (this registry's own wording, spaced) are different
    /// failures with different fixes, and the two substrings do not collide.
    fn with_hint(message: String) -> String {
        let lower = message.to_lowercase();
        let hint = if lower.contains("channel not found:") {
            // The *transport* is not connected — no conversation id can help.
            Some(
                "That channel_id is not a connected channel. It is the transport \
                 (\"slack\", \"telegram\", …), not the conversation.",
            )
        } else if lower.contains("channel_not_found") || lower.contains("not_in_channel") {
            // The transport is fine; the conversation handle is wrong or
            // unreachable. This is exactly what the directory answers.
            Some(
                "Resolve the destination first: channel_directory(channel_id, query=\"<name>\") \
                 returns the real conversation_id, and an entry with is_member=false means this \
                 account must be invited there before it can post. Do not retry unchanged.",
            )
        } else {
            None
        };
        match hint {
            Some(h) => format!("{message} — {h}"),
            None => message,
        }
    }

    /// Require a field, returning a clear tool error naming the missing field.
    fn require<'a>(value: &'a Option<String>, field: &str, action: &str) -> Result<&'a str> {
        value
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                AlephError::tool(format!("`{field}` is required for the `{action}` action"))
            })
    }
}

#[async_trait]
impl AlephTool for ChannelMessageTool {
    const NAME: &'static str = "channel_message";
    const DESCRIPTION: &'static str =
        "Proactively send a message, react with an emoji, edit a message, or show a typing \
         indicator on a specific channel conversation (Telegram, Slack, Discord, etc.). \
         Use this to reach the user on their own channel — for example to deliver a scheduled \
         notification, follow up on a task, or coordinate across channels — rather than only \
         replying in the current turn. Requires the target channel_id and conversation_id. \
         conversation_id is an opaque platform handle, not a name: when the user names a \
         destination (\"#eng-releases\", \"DM Alice\"), call channel_directory first to resolve \
         it instead of guessing.";

    type Args = ChannelMessageArgs;
    type Output = ChannelMessageOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"channel_message(action="send", channel_id="telegram", conversation_id="12345", text="Your build finished ✅")"#.to_string(),
            r#"channel_message(action="react", channel_id="slack", conversation_id="C0XX", message_id="170...", reaction="👍")"#.to_string(),
            r#"channel_message(action="edit", channel_id="discord", conversation_id="98765", message_id="111", text="Updated answer")"#.to_string(),
            r#"channel_message(action="typing", channel_id="telegram", conversation_id="12345")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let channel_id = ChannelId::new(args.channel_id.clone());
        let conversation_id = ConversationId::new(args.conversation_id.clone());

        match args.action {
            ChannelMessageAction::Send => {
                let text = Self::require(&args.text, "text", "send")?;
                let mut outbound = OutboundMessage::text(args.conversation_id.clone(), text);
                if let Some(reply_to) = args.reply_to.as_deref().filter(|s| !s.trim().is_empty()) {
                    outbound = outbound.with_reply_to(reply_to);
                }

                let result = self
                    .channel_registry
                    .send(&channel_id, outbound)
                    .await
                    .map_err(|e| {
                        AlephError::tool(Self::with_hint(format!(
                            "Failed to send message on channel '{channel_id}': {e}"
                        )))
                    })?;

                info!(
                    tool = "channel_message",
                    channel = %channel_id,
                    conversation = conversation_id.as_str(),
                    "proactive message sent"
                );
                Ok(ChannelMessageOutput {
                    message: format!(
                        "Message sent to conversation '{}' on channel '{}'.",
                        conversation_id.as_str(),
                        channel_id
                    ),
                    delivered: true,
                    message_id: Some(result.message_id.as_str().to_string()),
                })
            }

            ChannelMessageAction::React => {
                let message_id = Self::require(&args.message_id, "message_id", "react")?;
                let reaction = Self::require(&args.reaction, "reaction", "react")?;
                self.channel_registry
                    .react(
                        &channel_id,
                        &conversation_id,
                        &MessageId::new(message_id),
                        reaction,
                    )
                    .await
                    .map_err(|e| {
                        AlephError::tool(Self::with_hint(format!(
                            "Failed to react on channel '{channel_id}': {e}"
                        )))
                    })?;

                info!(tool = "channel_message", channel = %channel_id, "reaction applied");
                Ok(ChannelMessageOutput {
                    message: format!("Reaction '{reaction}' applied on channel '{channel_id}'."),
                    delivered: true,
                    message_id: None,
                })
            }

            ChannelMessageAction::Edit => {
                let message_id = Self::require(&args.message_id, "message_id", "edit")?;
                let text = Self::require(&args.text, "text", "edit")?;
                self.channel_registry
                    .edit(
                        &channel_id,
                        &conversation_id,
                        &MessageId::new(message_id),
                        text,
                    )
                    .await
                    .map_err(|e| {
                        AlephError::tool(Self::with_hint(format!(
                            "Failed to edit message on channel '{channel_id}': {e}"
                        )))
                    })?;

                info!(tool = "channel_message", channel = %channel_id, "message edited");
                Ok(ChannelMessageOutput {
                    message: format!("Message edited on channel '{channel_id}'."),
                    delivered: true,
                    message_id: None,
                })
            }

            ChannelMessageAction::Typing => {
                self.channel_registry
                    .send_typing(&channel_id, &conversation_id)
                    .await
                    .map_err(|e| {
                        AlephError::tool(Self::with_hint(format!(
                            "Failed to send typing indicator on channel '{channel_id}': {e}"
                        )))
                    })?;

                Ok(ChannelMessageOutput {
                    message: format!(
                        "Typing indicator sent to conversation '{}' on channel '{}'.",
                        conversation_id.as_str(),
                        channel_id
                    ),
                    delivered: true,
                    message_id: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two "not found" failures must route to different advice: one means
    /// the transport is not connected, the other means the conversation handle
    /// is wrong. Collapsing them would send the model to fix the wrong thing.
    #[test]
    fn hint_distinguishes_a_missing_transport_from_a_missing_conversation() {
        let transport = ChannelMessageTool::with_hint(
            "Failed to send message on channel 'slak': Channel not found: slak".to_string(),
        );
        assert!(transport.contains("not a connected channel"), "{transport}");
        assert!(!transport.contains("channel_directory"), "{transport}");

        let conversation = ChannelMessageTool::with_hint(
            "Failed to send message on channel 'slack': Send failed: Slack chat.postMessage \
             failed: channel_not_found"
                .to_string(),
        );
        assert!(conversation.contains("channel_directory"), "{conversation}");

        let not_member = ChannelMessageTool::with_hint(
            "Send failed: Slack chat.postMessage failed: not_in_channel".to_string(),
        );
        assert!(not_member.contains("is_member=false"), "{not_member}");
    }

    /// An error we have no advice for must come back verbatim — a hint that
    /// fires on everything trains the model to ignore hints.
    #[test]
    fn hint_leaves_unrecognized_failures_untouched() {
        let msg = "Rate limited: retry after 30 seconds".to_string();
        assert_eq!(ChannelMessageTool::with_hint(msg.clone()), msg);
    }

    #[test]
    fn require_rejects_missing_and_blank() {
        assert!(ChannelMessageTool::require(&None, "text", "send").is_err());
        assert!(ChannelMessageTool::require(&Some("   ".to_string()), "text", "send").is_err());
        assert_eq!(
            ChannelMessageTool::require(&Some("hi".to_string()), "text", "send").unwrap(),
            "hi"
        );
    }

    #[test]
    fn args_deserialize_send() {
        let json = serde_json::json!({
            "action": "send",
            "channel_id": "telegram",
            "conversation_id": "12345",
            "text": "hello"
        });
        let args: ChannelMessageArgs = serde_json::from_value(json).unwrap();
        assert!(matches!(args.action, ChannelMessageAction::Send));
        assert_eq!(args.channel_id, "telegram");
        assert_eq!(args.text.as_deref(), Some("hello"));
        assert!(args.reply_to.is_none());
    }

    #[test]
    fn args_deserialize_react() {
        let json = serde_json::json!({
            "action": "react",
            "channel_id": "slack",
            "conversation_id": "C0XX",
            "message_id": "170",
            "reaction": "👍"
        });
        let args: ChannelMessageArgs = serde_json::from_value(json).unwrap();
        assert!(matches!(args.action, ChannelMessageAction::React));
        assert_eq!(args.reaction.as_deref(), Some("👍"));
    }

    #[tokio::test]
    async fn send_requires_text() {
        let tool = ChannelMessageTool::new(Arc::new(ChannelRegistry::new()));
        let args = ChannelMessageArgs {
            action: ChannelMessageAction::Send,
            channel_id: "telegram".to_string(),
            conversation_id: "123".to_string(),
            text: None,
            message_id: None,
            reaction: None,
            reply_to: None,
        };
        let err = tool.call(args).await.unwrap_err();
        assert!(err.to_string().contains("text"));
    }

    #[tokio::test]
    async fn react_requires_message_id_and_reaction() {
        let tool = ChannelMessageTool::new(Arc::new(ChannelRegistry::new()));
        let args = ChannelMessageArgs {
            action: ChannelMessageAction::React,
            channel_id: "slack".to_string(),
            conversation_id: "C0".to_string(),
            text: None,
            message_id: None,
            reaction: Some("👍".to_string()),
            reply_to: None,
        };
        let err = tool.call(args).await.unwrap_err();
        assert!(err.to_string().contains("message_id"));
    }
}

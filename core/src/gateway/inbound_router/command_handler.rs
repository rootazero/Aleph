//! Slash command handling for inbound messages

use tracing::{error, info, warn};

use crate::gateway::channel::{InboundMessage, OutboundMessage};
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::router::SessionKey;
use crate::intent::{DirectToolSource, IntentResult};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::types::{RoutingError, check_link_access};
use super::InboundMessageRouter;

/// Strip @botname suffix from Telegram-style slash commands.
///
/// In Telegram groups, commands are sent as `/command@botname args`.
/// This function normalizes to `/command args`.
pub(super) fn strip_bot_mention(input: &str) -> String {
    if !input.starts_with('/') {
        return input.to_string();
    }
    let (cmd_part, rest) = match input.split_once(char::is_whitespace) {
        Some((cmd, args)) => (cmd, Some(args)),
        None => (input, None),
    };
    let clean_cmd = match cmd_part.split_once('@') {
        Some((cmd, _)) => cmd,
        None => cmd_part,
    };
    match rest {
        Some(args) => format!("{} {}", clean_cmd, args),
        None => clean_cmd.to_string(),
    }
}

/// Truncate a string to max_chars at a char boundary
pub(super) fn truncate_for_topic(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Serialize an `IntentResult` to a JSON string for RunRequest metadata.
pub(super) fn serialize_intent_result(result: &IntentResult) -> Option<String> {
    match result {
        IntentResult::DirectTool {
            tool_id,
            args,
            source,
        } => {
            let source_str = match source {
                DirectToolSource::SlashCommand => "slash_command",
                DirectToolSource::Skill => "skill",
                DirectToolSource::Mcp => "mcp",
                DirectToolSource::Custom => "custom",
            };
            serde_json::to_string(&serde_json::json!({
                "type": "direct_tool",
                "tool_id": tool_id,
                "args": args,
                "source": source_str,
            }))
            .ok()
        }
        IntentResult::Execute { .. }
        | IntentResult::Converse { .. }
        | IntentResult::Abort => None,
    }
}

impl InboundMessageRouter {
    /// Handle /switch command: change active agent for this channel+peer
    pub(super) async fn handle_switch_command(
        &self,
        agent_name: &str,
        msg: &InboundMessage,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let sender_id = msg.sender_id.as_str();

        if let Some(ref manager) = self.workspace_manager {
            let agent_exists = if let Some(ref registry) = self.agent_registry {
                registry.get(agent_name).await.is_some()
            } else {
                false
            };

            let reply_text = if agent_exists {
                // Check link access control before switching
                let access_denied = if let Some(ref registry) = self.agent_registry {
                    if let Some(allowed_links) = registry.get_allowed_links(agent_name).await {
                        check_link_access(&allowed_links, channel_id, agent_name).err()
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(e) = access_denied {
                    format!("\u{26d4} {}", e)
                } else {
                    // Close current session before switching
                    let topic = self.generate_session_topic(&ctx.session_key).await;
                    if let Some(ref sm) = self.session_manager {
                        if let Err(e) = sm.close_session(&ctx.session_key, topic).await {
                            warn!("[Router] Failed to close session on switch: {}", e);
                        }
                    }

                    match manager.set_active_agent(channel_id, sender_id, agent_name) {
                        Ok(()) => {
                            info!("[Router] Switched agent for {}:{} -> {}", channel_id, sender_id, agent_name);
                            format!("✅ Switched to agent: {}", agent_name)
                        }
                        Err(e) => {
                            error!("[Router] Failed to switch agent: {}", e);
                            format!("❌ Failed to switch agent: {}", e)
                        }
                    }
                }
            } else {
                let available = if let Some(ref registry) = self.agent_registry {
                    registry.list().await.join(", ")
                } else {
                    "unknown".to_string()
                };
                format!("❌ Agent '{}' not found. Available: {}", agent_name, available)
            };

            let reply = OutboundMessage::text(msg.conversation_id.as_str(), reply_text);
            if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                error!("[Router] Failed to send /switch reply: {}", e);
            }
        }
        Ok(())
    }

    /// Handle /new command: close current session with topic, create new epoch
    pub(super) async fn handle_new_session(
        &self,
        msg: &InboundMessage,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        let old_key = &ctx.session_key;

        // Generate topic from recent history via LLM
        let topic = self.generate_session_topic(old_key).await;

        // Close old session in database
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm.close_session(old_key, topic.clone()).await {
                warn!("[Router] Failed to close session: {}", e);
            }
        }

        // Create new session with next epoch
        let new_key = old_key.to_new().with_next_epoch();
        if let Some(ref sm) = self.session_manager {
            let legacy_new = SessionKey::from_new(&new_key);
            if let Err(e) = sm.get_or_create(&legacy_new).await {
                warn!("[Router] Failed to create new session: {}", e);
            }
        }

        // Send confirmation reply
        let topic_suffix = topic.map(|t| format!(" ({})", t)).unwrap_or_default();
        let reply_text = format!("新对话已开始{}", topic_suffix);
        let reply = OutboundMessage::text(msg.conversation_id.as_str(), &reply_text);
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send /new reply: {}", e);
        }

        info!("[Router] New session created: {}", new_key.to_key_string());
        Ok(())
    }

    /// Generate a topic summary for the current session using LLM
    pub(super) async fn generate_session_topic(
        &self,
        session_key: &SessionKey,
    ) -> Option<String> {
        let sm = self.session_manager.as_ref()?;
        let llm = self.llm_provider.as_ref()?;

        // Get recent history
        let history = sm.get_history(session_key, Some(20)).await.ok()?;
        if history.len() < 2 {
            return None;
        }

        // Build conversation excerpt for LLM
        let conversation: String = history.iter()
            .map(|m| {
                let content = truncate_for_topic(&m.content, 100);
                format!("{}: {}", m.role, content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "用一句简短的中文概括以下对话的主题（10字以内，不要标点符号）：\n\n{}",
            conversation
        );

        let __msgs = [UnifiedMessage::user(&prompt)];
        match llm.process(RequestPayload::new(&__msgs)).await {
            Ok(resp) => {
                let topic = resp.text_content().trim().to_string();
                if topic.is_empty() { None } else { Some(topic) }
            }
            Err(e) => {
                warn!("[Router] Failed to generate session topic: {}", e);
                None
            }
        }
    }

    /// Convert a ParsedCommand to IntentResult
    pub(super) fn parsed_command_to_intent_result(&self, cmd: crate::command::ParsedCommand) -> IntentResult {
        use crate::command::CommandContext;

        let args = cmd.arguments.clone();

        let (tool_id, source) = match cmd.context {
            CommandContext::Builtin { tool_name } => (tool_name, DirectToolSource::SlashCommand),
            CommandContext::Skill { skill_id, .. } => (skill_id, DirectToolSource::Skill),
            CommandContext::Mcp {
                server_name,
                tool_name,
                ..
            } => {
                let id = tool_name.unwrap_or(server_name);
                (id, DirectToolSource::Mcp)
            }
            CommandContext::Custom { .. } => (cmd.command_name.clone(), DirectToolSource::Custom),
            CommandContext::None => (cmd.command_name.clone(), DirectToolSource::SlashCommand),
        };

        IntentResult::DirectTool {
            tool_id,
            args,
            source,
        }
    }
}

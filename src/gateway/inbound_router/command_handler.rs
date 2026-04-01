//! Slash command handling for inbound messages

use tracing::{error, info, warn};

use crate::gateway::channel::{InboundMessage, OutboundMessage};
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::router::SessionKey;
use crate::intent::{DirectToolSource, IntentResult};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::types::RoutingError;
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
        } => serde_json::to_string(&serde_json::json!({
            "type": "direct_tool",
            "tool_id": tool_id,
            "args": args,
            "source": source.as_str(),
        }))
        .ok(),
        IntentResult::Execute { .. } | IntentResult::Converse { .. } | IntentResult::Abort => None,
    }
}

impl InboundMessageRouter {
    /// Resolve user locale from app config.
    pub(super) async fn resolve_locale(&self) -> crate::gateway::i18n::Locale {
        if let Some(ref cfg) = self.app_config {
            let cfg = cfg.read().await;
            crate::gateway::i18n::Locale::from_config(cfg.general.language.as_deref())
        } else {
            crate::gateway::i18n::Locale::Zh
        }
    }

    /// Handle /btw command: ephemeral sidebar conversation that doesn't affect context.
    ///
    /// Creates a SessionKey::Ephemeral so the question/answer is not persisted
    /// to the current session history.
    pub(super) async fn handle_btw(
        &self,
        msg: &InboundMessage,
        agent_id: &str,
        btw_text: &str,
    ) -> Result<(), RoutingError> {
        use crate::gateway::inbound_context::{InboundContext, ReplyRoute};

        let reply_route = ReplyRoute::new(msg.channel_id.clone(), msg.conversation_id.clone())
            .with_inbound_message_id(msg.id.clone());

        // Use ephemeral session — no persistence, no context pollution
        let session_key = SessionKey::ephemeral(agent_id);

        // Create a modified message with just the btw text
        let mut btw_msg = msg.clone();
        btw_msg.text = btw_text.to_string();

        let ctx = InboundContext::new(btw_msg, reply_route, session_key);

        // Execute with btw metadata marker
        let metadata = serde_json::json!({"btw": true}).to_string();
        self.execute_for_context_with_metadata(&ctx, metadata)
            .await?;

        info!(
            "[Router] /btw handled as ephemeral session for agent '{}'",
            agent_id
        );
        Ok(())
    }

    /// Handle /new command: close current session with topic, create new epoch
    pub(super) async fn handle_new_session(
        &self,
        msg: &InboundMessage,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        let old_key = &ctx.session_key;

        // Resolve the actual current epoch from DB (old_key may already have it
        // from routing, but query to be safe in case the epoch was advanced externally)
        let current_epoch = if let Some(ref sm) = self.session_manager {
            let base = old_key.base_key_pattern();
            sm.get_current_epoch(&base).await.unwrap_or(old_key.epoch())
        } else {
            old_key.epoch()
        };
        let old_key_resolved = old_key.with_epoch(current_epoch);

        // Generate topic from recent history via LLM
        let topic = self.generate_session_topic(&old_key_resolved).await;

        // Close old session in database
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm.close_session(&old_key_resolved, topic.clone()).await {
                warn!("[Router] Failed to close session: {}", e);
            }
        }

        // Create new session with next epoch
        let new_key = old_key.with_epoch(current_epoch + 1);
        if let Some(ref sm) = self.session_manager {
            if let Err(e) = sm.get_or_create(&new_key).await {
                warn!("[Router] Failed to create new session: {}", e);
            }
        }

        // Send confirmation reply
        let locale = self.resolve_locale().await;
        let topic_suffix = topic.map(|t| format!(" ({})", t)).unwrap_or_default();
        let reply_text = crate::gateway::i18n::t(
            crate::gateway::i18n::Msg::NewSessionStarted {
                topic_suffix: &topic_suffix,
            },
            locale,
        );
        let reply = OutboundMessage::text(msg.conversation_id.as_str(), &reply_text);
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send /new reply: {}", e);
        }

        info!("[Router] New session created: {}", new_key.to_key_string());
        Ok(())
    }

    /// Generate a topic summary for the current session using LLM
    pub(super) async fn generate_session_topic(&self, session_key: &SessionKey) -> Option<String> {
        let sm = self.session_manager.as_ref()?;
        let llm = self.llm_provider.as_ref()?;

        // Get recent history
        let history = sm.get_history(session_key, Some(20)).await.ok()?;
        if history.len() < 2 {
            return None;
        }

        // Build conversation excerpt for LLM
        let conversation: String = history
            .iter()
            .map(|m| {
                let content = truncate_for_topic(&m.content, 100);
                format!("{}: {}", m.role, content)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let locale = self.resolve_locale().await;
        let prompt = crate::gateway::i18n::t(
            crate::gateway::i18n::Msg::TopicGenerationPrompt {
                conversation: &conversation,
            },
            locale,
        );

        let __msgs = [UnifiedMessage::user(&prompt)];
        match llm.process(RequestPayload::new(&__msgs)).await {
            Ok(resp) => {
                let topic = resp.text_content().trim().to_string();
                if topic.is_empty() {
                    None
                } else {
                    Some(topic)
                }
            }
            Err(e) => {
                warn!("[Router] Failed to generate session topic: {}", e);
                None
            }
        }
    }

    /// Convert a ParsedCommand to IntentResult
    pub(super) fn parsed_command_to_intent_result(
        &self,
        cmd: crate::command::ParsedCommand,
    ) -> IntentResult {
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

//! Slash command handling for inbound messages

use tracing::{error, info, warn};

use crate::gateway::channel::{InboundMessage, OutboundMessage};
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::router::SessionKey;
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

/// Serialize a `ParsedCommand` directly into the slash-command mode JSON used
/// by `ExecutionEngine` fast path. Preserves source-specific fields per
/// command kind:
/// * `Skill` — `skill_id`, `instructions`, `allowed_tools`, `display_name`
/// * `Custom` — `system_prompt`, `pattern`, `tool_id`
/// * `Mcp`   — `server_name`, `tool_name`
///
/// Without this, the fast-path's `match mode_type { "skill" => ... }` branch
/// was dead code: every slash command was misclassified as `direct_tool` and
/// skill instructions were silently dropped.
pub fn serialize_parsed_command(parsed: &crate::command::ParsedCommand) -> Option<String> {
    use crate::command::CommandContext;

    let args = parsed.arguments.as_deref().unwrap_or("");
    let value = match &parsed.context {
        CommandContext::Skill {
            skill_id,
            instructions,
            display_name,
            allowed_tools,
        } => serde_json::json!({
            "type": "skill",
            "skill_id": skill_id,
            "display_name": display_name,
            "instructions": instructions,
            "allowed_tools": allowed_tools,
            "args": args,
            "source": "skill",
        }),
        CommandContext::Custom {
            system_prompt,
            provider,
            pattern,
        } => serde_json::json!({
            "type": "custom",
            "tool_id": parsed.command_name,
            "system_prompt": system_prompt,
            "provider": provider,
            "pattern": pattern,
            "args": args,
            "source": "custom",
        }),
        CommandContext::Mcp {
            server_name,
            tool_name,
        } => serde_json::json!({
            "type": "mcp",
            "server_name": server_name,
            "tool_name": tool_name,
            "args": args,
            "source": "mcp",
        }),
        CommandContext::Builtin { tool_name } => serde_json::json!({
            "type": "direct_tool",
            "tool_id": tool_name,
            "args": args,
            "source": "slash_command",
        }),
        CommandContext::None => serde_json::json!({
            "type": "direct_tool",
            "tool_id": parsed.command_name,
            "args": args,
            "source": "slash_command",
        }),
    };
    serde_json::to_string(&value).ok()
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

    /// Send a "Unknown command — did you mean?" reply with up to 3 close
    /// matches from the unified tool registry. Returns `true` when a reply
    /// was sent (caller should NOT fall through to the agent), `false` when
    /// no plausible candidates exist (caller may fall through normally).
    pub(super) async fn try_send_unknown_command_help(
        &self,
        msg: &InboundMessage,
        unknown_cmd: &str,
    ) -> bool {
        use crate::builtin_tools::meta_tools::levenshtein_distance;

        let parser = match self.command_parser.as_ref() {
            Some(p) => p,
            None => return false,
        };
        let needle = unknown_cmd.to_lowercase();
        if needle.is_empty() {
            return false;
        }

        let all_tools = parser.tool_registry().list_all().await;
        // Score every tool name + alias against the needle. Threshold tuned
        // for short identifiers: at most 2 edits for ≤6-char names, 3 for
        // longer ones, plus substring fast-path.
        let mut scored: Vec<(usize, String)> = all_tools
            .iter()
            .filter_map(|t| {
                let name = t.name.to_lowercase();
                if name == needle {
                    return None; // exact match — caller should have resolved
                }
                let dist = levenshtein_distance(&name, &needle);
                let threshold = if name.len().max(needle.len()) <= 6 { 2 } else { 3 };
                let substring_hit = name.contains(&needle) || needle.contains(&name);
                if dist <= threshold || substring_hit {
                    let effective = if substring_hit { dist.min(2) } else { dist };
                    Some((effective, t.name.clone()))
                } else {
                    None
                }
            })
            .collect();

        if scored.is_empty() {
            return false;
        }

        scored.sort_by_key(|(d, name)| (*d, name.clone()));
        scored.truncate(3);
        let suggestions: Vec<String> = scored.into_iter().map(|(_, n)| format!("/{}", n)).collect();

        let text = format!(
            "Unknown command `/{}`. Did you mean: {}?",
            unknown_cmd,
            suggestions.join(", ")
        );
        let reply = OutboundMessage::text(msg.conversation_id.as_str(), &text);
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send unknown-command help: {}", e);
            return false;
        }
        info!(
            unknown = %unknown_cmd,
            suggestions = ?suggestions,
            "[Router] Sent unknown-command suggestions"
        );
        true
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
        let current_epoch = if let Some(ref sm) = self.session_store {
            let base = old_key.base_key_pattern();
            sm.get_current_epoch(&base).await.unwrap_or(old_key.epoch())
        } else {
            old_key.epoch()
        };
        let old_key_resolved = old_key.with_epoch(current_epoch);

        let new_key = old_key.with_epoch(current_epoch + 1);
        if let Some(ref sm) = self.session_store {
            if let Err(e) = sm.get_or_create(&new_key).await {
                warn!("[Router] Failed to create new session: {}", e);
            }
        }

        let locale = self.resolve_locale().await;
        let reply_text = crate::gateway::i18n::t(
            crate::gateway::i18n::Msg::NewSessionStarted { topic_suffix: "" },
            locale,
        );
        let reply = OutboundMessage::text(msg.conversation_id.as_str(), &reply_text);
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send /new reply: {}", e);
        }

        let topic = self.generate_session_topic(&old_key_resolved).await;

        if let Some(ref sm) = self.session_store {
            if let Err(e) = sm.close_session(&old_key_resolved, topic.as_deref()).await {
                warn!("[Router] Failed to close session: {}", e);
            }
        }

        info!("[Router] New session created: {}", new_key.to_key_string());
        Ok(())
    }

    /// Generate a topic summary for the current session using LLM
    pub(super) async fn generate_session_topic(&self, session_key: &SessionKey) -> Option<String> {
        let sm = self.session_store.as_ref()?;
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
        let payload = RequestPayload::new(&__msgs).with_system(Some(
            "You are a title generator. Output ONLY the title, nothing else.",
        ));
        match llm.process(payload).await {
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

}

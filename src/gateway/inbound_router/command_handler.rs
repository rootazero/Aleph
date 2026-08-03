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
        Some(args) => format!("{clean_cmd} {args}"),
        None => clean_cmd.to_string(),
    }
}

/// Special slash-command variants intercepted in the inbound router before the
/// generic `CommandParser` path. Case-insensitive over the command word, with
/// the `@botname` Telegram suffix tolerated. `Btw` carries the verbatim body
/// (original case preserved) so the model reads the user's actual phrasing
/// rather than a lowercased copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SpecialSlash {
    Help,
    Stop,
    Btw { body: String },
}

/// Classify a raw inbound text as a `SpecialSlash` variant.
///
/// Returns `None` for anything that is not `/help`, `/stop`, `/abort`, or
/// `/btw <body>` (case-insensitive, optional `@botname`). `/btw` without a
/// non-empty body is rejected — an empty sidebar question has no place to go.
pub(super) fn classify_special_slash(text: &str) -> Option<SpecialSlash> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let (head, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r),
        None => (trimmed, ""),
    };
    let cmd = head.split_once('@').map_or(head, |(c, _)| c);
    let cmd_lower = cmd.strip_prefix('/').unwrap_or(cmd).to_lowercase();
    match cmd_lower.as_str() {
        "help" => Some(SpecialSlash::Help),
        "stop" | "abort" => Some(SpecialSlash::Stop),
        "btw" => {
            let body = rest.trim();
            if body.is_empty() {
                None
            } else {
                Some(SpecialSlash::Btw {
                    body: body.to_string(),
                })
            }
        }
        _ => None,
    }
}

/// Parse the suffix after `clarify:` into a 1-based option index.
///
/// Returns `Some(n)` when the suffix (after trimming) is a positive integer,
/// matching the `ask_user::build_choice_keyboard` button contract — buttons
/// are emitted as `clarify:1`, `clarify:2`, … so `0` and any non-numeric
/// value are rejected as malformed.
///
/// `None` for empty, non-numeric, zero, or overflowing input. `@bot` is
/// unrelated to this parser: the router strips Telegram's `@botname`
/// suffix before this is called, and callback data never carries one.
pub(super) fn parse_clarify_index(suffix: &str) -> Option<usize> {
    let trimmed = suffix.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = trimmed.parse().ok()?;
    if n == 0 {
        return None;
    }
    Some(n)
}

/// Truncate a string to `max_chars` at a char boundary
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
#[must_use]
pub fn serialize_parsed_command(parsed: &crate::command::ParsedCommand) -> Option<String> {
    use crate::command::CommandContext;

    // Continuation-driven builtins (`/loop`, `/goal`) must fall through to the
    // full agent loop, NOT the direct-tool fast path: the fast path returns
    // before the post-run continuation hook that schedules the loop's first
    // tick / the goal's first pursuit, so serializing them here registers the
    // state but silently stalls it. Returning None skips SLASH_COMMAND_MODE and
    // routes the raw `/loop …` text through normal agent execution, where the
    // completion hook claims tick 1. Single-sourced with the Panel/CLI resolver
    // via `is_continuation_driven_slash` so the two surfaces cannot drift.
    if let CommandContext::Builtin { tool_name } = &parsed.context {
        if crate::gateway::execution_engine::is_continuation_driven_slash(tool_name) {
            return None;
        }
    }

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
            "tool_id": parsed.tool_id,
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

    /// Resolve the busy wait-lane knobs from `[execution]`. Read per message so
    /// a config hot-reload takes effect without a restart, mirroring how the
    /// locale above is resolved.
    pub(super) async fn busy_queue_config(&self) -> crate::gateway::busy_queue::BusyQueueConfig {
        match self.app_config {
            Some(ref cfg) => crate::gateway::busy_queue::BusyQueueConfig::from_execution(
                &cfg.read().await.execution,
            ),
            None => crate::gateway::busy_queue::BusyQueueConfig::default(),
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
        let parser = match self.command_parser.as_ref() {
            Some(p) => p,
            None => return false,
        };

        // Delegate scoring (canonical name + aliases, edit-distance + substring
        // fast-path) to the registry's shared suggester so the channel router
        // and the panel `command.execute` RPC path stay in lockstep.
        let suggestions: Vec<String> = parser
            .tool_registry()
            .suggest_commands(unknown_cmd, 3)
            .await
            .into_iter()
            .map(|n| format!("/{n}"))
            .collect();

        if suggestions.is_empty() {
            return false;
        }

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
    /// Creates a `SessionKey::Ephemeral` so the question/answer is not persisted
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

        // /btw is a plain ephemeral turn — it does NOT take the slash-command
        // fast path. Sending a non-mode JSON through
        // `execute_for_context_with_metadata` previously injected `{"btw":true}`
        // into the SLASH_COMMAND_MODE_KEY, which the engine's fast path
        // parsed as an unknown `mode_type` and rejected with
        // "Unknown slash command type:". Route through the regular
        // execution path so the model gets the btw prompt directly.
        self.execute_for_context(&ctx).await?;

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

        // Terminate the closed session's autonomous continuations BEFORE the
        // epoch bump — after it, `/loop stop` / `goal clear` route to the new
        // epoch and the old chain becomes uncancellable (shared seam with the
        // Panel `sessions.new` RPC).
        crate::gateway::continuation_lifecycle::terminate_session_continuations(
            &old_key_resolved.to_key_string(),
            "/new",
        );

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

    /// Handle /stop (alias /abort): cancel the run currently executing on this
    /// session and confirm to the user.
    ///
    /// `OpenClaw` `/stop` / codex `Op::Interrupt` parity — the cancel machinery
    /// (`ExecutionEngine::cancel`, Panel `chat.abort`) predates this but was
    /// unreachable from channels, so a Telegram user could steer a long
    /// autonomous loop but never stop it. The command is intercepted here and
    /// never reaches the agent loop; steering messages already injected into
    /// the session log stay in place — the next run sees them alongside the
    /// interruption marker and the model decides whether they still apply (R7).
    pub(super) async fn handle_stop(
        &self,
        msg: &InboundMessage,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        // An explicit stop means "I do not want this work" — a queued backlog
        // firing one message at a time right after the stop is the opposite of
        // that (codex clears pending input on `Op::Interrupt` for the same
        // reason). Scoped to this handler on purpose: the `Interrupt`
        // busy-input mode *depends* on the lane to restart its own message
        // after cancelling the sibling, so `cancel_session` itself must not
        // purge.
        //
        // Purge BEFORE cancelling, not after. Cancelling releases the session
        // slot, `release` notifies the lane, and the woken front waiter can win
        // `try_claim` — whose `mark_admitted` pulls its ticket out of the lane
        // unconditionally — before the purge is reached. That message then
        // escapes the stop *and* is missing from the "N dropped" the receipt
        // promises. While the sibling still holds the claim no waiter can be
        // admitted, so purging first marks every ticket that was waiting when
        // the user asked to stop, and `deliver_with_ticket` checks
        // `is_cancelled` ahead of `is_front`, so the wake finds it already dead.
        let dropped = crate::gateway::busy_queue::purge(&ctx.session_key.to_key_string());
        if dropped > 0 {
            info!(
                session = %ctx.session_key.to_key_string(),
                dropped,
                "[Router] /stop: dropped queued messages waiting on this session"
            );
        }

        let cancelled = match self.execution_adapter.as_ref() {
            Some(adapter) => adapter
                .cancel_session(&ctx.session_key)
                .await
                .unwrap_or_else(|e| {
                    warn!(
                        session = %ctx.session_key.to_key_string(),
                        error = %e,
                        "[Router] /stop: cancel_session failed"
                    );
                    None
                }),
            None => None,
        };

        if let Some(ref run_id) = cancelled {
            info!(
                session = %ctx.session_key.to_key_string(),
                run_id = %run_id,
                "[Router] /stop: cancelled running run"
            );
        }

        let locale = self.resolve_locale().await;
        let mut reply_text = crate::gateway::i18n::t(
            if cancelled.is_some() {
                crate::gateway::i18n::Msg::RunStopped
            } else {
                crate::gateway::i18n::Msg::NoActiveRun
            },
            locale,
        );
        if dropped > 0 {
            // One receipt for the whole batch — the individual waiters exit
            // silently rather than each announcing its own cancellation.
            reply_text.push(' ');
            reply_text.push_str(&crate::gateway::i18n::t(
                crate::gateway::i18n::Msg::QueuedMessagesDropped { count: dropped },
                locale,
            ));
        }
        let reply = OutboundMessage::text(msg.conversation_id.as_str(), &reply_text);
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send /stop reply: {}", e);
        }
        Ok(())
    }

    /// Handle /help: reply with the curated slash-command listing.
    ///
    /// Text channels (Telegram/Slack/Discord) have no completion menu, so
    /// `/help` is intercepted here and answered directly from the live
    /// `ToolCatalog` (the same source the completion menu and "did you mean?"
    /// suggester read). Panel/CLI surface discovery via `commands.list` + the
    /// completion UI, so this is the channel-side counterpart. Intercepted
    /// before agent dispatch like `/stop` — a read-only listing must never be
    /// queued behind a running turn.
    pub(super) async fn handle_help(&self, msg: &InboundMessage) -> Result<(), RoutingError> {
        let Some(parser) = self.command_parser.as_ref() else {
            // No unified catalog wired (simulated mode) — nothing to list.
            return Ok(());
        };

        let text =
            crate::gateway::handlers::commands::render_command_help(parser.tool_registry(), None)
                .await;

        let reply = OutboundMessage::text(msg.conversation_id.as_str(), &text);
        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
            error!("[Router] Failed to send /help reply: {}", e);
        }
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

#[cfg(test)]
mod tests {
    use super::{classify_special_slash, parse_clarify_index, SpecialSlash};

    #[test]
    fn classify_help_lowercase() {
        assert_eq!(classify_special_slash("/help"), Some(SpecialSlash::Help));
    }

    #[test]
    fn classify_help_mixed_case() {
        assert_eq!(classify_special_slash("/Help"), Some(SpecialSlash::Help));
        assert_eq!(classify_special_slash("/HELP"), Some(SpecialSlash::Help));
    }

    #[test]
    fn classify_help_at_bot() {
        assert_eq!(
            classify_special_slash("/help@MyBot"),
            Some(SpecialSlash::Help)
        );
    }

    #[test]
    fn classify_help_with_extra_text_is_still_help() {
        assert_eq!(
            classify_special_slash("/HELP please"),
            Some(SpecialSlash::Help)
        );
    }

    #[test]
    fn classify_stop_uppercase() {
        assert_eq!(classify_special_slash("/STOP"), Some(SpecialSlash::Stop));
    }

    #[test]
    fn classify_abort_uppercase() {
        assert_eq!(classify_special_slash("/ABORT"), Some(SpecialSlash::Stop));
    }

    #[test]
    fn classify_stop_at_bot() {
        assert_eq!(
            classify_special_slash("/stop@MyBot"),
            Some(SpecialSlash::Stop)
        );
        assert_eq!(
            classify_special_slash("/abort@MyBot extra"),
            Some(SpecialSlash::Stop)
        );
    }

    #[test]
    fn classify_btw_lowercase() {
        assert_eq!(
            classify_special_slash("/btw What is X?"),
            Some(SpecialSlash::Btw {
                body: "What is X?".to_string()
            })
        );
    }

    #[test]
    fn classify_btw_uppercase_preserves_body_case() {
        let body = "What is the Weather Forecast for Tokyo?";
        assert_eq!(
            classify_special_slash("/BTW What is the Weather Forecast for Tokyo?"),
            Some(SpecialSlash::Btw {
                body: body.to_string()
            })
        );
    }

    #[test]
    fn classify_btw_at_bot_preserves_body_case() {
        assert_eq!(
            classify_special_slash("/btw@MyBot Explain Async/Await in Rust"),
            Some(SpecialSlash::Btw {
                body: "Explain Async/Await in Rust".to_string()
            })
        );
        assert_eq!(
            classify_special_slash("/BTW@MyBot Hello, World!"),
            Some(SpecialSlash::Btw {
                body: "Hello, World!".to_string()
            })
        );
    }

    #[test]
    fn classify_btw_newline_separator() {
        assert_eq!(
            classify_special_slash("/btw\nQuestion on next line"),
            Some(SpecialSlash::Btw {
                body: "Question on next line".to_string()
            })
        );
    }

    #[test]
    fn classify_btw_empty_body_is_not_a_command() {
        assert_eq!(classify_special_slash("/btw"), None);
        assert_eq!(classify_special_slash("/btw "), None);
        assert_eq!(classify_special_slash("/btw@MyBot"), None);
        assert_eq!(classify_special_slash("/BTW\n   "), None);
    }

    #[test]
    fn classify_no_slash_prefix_is_not_a_command() {
        assert_eq!(classify_special_slash("help"), None);
        assert_eq!(classify_special_slash("STOP"), None);
        assert_eq!(classify_special_slash("btw hi"), None);
    }

    #[test]
    fn classify_unknown_command_is_none() {
        assert_eq!(classify_special_slash("/foo"), None);
        assert_eq!(classify_special_slash("/HELLO"), None);
        assert_eq!(classify_special_slash("/"), None);
        assert_eq!(classify_special_slash(""), None);
    }

    #[test]
    fn parse_clarify_index_accepts_positive_int() {
        assert_eq!(parse_clarify_index("1"), Some(1));
        assert_eq!(parse_clarify_index("2"), Some(2));
        assert_eq!(parse_clarify_index("42"), Some(42));
    }

    #[test]
    fn parse_clarify_index_trims_whitespace() {
        assert_eq!(parse_clarify_index("  1  "), Some(1));
        assert_eq!(parse_clarify_index("\t3\n"), Some(3));
    }

    #[test]
    fn parse_clarify_index_rejects_zero() {
        assert_eq!(parse_clarify_index("0"), None);
        assert_eq!(parse_clarify_index(" 0 "), None);
    }

    #[test]
    fn parse_clarify_index_rejects_empty() {
        assert_eq!(parse_clarify_index(""), None);
        assert_eq!(parse_clarify_index("   "), None);
        assert_eq!(parse_clarify_index("\n"), None);
    }

    #[test]
    fn parse_clarify_index_rejects_non_numeric() {
        assert_eq!(parse_clarify_index("abc"), None);
        assert_eq!(parse_clarify_index("1a"), None);
        assert_eq!(parse_clarify_index("a1"), None);
        assert_eq!(parse_clarify_index("-1"), None);
        assert_eq!(parse_clarify_index("1.0"), None);
    }

    #[test]
    fn parse_clarify_index_rejects_bot_suffix_artifact() {
        assert_eq!(parse_clarify_index("1@MyBot"), None);
        assert_eq!(parse_clarify_index("clarify:1"), None);
    }
}

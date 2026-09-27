use crate::gateway::channel::ChannelResult;
use crate::gateway::event_emitter::StreamEvent;
use crate::gateway::interfaces::telegram::config_v2::StatusReactionConfig;
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;

/// State machine for the inbound-message reaction.
///
/// Mirrors the run's lifecycle so a Telegram reader can read one emoji and
/// know what stage the model is at without scrolling the chat. Transitions
/// are inferred from `StreamEvent`s — there is no separate state frame on
/// the wire because the events already carry the information.
///
/// State | Trigger | Default emoji (overridable via `StatusReactionConfig`)
/// --- | --- | ---
/// `Idle` | (initial) | (no reaction)
/// `Queued` | `RunQueued` | "👀" (same as processing — "we got the message")
/// `Thinking` | `RunAccepted`, `ResponseChunk`, `Reasoning`, `ToolEnd` | "👀" / "🤔"
/// `ToolActive` | `ToolStart`, `ToolUpdate` | "🔧"
/// `Done` | `RunComplete` | (config.complete)
/// `Error` | `RunError` | "👎"
///
/// `Reasoning` events keep the Thinking-state reaction unless `thinking`
/// override is set in the config (so a deploy that wants a distinct
/// "the model is mid-thought" cue can pick one emoji; the default is the
/// processing emoji, which matches openclaw's "we are working on it" reading).
pub struct StatusReactionController {
    delivery: TelegramDelivery,
    config: StatusReactionConfig,
    current_reaction: Arc<Mutex<Option<String>>>,
    state: Arc<Mutex<ReactionState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReactionState {
    Idle,
    Queued,
    Thinking,
    ToolActive,
    Done,
    Error,
}

impl StatusReactionController {
    #[must_use]
    pub fn new(delivery: TelegramDelivery, config: StatusReactionConfig) -> Self {
        Self {
            delivery,
            config,
            current_reaction: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(ReactionState::Idle)),
        }
    }

    /// Read the current state — exposed for tests and the doctor, NOT for
    /// callers that want to influence the reaction (they should emit the
    /// right `StreamEvent` instead).
    #[cfg(test)]
    pub(crate) async fn current_state(&self) -> ReactionState {
        *self.state.lock().await
    }

    /// Handle a stream event and update the reaction accordingly.
    pub async fn handle_event(&self, event: &StreamEvent, message_id: i64) -> ChannelResult<()> {
        let (target_state, target_emoji) = self.derive_target(event);

        let mut state = self.state.lock().await;
        let mut current = self.current_reaction.lock().await;

        // Only transition states that actually change the visible reaction
        // — a `ResponseChunk` while we're already in `Thinking` is a no-op
        // for the reaction but still a legal state-machine event.
        if *state != target_state {
            *state = target_state;
        } else {
            // Same state — nothing to render.
            return Ok(());
        }

        if let Some(emoji) = target_emoji {
            if current.as_ref() != Some(&emoji) {
                self.delivery.set_reaction(message_id, &emoji).await?;
                *current = Some(emoji);
            }
        } else if matches!(target_state, ReactionState::Idle | ReactionState::Done) {
            // Done without an explicit complete emoji — clear any leftover
            // reaction so the user sees the bot finished (Telegram stops
            // highlighting a finished task without a Done-state clear).
            if current.is_some() {
                let _ = self.delivery.set_reaction(message_id, "").await;
                *current = None;
            }
        }

        Ok(())
    }

    /// Reduce a `StreamEvent` to (state, target emoji).
    ///
    /// Pure: no side effect, no I/O. Kept on the controller so the mapping
    /// lives next to the state enum and a future state-frame on the wire
    /// can re-use the same derivation.
    fn derive_target(&self, event: &StreamEvent) -> (ReactionState, Option<String>) {
        match event {
            StreamEvent::RunQueued { .. } => (ReactionState::Queued, self.config.processing.clone()),
            StreamEvent::RunAccepted { .. } => {
                (ReactionState::Thinking, self.config.processing.clone())
            }
            StreamEvent::Reasoning { .. } => {
                // Reasoning keeps the Thinking reaction so the user sees
                // "still working on it" — the new `thinking` config field,
                // if set, overrides to a distinct emoji (some deploys prefer
                // a brain symbol). Defaults to `processing` so out-of-the-
                // box config keeps the prior single-emoji behaviour.
                let emoji = self
                    .config
                    .thinking
                    .clone()
                    .or_else(|| self.config.processing.clone());
                (ReactionState::Thinking, emoji)
            }
            StreamEvent::ResponseChunk { .. } => {
                (ReactionState::Thinking, self.config.processing.clone())
            }
            StreamEvent::ToolStart { .. } | StreamEvent::ToolUpdate { .. } => {
                (ReactionState::ToolActive, self.config.tool_active.clone())
            }
            StreamEvent::ToolEnd { .. } => {
                // Tool finished — back to Thinking (model resumes synthesis).
                (ReactionState::Thinking, self.config.processing.clone())
            }
            StreamEvent::RunComplete { .. } => {
                (ReactionState::Done, self.config.complete.clone())
            }
            StreamEvent::RunError { .. } => (ReactionState::Error, Some("👎".to_string())),
            _ => (
                // Unknown event — keep current state, don't change the
                // reaction. Avoids the previously-implicit "any event is a
                // ReactionChange" hazard.
                ReactionState::Idle,
                None,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig;
    use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;

    #[tokio::test]
    async fn test_reaction_state_transitions() {
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            ResolvedConfig {
                account_id: "test".to_string(),
                bot_token: "test".to_string(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: false,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: Default::default(),
                error_policy: Default::default(),
                max_retries: 0,
                html_fallback: true,
                link_preview:
                    crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
            },
            Arc::new(ErrorCooldown::new()),
            "123",
        );
        let config = StatusReactionConfig {
            processing: Some("👀".to_string()),
            tool_active: Some("🔧".to_string()),
            complete: Some("👍".to_string()),
            thinking: None,};
        let controller = StatusReactionController::new(delivery, config);

        controller
            .handle_event(
                &StreamEvent::ResponseChunk {
                    run_id: "r1".to_string(),
                    seq: 1,
                    delta: "hi".to_string(),
                    full_text: "hi".to_string(),
                    chunk_index: 0,
                    is_final: false,
                    is_intermediate: false,
                },
                100,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.current_reaction.lock().await,
            Some("👀".to_string())
        );
    }
}

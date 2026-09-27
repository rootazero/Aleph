use crate::gateway::channel::{ChannelError, ChannelResult};
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::sync_primitives::Arc;
use tokio::sync::Mutex;

use super::lane_tracker::{LaneDeliveryTracker, LaneId};

/// Marker prefix that pins a Reasoning-lane message as "this is the model's
/// internal thoughts, not the answer" for the duration of one turn. openclaw
/// parity: Telegram renders the italic body the same way it does for Discord
/// (which uses the same `🧠` marker), so a thinking message carries a single
/// readable cue rather than the platform-default "Thinking" prefix Aleph's
/// core currently emits — which Telegram would otherwise render as plain
/// bold/italic body without a marker distinguishing it from the answer.
///
/// `REASONING_MESSAGE_PREFIX` is matched by `strip_reasoning_prefix` so a
/// re-splitter (when the same content is split / re-emitted) recognises the
/// marker instead of re-prefixing and growing the line by one marker each
/// pass.
pub(crate) const REASONING_MESSAGE_PREFIX: &str = "🧠";

/// Return the marker-prefixed form of `body`, unless `body` already starts
/// with the marker (a re-split case). Keeping this in one place means the
/// Reasoning lane and the answer lane cannot drift on what "a thinking
/// message" looks like.
pub(crate) fn mark_reasoning_message(body: &str) -> String {
    if body.starts_with(REASONING_MESSAGE_PREFIX) {
        body.to_string()
    } else {
        format!("{REASONING_MESSAGE_PREFIX} {body}")
    }
}

/// Handle to a single lane for writing stream chunks.
#[derive(Clone)]
pub struct LaneHandle {
    lane_id: LaneId,
    tracker: Arc<Mutex<LaneDeliveryTracker>>,
    delivery: TelegramDelivery,
}

impl LaneHandle {
    pub const fn new(
        lane_id: LaneId,
        tracker: Arc<Mutex<LaneDeliveryTracker>>,
        delivery: TelegramDelivery,
    ) -> Self {
        Self {
            lane_id,
            tracker,
            delivery,
        }
    }

    /// Append a streaming **delta** to this lane.
    ///
    /// Deltas are accumulated into the lane's cumulative text; the preview is
    /// then created (first delta) or edited (subsequent deltas) with the full
    /// accumulated text. Two openclaw-style disciplines apply:
    /// - **First-preview withholding** (`min_initial_chars`): the preview is not
    ///   sent until enough text has accrued, so a tiny partial doesn't fire a
    ///   push notification.
    /// - **Edit throttling** (`debounce_ms`): edits are coalesced to at most one
    ///   per interval. A throttled edit is not lost — the next delta past the
    ///   interval (or `finalize`) flushes the latest accumulated text.
    ///
    /// Reasoning lane additionally wraps the accumulated text with the
    /// [`REASONING_MESSAGE_PREFIX`] marker so a reader can tell the message
    /// is "the model thought this" rather than the answer. The marker is
    /// idempotent across re-splits.
    pub async fn write_chunk(&self, delta: &str) -> ChannelResult<()> {
        let stream_cfg = &self.delivery.config.streaming;
        let min_initial_chars = stream_cfg.min_initial_chars;
        let debounce = std::time::Duration::from_millis(stream_cfg.debounce_ms);

        let mut tracker = self.tracker.lock().await;
        let state = tracker
            .get_mut(self.lane_id)
            .ok_or_else(|| ChannelError::Internal("lane not initialized in tracker".to_string()))?;

        state.accumulated.push_str(delta);

        match state.preview_message_id {
            None => {
                // Withhold the first preview until enough text has accrued.
                if state.accumulated.chars().count() < min_initial_chars {
                    return Ok(());
                }
                let text = self.lane_marked_text(&state.accumulated);
                // Reserve the throttle slot before releasing the lock so a
                // concurrent delta can't race a second send.
                state.last_update = std::time::Instant::now();
                drop(tracker);
                let id = self.delivery.send_text_message(&text).await?;
                let mut tracker = self.tracker.lock().await;
                if let Some(state) = tracker.get_mut(self.lane_id) {
                    state.preview_message_id = Some(id);
                    state.is_streaming = true;
                }
                Ok(())
            }
            Some(message_id) => {
                if state.last_update.elapsed() < debounce {
                    return Ok(());
                }
                let text = self.lane_marked_text(&state.accumulated);
                state.last_update = std::time::Instant::now();
                drop(tracker);
                self.delivery.edit_text_message(message_id, &text).await?;
                Ok(())
            }
        }
    }

    /// Apply the lane-specific marker (only Reasoning lane wraps with
    /// `🧠`). Answer / Draft lanes pass through. Kept on `LaneHandle` (not
    /// the tracker) because the marker is a delivery-side concern, not a
    /// stream-state concern — the stored `accumulated` text is the bare
    /// reasoning body so a re-split that lands in a non-Reasoning surface
    /// (e.g. error-path finalisation that uses Answer lane) does not carry
    /// a stale marker into the answer.
    fn lane_marked_text(&self, accumulated: &str) -> String {
        match self.lane_id {
            LaneId::Reasoning => mark_reasoning_message(accumulated),
            LaneId::Answer | LaneId::Draft => accumulated.to_string(),
        }
    }

    /// Finalize the lane with final text.
    /// Returns the Telegram message ID of the finalized message.
    /// Settle the lane on the text it has already streamed.
    ///
    /// For the case where the run produced nothing deliverable — a pure-thinking
    /// turn, where `sanitize_final_response` returns `None`. Finalising with the
    /// raw `final_response` there posts the model's internal reasoning to the
    /// chat: sanitisation returning `None` means "there is no answer here", not
    /// "use the unsanitised text". The already-streamed preview is clean (the
    /// drain scrubbed it), so settling on it keeps the promise that a
    /// pure-reasoning turn never blanks an existing message without delivering
    /// anything the sanitiser refused.
    pub async fn finalize_streamed(&self) -> ChannelResult<i64> {
        let streamed = {
            let tracker = self.tracker.lock().await;
            tracker
                .get(self.lane_id)
                .map(|state| state.accumulated.clone())
                .unwrap_or_default()
        };
        self.finalize(&streamed).await
    }

    pub async fn finalize(&self, final_text: &str) -> ChannelResult<i64> {
        let mut tracker = self.tracker.lock().await;
        let state = tracker
            .get_mut(self.lane_id)
            .ok_or_else(|| ChannelError::Internal("lane not initialized in tracker".to_string()))?;

        let message_id = if let Some(id) = state.preview_message_id {
            id
        } else {
            let text = self.lane_marked_text(final_text);
            let id = self.delivery.send_text_message(&text).await?;
            state.final_message_id = Some(id);
            state.is_streaming = false;
            return Ok(id);
        };

        drop(tracker);
        let text = self.lane_marked_text(final_text);
        self.delivery
            .edit_text_message(message_id, &text)
            .await?;

        let mut tracker = self.tracker.lock().await;
        let state = tracker
            .get_mut(self.lane_id)
            .ok_or_else(|| ChannelError::Internal("lane not initialized in tracker".to_string()))?;
        state.final_message_id = Some(message_id);
        state.is_streaming = false;
        Ok(message_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
    use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;
    use crate::sync_primitives::Arc;

    #[tokio::test]
    async fn test_lane_handle_creation() {
        let tracker = Arc::new(Mutex::new(LaneDeliveryTracker::new(123, None)));
        let delivery = TelegramDelivery::new(
            teloxide::Bot::new("test"),
            crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig {
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
        let handle = LaneHandle::new(LaneId::Answer, tracker, delivery);
        assert!(matches!(handle.lane_id, LaneId::Answer));
    }

    /// The Reasoning-lane marker is the single place the 🧠 prefix lives.
    /// Pinned here so a refactor that drops the marker (or accidentally
    /// promotes it to the Answer lane) is observable by name.
    #[test]
    fn mark_reasoning_message_adds_marker_once() {
        assert_eq!(mark_reasoning_message("hello"), "🧠 hello");
        // Idempotent on re-split: a message that already carries the marker
        // must not double-marker.
        assert_eq!(mark_reasoning_message("🧠 hello"), "🧠 hello");
        // Whitespace is significant — the marker sits with a space so the
        // italic body opens cleanly when Telegram re-parses the markdown.
        assert!(mark_reasoning_message("x").starts_with("🧠 "));
    }
}

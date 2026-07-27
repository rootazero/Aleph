use crate::gateway::channel::{ConversationId, MessageId};
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::interfaces::telegram::config_v2::StreamingOptions;
use crate::gateway::interfaces::telegram::delivery::TelegramDelivery;
use crate::gateway::interfaces::telegram::error_cooldown::ErrorCooldown;
use crate::gateway::reply_emitter::ReplyEmitter;
use crate::sync_primitives::{Arc, Ordering};
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::StreamOrchestrator;

/// Telegram-specific event emitter that routes stream events through the orchestrator.
pub struct TelegramEventEmitter {
    event_tx: mpsc::Sender<StreamEvent>,
    seq_counter: Arc<crate::sync_primitives::AtomicU64>,
    /// Media-only leg — **not** a text fallback.
    ///
    /// The orchestrator owns text delivery here and knows nothing about
    /// `_media`: it turns `StreamEvent`s into lane edits and never looks at the
    /// run's [`PendingMedia`](crate::gateway::media::PendingMedia) buffer. So
    /// under the orchestrated config (any of `draft_api_enabled` /
    /// `reasoning_lane_enabled` / `status_reactions`) the buffer the tool
    /// chokepoint fills had **no drainer at all**, and every attachment —
    /// including the slash fast path's, which holds the buffer directly — was
    /// silently dropped.
    ///
    /// This `ReplyEmitter` exists for exactly one thing: reuse the single
    /// drain → download → send implementation on run end. No `StreamEvent` is
    /// ever forwarded to it, so it cannot double-post text.
    media: ReplyEmitter,
}

impl TelegramEventEmitter {
    #[must_use]
    pub fn new(
        bot: teloxide::Bot,
        config: StreamingOptions,
        conversation_id: String,
        route: crate::gateway::inbound_context::ReplyRoute,
        media: ReplyEmitter,
    ) -> Self {
        let delivery = TelegramDelivery::new(
            bot,
            crate::gateway::interfaces::telegram::config_resolver::ResolvedConfig {
                account_id: "telegram".to_string(),
                bot_token: String::new(),
                bot_username: None,
                default_agent: None,
                dm_policy: Default::default(),
                group_policy: Default::default(),
                send_typing: true,
                allowed_users: vec![],
                allowed_groups: vec![],
                streaming: config.clone(),
                error_policy: Default::default(),
                max_retries: 3,
                html_fallback: true,
                link_preview:
                    crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,
            },
            Arc::new(ErrorCooldown::new()),
            conversation_id.clone(),
        );

        let (orchestrator, event_tx) = StreamOrchestrator::new(delivery, config);

        let inbound = crate::gateway::channel::InboundMessage {
            id: MessageId::new("1"),
            conversation_id: ConversationId::new(conversation_id),
            channel_id: route.channel_id.clone(),
            sender_id: crate::gateway::channel::UserId::new("user"),
            sender_name: None,
            text: String::new(),
            timestamp: chrono::Utc::now(),
            attachments: vec![],
            metadata: Default::default(),
            reply_to: route.reply_to.clone(),
            is_group: false,
            raw: None,
        };

        tokio::spawn(orchestrator.run(inbound));

        Self {
            event_tx,
            seq_counter: Arc::new(crate::sync_primitives::AtomicU64::new(0)),
            media,
        }
    }
}

#[async_trait]
impl EventEmitter for TelegramEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        let run_ended = matches!(event, StreamEvent::RunComplete { .. });
        let forwarded = self
            .event_tx
            .send(event)
            .await
            .map_err(|_| EventEmitError::ChannelClosed);
        // Media rides its own message, so it is handed off *after* the
        // orchestrator has the final text — the download it has to do first
        // leaves the lane's finalize edit ahead of it in practice. Runs
        // regardless of `forwarded`: a dead orchestrator loses the text, it
        // must not also swallow the attachments.
        if run_ended {
            self.media.deliver_run_media().await;
        }
        forwarded
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::media::{MediaItem, PendingMedia};

    fn route() -> crate::gateway::inbound_context::ReplyRoute {
        crate::gateway::inbound_context::ReplyRoute::new(
            crate::gateway::channel::ChannelId::new("telegram"),
            crate::gateway::channel::ConversationId::new("123"),
        )
    }

    /// Build the emitter the way `try_create_telegram_emitter` does, over a
    /// registry with no channels registered — delivery attempts fail loudly in
    /// the log but the drain still runs, which is what these tests observe.
    fn emitter_with(run_id: &str, pending: PendingMedia) -> TelegramEventEmitter {
        let media = ReplyEmitter::new(
            Arc::new(crate::gateway::channel_registry::ChannelRegistry::new()),
            route(),
            run_id.to_string(),
            pending,
        );
        TelegramEventEmitter::new(
            teloxide::Bot::new("test"),
            StreamingOptions::default(),
            "123".to_string(),
            route(),
            media,
        )
    }

    fn run_complete(run_id: &str) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: run_id.to_string(),
            seq: 2,
            summary: Default::default(),
            total_duration_ms: 1,
        }
    }

    /// THE wire: the orchestrated emitter streams text independently of
    /// `ReplyEmitter`, so without an explicit media leg the run's buffer has no
    /// drainer and every attachment is silently dropped under
    /// `draft_api_enabled` / `reasoning_lane_enabled` / `status_reactions`.
    #[tokio::test]
    async fn run_complete_drains_the_runs_media_buffer() {
        let pending: PendingMedia = Arc::new(tokio::sync::Mutex::new(vec![MediaItem {
            // Inline data URL — resolved without touching the network.
            url: "data:image/png;base64,SGVsbG8=".to_string(),
            media_type: "image".to_string(),
            mime_type: None,
            filename: None,
        }]));
        let emitter = emitter_with("tg-media-run", pending.clone());

        emitter.emit(run_complete("tg-media-run")).await.unwrap();

        assert!(
            pending.lock().await.is_empty(),
            "RunComplete must hand the run's media to the channel, not leave it in the buffer"
        );
    }

    /// A run that produced no media must not post an empty attachment message.
    #[tokio::test]
    async fn run_complete_without_media_is_a_no_op() {
        let pending: PendingMedia = PendingMedia::default();
        let emitter = emitter_with("tg-empty-run", pending.clone());

        emitter.emit(run_complete("tg-empty-run")).await.unwrap();

        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_telegram_event_emitter_creation() {
        let emitter = emitter_with("r1", PendingMedia::default());

        emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: "r1".to_string(),
                seq: 1,
                delta: "hi".to_string(),
                full_text: "hi".to_string(),
                chunk_index: 0,
                is_final: false,
                is_intermediate: false,
            })
            .await
            .unwrap();

        assert_eq!(emitter.next_seq(), 0);
        assert_eq!(emitter.next_seq(), 1);
    }
}

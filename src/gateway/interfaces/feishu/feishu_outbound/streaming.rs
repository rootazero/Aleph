use crate::sync_primitives::{AtomicBool, AtomicU32, AtomicU64, Mutex as StdMutex, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::inbound_context::ReplyRoute;
use crate::gateway::reply_emitter::ReplyEmitter;
use crate::sync_primitives::Arc;

use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::types::TypingState;

const STREAM_THROTTLE_MS: u64 = 100;

/// Manages a single streaming card lifecycle.
pub struct FeishuStreamingCard {
    card_id: String,
    sequence: AtomicU32,
    accumulated_text: Mutex<String>,
    last_update: Mutex<Instant>,
    closed: AtomicBool,
}

impl FeishuStreamingCard {
    fn new(card_id: String) -> Self {
        Self {
            card_id,
            sequence: AtomicU32::new(1),
            accumulated_text: Mutex::new(String::new()),
            last_update: Mutex::new(Instant::now()),
            closed: AtomicBool::new(false),
        }
    }

    fn next_sequence(&self) -> u32 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    async fn update(&self, api: &FeishuApi, chunk: &str) {
        if self.is_closed() {
            return;
        }

        {
            self.accumulated_text.lock().await.push_str(chunk);
        }

        let should_send = {
            let last = self.last_update.lock().await;
            last.elapsed() >= Duration::from_millis(STREAM_THROTTLE_MS)
        };

        if should_send {
            self.flush(api).await;
        }
    }

    async fn flush(&self, api: &FeishuApi) {
        if self.is_closed() {
            return;
        }

        let text = self.accumulated_text.lock().await.clone();
        if text.is_empty() {
            return;
        }

        let seq = self.next_sequence();
        match api.update_streaming_card(&self.card_id, &text, seq).await {
            Ok(()) => {
                *self.last_update.lock().await = Instant::now();
            }
            Err(e) => {
                warn!("Failed to update streaming card: {e}");
            }
        }
    }

    async fn close(&self, api: &FeishuApi, final_text: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }

        let seq = self.next_sequence();
        if let Err(e) = api
            .update_streaming_card(&self.card_id, final_text, seq)
            .await
        {
            warn!("Failed to send final streaming card update: {e}");
        }

        let summary = match final_text.char_indices().nth(50) {
            Some((idx, _)) => &final_text[..idx],
            None => final_text,
        };
        let close_seq = self.next_sequence();
        if let Err(e) = api
            .close_streaming_card(&self.card_id, summary, close_seq)
            .await
        {
            warn!("Failed to close streaming card: {e}");
        }
    }
}

/// `EventEmitter` that streams to Feishu cards in real-time.
pub struct FeishuEventEmitter {
    inner: ReplyEmitter,
    api: Arc<FeishuApi>,
    card: Arc<Mutex<Option<FeishuStreamingCard>>>,
    chat_id: String,
    reply_to_message_id: Option<String>,
    streaming_enabled: bool,
    typing_enabled: bool,
    typing_state: Arc<StdMutex<Option<TypingState>>>,
    seq_counter: AtomicU64,
}

impl FeishuEventEmitter {
    pub fn new(
        inner: ReplyEmitter,
        api: Arc<FeishuApi>,
        _route: ReplyRoute,
        chat_id: String,
        reply_to_message_id: Option<String>,
        streaming_enabled: bool,
        typing_enabled: bool,
    ) -> Self {
        Self {
            inner,
            api,
            card: Arc::new(Mutex::new(None)),
            chat_id,
            reply_to_message_id,
            streaming_enabled,
            typing_enabled,
            typing_state: Arc::new(StdMutex::new(None)),
            seq_counter: AtomicU64::new(0),
        }
    }

    async fn start_typing(&self) {
        if !self.typing_enabled {
            return;
        }
        let msg_id = match &self.reply_to_message_id {
            Some(id) => id.clone(),
            None => return,
        };
        match self.api.add_reaction(&msg_id, "Typing").await {
            Ok(reaction_id) => {
                let mut state = self.typing_state.lock().unwrap_or_else(|e| e.into_inner());
                *state = Some(TypingState {
                    message_id: msg_id,
                    reaction_id,
                });
                debug!("Added typing indicator");
            }
            Err(e) => debug!("Failed to add typing indicator (non-critical): {e}"),
        }
    }

    async fn stop_typing(&self) {
        let state = {
            let mut guard = self.typing_state.lock().unwrap_or_else(|e| e.into_inner());
            guard.take()
        };
        if let Some(typing) = state {
            if let Err(e) = self
                .api
                .remove_reaction(&typing.message_id, &typing.reaction_id)
                .await
            {
                debug!("Failed to remove typing indicator (non-critical): {e}");
            } else {
                debug!("Removed typing indicator");
            }
        }
    }

    async fn create_card(&self) -> Option<FeishuStreamingCard> {
        let card_id = match self.api.create_streaming_card("⏳ Thinking...").await {
            Ok(id) => id,
            Err(e) => {
                warn!("Failed to create streaming card, falling back to instant: {e}");
                return None;
            }
        };
        let reply_to = self.reply_to_message_id.as_deref();
        match self
            .api
            .send_card_message(&self.chat_id, &card_id, reply_to)
            .await
        {
            Ok(_msg_id) => {
                debug!("Streaming card created and sent: {card_id}");
                Some(FeishuStreamingCard::new(card_id))
            }
            Err(e) => {
                warn!("Failed to send streaming card message: {:?}", e);
                None
            }
        }
    }
}

#[async_trait]
impl EventEmitter for FeishuEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match &event {
            StreamEvent::ResponseChunk {
                delta,
                is_final,
                is_intermediate,
                ..
            } => {
                if *is_intermediate || !self.streaming_enabled {
                    return self.inner.emit(event).await;
                }

                // Start typing on first chunk
                {
                    let card = self.card.lock().await;
                    if card.is_none() {
                        drop(card);
                        self.start_typing().await;
                    }
                }

                // Create card on first non-empty chunk
                if !delta.is_empty() {
                    let mut card_guard = self.card.lock().await;
                    if card_guard.is_none() {
                        if let Some(new_card) = self.create_card().await {
                            *card_guard = Some(new_card);
                        } else {
                            drop(card_guard);
                            return self.inner.emit(event).await;
                        }
                    }
                    if let Some(card) = card_guard.as_ref() {
                        card.update(&self.api, delta).await;
                    }
                }

                if *is_final {
                    let card_guard = self.card.lock().await;
                    if let Some(card) = card_guard.as_ref() {
                        let final_text = card.accumulated_text.lock().await.clone();
                        card.close(&self.api, &final_text).await;
                    }
                    drop(card_guard);
                    self.stop_typing().await;
                }

                Ok(())
            }

            StreamEvent::RunComplete { .. } => {
                let card_guard = self.card.lock().await;
                if let Some(card) = card_guard.as_ref() {
                    if !card.is_closed() {
                        let final_text = card.accumulated_text.lock().await.clone();
                        card.close(&self.api, &final_text).await;
                    }
                    drop(card_guard);
                    self.stop_typing().await;
                    // The card owns the *text*, so this branch deliberately does
                    // not forward to `inner` (its `RunComplete` would re-send the
                    // whole answer off `summary.final_response` as a second
                    // message). But the media leg lives on `inner` too, and
                    // skipping it dropped every attachment — and leaked the temp
                    // files — for any run that got a card. Close that leg
                    // explicitly; it touches only the media buffer, never text.
                    self.inner.deliver_run_media().await;
                    Ok(())
                } else {
                    drop(card_guard);
                    self.stop_typing().await;
                    self.inner.emit(event).await
                }
            }

            StreamEvent::RunError { .. } => {
                let card_guard = self.card.lock().await;
                if let Some(card) = card_guard.as_ref() {
                    if !card.is_closed() {
                        let text = card.accumulated_text.lock().await.clone();
                        let error_text = if text.is_empty() {
                            "An error occurred".to_string()
                        } else {
                            text
                        };
                        card.close(&self.api, &error_text).await;
                    }
                }
                drop(card_guard);
                self.stop_typing().await;
                self.inner.emit(event).await
            }

            _ => self.inner.emit(event).await,
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId};
    use crate::gateway::channel_registry::ChannelRegistry;
    use crate::gateway::event_emitter::RunSummary;
    use crate::gateway::interfaces::feishu::auth::TokenManager;
    use crate::gateway::media::PendingMedia;

    /// Emitter in the state that used to lose media: a card already streamed
    /// (and closed, so finishing it needs no API call) and a run's worth of
    /// `_media` sitting in the buffer.
    ///
    /// The `FeishuApi` here is never called — `base_url` points nowhere and the
    /// card is pre-closed — so the test does no I/O.
    async fn emitter_with_closed_card(pending: PendingMedia) -> FeishuEventEmitter {
        let http = reqwest::Client::new();
        let auth = Arc::new(TokenManager::new(
            "app",
            "secret",
            "http://127.0.0.1:1",
            http.clone(),
        ));
        let api = Arc::new(FeishuApi::new(auth, "http://127.0.0.1:1", http));
        let inner = ReplyEmitter::new(
            Arc::new(ChannelRegistry::new()),
            ReplyRoute::new(ChannelId::new("feishu"), ConversationId::new("oc_1")),
            "feishu-run".to_string(),
            pending,
        );
        let emitter = FeishuEventEmitter::new(
            inner,
            api,
            ReplyRoute::new(ChannelId::new("feishu"), ConversationId::new("oc_1")),
            "oc_1".to_string(),
            None,
            true,
            false,
        );
        let card = FeishuStreamingCard::new("card-1".to_string());
        card.closed.store(true, Ordering::SeqCst);
        *emitter.card.lock().await = Some(card);
        emitter
    }

    fn run_complete() -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: "feishu-run".to_string(),
            seq: 1,
            summary: RunSummary::default(),
            total_duration_ms: 1,
        }
    }

    /// THE wire: the card branch deliberately does not forward `RunComplete` to
    /// `inner` (that would re-post the whole answer as a second message), and
    /// that skip took the media leg down with it — every attachment was lost
    /// for any run that got a streaming card.
    #[tokio::test]
    async fn run_complete_with_a_card_still_drains_the_media_buffer() {
        let pending: PendingMedia = Arc::new(tokio::sync::Mutex::new(vec![
            crate::gateway::media::resolved_test_attachment(),
        ]));
        let emitter = emitter_with_closed_card(pending.clone()).await;

        emitter.emit(run_complete()).await.unwrap();

        assert!(
            pending.lock().await.is_empty(),
            "a streamed card must not swallow the run's attachments"
        );
    }

    /// The same branch with nothing to deliver stays a no-op.
    #[tokio::test]
    async fn run_complete_with_a_card_and_no_media_is_a_no_op() {
        let pending = PendingMedia::default();
        let emitter = emitter_with_closed_card(pending.clone()).await;

        emitter.emit(run_complete()).await.unwrap();

        assert!(pending.lock().await.is_empty());
    }
}

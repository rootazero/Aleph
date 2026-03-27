use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::sync_primitives::Arc;
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::reply_emitter::ReplyEmitter;
use crate::gateway::inbound_context::ReplyRoute;

use super::api::FeishuApi;
use super::types::TypingState;

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

    async fn update(&self, client: &FeishuApi, chunk: &str) {
        if self.is_closed() { return; }

        { self.accumulated_text.lock().await.push_str(chunk); }

        let should_send = {
            let last = self.last_update.lock().await;
            last.elapsed() >= Duration::from_millis(STREAM_THROTTLE_MS)
        };

        if should_send {
            self.flush(client).await;
        }
    }

    async fn flush(&self, client: &FeishuApi) {
        if self.is_closed() { return; }

        let text = self.accumulated_text.lock().await.clone();
        if text.is_empty() { return; }

        let seq = self.next_sequence();
        match client.update_streaming_card(&self.card_id, &text, seq).await {
            Ok(()) => { *self.last_update.lock().await = Instant::now(); }
            Err(e) => { warn!("Failed to update streaming card: {e}"); }
        }
    }

    async fn close(&self, client: &FeishuApi, final_text: &str) {
        if self.closed.swap(true, Ordering::SeqCst) { return; }

        let seq = self.next_sequence();
        if let Err(e) = client.update_streaming_card(&self.card_id, final_text, seq).await {
            warn!("Failed to send final streaming card update: {e}");
        }

        let summary = final_text.get(..50).unwrap_or(final_text);
        let close_seq = self.next_sequence();
        if let Err(e) = client.close_streaming_card(&self.card_id, summary, close_seq).await {
            warn!("Failed to close streaming card: {e}");
        }
    }
}

/// EventEmitter that streams to Feishu cards in real-time.
pub struct FeishuEventEmitter {
    inner: ReplyEmitter,
    client: Arc<FeishuApi>,
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
        client: Arc<FeishuApi>,
        _route: ReplyRoute,
        chat_id: String,
        reply_to_message_id: Option<String>,
        streaming_enabled: bool,
        typing_enabled: bool,
    ) -> Self {
        Self {
            inner,
            client,
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
        if !self.typing_enabled { return; }
        let msg_id = match &self.reply_to_message_id {
            Some(id) => id.clone(),
            None => return,
        };
        match self.client.add_reaction(&msg_id, "Typing").await {
            Ok(reaction_id) => {
                let mut state = self.typing_state.lock().unwrap_or_else(|e| e.into_inner());
                *state = Some(TypingState { message_id: msg_id, reaction_id });
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
            if let Err(e) = self.client.remove_reaction(&typing.message_id, &typing.reaction_id).await {
                debug!("Failed to remove typing indicator (non-critical): {e}");
            } else {
                debug!("Removed typing indicator");
            }
        }
    }

    async fn create_card(&self) -> Option<FeishuStreamingCard> {
        let card_id = match self.client.create_streaming_card("⏳ Thinking...").await {
            Ok(id) => id,
            Err(e) => {
                warn!("Failed to create streaming card, falling back to instant: {e}");
                return None;
            }
        };
        let reply_to = self.reply_to_message_id.as_deref();
        match self.client.send_card_message(&self.chat_id, &card_id, reply_to).await {
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
            StreamEvent::ResponseChunk { content, is_final, is_intermediate, .. } => {
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
                if !content.is_empty() {
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
                        card.update(&self.client, content).await;
                    }
                }

                if *is_final {
                    let card_guard = self.card.lock().await;
                    if let Some(card) = card_guard.as_ref() {
                        let final_text = card.accumulated_text.lock().await.clone();
                        card.close(&self.client, &final_text).await;
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
                        card.close(&self.client, &final_text).await;
                    }
                    drop(card_guard);
                    self.stop_typing().await;
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
                        card.close(&self.client, &error_text).await;
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

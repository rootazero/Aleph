//! Microsoft Teams Native Streaming Handler
//!
//! Implements the Teams streaminfo protocol for real-time AI response streaming.
//!
//! # Protocol Flow
//!
//! 1. `stream_start` — sends a typing activity with `streaminfo` entity (streamType: "informative")
//!    to establish the stream. Returns the activity ID as the `stream_id`.
//! 2. `stream_update` — sends typing activities with `streaminfo` (streamType: "streaming",
//!    streamSequence: N) carrying accumulated text chunks.
//! 3. `stream_finalize` — sends the final message activity with `streaminfo` (streamType: "final")
//!    and an AI-generated entity, completing the stream.
//!
//! # Streaming Coalescing
//!
//! For efficiency, updates are coalesced to reduce HTTP requests:
//! - Buffer accumulates text until `min_chars` (1500) threshold OR `idle_timeout` (1000ms)
//! - On threshold: immediate flush of buffered text
//! - On timeout: flush any buffered text
//! - On finalize: immediate flush of remaining buffer

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::warn;

use crate::gateway::channel::{
    ChannelError, ChannelResult, ConversationId, MessageId, NativeStreamHandler, OutboundMessage,
    SendResult,
};
use crate::sync_primitives::Arc;

use super::api::BotFrameworkClient;
use super::types::{
    build_ai_generated_entity, build_stream_info_entity, Activity, ActivityAttachment,
};

// ── ConversationReference (local copy for handler use) ───────────────────────

/// Minimal conversation reference needed for stream operations.
#[derive(Debug, Clone)]
pub struct StreamConversationRef {
    pub service_url: String,
}

// ── MsTeamsStreamHandler ──────────────────────────────────────────────────────

/// Native streaming handler for Microsoft Teams using the streaminfo protocol.
///
/// This struct wraps the `BotFrameworkClient` and a snapshot of cached conversation
/// references so it can be returned as an `Arc<dyn NativeStreamHandler>` without
/// requiring a reference to the parent `MsTeamsChannel`.
pub struct MsTeamsStreamHandler {
    client: Arc<BotFrameworkClient>,
    /// Shared conversation refs map (same Arc as `MsTeamsChannel` holds).
    conversation_refs: Arc<RwLock<HashMap<String, super::ConversationReference>>>,
}

impl MsTeamsStreamHandler {
    pub(super) const fn new(
        client: Arc<BotFrameworkClient>,
        conversation_refs: Arc<RwLock<HashMap<String, super::ConversationReference>>>,
    ) -> Self {
        Self {
            client,
            conversation_refs,
        }
    }

    async fn resolve_service_url(&self, conversation_id: &str) -> Option<String> {
        let refs = self.conversation_refs.read().await;
        refs.get(conversation_id).map(|r| r.service_url.clone())
    }
}

// ── NativeStreamHandler impl ──────────────────────────────────────────────────

#[async_trait]
impl NativeStreamHandler for MsTeamsStreamHandler {
    /// Send an "informative" typing activity to start a Teams stream.
    ///
    /// Returns the activity ID from the Bot Framework response, which callers
    /// must pass back as `stream_id` in subsequent `stream_update` and
    /// `stream_finalize` calls.
    async fn stream_start(
        &self,
        conversation_id: &ConversationId,
        status_text: &str,
    ) -> ChannelResult<String> {
        let service_url = self
            .resolve_service_url(conversation_id.as_str())
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id.as_str()
                ))
            })?;

        // Informative typing activity — establishes the stream
        let stream_entity = build_stream_info_entity(None, "informative", 0);
        let mut activity = Activity {
            activity_type: "typing".into(),
            text: Some(status_text.to_string()),
            entities: Some(vec![stream_entity]),
            ..Default::default()
        };

        // Also inject channelData for Teams streaming support
        activity.channel_data = Some(serde_json::json!({
            "streamType": "informative"
        }));

        let resp = self
            .client
            .send_activity(&service_url, conversation_id.as_str(), &activity)
            .await?;

        Ok(resp.id)
    }

    /// Send a streaming text chunk as a typing activity.
    ///
    /// `text` should be the **accumulated** text so far (not just the delta),
    /// as Teams displays the latest chunk to the user and discards earlier ones.
    /// `sequence` must be monotonically increasing (starting from 1).
    async fn stream_update(
        &self,
        conversation_id: &ConversationId,
        stream_id: &str,
        text: &str,
        sequence: u32,
    ) -> ChannelResult<()> {
        let service_url = self
            .resolve_service_url(conversation_id.as_str())
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id.as_str()
                ))
            })?;

        let stream_entity = build_stream_info_entity(Some(stream_id), "streaming", sequence);
        let mut activity = Activity {
            activity_type: "typing".into(),
            text: Some(text.to_string()),
            entities: Some(vec![stream_entity]),
            ..Default::default()
        };

        activity.channel_data = Some(serde_json::json!({
            "streamType": "streaming",
            "streamSequence": sequence,
            "streamId": stream_id
        }));

        self.client
            .send_activity(&service_url, conversation_id.as_str(), &activity)
            .await
            .map(|resp| {
                // We don't need the response ID for updates, but log any issues
                let _ = resp;
            })
    }

    /// Finalize the stream by sending the complete message as a "final" activity.
    ///
    /// Converts the `OutboundMessage` into a Bot Framework message activity with:
    /// - `streaminfo` entity (streamType: "final")
    /// - AI-generated content entity
    /// - Optional attachments from the outbound message
    async fn stream_finalize(
        &self,
        conversation_id: &ConversationId,
        stream_id: &str,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        let service_url = self
            .resolve_service_url(conversation_id.as_str())
            .await
            .ok_or_else(|| {
                ChannelError::SendFailed(format!(
                    "No cached service URL for conversation '{}'",
                    conversation_id.as_str()
                ))
            })?;

        // Build final activity with streaminfo + AI entity
        let stream_entity = build_stream_info_entity(Some(stream_id), "final", 0);
        let ai_entity = build_ai_generated_entity();

        let mut activity = Activity {
            activity_type: "message".into(),
            text: Some(message.text.clone()),
            text_format: Some("markdown".into()),
            entities: Some(vec![stream_entity, ai_entity]),
            ..Default::default()
        };

        // channelData to signal stream completion
        activity.channel_data = Some(serde_json::json!({
            "streamType": "final",
            "streamId": stream_id
        }));

        // Set reply-to if present
        if let Some(ref reply_to) = message.reply_to {
            activity.reply_to_id = Some(reply_to.as_str().to_string());
        }

        // Convert attachments
        if !message.attachments.is_empty() {
            let att_list: Vec<ActivityAttachment> = message
                .attachments
                .iter()
                .map(|att| ActivityAttachment {
                    content_type: att.mime_type.clone(),
                    content_url: att.url.clone(),
                    content: None,
                    name: att.filename.clone(),
                })
                .collect();
            activity.attachments = Some(att_list);
        }

        let resp = self
            .client
            .send_activity(&service_url, conversation_id.as_str(), &activity)
            .await
            .map_err(|e| {
                warn!(
                    stream_id = %stream_id,
                    error = %e,
                    "Failed to finalize Teams stream"
                );
                e
            })?;

        Ok(SendResult {
            message_id: MessageId::new(&resp.id),
            timestamp: Utc::now(),
        })
    }
}

// ── Streaming Coalescer ────────────────────────────────────────────────────────

/// Per-stream buffering state.
struct PendingStream {
    buffer: String,
    sequence: u32,
    idle_task: tokio::task::JoinHandle<()>,
}

impl PendingStream {
    const fn new(idle_task: tokio::task::JoinHandle<()>) -> Self {
        Self {
            buffer: String::new(),
            sequence: 0,
            idle_task,
        }
    }

    fn take_buffer(&mut self) -> Option<(String, u32)> {
        if self.buffer.is_empty() {
            return None;
        }
        let text = std::mem::take(&mut self.buffer);
        let sequence = self.sequence;
        self.sequence = 0;
        Some((text, sequence))
    }
}

/// Coalescing wrapper for `NativeStreamHandler` that buffers updates.
///
/// Reduces HTTP request overhead by batching text chunks until:
/// - `min_chars` (1500) threshold reached → immediate flush
/// - `idle_timeout` (1000ms) elapsed → flush buffered text
/// - `stream_finalize` called → flush remaining buffer
pub struct StreamCoalescer<H: NativeStreamHandler> {
    inner: H,
    min_chars: usize,
    idle_timeout: std::time::Duration,
    pending: Arc<RwLock<HashMap<String, PendingStream>>>,
}

impl<H: NativeStreamHandler> StreamCoalescer<H> {
    pub fn new(inner: H) -> Self {
        Self {
            inner,
            min_chars: 1500,
            idle_timeout: std::time::Duration::from_millis(1000),
            pending: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub const fn min_chars(mut self, min_chars: usize) -> Self {
        self.min_chars = min_chars;
        self
    }

    pub const fn idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }
}

#[async_trait]
impl<H: NativeStreamHandler + Clone + 'static> NativeStreamHandler for StreamCoalescer<H> {
    async fn stream_start(
        &self,
        conversation_id: &ConversationId,
        status_text: &str,
    ) -> ChannelResult<String> {
        let stream_id = self
            .inner
            .stream_start(conversation_id, status_text)
            .await?;

        let mut pending_map = self.pending.write().await;
        if let Some(stale) = pending_map.remove(&stream_id) {
            stale.idle_task.abort();
        }

        Ok(stream_id)
    }

    async fn stream_update(
        &self,
        conversation_id: &ConversationId,
        stream_id: &str,
        text: &str,
        sequence: u32,
    ) -> ChannelResult<()> {
        let stream_key = stream_id.to_string();

        let must_flush = {
            let mut pending_map = self.pending.write().await;
            let entry = pending_map
                .entry(stream_key.clone())
                .or_insert_with(|| PendingStream::new(tokio::spawn(async move {})));
            entry.buffer.clear();
            entry.buffer.push_str(text);
            entry.sequence = sequence;
            entry.buffer.len() >= self.min_chars
        };

        if must_flush {
            let buffered = {
                let mut pending_map = self.pending.write().await;
                pending_map.get_mut(&stream_key).and_then(|pending| {
                    pending.idle_task.abort();
                    pending.take_buffer()
                })
            };
            if let Some((text, sequence)) = buffered {
                self.inner
                    .stream_update(conversation_id, &stream_key, &text, sequence)
                    .await?;
            }
        } else {
            let inner = self.inner.clone();
            let conv = ConversationId::new(conversation_id.as_str().to_string());
            let sid = stream_key.clone();
            let timeout = self.idle_timeout;

            let pending_map = self.pending.clone();
            let new_task = tokio::spawn(async move {
                let sleep_duration = sleep(timeout);
                tokio::pin!(sleep_duration);
                let _ = sleep_duration.await;

                let pending = {
                    let mut map = pending_map.write().await;
                    map.remove(&sid)
                };
                if let Some(pending) = pending {
                    let _ = inner
                        .stream_update(&conv, &sid, &pending.buffer, pending.sequence)
                        .await;
                }
            });

            let mut pending_map = self.pending.write().await;
            if let Some(entry) = pending_map.get_mut(&stream_key) {
                entry.idle_task.abort();
                entry.idle_task = new_task;
            }
        }

        Ok(())
    }

    async fn stream_finalize(
        &self,
        conversation_id: &ConversationId,
        stream_id: &str,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        {
            let mut pending_map = self.pending.write().await;
            if let Some(mut pending) = pending_map.remove(stream_id) {
                pending.idle_task.abort();
                if let Some((text, seq)) = pending.take_buffer() {
                    drop(pending_map);
                    self.inner
                        .stream_update(conversation_id, stream_id, &text, seq)
                        .await?;
                }
            }
        }

        self.inner
            .stream_finalize(conversation_id, stream_id, message)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::msteams::types::build_stream_info_entity;

    #[test]
    fn test_build_stream_info_entities() {
        let informative = build_stream_info_entity(None, "informative", 0);
        assert_eq!(informative["type"], "streaminfo");
        assert_eq!(informative["streamType"], "informative");
        assert_eq!(informative["streamSequence"], 0);
        assert!(
            informative.get("streamId").is_none()
                || informative["streamId"] == serde_json::Value::Null,
            "informative entity should not have streamId"
        );

        let streaming = build_stream_info_entity(Some("stream-abc-123"), "streaming", 5);
        assert_eq!(streaming["type"], "streaminfo");
        assert_eq!(streaming["streamType"], "streaming");
        assert_eq!(streaming["streamId"], "stream-abc-123");
        assert_eq!(streaming["streamSequence"], 5);

        let final_entity = build_stream_info_entity(Some("stream-abc-123"), "final", 0);
        assert_eq!(final_entity["type"], "streaminfo");
        assert_eq!(final_entity["streamType"], "final");
        assert_eq!(final_entity["streamId"], "stream-abc-123");
        assert_eq!(final_entity["streamSequence"], 0);
    }

    #[test]
    fn test_build_ai_generated_entity_structure() {
        let ai_entity = build_ai_generated_entity();
        assert_eq!(ai_entity["type"], "https://schema.org/Message");
        assert_eq!(ai_entity["@type"], "Message");
        assert_eq!(ai_entity["@id"], "");
        let additional_types = ai_entity["additionalType"].as_array().unwrap();
        assert!(additional_types.contains(&serde_json::json!("AIGeneratedContent")));
    }

    #[test]
    fn test_informative_entity_no_stream_id() {
        let entity = build_stream_info_entity(None, "informative", 0);
        assert!(entity.as_object().unwrap().get("streamId").is_none());
    }
}

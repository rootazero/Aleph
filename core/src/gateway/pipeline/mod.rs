//! Message Pipeline
//!
//! Processes inbound messages through debounce, media download,
//! media understanding, and enrichment stages.

pub mod types;
pub mod debounce;
pub use debounce::{DebounceBuffer, DebounceConfig};
pub mod media_download;
pub use media_download::MediaDownloader;
pub mod media_understanding;
pub use media_understanding::{MediaUnderstander, UnderstandingProvider};

pub use types::*;

use tracing::info;

// ---------------------------------------------------------------------------
// MessagePipeline
// ---------------------------------------------------------------------------

/// Orchestrates the message pipeline: download → understand → enrich.
pub struct MessagePipeline {
    downloader: MediaDownloader,
    understander: MediaUnderstander,
}

impl MessagePipeline {
    pub fn new(downloader: MediaDownloader, understander: MediaUnderstander) -> Self {
        Self {
            downloader,
            understander,
        }
    }

    /// Process a merged message through all pipeline stages.
    pub async fn process(
        &self,
        merged: MergedMessage,
        agent_understanding_model: Option<&str>,
    ) -> Result<EnrichedMessage, PipelineError> {
        info!(
            merge_count = merged.merge_count,
            has_attachments = !merged.attachments.is_empty(),
            text_len = merged.text.len(),
            "Pipeline: processing merged message"
        );

        // Stage 1: Download all media
        let local_media = self.downloader.download_all(&merged).await;

        // Stage 2: Understand media (skip if nothing to understand)
        let (understandings, tokens) = if !local_media.is_empty() {
            self.understander
                .understand_all(&local_media, agent_understanding_model)
                .await
        } else {
            (vec![], 0)
        };

        // Stage 3: Build enriched message
        let enriched = EnrichedMessage::build(merged, local_media, understandings, tokens);

        info!(
            enriched_len = enriched.enriched_text.len(),
            media_count = enriched.local_media.len(),
            understanding_tokens = enriched.understanding_tokens,
            "Pipeline: enrichment complete"
        );

        Ok(enriched)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod integration_test;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use chrono::Utc;

    use crate::gateway::channel::{
        Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
    };
    use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
    use crate::gateway::router::SessionKey;
    use crate::sync_primitives::Arc;

    /// A no-op provider that always returns ("understood", 10).
    struct NoOpProvider;

    #[async_trait::async_trait]
    impl UnderstandingProvider for NoOpProvider {
        async fn understand(
            &self,
            _local_path: &Path,
            _category: &MediaCategory,
            _model: &str,
        ) -> Result<(String, u64), String> {
            Ok(("understood".to_string(), 10))
        }
    }

    fn make_merged(text: &str, attachments: Vec<Attachment>) -> MergedMessage {
        let msg = InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("ch-1"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: attachments.clone(),
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("ch-1"), ConversationId::new("conv-1"));
        let session_key = SessionKey::main("main");
        let ctx = InboundContext::new(msg, route, session_key);
        MergedMessage {
            text: text.to_string(),
            attachments,
            primary_context: ctx,
            merged_message_ids: vec![MessageId::new("msg-1")],
            merge_count: 1,
        }
    }

    fn make_pipeline(workspace: PathBuf) -> MessagePipeline {
        let downloader = MediaDownloader::new(workspace);
        let provider: Arc<dyn UnderstandingProvider> = Arc::new(NoOpProvider);
        let understander = MediaUnderstander::new(provider, "test-model".to_string());
        MessagePipeline::new(downloader, understander)
    }

    #[tokio::test]
    async fn test_pipeline_text_only() {
        let tmp = tempfile::tempdir().unwrap();
        let pipeline = make_pipeline(tmp.path().to_path_buf());

        let merged = make_merged("Hello, just text", vec![]);
        let enriched = pipeline.process(merged, None).await.unwrap();

        assert_eq!(enriched.enriched_text, "Hello, just text");
        assert!(enriched.local_media.is_empty());
        assert_eq!(enriched.understanding_tokens, 0);
    }

    #[tokio::test]
    async fn test_pipeline_with_inline_attachment() {
        let tmp = tempfile::tempdir().unwrap();
        let pipeline = make_pipeline(tmp.path().to_path_buf());

        let attachment = Attachment {
            id: "a1".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("photo.png".to_string()),
            size: None,
            url: None,
            path: None,
            data: Some(b"fake-image-data".to_vec()),
        };

        let merged = make_merged("Check this image", vec![attachment]);
        let enriched = pipeline.process(merged, None).await.unwrap();

        assert!(enriched.enriched_text.contains("[Attachment Understanding]"));
        assert_eq!(enriched.local_media.len(), 1);
        assert!(enriched.understanding_tokens > 0);
    }
}

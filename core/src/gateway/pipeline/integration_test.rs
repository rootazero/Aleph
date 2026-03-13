//! Integration tests for the full debounce → pipeline → enrichment chain.

use std::path::Path;

use chrono::Utc;
use tokio::sync::{Mutex, Notify};
use tokio::time::Duration;

use crate::gateway::channel::{
    Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
};
use crate::gateway::inbound_context::{InboundContext, ReplyRoute};
use crate::gateway::pipeline::debounce::{DebounceBuffer, DebounceConfig};
use crate::gateway::pipeline::media_download::MediaDownloader;
use crate::gateway::pipeline::media_understanding::{MediaUnderstander, UnderstandingProvider};
use crate::gateway::pipeline::types::MediaCategory;
use crate::gateway::pipeline::MessagePipeline;
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;

use super::types::EnrichedMessage;

// ---------------------------------------------------------------------------
// MockUnderstandingProvider
// ---------------------------------------------------------------------------

/// Returns category-dependent mock results.
struct MockUnderstandingProvider;

#[async_trait::async_trait]
impl UnderstandingProvider for MockUnderstandingProvider {
    async fn understand(
        &self,
        _local_path: &Path,
        category: &MediaCategory,
        _model: &str,
    ) -> Result<(String, u64), String> {
        match category {
            MediaCategory::Image => Ok(("A test image".to_string(), 25)),
            _ => Ok(("Some content".to_string(), 15)),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_context(text: &str, msg_id: &str, attachments: Vec<Attachment>) -> InboundContext {
    let msg = InboundMessage {
        id: MessageId::new(msg_id),
        channel_id: ChannelId::new("test-ch"),
        conversation_id: ConversationId::new("conv-1"),
        sender_id: UserId::new("user-1"),
        sender_name: None,
        text: text.to_string(),
        attachments,
        timestamp: Utc::now(),
        reply_to: None,
        is_group: false,
        raw: None,
    };
    let route = ReplyRoute::new(ChannelId::new("test-ch"), ConversationId::new("conv-1"));
    let session_key = SessionKey::main("main");
    InboundContext::new(msg, route, session_key)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_full_pipeline_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let downloader = MediaDownloader::new(tmp.path().to_path_buf());
    let provider: Arc<dyn UnderstandingProvider> = Arc::new(MockUnderstandingProvider);
    let understander = MediaUnderstander::new(provider, "test-model".to_string());
    let pipeline = Arc::new(MessagePipeline::new(downloader, understander));

    let results: Arc<Mutex<Vec<EnrichedMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(Notify::new());

    let p = Arc::clone(&pipeline);
    let r = Arc::clone(&results);
    let n = Arc::clone(&notify);

    let config = DebounceConfig {
        default_window_ms: 100,
        max_window_ms: 2000,
        max_messages: 10,
        ..Default::default()
    };

    let on_ready = Arc::new(move |merged| {
        let p = Arc::clone(&p);
        let r = Arc::clone(&r);
        let n = Arc::clone(&n);
        tokio::spawn(async move {
            match p.process(merged, None).await {
                Ok(enriched) => {
                    r.lock().await.push(enriched);
                    n.notify_one();
                }
                Err(e) => panic!("Pipeline failed: {}", e),
            }
        });
    });

    let buffer = DebounceBuffer::new(config, on_ready);

    // Submit a single message with an inline image attachment
    let attachment = Attachment {
        id: "a1".to_string(),
        mime_type: "image/png".to_string(),
        filename: Some("photo.png".to_string()),
        size: None,
        url: None,
        path: None,
        data: Some(b"fake-image-bytes".to_vec()),
    };
    let ctx = make_context("Check this image", "msg-1", vec![attachment]);
    buffer.submit(ctx).await;

    // Wait for debounce → pipeline to complete
    tokio::time::timeout(Duration::from_secs(5), notify.notified())
        .await
        .expect("Timed out waiting for pipeline");

    let guard = results.lock().await;
    assert_eq!(guard.len(), 1);
    let enriched = &guard[0];

    // Verify enriched_text contains original text + [Attachment Understanding]
    assert!(
        enriched.enriched_text.contains("Check this image"),
        "enriched_text should contain original text"
    );
    assert!(
        enriched.enriched_text.contains("[Attachment Understanding]"),
        "enriched_text should contain attachment understanding section"
    );

    // Verify local_media has 1 item
    assert_eq!(enriched.local_media.len(), 1);

    // Verify understanding_tokens > 0
    assert!(
        enriched.understanding_tokens > 0,
        "understanding_tokens should be > 0 for image attachment"
    );

    // Verify merge_count == 1
    assert_eq!(enriched.merged.merge_count, 1);
}

#[tokio::test]
async fn test_debounce_merge_then_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let downloader = MediaDownloader::new(tmp.path().to_path_buf());
    let provider: Arc<dyn UnderstandingProvider> = Arc::new(MockUnderstandingProvider);
    let understander = MediaUnderstander::new(provider, "test-model".to_string());
    let pipeline = Arc::new(MessagePipeline::new(downloader, understander));

    let results: Arc<Mutex<Vec<EnrichedMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(Notify::new());

    let p = Arc::clone(&pipeline);
    let r = Arc::clone(&results);
    let n = Arc::clone(&notify);

    let config = DebounceConfig {
        default_window_ms: 200,
        max_window_ms: 2000,
        max_messages: 10,
        ..Default::default()
    };

    let on_ready = Arc::new(move |merged| {
        let p = Arc::clone(&p);
        let r = Arc::clone(&r);
        let n = Arc::clone(&n);
        tokio::spawn(async move {
            match p.process(merged, None).await {
                Ok(enriched) => {
                    r.lock().await.push(enriched);
                    n.notify_one();
                }
                Err(e) => panic!("Pipeline failed: {}", e),
            }
        });
    });

    let buffer = DebounceBuffer::new(config, on_ready);

    // Submit 3 rapid-fire text messages (no attachments), 50ms apart
    let ctx1 = make_context("Hello there", "msg-1", vec![]);
    buffer.submit(ctx1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let ctx2 = make_context("How are you", "msg-2", vec![]);
    buffer.submit(ctx2).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let ctx3 = make_context("Fine thanks", "msg-3", vec![]);
    buffer.submit(ctx3).await;

    // Wait for debounce → pipeline to complete
    tokio::time::timeout(Duration::from_secs(5), notify.notified())
        .await
        .expect("Timed out waiting for pipeline");

    let guard = results.lock().await;
    assert_eq!(guard.len(), 1);
    let enriched = &guard[0];

    // Verify merge_count == 3
    assert_eq!(enriched.merged.merge_count, 3);

    // Verify enriched_text contains all 3 message texts
    assert!(
        enriched.enriched_text.contains("Hello there"),
        "enriched_text should contain first message"
    );
    assert!(
        enriched.enriched_text.contains("How are you"),
        "enriched_text should contain second message"
    );
    assert!(
        enriched.enriched_text.contains("Fine thanks"),
        "enriched_text should contain third message"
    );

    // Verify NO [Attachment Understanding] section (no attachments)
    assert!(
        !enriched.enriched_text.contains("[Attachment Understanding]"),
        "enriched_text should NOT contain attachment understanding for text-only messages"
    );

    // Verify understanding_tokens == 0
    assert_eq!(
        enriched.understanding_tokens, 0,
        "understanding_tokens should be 0 for text-only messages"
    );
}

//! Layer A integration tests for the multimodal pipeline.
//!
//! Tests MediaProcessor directly with mock/fake data. No external services needed.

use alephcore::gateway::channel::Attachment;
use alephcore::media::cache::CachedMedia;
use alephcore::media::processor::MediaProcessor;
use alephcore::media::transcription::{TranscriptionResult, TranscriptionService};
use alephcore::providers::message::{ContentBlock, UnifiedMessage};
use async_trait::async_trait;

// =============================================================================
// Mock TranscriptionService
// =============================================================================

struct MockTranscription(String);

#[async_trait]
impl TranscriptionService for MockTranscription {
    async fn transcribe(
        &self,
        _audio: &CachedMedia,
        _language: Option<&str>,
    ) -> anyhow::Result<TranscriptionResult> {
        Ok(TranscriptionResult {
            text: self.0.clone(),
            language: Some("en".to_string()),
        })
    }
}

// =============================================================================
// Helper
// =============================================================================

fn make_attachment(id: &str, mime: &str, data: Vec<u8>) -> Attachment {
    Attachment {
        id: id.into(),
        mime_type: mime.into(),
        filename: Some(format!("{}.test", id)),
        size: Some(data.len() as u64),
        url: None,
        path: None,
        data: Some(data),
    }
}

// =============================================================================
// Tests
// =============================================================================

/// 1. Image with vision support -> ContentBlock::Image
#[tokio::test]
async fn test_image_native_injection() {
    let processor = MediaProcessor::new(None, None);
    let jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    let att = make_attachment("jpeg1", "image/jpeg", jpeg_data);
    let session_id = "multimodal-probe-native-inject";

    let blocks = processor.process(&[att], true, session_id, "run-1").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Image { data, mime_type } => {
            assert_eq!(mime_type, "image/jpeg");
            assert!(!data.is_empty(), "base64 data must not be empty");
        }
        other => panic!("expected Image block, got {:?}", other),
    }

    processor.cleanup(session_id);
}

/// 2. Image without vision support and no VisionPipeline -> fallback text
#[tokio::test]
async fn test_image_vision_fallback() {
    let processor = MediaProcessor::new(None, None);
    let png_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A];
    let att = make_attachment("png1", "image/png", png_data);
    let session_id = "multimodal-probe-vision-fallback";

    let blocks = processor.process(&[att], false, session_id, "run-2").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text, .. } => {
            // A self-describing summary, not the old dangling
            // `{{media:image:png1}}` token: the model must be able to name what
            // it received even when it cannot see it.
            assert!(
                text.starts_with("[Image: png1.test (image/png"),
                "expected a self-describing image summary, got: {}",
                text
            );
            assert!(!text.contains("{{media:"), "no dangling handle: {}", text);
        }
        other => panic!("expected Text block for vision fallback, got {:?}", other),
    }

    processor.cleanup(session_id);
}

/// 3. Audio with mock TranscriptionService -> transcript text
#[tokio::test]
async fn test_audio_transcription() {
    let mock = MockTranscription("Hello world".to_string());
    let processor = MediaProcessor::new(Some(Box::new(mock)), None);
    let ogg_data = vec![0x4F, 0x67, 0x67, 0x53, 0x00, 0x02];
    let att = make_attachment("audio1", "audio/ogg", ogg_data);
    let session_id = "multimodal-probe-audio-transcribe";

    let blocks = processor.process(&[att], true, session_id, "run-3").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text, .. } => {
            assert!(
                text.contains("Hello world"),
                "expected transcript text, got: {}",
                text
            );
        }
        other => panic!("expected Text block with transcript, got {:?}", other),
    }

    processor.cleanup(session_id);
}

/// 4. Unknown MIME type -> self-describing summary text
#[tokio::test]
async fn test_unknown_type_placeholder() {
    let processor = MediaProcessor::new(None, None);
    let att = Attachment {
        id: "pdf1".into(),
        mime_type: "application/pdf".into(),
        filename: Some("report.pdf".into()),
        size: None,
        url: None,
        path: None,
        data: None,
    };
    let session_id = "multimodal-probe-unknown-type";

    let blocks = processor.process(&[att], true, session_id, "run-4").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text, .. } => {
            assert_eq!(text, "[Attachment: report.pdf (application/pdf)]");
        }
        other => panic!("expected Text summary block, got {:?}", other),
    }

    processor.cleanup(session_id);
}

/// 5. Mixed image and audio -> [Image block, Text transcript block]
#[tokio::test]
async fn test_mixed_image_and_audio() {
    let mock = MockTranscription("Transcribed audio".to_string());
    let processor = MediaProcessor::new(Some(Box::new(mock)), None);

    let jpeg_data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    let ogg_data = vec![0x4F, 0x67, 0x67, 0x53, 0x00, 0x02];

    let attachments = vec![
        make_attachment("img-mix", "image/jpeg", jpeg_data),
        make_attachment("aud-mix", "audio/ogg", ogg_data),
    ];
    let session_id = "multimodal-probe-mixed";

    let blocks = processor
        .process(&attachments, true, session_id, "run-5")
        .await;

    assert_eq!(blocks.len(), 2, "should produce one block per attachment");

    // First block: image (vision supported)
    match &blocks[0] {
        ContentBlock::Image { mime_type, .. } => {
            assert_eq!(mime_type, "image/jpeg");
        }
        other => panic!("expected Image block for first attachment, got {:?}", other),
    }

    // Second block: transcript text
    match &blocks[1] {
        ContentBlock::Text { text, .. } => {
            assert!(
                text.contains("Transcribed audio"),
                "expected transcript, got: {}",
                text
            );
        }
        other => panic!("expected Text block for audio attachment, got {:?}", other),
    }

    processor.cleanup(session_id);
}

/// 6. Download failure (no data/path/url) -> graceful error fallback
#[tokio::test]
async fn test_download_failure_graceful() {
    let processor = MediaProcessor::new(None, None);
    let att = Attachment {
        id: "bad-att".into(),
        mime_type: "image/jpeg".into(),
        filename: Some("broken.jpg".into()),
        size: None,
        url: None,
        path: None,
        data: None, // no source at all
    };
    let session_id = "multimodal-probe-download-fail";

    let blocks = processor.process(&[att], true, session_id, "run-6").await;

    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        ContentBlock::Text { text, .. } => {
            // The fallback names the file and the reason it is missing, so the
            // model can tell the user what failed rather than relaying an
            // opaque id.
            assert!(
                text.starts_with("[Image: broken.jpg (image/jpeg)"),
                "expected a self-describing failure summary, got: {}",
                text
            );
            assert!(
                text.contains("could not be retrieved"),
                "expected the failure reason, got: {}",
                text
            );
        }
        other => panic!("expected fallback Text block, got {:?}", other),
    }

    processor.cleanup(session_id);
}

/// 7. UnifiedMessage::user_with_content multimodal structure
#[tokio::test]
async fn test_openai_multimodal_serialization() {
    let content = vec![
        ContentBlock::Text {
            text: "Describe this image".to_string(),
            cache_control: None,
        },
        ContentBlock::Image {
            data: "aWZha2ViYXNlNjQ=".to_string(), // fake base64
            mime_type: "image/png".to_string(),
        },
    ];

    let msg = UnifiedMessage::user_with_content(content);

    // Verify structure
    match &msg {
        UnifiedMessage::User { content } => {
            assert_eq!(content.len(), 2, "should have 2 content blocks");

            match &content[0] {
                ContentBlock::Text { text, .. } => {
                    assert_eq!(text, "Describe this image");
                }
                other => panic!("first block should be Text, got {:?}", other),
            }

            match &content[1] {
                ContentBlock::Image { data, mime_type } => {
                    assert_eq!(data, "aWZha2ViYXNlNjQ=");
                    assert_eq!(mime_type, "image/png");
                }
                other => panic!("second block should be Image, got {:?}", other),
            }
        }
        other => panic!("expected User message, got {:?}", other),
    }

    // Verify JSON serialization round-trip
    let json = serde_json::to_string(&msg).expect("should serialize");
    assert!(json.contains("Describe this image"));
    assert!(json.contains("aWZha2ViYXNlNjQ="));
    let _: UnifiedMessage = serde_json::from_str(&json).expect("should deserialize");
}

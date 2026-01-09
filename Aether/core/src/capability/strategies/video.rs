//! Video capability strategy.
//!
//! This strategy extracts transcripts from YouTube videos found in user input.

use crate::capability::strategy::CapabilityStrategy;
use crate::config::VideoConfig;
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use crate::video::{extract_youtube_url, YouTubeExtractor};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Video capability strategy
///
/// Extracts transcripts from YouTube videos found in user input.
pub struct VideoStrategy {
    /// Video configuration
    video_config: Option<Arc<VideoConfig>>,
}

impl VideoStrategy {
    /// Create a new video strategy
    pub fn new(video_config: Option<Arc<VideoConfig>>) -> Self {
        Self { video_config }
    }

    /// Update the video configuration
    pub fn set_config(&mut self, config: Arc<VideoConfig>) {
        self.video_config = Some(config);
    }
}

#[async_trait]
impl CapabilityStrategy for VideoStrategy {
    fn capability_type(&self) -> Capability {
        Capability::Video
    }

    fn priority(&self) -> u32 {
        3 // Video executes last
    }

    fn is_available(&self) -> bool {
        // Video is always "available" but may be disabled in config
        true
    }

    fn validate_config(&self) -> Result<()> {
        // Config validation is optional - default config is always valid
        // max_transcript_length of 0 means no limit, which is valid
        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        // Video capability is always healthy if available
        // We could optionally test YouTube API connectivity here
        Ok(self.is_available())
    }

    fn status_info(&self) -> std::collections::HashMap<String, String> {
        let mut info = std::collections::HashMap::new();
        info.insert("capability".to_string(), "Video".to_string());
        info.insert("name".to_string(), "video".to_string());
        info.insert("priority".to_string(), "3".to_string());
        info.insert("available".to_string(), self.is_available().to_string());
        info.insert(
            "has_config".to_string(),
            self.video_config.is_some().to_string(),
        );
        if let Some(config) = &self.video_config {
            info.insert("enabled".to_string(), config.enabled.to_string());
            info.insert(
                "youtube_transcript".to_string(),
                config.youtube_transcript.to_string(),
            );
            info.insert(
                "max_transcript_length".to_string(),
                config.max_transcript_length.to_string(),
            );
        } else {
            info.insert("enabled".to_string(), "true".to_string());
        }
        info
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        // Use provided config or default
        let default_config = VideoConfig::default();
        let config = self
            .video_config
            .as_ref()
            .map(|c| c.as_ref())
            .unwrap_or(&default_config);

        if !config.enabled {
            debug!("Video capability disabled in config");
            return Ok(payload);
        }

        if !config.youtube_transcript {
            debug!("YouTube transcript extraction disabled in config");
            return Ok(payload);
        }

        // Extract YouTube URL from user input
        let Some(video_url) = extract_youtube_url(&payload.user_input) else {
            debug!("No YouTube URL found in user input");
            return Ok(payload);
        };

        info!(
            video_url = %video_url,
            "Found YouTube URL in user input, extracting transcript"
        );

        // Create extractor and fetch transcript
        let extractor = YouTubeExtractor::new(config.clone());

        match extractor.extract_transcript(&video_url).await {
            Ok(transcript) => {
                let formatted = transcript.format_for_context();
                info!(
                    video_id = %transcript.video_id,
                    title = %transcript.title,
                    segments = transcript.segments.len(),
                    truncated = transcript.was_truncated,
                    formatted_len = formatted.len(),
                    "Successfully extracted video transcript"
                );
                debug!(
                    preview = %formatted.chars().take(500).collect::<String>(),
                    "Transcript preview"
                );
                payload.context.video_transcript = Some(transcript);
            }
            Err(e) => {
                warn!(
                    error = %e,
                    video_url = %video_url,
                    "Failed to extract video transcript, continuing without it"
                );
                // Don't fail the request - continue without transcript
            }
        }

        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

    #[tokio::test]
    async fn test_video_strategy_available() {
        let strategy = VideoStrategy::new(None);
        assert!(strategy.is_available());
    }

    #[tokio::test]
    async fn test_video_strategy_no_url() {
        let strategy = VideoStrategy::new(None);

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Video],
                ContextFormat::Markdown,
            )
            .user_input("No video URL here".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.video_transcript.is_none());
    }

    #[tokio::test]
    async fn test_video_strategy_disabled() {
        let mut config = VideoConfig::default();
        config.enabled = false;

        let strategy = VideoStrategy::new(Some(Arc::new(config)));

        let anchor = ContextAnchor::new("com.app".to_string(), "App".to_string(), None);
        let payload = PayloadBuilder::new()
            .meta(Intent::GeneralChat, 1000, anchor)
            .config(
                "openai".to_string(),
                vec![Capability::Video],
                ContextFormat::Markdown,
            )
            .user_input("https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string())
            .build()
            .unwrap();

        let result = strategy.execute(payload).await.unwrap();
        assert!(result.context.video_transcript.is_none());
    }
}

//! YouTube transcript tool for extracting video transcripts
//!
//! Implements AetherTool trait for AI agent integration.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::config::VideoConfig;
use crate::error::Result;
use crate::tools::AetherTool;
use crate::video::{VideoTranscript, YouTubeExtractor};

use super::error::ToolError;

/// Arguments for YouTube tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct YouTubeArgs {
    /// YouTube video URL (supports youtube.com/watch?v=, youtu.be/, etc.)
    pub url: String,
    /// Preferred language for transcript (ISO 639-1 code, e.g., "en", "zh")
    #[serde(default = "default_language")]
    pub language: Option<String>,
}

fn default_language() -> Option<String> {
    Some("en".to_string())
}

/// YouTube transcript result
#[derive(Debug, Clone, Serialize)]
pub struct YouTubeResult {
    /// Video ID
    pub video_id: String,
    /// Video title
    pub title: String,
    /// Transcript language
    pub language: String,
    /// Formatted transcript for context
    pub transcript: String,
    /// Total duration in seconds
    pub duration_seconds: f64,
    /// Whether transcript was truncated
    pub was_truncated: bool,
}

impl From<VideoTranscript> for YouTubeResult {
    fn from(t: VideoTranscript) -> Self {
        let transcript = t.format_for_context();
        Self {
            video_id: t.video_id,
            title: t.title,
            language: t.language,
            transcript,
            duration_seconds: t.total_duration_seconds,
            was_truncated: t.was_truncated,
        }
    }
}

/// YouTube transcript extraction tool
pub struct YouTubeTool {
    config: VideoConfig,
}

impl YouTubeTool {
    /// Tool name constant
    pub const NAME: &'static str = "youtube";

    /// Tool description for AI
    pub const DESCRIPTION: &'static str =
        "Extract transcript from a YouTube video URL. Returns the video title and full transcript text. Use this when user provides a YouTube URL or asks about video content.";

    /// Create a new YouTubeTool with default config
    pub fn new() -> Self {
        Self {
            config: VideoConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: VideoConfig) -> Self {
        Self { config }
    }

    /// Execute transcript extraction (internal implementation)
    async fn call_impl(&self, args: YouTubeArgs) -> std::result::Result<YouTubeResult, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Notify tool start
        let url_display = if args.url.len() > 50 {
            format!("{}...", &args.url[..50])
        } else {
            args.url.clone()
        };
        notify_tool_start(Self::NAME, &format!("提取视频字幕: {}", url_display));

        info!("Extracting YouTube transcript: {}", args.url);

        // Create config with preferred language from args
        let mut config = self.config.clone();
        if let Some(lang) = args.language {
            config.preferred_language = lang;
        }

        let extractor = YouTubeExtractor::new(config);

        let transcript = extractor
            .extract_transcript(&args.url)
            .await
            .map_err(|e| {
                let error_msg = format!("Failed to extract transcript: {}", e);
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Execution(error_msg)
            })?;

        debug!(
            video_id = %transcript.video_id,
            title = %transcript.title,
            segments = transcript.segments.len(),
            "Transcript extracted successfully"
        );

        // Notify success
        let result_summary = format!(
            "已提取视频 \"{}\" 的字幕 ({} 秒)",
            transcript.title,
            transcript.total_duration_seconds as i64
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(YouTubeResult::from(transcript))
    }
}

impl Default for YouTubeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for YouTubeTool {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
        }
    }
}

/// Implementation of AetherTool trait for YouTubeTool
#[async_trait]
impl AetherTool for YouTubeTool {
    const NAME: &'static str = "youtube";
    const DESCRIPTION: &'static str =
        "Extract transcript from a YouTube video URL. Returns the video title and full transcript text. Use this when user provides a YouTube URL or asks about video content.";

    type Args = YouTubeArgs;
    type Output = YouTubeResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// rig::tool::Tool implementation required for ToolServer registration
impl rig::tool::Tool for YouTubeTool {
    const NAME: &'static str = "youtube";

    type Error = ToolError;
    type Args = YouTubeArgs;
    type Output = YouTubeResult;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        let schema = schemars::schema_for!(YouTubeArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_default();

        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> std::result::Result<Self::Output, Self::Error> {
        self.call_impl(args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AetherTool;

    #[test]
    fn test_youtube_args() {
        let args: YouTubeArgs =
            serde_json::from_str(r#"{"url": "https://youtube.com/watch?v=dQw4w9WgXcQ"}"#).unwrap();
        assert_eq!(args.url, "https://youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(args.language, Some("en".to_string())); // default
    }

    #[test]
    fn test_youtube_args_with_language() {
        let args: YouTubeArgs =
            serde_json::from_str(r#"{"url": "https://youtube.com/watch?v=xxx", "language": "zh"}"#)
                .unwrap();
        assert_eq!(args.language, Some("zh".to_string()));
    }

    #[test]
    fn test_youtube_tool_creation() {
        let tool = YouTubeTool::new();
        assert_eq!(YouTubeTool::NAME, "youtube");
        assert!(!YouTubeTool::DESCRIPTION.is_empty());
        drop(tool);
    }

    #[test]
    fn test_youtube_tool_definition() {
        let tool = YouTubeTool::new();
        // Test AetherTool::definition()
        let def = AetherTool::definition(&tool);

        assert_eq!(def.name, "youtube");
        assert!(!def.description.is_empty());
        assert!(def.parameters.is_object());
    }
}

//! YouTube transcript tool for extracting video transcripts
//!
//! Implements rig's Tool trait for AI agent integration.

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use crate::config::VideoConfig;
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

    /// Execute transcript extraction
    pub async fn call(&self, args: YouTubeArgs) -> Result<YouTubeResult, ToolError> {
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
            .map_err(|e| ToolError::Execution(format!("Failed to extract transcript: {}", e)))?;

        debug!(
            video_id = %transcript.video_id,
            title = %transcript.title,
            segments = transcript.segments.len(),
            "Transcript extracted successfully"
        );

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

/// Implementation of rig's Tool trait for YouTubeTool
impl Tool for YouTubeTool {
    const NAME: &'static str = "youtube";

    type Error = ToolError;
    type Args = YouTubeArgs;
    type Output = YouTubeResult;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let schema = schema_for!(YouTubeArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_else(|_| {
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "YouTube video URL"
                    },
                    "language": {
                        "type": "string",
                        "description": "Preferred transcript language (ISO 639-1)",
                        "default": "en"
                    }
                },
                "required": ["url"]
            })
        });

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        YouTubeTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_youtube_args() {
        let args: YouTubeArgs =
            serde_json::from_str(r#"{"url": "https://youtube.com/watch?v=dQw4w9WgXcQ"}"#).unwrap();
        assert_eq!(args.url, "https://youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(args.language, Some("en".to_string())); // default
    }

    #[test]
    fn test_youtube_args_with_language() {
        let args: YouTubeArgs = serde_json::from_str(
            r#"{"url": "https://youtube.com/watch?v=xxx", "language": "zh"}"#,
        )
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

    #[tokio::test]
    async fn test_youtube_tool_definition() {
        let tool = YouTubeTool::new();
        let def = tool.definition("test".to_string()).await;

        assert_eq!(def.name, "youtube");
        assert!(!def.description.is_empty());
        assert!(def.parameters.is_object());
    }
}

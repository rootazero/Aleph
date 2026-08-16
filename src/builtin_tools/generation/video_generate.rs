//! Video generation tool — generates videos from text descriptions.

use crate::sync_primitives::{Arc, RwLock};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::info;

use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::gateway::media::{detect_mime, MediaItem};
use crate::generation::{
    GenerationData, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::tools::AlephTool;

/// Arguments for the video generation tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VideoGenerateArgs {
    /// Text description of the video to generate
    pub prompt: String,
    /// Optional provider name (uses default video provider if not specified)
    pub provider: Option<String>,
    /// Optional aspect ratio (e.g. "16:9", "9:16", "1:1")
    pub aspect_ratio: Option<String>,
}

/// Output from the video generation tool.
#[derive(Debug, Clone, Serialize)]
pub struct VideoGenerateOutput {
    /// Human-readable display text (used by fast-path slash commands)
    pub _display: String,
    /// Media items for channel delivery
    pub _media: Vec<MediaItem>,
    /// Location of the generated video (URL or local file path)
    pub video_location: String,
    /// Type of location: "url" or "file"
    pub location_type: String,
    /// The prompt used for generation
    pub prompt: String,
    /// Provider that generated the video
    pub provider: String,
    /// Model used for generation
    pub model: Option<String>,
    /// Wall-clock time for generation in milliseconds
    pub duration_ms: u64,
}

/// Tool for generating videos from text descriptions.
pub struct VideoGenerateTool {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
}

impl VideoGenerateTool {
    pub const NAME: &'static str = "video_generate";
    pub const DESCRIPTION: &'static str =
        "Generate a video from a text description. Provide a detailed prompt describing the scene, motion, style, and camera movement.";

    pub const fn new(registry: Arc<RwLock<GenerationProviderRegistry>>) -> Self {
        Self { registry }
    }

    async fn call_impl(
        &self,
        args: VideoGenerateArgs,
    ) -> std::result::Result<VideoGenerateOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let start = Instant::now();

        // Truncate prompt for display (UTF-8 safe)
        let prompt_display = if args.prompt.chars().count() > 30 {
            let end = args
                .prompt
                .char_indices()
                .nth(30)
                .map_or(args.prompt.len(), |(i, _)| i);
            format!("{}...", &args.prompt[..end])
        } else {
            args.prompt.clone()
        };
        notify_tool_start(Self::NAME, &format!("生成视频: {prompt_display}"));

        info!(prompt = %args.prompt, provider = ?args.provider, "Starting video generation");

        // Acquire lock in scoped block — drop before await
        let (provider_name, provider) = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());

            if let Some(ref name) = args.provider {
                let p = reg.get(name).ok_or_else(|| {
                    let error_msg = format!("Video provider '{name}' not found");
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::InvalidArgs(error_msg)
                })?;
                if !p.supports(GenerationType::Video) {
                    let error_msg = format!("Provider '{name}' does not support video generation");
                    notify_tool_result(Self::NAME, &error_msg, false);
                    return Err(ToolError::InvalidArgs(error_msg));
                }
                (name.clone(), p)
            } else {
                reg.first_for_type(GenerationType::Video).ok_or_else(|| {
                    let error_msg = "No video generation provider available".to_string();
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::Execution(error_msg)
                })?
            }
            // Lock is dropped here at end of block
        };

        info!(provider = %provider_name, "Using video provider");

        // Build request
        let mut request = GenerationRequest::video(&args.prompt);
        if let Some(ref ar) = args.aspect_ratio {
            request.params.aspect_ratio = Some(ar.clone());
        }

        // Execute generation
        let output = provider.generate(request).await.map_err(|e| {
            let error_msg = format!("Video generation failed: {e}");
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::from(e)
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Process output
        let (video_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url"),
            GenerationData::LocalPath(path) => (path.clone(), "file"),
            GenerationData::Bytes(_) => {
                let error_msg =
                    "Video provider returned raw bytes — expected URL or file path".to_string();
                notify_tool_result(Self::NAME, &error_msg, false);
                return Err(ToolError::Execution(error_msg));
            }
        };

        // Notify success
        let result_summary = format!("视频生成完成 ({duration_ms} ms, provider: {provider_name})");
        notify_tool_result(Self::NAME, &result_summary, true);

        let display = format!(
            "🎬 视频已生成 ({:.1}s)\n{}",
            duration_ms as f64 / 1000.0,
            video_location
        );

        Ok(VideoGenerateOutput {
            _display: display,
            _media: vec![MediaItem {
                url: video_location.to_string(),
                media_type: "video".into(),
                mime_type: Some(detect_mime(&video_location, "video")),
                filename: None,
            }],
            video_location: video_location.to_string(),
            location_type: location_type.to_string(),
            prompt: args.prompt,
            provider: provider_name,
            model: output.metadata.model,
            duration_ms,
        })
    }
}

impl Clone for VideoGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

#[async_trait]
impl AlephTool for VideoGenerateTool {
    const NAME: &'static str = "video_generate";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;
    type Args = VideoGenerateArgs;
    type Output = VideoGenerateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

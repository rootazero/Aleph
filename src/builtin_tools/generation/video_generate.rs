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
        let start = Instant::now();

        info!(prompt = %args.prompt, provider = ?args.provider, "Starting video generation");

        // Find provider — lock must be dropped before any .await call.
        let (provider_name, provider) = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());

            if let Some(ref name) = args.provider {
                let p = reg.get(name).ok_or_else(|| {
                    ToolError::InvalidArgs(format!("Video provider '{name}' not found"))
                })?;
                if !p.supports(GenerationType::Video) {
                    return Err(ToolError::InvalidArgs(format!(
                        "Provider '{name}' does not support video generation"
                    )));
                }
                (name.clone(), p)
            } else {
                reg.first_for_type(GenerationType::Video).ok_or_else(|| {
                    ToolError::Execution("No video generation provider available".to_string())
                })?
            }
        };

        info!(provider = %provider_name, "Using video provider");

        // Build request
        let mut request = GenerationRequest::video(&args.prompt);
        request.params.aspect_ratio = args.aspect_ratio;

        let output = provider.generate(request).await.map_err(ToolError::from)?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let (video_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url"),
            GenerationData::LocalPath(path) => (path.clone(), "file"),
            GenerationData::Bytes(_) => {
                return Err(ToolError::Execution(
                    "Video provider returned raw bytes — expected URL or file path".to_string(),
                ));
            }
        };

        let display = format!(
            "🎬 视频已生成 ({:.1}s)\n{}",
            duration_ms as f64 / 1000.0,
            video_location
        );

        Ok(VideoGenerateOutput {
            _display: display,
            _media: vec![MediaItem {
                url: video_location.clone(),
                media_type: "video".into(),
                mime_type: Some(detect_mime(&video_location, "video")),
                filename: None,
            }],
            video_location,
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

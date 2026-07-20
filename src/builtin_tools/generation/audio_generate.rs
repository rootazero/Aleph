//! Audio/music generation tool — generates audio from text descriptions.

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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AudioGenerateArgs {
    /// Text description of the audio/music to generate
    pub prompt: String,
    /// Optional provider name (uses default audio provider if not specified)
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioGenerateOutput {
    /// Human-readable display text
    pub _display: String,
    /// Media items for channel delivery
    pub _media: Vec<MediaItem>,
    pub audio_location: String,
    pub location_type: String,
    pub prompt: String,
    pub provider: String,
    pub model: Option<String>,
    pub duration_ms: u64,
}

pub struct AudioGenerateTool {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
}

impl AudioGenerateTool {
    pub const NAME: &'static str = "audio_generate";
    pub const DESCRIPTION: &'static str =
        "Generate audio or music from a text description. Provide a prompt describing the genre, mood, instruments, tempo, and style.";

    pub const fn new(registry: Arc<RwLock<GenerationProviderRegistry>>) -> Self {
        Self { registry }
    }

    async fn call_impl(
        &self,
        args: AudioGenerateArgs,
    ) -> std::result::Result<AudioGenerateOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let start = Instant::now();

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
        notify_tool_start(Self::NAME, &format!("生成音频: {prompt_display}"));

        info!(prompt = %args.prompt, provider = ?args.provider, "Starting audio generation");

        let (provider_name, provider) = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());

            if let Some(ref name) = args.provider {
                let p = reg.get(name).ok_or_else(|| {
                    let error_msg = format!("Audio provider '{name}' not found");
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::InvalidArgs(error_msg)
                })?;
                if !p.supports(GenerationType::Audio) {
                    let error_msg = format!("Provider '{name}' does not support audio generation");
                    notify_tool_result(Self::NAME, &error_msg, false);
                    return Err(ToolError::InvalidArgs(error_msg));
                }
                (name.clone(), p)
            } else {
                reg.first_for_type(GenerationType::Audio).ok_or_else(|| {
                    let error_msg = "No audio generation provider available".to_string();
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::Execution(error_msg)
                })?
            }
        };

        info!(provider = %provider_name, "Using audio provider");

        let request = GenerationRequest::audio(&args.prompt);
        let output = provider.generate(request).await.map_err(|e| {
            let error_msg = format!("Audio generation failed: {e}");
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::from(e)
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let (audio_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url"),
            GenerationData::LocalPath(path) => (path.clone(), "file"),
            GenerationData::Bytes(_) => {
                let error_msg =
                    "Audio provider returned raw bytes — expected URL or file path".to_string();
                notify_tool_result(Self::NAME, &error_msg, false);
                return Err(ToolError::Execution(error_msg));
            }
        };

        let result_summary = format!("音频生成完成 ({duration_ms} ms, provider: {provider_name})");
        notify_tool_result(Self::NAME, &result_summary, true);

        let display = format!(
            "🎵 音频已生成 ({:.1}s)\n{}",
            duration_ms as f64 / 1000.0,
            audio_location
        );

        Ok(AudioGenerateOutput {
            _display: display,
            _media: vec![MediaItem {
                url: audio_location.to_string(),
                media_type: "audio".into(),
                mime_type: Some(detect_mime(&audio_location, "audio")),
                filename: None,
            }],
            audio_location: audio_location.to_string(),
            location_type: location_type.to_string(),
            prompt: args.prompt,
            provider: provider_name,
            model: output.metadata.model,
            duration_ms,
        })
    }
}

impl Clone for AudioGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

#[async_trait]
impl AlephTool for AudioGenerateTool {
    const NAME: &'static str = "audio_generate";
    const DESCRIPTION: &'static str =
        "Generate audio or music from a text description. Provide a prompt describing the genre, mood, instruments, tempo, and style.";
    type Args = AudioGenerateArgs;
    type Output = AudioGenerateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

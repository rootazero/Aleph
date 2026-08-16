//! Speech generation tool (Text-to-Speech)
//!
//! Generates speech audio from text using configured AI providers.
//! Implements `AlephTool` trait for AI agent integration.

use crate::sync_primitives::{Arc, RwLock};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{debug, info};

use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::gateway::media::{detect_mime, MediaItem};
use crate::generation::{
    GenerationParams, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::tools::AlephTool;

/// Arguments for speech generation
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SpeechGenerateArgs {
    /// The text to convert to speech
    pub text: String,

    /// Voice to use (provider-specific, e.g., "alloy", "echo", "fable")
    #[serde(default)]
    pub voice: Option<String>,

    /// Speaking speed (0.25 to 4.0, default: 1.0)
    #[serde(default)]
    pub speed: Option<f32>,

    /// Output audio format: "mp3", "opus", "aac", "flac", "wav" (default: "mp3")
    #[serde(default)]
    pub format: Option<String>,

    /// Provider name to use (default: first available speech provider)
    #[serde(default)]
    pub provider: Option<String>,
}

/// Output from speech generation tool
#[derive(Debug, Clone, Serialize)]
pub struct SpeechGenerateOutput {
    /// Human-readable display text
    pub _display: String,

    /// Media items for channel delivery
    pub _media: Vec<MediaItem>,

    /// Location of the generated audio (URL, file path, or data URL)
    pub audio_location: String,

    /// Type of location: "url", "file", or "`data_url`"
    pub location_type: String,

    /// Original text that was converted
    pub text: String,

    /// Voice used for generation
    pub voice: Option<String>,

    /// Audio format
    pub format: String,

    /// Provider that generated the speech
    pub provider: String,

    /// Audio file size in bytes (if available)
    pub size_bytes: Option<u64>,

    /// Generation duration in milliseconds
    pub duration_ms: u64,
}

/// Speech generation tool using `GenerationProviderRegistry`
pub struct SpeechGenerateTool {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
}

/// Voice ids echoed back when synthesis fails on an unpublished voice. Azure
/// ships hundreds; a truncated list plus the remaining count is enough for the
/// model to self-correct without flooding the turn.
const MAX_LISTED_VOICES: usize = 24;

/// Render a provider's voice catalog as a hint appended to a failed synthesis.
///
/// A2 (compress errors into context, don't pick a recovery): the harness does
/// **not** gate on the voice id up front — `openai_tts` deliberately allows
/// unknown ids so newly-released voices and third-party proxies keep working, and
/// pre-rejecting would regress that. But once synthesis has actually failed with
/// a voice the provider does not publish, that catalog is precisely what the
/// model needs to fix its next call, so it rides along on the error.
fn unpublished_voice_hint(
    provider_name: &str,
    voice: Option<&str>,
    catalog: &[crate::generation::VoiceInfo],
) -> Option<String> {
    // An empty catalog means "unknown", not "none valid" (BYO `local`, or an
    // OpenAI-compatible proxy fronting some other engine) — say nothing.
    let voice = voice?;
    if catalog.is_empty() || catalog.iter().any(|v| v.id.eq_ignore_ascii_case(voice)) {
        return None;
    }
    let listed: Vec<&str> = catalog
        .iter()
        .take(MAX_LISTED_VOICES)
        .map(|v| v.id.as_str())
        .collect();
    let more = catalog.len().saturating_sub(listed.len());
    let tail = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    Some(format!(
        " Voice '{voice}' is not in the catalog published by '{provider_name}': {}{tail}",
        listed.join(", ")
    ))
}

impl SpeechGenerateTool {
    /// Tool identifier
    pub const NAME: &'static str = "speech_generate";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Convert text to speech audio. Use this when you need to generate spoken audio from text content.";

    /// Create a new `SpeechGenerateTool` with the given provider registry
    pub const fn new(registry: Arc<RwLock<GenerationProviderRegistry>>) -> Self {
        Self { registry }
    }

    /// Validate the arguments
    fn validate_args(args: &SpeechGenerateArgs) -> std::result::Result<(), ToolError> {
        // Validate text is not empty
        if args.text.trim().is_empty() {
            return Err(ToolError::InvalidArgs("Text cannot be empty".to_string()));
        }

        // Validate speed range (0.25 to 4.0)
        if let Some(speed) = args.speed {
            if !(0.25..=4.0).contains(&speed) {
                return Err(ToolError::InvalidArgs(format!(
                    "Speed must be between 0.25 and 4.0, got {speed}"
                )));
            }
        }

        Ok(())
    }

    /// Execute speech generation (internal implementation)
    async fn call_impl(
        &self,
        args: SpeechGenerateArgs,
    ) -> std::result::Result<SpeechGenerateOutput, ToolError> {
        // Validate arguments first
        Self::validate_args(&args)?;

        let start = Instant::now();

        info!(
            text_length = args.text.len(),
            voice = ?args.voice,
            provider = ?args.provider,
            "Starting speech generation"
        );

        // Find provider — lock must be dropped before any .await call. The voice
        // catalog is snapshotted under the same lock (it is a sync trait call)
        // so a failed synthesis can name the valid ids without re-locking.
        let (provider_name, provider, voice_catalog) = {
            let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());

            let (name, provider) = if let Some(name) = &args.provider {
                let provider = reg.get(name).ok_or_else(|| {
                    ToolError::InvalidArgs(format!("Provider '{name}' not found"))
                })?;

                // Check if provider supports speech generation
                if !provider.supports(GenerationType::Speech) {
                    return Err(ToolError::InvalidArgs(format!(
                        "Provider '{name}' does not support speech generation"
                    )));
                }

                (name.clone(), provider)
            } else {
                // Find first provider that supports speech generation
                reg.first_for_type(GenerationType::Speech).ok_or_else(|| {
                    ToolError::InvalidArgs("No speech generation provider available".to_string())
                })?
            };
            let catalog = reg.get_voices_for_provider(&name);
            (name, provider, catalog)
        };

        debug!(provider = %provider_name, "Using provider for speech generation");

        // Build generation parameters
        let mut params = GenerationParams::new();
        if let Some(voice) = args.voice.clone() {
            params.voice = Some(voice);
        }
        if let Some(speed) = args.speed {
            params.speed = Some(speed);
        }
        if let Some(format) = args.format.clone() {
            params.format = Some(format);
        }

        // Create generation request
        let request = GenerationRequest::speech(&args.text).with_params(params);

        // Execute generation. A failure carrying a voice the provider does not
        // publish gets the catalog attached so the model can self-correct.
        let output =
            provider.generate(request).await.map_err(|e| {
                match unpublished_voice_hint(&provider_name, args.voice.as_deref(), &voice_catalog)
                {
                    Some(hint) => ToolError::Execution(format!("{e}.{hint}")),
                    None => ToolError::from(e),
                }
            })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Determine location and type from the generation data
        let (audio_location, location_type, size_bytes) = match &output.data {
            crate::generation::GenerationData::Url(url) => {
                (url.clone(), "url".to_string(), output.metadata.size_bytes)
            }
            crate::generation::GenerationData::LocalPath(path) => {
                (path.clone(), "file".to_string(), output.metadata.size_bytes)
            }
            crate::generation::GenerationData::Bytes(bytes) => {
                // Convert bytes to base64 data URL
                use base64::Engine;
                let base64_data = base64::engine::general_purpose::STANDARD.encode(bytes);
                let content_type = output
                    .metadata
                    .content_type
                    .as_deref()
                    .unwrap_or("audio/mpeg");
                let data_url = format!("data:{content_type};base64,{base64_data}");
                let size = bytes.len() as u64;
                (data_url, "data_url".to_string(), Some(size))
            }
        };

        // Determine format from metadata or args
        let format = output
            .metadata
            .content_type
            .as_ref()
            .map_or_else(
                || args.format.as_deref().unwrap_or("mp3"),
                |ct| {
                    // Extract format from content type (e.g., "audio/mpeg" -> "mp3")
                    match ct.as_str() {
                        "audio/mpeg" => "mp3",
                        "audio/opus" => "opus",
                        "audio/aac" => "aac",
                        "audio/flac" => "flac",
                        "audio/wav" => "wav",
                        _ => "mp3",
                    }
                },
            )
            .to_string();

        info!(
            provider = %provider_name,
            duration_ms = duration_ms,
            location_type = %location_type,
            format = %format,
            "Speech generation completed"
        );

        let display = if location_type == "data_url" {
            format!(
                "🔊 语音已生成 ({:.1}s) [{} format, {} bytes]",
                duration_ms as f64 / 1000.0,
                format,
                size_bytes.unwrap_or(0)
            )
        } else {
            format!(
                "🔊 语音已生成 ({:.1}s)\n{}",
                duration_ms as f64 / 1000.0,
                audio_location
            )
        };

        Ok(SpeechGenerateOutput {
            _display: display,
            _media: vec![MediaItem {
                url: audio_location.clone(),
                media_type: "audio".into(),
                mime_type: Some(detect_mime(&audio_location, "audio")),
                filename: None,
            }],
            audio_location,
            location_type,
            text: args.text,
            voice: args.voice,
            format,
            provider: provider_name,
            size_bytes,
            duration_ms,
        })
    }
}

impl Clone for SpeechGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

/// Implementation of `AlephTool` trait for `SpeechGenerateTool`
#[async_trait]
impl AlephTool for SpeechGenerateTool {
    const NAME: &'static str = "speech_generate";
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

    type Args = SpeechGenerateArgs;
    type Output = SpeechGenerateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::MockGenerationProvider;
    use crate::tools::AlephTool;

    fn create_test_registry() -> Arc<RwLock<GenerationProviderRegistry>> {
        let mut registry = GenerationProviderRegistry::new();
        // MockGenerationProvider::new supports Image and Speech by default
        let mock = Arc::new(MockGenerationProvider::new("mock-tts"));
        registry.register("mock-tts".to_string(), mock).unwrap();
        Arc::new(RwLock::new(registry))
    }

    #[test]
    fn test_args_deserialization_minimal() {
        let json = r#"{"text": "Hello, world!"}"#;
        let args: SpeechGenerateArgs = serde_json::from_str(json).unwrap();

        assert_eq!(args.text, "Hello, world!");
        assert!(args.voice.is_none());
        assert!(args.speed.is_none());
        assert!(args.format.is_none());
        assert!(args.provider.is_none());
    }

    #[test]
    fn test_args_deserialization_full() {
        let json = r#"{
            "text": "This is a test.",
            "voice": "alloy",
            "speed": 1.5,
            "format": "opus",
            "provider": "openai"
        }"#;
        let args: SpeechGenerateArgs = serde_json::from_str(json).unwrap();

        assert_eq!(args.text, "This is a test.");
        assert_eq!(args.voice, Some("alloy".to_string()));
        assert_eq!(args.speed, Some(1.5));
        assert_eq!(args.format, Some("opus".to_string()));
        assert_eq!(args.provider, Some("openai".to_string()));
    }

    #[test]
    fn test_validate_empty_text() {
        let args = SpeechGenerateArgs {
            text: "   ".to_string(),
            voice: None,
            speed: None,
            format: None,
            provider: None,
        };

        let result = SpeechGenerateTool::validate_args(&args);
        assert!(result.is_err());

        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("empty"));
        } else {
            panic!("Expected InvalidArgs error");
        }
    }

    #[test]
    fn test_validate_speed_too_low() {
        let args = SpeechGenerateArgs {
            text: "Valid text".to_string(),
            voice: None,
            speed: Some(0.1), // Too low
            format: None,
            provider: None,
        };

        let result = SpeechGenerateTool::validate_args(&args);
        assert!(result.is_err());

        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("0.25"));
        } else {
            panic!("Expected InvalidArgs error");
        }
    }

    #[test]
    fn test_validate_speed_too_high() {
        let args = SpeechGenerateArgs {
            text: "Valid text".to_string(),
            voice: None,
            speed: Some(5.0), // Too high
            format: None,
            provider: None,
        };

        let result = SpeechGenerateTool::validate_args(&args);
        assert!(result.is_err());

        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("4.0"));
        } else {
            panic!("Expected InvalidArgs error");
        }
    }

    #[test]
    fn test_validate_speed_valid() {
        let args = SpeechGenerateArgs {
            text: "Valid text".to_string(),
            voice: None,
            speed: Some(1.5), // Valid
            format: None,
            provider: None,
        };

        let result = SpeechGenerateTool::validate_args(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_definition() {
        assert_eq!(SpeechGenerateTool::NAME, "speech_generate");
        assert!(!SpeechGenerateTool::DESCRIPTION.is_empty());
    }

    #[tokio::test]
    async fn test_generate_speech_success() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "Hello, this is a test.".to_string(),
            voice: Some("alloy".to_string()),
            speed: Some(1.0),
            format: Some("mp3".to_string()),
            provider: Some("mock-tts".to_string()),
        };

        // Use fully qualified syntax
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.text, "Hello, this is a test.");
        assert_eq!(output.provider, "mock-tts");
        assert_eq!(output.location_type, "url");
        // duration_ms is set correctly (it's u64, so always >= 0)
    }

    #[tokio::test]
    async fn test_generate_speech_provider_not_found() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "Test text".to_string(),
            voice: None,
            speed: None,
            format: None,
            provider: Some("nonexistent".to_string()),
        };

        // Use fully qualified syntax
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());

        // Error is now AlephError
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("not found"),
            "Error should contain 'not found': {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_speech_no_provider_available() {
        let registry = Arc::new(RwLock::new(GenerationProviderRegistry::new()));
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "Test text".to_string(),
            voice: None,
            speed: None,
            format: None,
            provider: None,
        };

        // Use fully qualified syntax
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());

        // Error is now AlephError
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("No speech generation provider"),
            "Error should contain 'No speech generation provider': {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_speech_validation_fails() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "".to_string(), // Empty text should fail validation
            voice: None,
            speed: None,
            format: None,
            provider: Some("mock-tts".to_string()),
        };

        // Use fully qualified syntax
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());

        // Error is now AlephError
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("empty"),
            "Error should contain 'empty': {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_generate_speech_auto_select_provider() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "Auto-selected provider test".to_string(),
            voice: None,
            speed: None,
            format: None,
            provider: None, // Let it auto-select
        };

        // Use fully qualified syntax
        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.provider, "mock-tts");
    }

    #[tokio::test]
    async fn test_aleph_tool_trait() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        // Test definition via AlephTool trait
        let definition = AlephTool::definition(&tool);
        assert_eq!(definition.name, "speech_generate");
        assert!(!definition.description.is_empty());

        // Test call via AlephTool trait
        let args = SpeechGenerateArgs {
            text: "Trait test".to_string(),
            voice: None,
            speed: None,
            format: None,
            provider: Some("mock-tts".to_string()),
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // Voice-catalog hint (connects `registry.get_voices_for_provider`, which had
    // no consumer at all, to the failed-synthesis path)
    // -----------------------------------------------------------------------

    fn catalog(ids: &[&str]) -> Vec<crate::generation::VoiceInfo> {
        ids.iter()
            .map(|id| crate::generation::VoiceInfo {
                id: (*id).to_string(),
                name: (*id).to_string(),
                gender: "neutral".into(),
                description: String::new(),
            })
            .collect()
    }

    #[test]
    fn hint_names_the_valid_voices_for_an_unpublished_id() {
        let hint =
            unpublished_voice_hint("openai_tts", Some("rachel"), &catalog(&["alloy", "nova"]))
                .expect("an unpublished voice must produce a hint");
        assert!(hint.contains("rachel"));
        assert!(hint.contains("alloy"));
        assert!(hint.contains("nova"));
    }

    #[test]
    fn hint_is_silent_for_a_published_voice() {
        // Case-insensitive: the failure was something else entirely (network,
        // auth) and must not be mislabelled as a voice problem.
        assert!(unpublished_voice_hint("p", Some("Alloy"), &catalog(&["alloy"])).is_none());
    }

    #[test]
    fn hint_is_silent_without_a_catalog_or_a_voice() {
        // Empty catalog = "unknown", not "none valid" (BYO local, compat proxies).
        assert!(unpublished_voice_hint("local", Some("zf_088"), &[]).is_none());
        assert!(unpublished_voice_hint("p", None, &catalog(&["alloy"])).is_none());
    }

    #[test]
    fn hint_truncates_a_huge_catalog() {
        let ids: Vec<String> = (0..200).map(|i| format!("v{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let hint = unpublished_voice_hint("azure_speech", Some("nope"), &catalog(&refs)).unwrap();
        assert!(hint.contains("+176 more"), "hint was: {hint}");
    }

    #[test]
    fn test_output_serialization() {
        let output = SpeechGenerateOutput {
            _display: "🔊 语音已生成 (1.5s)\nhttps://example.com/audio.mp3".to_string(),
            _media: vec![],
            audio_location: "https://example.com/audio.mp3".to_string(),
            location_type: "url".to_string(),
            text: "Hello world".to_string(),
            voice: Some("alloy".to_string()),
            format: "mp3".to_string(),
            provider: "openai".to_string(),
            size_bytes: Some(12345),
            duration_ms: 500,
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("audio_location"));
        assert!(json.contains("https://example.com/audio.mp3"));
        assert!(json.contains("voice"));
        assert!(json.contains("alloy"));
    }
}

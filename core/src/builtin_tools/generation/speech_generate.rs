//! Speech generation tool (Text-to-Speech)
//!
//! Generates speech audio from text using configured AI providers.
//! Implements AlephTool trait for AI agent integration.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

use crate::error::Result;
use crate::generation::{
    GenerationParams, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::builtin_tools::error::ToolError;
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
    /// Location of the generated audio (URL, file path, or data URL)
    pub audio_location: String,

    /// Type of location: "url", "file", or "data_url"
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

/// Speech generation tool using GenerationProviderRegistry
pub struct SpeechGenerateTool {
    registry: Arc<GenerationProviderRegistry>,
}

impl SpeechGenerateTool {
    /// Tool identifier
    pub const NAME: &'static str = "generate_speech";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Convert text to speech audio. Use this when you need to generate spoken audio from text content.";

    /// Create a new SpeechGenerateTool with the given provider registry
    pub fn new(registry: Arc<GenerationProviderRegistry>) -> Self {
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
                    "Speed must be between 0.25 and 4.0, got {}",
                    speed
                )));
            }
        }

        Ok(())
    }

    /// Execute speech generation (internal implementation)
    async fn call_impl(&self, args: SpeechGenerateArgs) -> std::result::Result<SpeechGenerateOutput, ToolError> {
        // Validate arguments first
        Self::validate_args(&args)?;

        let start = Instant::now();

        info!(
            text_length = args.text.len(),
            voice = ?args.voice,
            provider = ?args.provider,
            "Starting speech generation"
        );

        // Find provider
        let (provider_name, provider) = if let Some(name) = &args.provider {
            let provider = self
                .registry
                .get(name)
                .ok_or_else(|| ToolError::InvalidArgs(format!("Provider '{}' not found", name)))?;

            // Check if provider supports speech generation
            if !provider.supports(GenerationType::Speech) {
                return Err(ToolError::InvalidArgs(format!(
                    "Provider '{}' does not support speech generation",
                    name
                )));
            }

            (name.clone(), provider)
        } else {
            // Find first provider that supports speech generation
            self.registry
                .first_for_type(GenerationType::Speech)
                .ok_or_else(|| {
                    ToolError::InvalidArgs("No speech generation provider available".to_string())
                })?
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

        // Execute generation
        let output = provider.generate(request).await.map_err(ToolError::from)?;

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
                let data_url = format!("data:{};base64,{}", content_type, base64_data);
                let size = bytes.len() as u64;
                (data_url, "data_url".to_string(), Some(size))
            }
        };

        // Determine format from metadata or args
        let format = output
            .metadata
            .content_type
            .as_ref()
            .map(|ct| {
                // Extract format from content type (e.g., "audio/mpeg" -> "mp3")
                match ct.as_str() {
                    "audio/mpeg" => "mp3",
                    "audio/opus" => "opus",
                    "audio/aac" => "aac",
                    "audio/flac" => "flac",
                    "audio/wav" => "wav",
                    _ => "mp3",
                }
            })
            .unwrap_or_else(|| args.format.as_deref().unwrap_or("mp3"))
            .to_string();

        info!(
            provider = %provider_name,
            duration_ms = duration_ms,
            location_type = %location_type,
            format = %format,
            "Speech generation completed"
        );

        Ok(SpeechGenerateOutput {
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

/// Implementation of AlephTool trait for SpeechGenerateTool
#[async_trait]
impl AlephTool for SpeechGenerateTool {
    const NAME: &'static str = "generate_speech";
    const DESCRIPTION: &'static str = "Convert text to speech audio. Use this when you need to generate spoken audio from text content.";

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

    fn create_test_registry() -> Arc<GenerationProviderRegistry> {
        let mut registry = GenerationProviderRegistry::new();
        // MockGenerationProvider::new supports Image and Speech by default
        let mock = Arc::new(MockGenerationProvider::new("mock-tts"));
        registry.register("mock-tts".to_string(), mock).unwrap();
        Arc::new(registry)
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
        assert_eq!(SpeechGenerateTool::NAME, "generate_speech");
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
        assert!(err_msg.contains("not found"), "Error should contain 'not found': {}", err_msg);
    }

    #[tokio::test]
    async fn test_generate_speech_no_provider_available() {
        let registry = Arc::new(GenerationProviderRegistry::new());
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
        assert!(err_msg.contains("No speech generation provider"), "Error should contain 'No speech generation provider': {}", err_msg);
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
        assert!(err_msg.contains("empty"), "Error should contain 'empty': {}", err_msg);
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
        assert_eq!(definition.name, "generate_speech");
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

    #[test]
    fn test_output_serialization() {
        let output = SpeechGenerateOutput {
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

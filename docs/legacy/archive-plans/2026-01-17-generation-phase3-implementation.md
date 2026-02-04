# Phase 3: 生成工具集成实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 将生成供应商集成到 Aleph 工具系统，使 AI agent 和用户可以通过工具调用生成媒体

**Architecture:** 创建 `ImageGenerateTool` 和 `SpeechGenerateTool` 实现 rig 的 `Tool` trait，并在 `ToolRegistry` 中注册为 builtin 工具

**Tech Stack:** Rust, rig-core (Tool trait), schemars, serde

---

## Task 1: 创建 generation tool 模块结构

**Files:**
- Create: `Aleph/core/src/rig_tools/generation.rs`
- Modify: `Aleph/core/src/rig_tools/mod.rs`

**Step 1: Create the generation tool module**

Create `src/rig_tools/generation.rs`:

```rust
//! Media generation tools
//!
//! Tools for generating images, speech, and other media using AI providers.
//!
//! # Available Tools
//!
//! - [`ImageGenerateTool`] - Generate images from text prompts (DALL·E, etc.)
//! - [`SpeechGenerateTool`] - Generate speech from text (OpenAI TTS, etc.)

use crate::generation::{
    GenerationData, GenerationError, GenerationOutput, GenerationParams,
    GenerationProvider, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::rig_tools::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error, info};

// Re-export tools
mod image_generate;
mod speech_generate;

pub use image_generate::ImageGenerateTool;
pub use speech_generate::SpeechGenerateTool;

/// Convert GenerationError to ToolError
impl From<GenerationError> for ToolError {
    fn from(err: GenerationError) -> Self {
        match err {
            GenerationError::NetworkError { message } => ToolError::Network(message),
            GenerationError::InvalidParametersError { message, .. } => ToolError::InvalidArgs(message),
            GenerationError::AuthenticationError { message, .. } => ToolError::InvalidArgs(message),
            _ => ToolError::Execution(err.to_string()),
        }
    }
}
```

**Step 2: Update rig_tools/mod.rs**

Add to `src/rig_tools/mod.rs`:
```rust
pub mod generation;

pub use generation::{ImageGenerateTool, SpeechGenerateTool};
```

**Step 3: Run cargo check**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo check`
Expected: Compile errors (modules not yet created)

**Step 4: Commit (after Task 2-3 are done)**

---

## Task 2: 实现 ImageGenerateTool

**Files:**
- Create: `Aleph/core/src/rig_tools/generation/image_generate.rs`

**Step 1: Create ImageGenerateTool with args, output, and Tool impl**

Create `src/rig_tools/generation/image_generate.rs`:

```rust
//! Image generation tool
//!
//! Implements rig's Tool trait for AI-callable image generation.

use crate::generation::{
    providers::create_provider, GenerationData, GenerationOutput, GenerationParams,
    GenerationProvider, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::rig_tools::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Arguments for image generation tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImageGenerateArgs {
    /// The text prompt describing the image to generate
    pub prompt: String,

    /// Image width in pixels (default: 1024)
    #[serde(default = "default_width")]
    pub width: u32,

    /// Image height in pixels (default: 1024)
    #[serde(default = "default_height")]
    pub height: u32,

    /// Quality level: "standard" or "hd" (default: "standard")
    #[serde(default = "default_quality")]
    pub quality: String,

    /// Style: "vivid" or "natural" (default: "vivid")
    #[serde(default = "default_style")]
    pub style: String,

    /// Provider name to use (optional, uses default if not specified)
    #[serde(default)]
    pub provider: Option<String>,
}

fn default_width() -> u32 { 1024 }
fn default_height() -> u32 { 1024 }
fn default_quality() -> String { "standard".to_string() }
fn default_style() -> String { "vivid".to_string() }

/// Output from image generation tool
#[derive(Debug, Clone, Serialize)]
pub struct ImageGenerateOutput {
    /// URL or local path to the generated image
    pub image_location: String,
    /// Type of location: "url" or "local_path"
    pub location_type: String,
    /// The original prompt used
    pub prompt: String,
    /// The revised prompt if the provider modified it
    pub revised_prompt: Option<String>,
    /// Provider that generated the image
    pub provider: String,
    /// Model used for generation
    pub model: Option<String>,
    /// Generation duration in milliseconds
    pub duration_ms: Option<u64>,
}

/// Image generation tool using GenerationProviderRegistry
pub struct ImageGenerateTool {
    /// Registry of available generation providers
    registry: Arc<GenerationProviderRegistry>,
    /// Default provider name
    default_provider: Option<String>,
}

impl ImageGenerateTool {
    /// Tool identifier
    pub const NAME: &'static str = "generate_image";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Generate images from text descriptions using AI models like DALL-E 3. \
         Returns the URL or path to the generated image.";

    /// Create a new ImageGenerateTool with a provider registry
    pub fn new(registry: Arc<GenerationProviderRegistry>) -> Self {
        Self {
            registry,
            default_provider: None,
        }
    }

    /// Create with a specific default provider
    pub fn with_default_provider(mut self, provider: String) -> Self {
        self.default_provider = Some(provider);
        self
    }

    /// Execute image generation
    pub async fn call(&self, args: ImageGenerateArgs) -> Result<ImageGenerateOutput, ToolError> {
        info!(prompt = %args.prompt, "Generating image");

        // Determine which provider to use
        let provider_name = args.provider
            .as_ref()
            .or(self.default_provider.as_ref())
            .cloned();

        // Get provider from registry
        let provider = if let Some(name) = &provider_name {
            self.registry
                .get(name)
                .ok_or_else(|| ToolError::InvalidArgs(format!("Provider '{}' not found", name)))?
        } else {
            // Get first available image provider
            self.registry
                .filter_by_type(GenerationType::Image)
                .first()
                .cloned()
                .ok_or_else(|| ToolError::InvalidArgs("No image generation provider available".to_string()))?
        };

        // Build generation request
        let params = GenerationParams::builder()
            .width(args.width)
            .height(args.height)
            .quality(&args.quality)
            .style(&args.style)
            .build();

        let request = GenerationRequest::image(&args.prompt)
            .with_params(params);

        debug!(provider = %provider.name(), "Using provider for image generation");

        // Execute generation
        let output = provider.generate(request).await.map_err(|e| {
            error!(error = %e, "Image generation failed");
            ToolError::from(e)
        })?;

        // Extract location info
        let (image_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url".to_string()),
            GenerationData::LocalPath(path) => (path.clone(), "local_path".to_string()),
            GenerationData::Bytes(_) => {
                // For bytes, we'd need to save to a temp file or encode to base64
                // For now, return as base64 data URL
                return Err(ToolError::Execution(
                    "Raw bytes output not yet supported in tool".to_string()
                ));
            }
        };

        let duration_ms = output.metadata.duration.map(|d| d.as_millis() as u64);

        info!(
            location = %image_location,
            provider = %provider.name(),
            duration_ms = ?duration_ms,
            "Image generated successfully"
        );

        Ok(ImageGenerateOutput {
            image_location,
            location_type,
            prompt: args.prompt,
            revised_prompt: output.metadata.revised_prompt,
            provider: provider.name().to_string(),
            model: output.metadata.model,
            duration_ms,
        })
    }
}

impl Clone for ImageGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            default_provider: self.default_provider.clone(),
        }
    }
}

impl Tool for ImageGenerateTool {
    const NAME: &'static str = "generate_image";

    type Error = ToolError;
    type Args = ImageGenerateArgs;
    type Output = ImageGenerateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let schema = schema_for!(ImageGenerateArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_else(|_| {
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Text description of the image to generate"
                    },
                    "width": {
                        "type": "integer",
                        "description": "Image width in pixels",
                        "default": 1024
                    },
                    "height": {
                        "type": "integer",
                        "description": "Image height in pixels",
                        "default": 1024
                    },
                    "quality": {
                        "type": "string",
                        "enum": ["standard", "hd"],
                        "description": "Quality level",
                        "default": "standard"
                    },
                    "style": {
                        "type": "string",
                        "enum": ["vivid", "natural"],
                        "description": "Image style",
                        "default": "vivid"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Optional: specific provider to use"
                    }
                },
                "required": ["prompt"]
            })
        });

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        ImageGenerateTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::MockGenerationProvider;

    fn create_test_registry() -> Arc<GenerationProviderRegistry> {
        let registry = GenerationProviderRegistry::new();
        let mock = Arc::new(MockGenerationProvider::image_only("mock-dalle"));
        registry.register("mock-dalle", mock);
        Arc::new(registry)
    }

    #[test]
    fn test_args_deserialization_defaults() {
        let args: ImageGenerateArgs = serde_json::from_str(r#"{"prompt": "A cat"}"#).unwrap();
        assert_eq!(args.prompt, "A cat");
        assert_eq!(args.width, 1024);
        assert_eq!(args.height, 1024);
        assert_eq!(args.quality, "standard");
        assert_eq!(args.style, "vivid");
        assert!(args.provider.is_none());
    }

    #[test]
    fn test_args_deserialization_custom() {
        let args: ImageGenerateArgs = serde_json::from_str(r#"{
            "prompt": "A sunset",
            "width": 1792,
            "height": 1024,
            "quality": "hd",
            "style": "natural",
            "provider": "dalle3"
        }"#).unwrap();

        assert_eq!(args.width, 1792);
        assert_eq!(args.height, 1024);
        assert_eq!(args.quality, "hd");
        assert_eq!(args.style, "natural");
        assert_eq!(args.provider, Some("dalle3".to_string()));
    }

    #[test]
    fn test_tool_metadata() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        assert_eq!(ImageGenerateTool::NAME, "generate_image");
        assert!(!ImageGenerateTool::DESCRIPTION.is_empty());
    }

    #[tokio::test]
    async fn test_tool_definition() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        let def = tool.definition("".to_string()).await;

        assert_eq!(def.name, "generate_image");
        assert!(!def.description.is_empty());
        assert!(def.parameters.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_generate_image_success() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        let args = ImageGenerateArgs {
            prompt: "A beautiful sunset".to_string(),
            width: 1024,
            height: 1024,
            quality: "standard".to_string(),
            style: "vivid".to_string(),
            provider: Some("mock-dalle".to_string()),
        };

        let result = tool.call(args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.prompt, "A beautiful sunset");
        assert_eq!(output.provider, "mock-dalle");
        assert_eq!(output.location_type, "url");
    }

    #[tokio::test]
    async fn test_generate_image_provider_not_found() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        let args = ImageGenerateArgs {
            prompt: "Test".to_string(),
            width: 1024,
            height: 1024,
            quality: "standard".to_string(),
            style: "vivid".to_string(),
            provider: Some("nonexistent".to_string()),
        };

        let result = tool.call(args).await;
        assert!(result.is_err());
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test rig_tools::generation::image_generate`
Expected: All tests PASS

---

## Task 3: 实现 SpeechGenerateTool

**Files:**
- Create: `Aleph/core/src/rig_tools/generation/speech_generate.rs`

**Step 1: Create SpeechGenerateTool**

Create `src/rig_tools/generation/speech_generate.rs`:

```rust
//! Speech generation (TTS) tool
//!
//! Implements rig's Tool trait for AI-callable text-to-speech generation.

use crate::generation::{
    GenerationData, GenerationParams, GenerationProvider, GenerationProviderRegistry,
    GenerationRequest, GenerationType,
};
use crate::rig_tools::error::ToolError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error, info};

/// Arguments for speech generation tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SpeechGenerateArgs {
    /// The text to convert to speech
    pub text: String,

    /// Voice to use: "alloy", "echo", "fable", "onyx", "nova", "shimmer"
    #[serde(default = "default_voice")]
    pub voice: String,

    /// Speaking speed (0.25 to 4.0, default: 1.0)
    #[serde(default = "default_speed")]
    pub speed: f32,

    /// Output format: "mp3", "opus", "aac", "flac" (default: "mp3")
    #[serde(default = "default_format")]
    pub format: String,

    /// Provider name to use (optional)
    #[serde(default)]
    pub provider: Option<String>,
}

fn default_voice() -> String { "alloy".to_string() }
fn default_speed() -> f32 { 1.0 }
fn default_format() -> String { "mp3".to_string() }

/// Output from speech generation tool
#[derive(Debug, Clone, Serialize)]
pub struct SpeechGenerateOutput {
    /// Location of the generated audio (URL or local path)
    pub audio_location: String,
    /// Type of location: "url", "local_path", or "base64"
    pub location_type: String,
    /// The original text
    pub text: String,
    /// Voice used
    pub voice: String,
    /// Audio format
    pub format: String,
    /// Provider that generated the speech
    pub provider: String,
    /// File size in bytes (if available)
    pub size_bytes: Option<u64>,
    /// Duration in milliseconds
    pub duration_ms: Option<u64>,
}

/// Speech generation tool using GenerationProviderRegistry
pub struct SpeechGenerateTool {
    registry: Arc<GenerationProviderRegistry>,
    default_provider: Option<String>,
    /// Directory to save generated audio files
    output_dir: Option<String>,
}

impl SpeechGenerateTool {
    pub const NAME: &'static str = "generate_speech";

    pub const DESCRIPTION: &'static str =
        "Convert text to speech using AI voices. Returns the audio file location.";

    pub fn new(registry: Arc<GenerationProviderRegistry>) -> Self {
        Self {
            registry,
            default_provider: None,
            output_dir: None,
        }
    }

    pub fn with_default_provider(mut self, provider: String) -> Self {
        self.default_provider = Some(provider);
        self
    }

    pub fn with_output_dir(mut self, dir: String) -> Self {
        self.output_dir = Some(dir);
        self
    }

    pub async fn call(&self, args: SpeechGenerateArgs) -> Result<SpeechGenerateOutput, ToolError> {
        info!(text_len = args.text.len(), voice = %args.voice, "Generating speech");

        if args.text.is_empty() {
            return Err(ToolError::InvalidArgs("Text cannot be empty".to_string()));
        }

        // Validate speed
        if args.speed < 0.25 || args.speed > 4.0 {
            return Err(ToolError::InvalidArgs(
                format!("Speed must be between 0.25 and 4.0, got {}", args.speed)
            ));
        }

        // Get provider
        let provider_name = args.provider
            .as_ref()
            .or(self.default_provider.as_ref())
            .cloned();

        let provider = if let Some(name) = &provider_name {
            self.registry
                .get(name)
                .ok_or_else(|| ToolError::InvalidArgs(format!("Provider '{}' not found", name)))?
        } else {
            self.registry
                .filter_by_type(GenerationType::Speech)
                .first()
                .cloned()
                .ok_or_else(|| ToolError::InvalidArgs("No speech generation provider available".to_string()))?
        };

        // Build request
        let params = GenerationParams::builder()
            .voice(&args.voice)
            .speed(args.speed)
            .format(&args.format)
            .build();

        let request = GenerationRequest::speech(&args.text)
            .with_params(params);

        debug!(provider = %provider.name(), "Using provider for speech generation");

        // Execute generation
        let output = provider.generate(request).await.map_err(|e| {
            error!(error = %e, "Speech generation failed");
            ToolError::from(e)
        })?;

        // Handle output data
        let (audio_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url".to_string()),
            GenerationData::LocalPath(path) => (path.clone(), "local_path".to_string()),
            GenerationData::Bytes(bytes) => {
                // Save to file if output_dir is configured
                if let Some(dir) = &self.output_dir {
                    let filename = format!(
                        "speech_{}.{}",
                        chrono::Utc::now().format("%Y%m%d_%H%M%S"),
                        args.format
                    );
                    let path = std::path::Path::new(dir).join(&filename);

                    // Create directory if needed
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            ToolError::Execution(format!("Failed to create output directory: {}", e))
                        })?;
                    }

                    std::fs::write(&path, bytes).map_err(|e| {
                        ToolError::Execution(format!("Failed to write audio file: {}", e))
                    })?;

                    (path.to_string_lossy().to_string(), "local_path".to_string())
                } else {
                    // Return as base64
                    let b64 = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        bytes
                    );
                    let data_url = format!("data:audio/{};base64,{}", args.format, b64);
                    (data_url, "base64".to_string())
                }
            }
        };

        let duration_ms = output.metadata.duration.map(|d| d.as_millis() as u64);

        info!(
            provider = %provider.name(),
            format = %args.format,
            size_bytes = ?output.metadata.size_bytes,
            "Speech generated successfully"
        );

        Ok(SpeechGenerateOutput {
            audio_location,
            location_type,
            text: args.text,
            voice: args.voice,
            format: args.format,
            provider: provider.name().to_string(),
            size_bytes: output.metadata.size_bytes,
            duration_ms,
        })
    }
}

impl Clone for SpeechGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            default_provider: self.default_provider.clone(),
            output_dir: self.output_dir.clone(),
        }
    }
}

impl Tool for SpeechGenerateTool {
    const NAME: &'static str = "generate_speech";

    type Error = ToolError;
    type Args = SpeechGenerateArgs;
    type Output = SpeechGenerateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let schema = schema_for!(SpeechGenerateArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_else(|_| {
            json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to convert to speech"
                    },
                    "voice": {
                        "type": "string",
                        "enum": ["alloy", "echo", "fable", "onyx", "nova", "shimmer"],
                        "description": "Voice to use",
                        "default": "alloy"
                    },
                    "speed": {
                        "type": "number",
                        "minimum": 0.25,
                        "maximum": 4.0,
                        "description": "Speaking speed",
                        "default": 1.0
                    },
                    "format": {
                        "type": "string",
                        "enum": ["mp3", "opus", "aac", "flac"],
                        "description": "Audio format",
                        "default": "mp3"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Optional: specific provider to use"
                    }
                },
                "required": ["text"]
            })
        });

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        SpeechGenerateTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::MockGenerationProvider;

    fn create_test_registry() -> Arc<GenerationProviderRegistry> {
        let registry = GenerationProviderRegistry::new();
        let mock = Arc::new(
            MockGenerationProvider::new("mock-tts")
                .with_types(vec![GenerationType::Speech])
        );
        registry.register("mock-tts", mock);
        Arc::new(registry)
    }

    #[test]
    fn test_args_deserialization_defaults() {
        let args: SpeechGenerateArgs = serde_json::from_str(r#"{"text": "Hello"}"#).unwrap();
        assert_eq!(args.text, "Hello");
        assert_eq!(args.voice, "alloy");
        assert_eq!(args.speed, 1.0);
        assert_eq!(args.format, "mp3");
    }

    #[test]
    fn test_args_deserialization_custom() {
        let args: SpeechGenerateArgs = serde_json::from_str(r#"{
            "text": "Test speech",
            "voice": "nova",
            "speed": 1.5,
            "format": "opus"
        }"#).unwrap();

        assert_eq!(args.voice, "nova");
        assert_eq!(args.speed, 1.5);
        assert_eq!(args.format, "opus");
    }

    #[tokio::test]
    async fn test_tool_definition() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let def = tool.definition("".to_string()).await;

        assert_eq!(def.name, "generate_speech");
        assert!(def.parameters.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_generate_speech_empty_text() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "".to_string(),
            voice: "alloy".to_string(),
            speed: 1.0,
            format: "mp3".to_string(),
            provider: None,
        };

        let result = tool.call(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_generate_speech_invalid_speed() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "Test".to_string(),
            voice: "alloy".to_string(),
            speed: 5.0, // Invalid: > 4.0
            format: "mp3".to_string(),
            provider: None,
        };

        let result = tool.call(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_generate_speech_success() {
        let registry = create_test_registry();
        let tool = SpeechGenerateTool::new(registry);

        let args = SpeechGenerateArgs {
            text: "Hello world".to_string(),
            voice: "alloy".to_string(),
            speed: 1.0,
            format: "mp3".to_string(),
            provider: Some("mock-tts".to_string()),
        };

        let result = tool.call(args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.text, "Hello world");
        assert_eq!(output.provider, "mock-tts");
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test rig_tools::generation::speech_generate`
Expected: All tests PASS

---

## Task 4: 创建 generation/mod.rs 模块入口

**Files:**
- Create: `Aleph/core/src/rig_tools/generation/mod.rs`

**Step 1: Create the module file**

Create `src/rig_tools/generation/mod.rs`:

```rust
//! Media generation tools
//!
//! Tools for generating images, speech, and other media using AI providers.
//!
//! # Available Tools
//!
//! - [`ImageGenerateTool`] - Generate images from text prompts (DALL·E, etc.)
//! - [`SpeechGenerateTool`] - Generate speech from text (OpenAI TTS, etc.)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::GenerationProviderRegistry;
//! use alephcore::rig_tools::generation::ImageGenerateTool;
//! use std::sync::Arc;
//!
//! let registry = Arc::new(GenerationProviderRegistry::new());
//! let tool = ImageGenerateTool::new(registry);
//! ```

mod image_generate;
mod speech_generate;

pub use image_generate::{ImageGenerateArgs, ImageGenerateOutput, ImageGenerateTool};
pub use speech_generate::{SpeechGenerateArgs, SpeechGenerateOutput, SpeechGenerateTool};

use crate::generation::GenerationError;
use crate::rig_tools::error::ToolError;

/// Convert GenerationError to ToolError for tool execution
impl From<GenerationError> for ToolError {
    fn from(err: GenerationError) -> Self {
        match &err {
            GenerationError::NetworkError { message } => ToolError::Network(message.clone()),
            GenerationError::InvalidParametersError { message, .. } => {
                ToolError::InvalidArgs(message.clone())
            }
            GenerationError::AuthenticationError { message, .. } => {
                ToolError::InvalidArgs(format!("Authentication failed: {}", message))
            }
            GenerationError::RateLimitError { message, .. } => {
                ToolError::Execution(format!("Rate limited: {}", message))
            }
            GenerationError::ContentFilteredError { message, .. } => {
                ToolError::Execution(format!("Content filtered: {}", message))
            }
            _ => ToolError::Execution(err.to_string()),
        }
    }
}
```

**Step 2: Update rig_tools/mod.rs**

Update `src/rig_tools/mod.rs` to include generation module:

```rust
//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.
//!
//! # Built-in Tools
//!
//! - [`SearchTool`] - Web search via Tavily
//! - [`WebFetchTool`] - Web page fetching
//! - [`YouTubeTool`] - YouTube video transcript extraction
//! - [`FileOpsTool`] - File system operations
//! - [`ImageGenerateTool`] - Image generation (DALL·E, etc.)
//! - [`SpeechGenerateTool`] - Text-to-speech (OpenAI TTS, etc.)
//!
//! # Tool Wrappers
//!
//! - [`McpToolWrapper`] - Wraps MCP server tools

pub mod error;
pub mod file_ops;
pub mod generation;  // NEW
pub mod mcp_wrapper;
pub mod search;
pub mod web_fetch;
pub mod youtube;

pub use error::ToolError;
pub use file_ops::FileOpsTool;
pub use generation::{ImageGenerateTool, SpeechGenerateTool};  // NEW
pub use mcp_wrapper::McpToolWrapper;
pub use search::SearchTool;
pub use web_fetch::WebFetchTool;
pub use youtube::YouTubeTool;
```

**Step 3: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test rig_tools::generation`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add src/rig_tools/generation/
git add src/rig_tools/mod.rs
git commit -m "feat(tools): add image and speech generation tools

Implement rig Tool trait for media generation:
- ImageGenerateTool: Generate images from text prompts
- SpeechGenerateTool: Convert text to speech
- Both integrate with GenerationProviderRegistry
- Full test coverage for args, validation, and execution

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: 在 ToolRegistry 中注册生成工具为 Builtin

**Files:**
- Modify: `Aleph/core/src/dispatcher/registry.rs`

**Step 1: Add builtin tool registration**

In `src/dispatcher/registry.rs`, find `register_builtin_tools` function and add generation tools:

```rust
// Add at the appropriate place in register_builtin_tools()

// Image generation tool
let image_generate = UnifiedTool::new(
    "builtin:generate_image",
    "generate_image",
    "Generate images from text descriptions using AI models",
    ToolSource::Builtin,
)
.with_icon("photo.badge.plus")
.with_usage("/generate_image A beautiful sunset over mountains")
.with_localization_key("tool.generate_image")
.with_sort_order(60);

self.register_with_conflict_resolution(image_generate).await;

// Speech generation tool
let speech_generate = UnifiedTool::new(
    "builtin:generate_speech",
    "generate_speech",
    "Convert text to speech using AI voices",
    ToolSource::Builtin,
)
.with_icon("speaker.wave.3")
.with_usage("/generate_speech Hello, how are you?")
.with_localization_key("tool.generate_speech")
.with_sort_order(61);

self.register_with_conflict_resolution(speech_generate).await;
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test dispatcher::registry`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add src/dispatcher/registry.rs
git commit -m "feat(tools): register generation tools as builtin commands

Add generate_image and generate_speech to ToolRegistry builtin tools.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: 完整构建验证

**Step 1: Run cargo build**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo build`
Expected: Build succeeds

**Step 2: Run cargo clippy**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo clippy -- -W clippy::all 2>&1 | grep generation`
Expected: No warnings in generation/rig_tools modules

**Step 3: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph/Aleph/core && cargo test`
Expected: All tests pass

---

## 验收标准

完成后应有：

1. **新文件**:
   - `rig_tools/generation/mod.rs` - 模块入口
   - `rig_tools/generation/image_generate.rs` - 图像生成工具
   - `rig_tools/generation/speech_generate.rs` - 语音生成工具

2. **修改文件**:
   - `rig_tools/mod.rs` - 导出 generation 模块
   - `dispatcher/registry.rs` - 注册 builtin 工具

3. **测试覆盖**:
   - ImageGenerateTool 单元测试
   - SpeechGenerateTool 单元测试
   - 参数验证测试
   - 工具定义测试

4. **功能**:
   - AI agent 可通过 `generate_image` 工具调用图像生成
   - AI agent 可通过 `generate_speech` 工具调用语音合成
   - 用户可使用 `/generate_image <prompt>` 命令
   - 用户可使用 `/generate_speech <text>` 命令

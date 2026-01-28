# Generation Providers Phase 1: Core Framework Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the foundational types, traits, and registry for media generation providers.

**Architecture:** New `generation/` module parallel to existing `providers/`. Uses async trait pattern with `Pin<Box<dyn Future>>` matching existing `AiProvider`. Shares config infrastructure via `config/types/generation.rs`.

**Tech Stack:** Rust, async-trait pattern, serde, thiserror, tokio

---

## Task 1: Create Generation Module Structure

**Files:**
- Create: `Aether/core/src/generation/mod.rs`
- Create: `Aether/core/src/generation/types.rs`

**Step 1: Create the generation module directory and mod.rs**

```rust
// Aether/core/src/generation/mod.rs

//! Media Generation Provider abstraction for Aether
//!
//! This module defines the `GenerationProvider` trait which provides a unified interface
//! for different media generation backends (DALL·E, Stability AI, Replicate, etc.).
//!
//! # Architecture
//!
//! All generation providers implement the `GenerationProvider` trait, which defines:
//! - `generate()`: Async method to generate media from a prompt
//! - `name()`: Provider identifier (e.g., "dalle3", "stability")
//! - `supported_types()`: List of supported generation types
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::{GenerationProvider, GenerationRequest, GenerationType};
//! use std::sync::Arc;
//!
//! async fn example(provider: Arc<dyn GenerationProvider>) {
//!     let request = GenerationRequest::new(
//!         GenerationType::Image,
//!         "A cat sitting on a rainbow",
//!     );
//!     let output = provider.generate(request).await.unwrap();
//!     println!("Generated: {:?}", output.media_type);
//! }
//! ```

pub mod error;
pub mod registry;
pub mod types;

// Re-exports
pub use error::GenerationError;
pub use registry::GenerationProviderRegistry;
pub use types::{
    GenerationData, GenerationMetadata, GenerationOutput, GenerationParams, GenerationProgress,
    GenerationRequest, GenerationType,
};

use crate::error::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Unified interface for media generation providers
///
/// All generation backends (DALL·E, Stability AI, Replicate, etc.) implement this trait
/// to provide a consistent API for generating images, videos, and audio.
///
/// # Thread Safety
///
/// The trait extends `Send + Sync` to ensure providers can be safely shared
/// across async tasks and stored in `Arc<dyn GenerationProvider>`.
///
/// # Async Design
///
/// All generation is async to avoid blocking the runtime during API calls.
pub trait GenerationProvider: Send + Sync {
    /// Generate media from a request
    ///
    /// # Arguments
    ///
    /// * `request` - The generation request containing prompt and parameters
    ///
    /// # Returns
    ///
    /// * `Ok(GenerationOutput)` - The generated media
    /// * `Err(AetherError)` - Various errors (see GenerationError)
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<GenerationOutput>> + Send + '_>>;

    /// Get provider name for logging and routing
    ///
    /// # Returns
    ///
    /// Provider identifier (e.g., "dalle3", "stability", "replicate")
    fn name(&self) -> &str;

    /// Get provider brand color for UI theming
    ///
    /// # Returns
    ///
    /// Hex color string (e.g., "#10a37f")
    fn color(&self) -> &str;

    /// Get supported generation types
    ///
    /// # Returns
    ///
    /// List of generation types this provider supports
    fn supported_types(&self) -> Vec<GenerationType>;

    /// Estimate generation duration for a request
    ///
    /// Used to decide between blocking and background execution.
    ///
    /// # Returns
    ///
    /// Estimated duration for the generation
    fn estimate_duration(&self, request: &GenerationRequest) -> Duration {
        // Default: 10 seconds for images, 60 for video, 30 for audio
        match request.generation_type {
            GenerationType::Image => Duration::from_secs(10),
            GenerationType::Video => Duration::from_secs(60),
            GenerationType::Audio => Duration::from_secs(30),
            GenerationType::Speech => Duration::from_secs(5),
        }
    }

    /// Check generation progress (if provider supports it)
    ///
    /// # Arguments
    ///
    /// * `task_id` - The task ID returned from async generation
    ///
    /// # Returns
    ///
    /// Current progress information
    fn check_progress(
        &self,
        _task_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<GenerationProgress>> + Send + '_>> {
        Box::pin(async move {
            Ok(GenerationProgress {
                task_id: String::new(),
                progress: 0.0,
                status: "unknown".to_string(),
                message: Some("Progress tracking not supported".to_string()),
            })
        })
    }

    /// Check if provider supports progress tracking
    fn supports_progress(&self) -> bool {
        false
    }
}

/// Create a mock provider for testing
pub fn create_mock_generation_provider() -> Arc<dyn GenerationProvider> {
    Arc::new(MockGenerationProvider::default())
}

/// Mock generation provider for testing
#[derive(Default)]
pub struct MockGenerationProvider {
    name: String,
    color: String,
}

impl MockGenerationProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: "#808080".to_string(),
        }
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = color.into();
        self
    }
}

impl GenerationProvider for MockGenerationProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            Ok(GenerationOutput {
                media_type: request.generation_type,
                mime_type: "image/png".to_string(),
                data: GenerationData::Bytes(vec![0x89, 0x50, 0x4E, 0x47]), // PNG magic bytes
                metadata: GenerationMetadata {
                    model: Some("mock".to_string()),
                    seed: None,
                    revised_prompt: None,
                    generation_time_ms: Some(100),
                },
            })
        })
    }

    fn name(&self) -> &str {
        if self.name.is_empty() {
            "mock"
        } else {
            &self.name
        }
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![
            GenerationType::Image,
            GenerationType::Video,
            GenerationType::Audio,
            GenerationType::Speech,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_provider_creation() {
        let provider = MockGenerationProvider::new("test");
        assert_eq!(provider.name(), "test");
    }

    #[test]
    fn test_generation_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn GenerationProvider>>();
    }

    #[tokio::test]
    async fn test_mock_provider_generate() {
        let provider = MockGenerationProvider::default();
        let request = GenerationRequest::new(GenerationType::Image, "test prompt");
        let output = provider.generate(request).await.unwrap();
        assert_eq!(output.media_type, GenerationType::Image);
    }
}
```

**Step 2: Run the build to verify syntax**

Run: `cd Aether/core && cargo check`
Expected: Compilation errors (types.rs and error.rs don't exist yet)

**Step 3: Commit module structure**

```bash
git add Aether/core/src/generation/mod.rs
git commit -m "feat(generation): add generation module structure with trait definition"
```

---

## Task 2: Create Core Types

**Files:**
- Create: `Aether/core/src/generation/types.rs`

**Step 1: Write the types module**

```rust
// Aether/core/src/generation/types.rs

//! Core types for media generation
//!
//! This module contains the fundamental types used across all generation providers:
//! - GenerationType: Enum of supported media types
//! - GenerationParams: Parameters for generation requests
//! - GenerationRequest: Complete generation request
//! - GenerationOutput: Result of generation
//! - GenerationData: The actual generated content

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Types of media that can be generated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationType {
    /// Image generation (DALL·E, Stable Diffusion, Flux, etc.)
    Image,
    /// Video generation (Stable Video, Runway, etc.)
    Video,
    /// Audio generation (music, sound effects)
    Audio,
    /// Speech synthesis (TTS)
    Speech,
}

impl GenerationType {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Speech => "Speech",
        }
    }

    /// Get default file extension
    pub fn default_extension(&self) -> &'static str {
        match self {
            Self::Image => "png",
            Self::Video => "mp4",
            Self::Audio => "mp3",
            Self::Speech => "mp3",
        }
    }

    /// Get default MIME type
    pub fn default_mime_type(&self) -> &'static str {
        match self {
            Self::Image => "image/png",
            Self::Video => "video/mp4",
            Self::Audio => "audio/mpeg",
            Self::Speech => "audio/mpeg",
        }
    }
}

impl std::fmt::Display for GenerationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Parameters for generation requests
///
/// This is a superset of parameters supported by all providers.
/// Each provider will use the parameters relevant to it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationParams {
    // === Image parameters ===
    /// Width in pixels
    #[serde(default)]
    pub width: Option<u32>,
    /// Height in pixels
    #[serde(default)]
    pub height: Option<u32>,
    /// Aspect ratio (e.g., "16:9", "1:1", "9:16")
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Style preset (e.g., "vivid", "natural", "anime", "photographic")
    #[serde(default)]
    pub style: Option<String>,
    /// Quality level (e.g., "standard", "hd")
    #[serde(default)]
    pub quality: Option<String>,
    /// Number of outputs to generate
    #[serde(default)]
    pub num_outputs: Option<u32>,

    // === Video parameters ===
    /// Duration in seconds
    #[serde(default)]
    pub duration_seconds: Option<f32>,
    /// Frames per second
    #[serde(default)]
    pub fps: Option<u32>,
    /// Motion strength (0.0 - 1.0)
    #[serde(default)]
    pub motion_strength: Option<f32>,

    // === Audio/Speech parameters ===
    /// Sample rate in Hz
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Voice ID for TTS
    #[serde(default)]
    pub voice_id: Option<String>,
    /// Speech speed multiplier
    #[serde(default)]
    pub speed: Option<f32>,

    // === Common parameters ===
    /// Random seed for reproducibility
    #[serde(default)]
    pub seed: Option<i64>,
    /// Guidance scale (CFG scale)
    #[serde(default)]
    pub guidance_scale: Option<f32>,
    /// Override default model
    #[serde(default)]
    pub model: Option<String>,
    /// Override default provider
    #[serde(default)]
    pub provider: Option<String>,

    // === Raw passthrough ===
    /// Extra parameters to pass to provider API
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
}

impl GenerationParams {
    /// Create new empty params
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set width
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Builder: set height
    pub fn with_height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }

    /// Builder: set size (width x height)
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Builder: set aspect ratio
    pub fn with_aspect_ratio(mut self, ratio: impl Into<String>) -> Self {
        self.aspect_ratio = Some(ratio.into());
        self
    }

    /// Builder: set style
    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }

    /// Builder: set quality
    pub fn with_quality(mut self, quality: impl Into<String>) -> Self {
        self.quality = Some(quality.into());
        self
    }

    /// Builder: set seed
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Builder: set model override
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Builder: set provider override
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Builder: set voice ID for TTS
    pub fn with_voice(mut self, voice_id: impl Into<String>) -> Self {
        self.voice_id = Some(voice_id.into());
        self
    }

    /// Builder: set speech speed
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Merge with another params, other takes precedence
    pub fn merge(&self, other: &GenerationParams) -> Self {
        Self {
            width: other.width.or(self.width),
            height: other.height.or(self.height),
            aspect_ratio: other.aspect_ratio.clone().or_else(|| self.aspect_ratio.clone()),
            style: other.style.clone().or_else(|| self.style.clone()),
            quality: other.quality.clone().or_else(|| self.quality.clone()),
            num_outputs: other.num_outputs.or(self.num_outputs),
            duration_seconds: other.duration_seconds.or(self.duration_seconds),
            fps: other.fps.or(self.fps),
            motion_strength: other.motion_strength.or(self.motion_strength),
            sample_rate: other.sample_rate.or(self.sample_rate),
            voice_id: other.voice_id.clone().or_else(|| self.voice_id.clone()),
            speed: other.speed.or(self.speed),
            seed: other.seed.or(self.seed),
            guidance_scale: other.guidance_scale.or(self.guidance_scale),
            model: other.model.clone().or_else(|| self.model.clone()),
            provider: other.provider.clone().or_else(|| self.provider.clone()),
            extra: other.extra.clone().or_else(|| self.extra.clone()),
        }
    }
}

/// A generation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationRequest {
    /// Type of media to generate
    pub generation_type: GenerationType,
    /// The prompt describing what to generate
    pub prompt: String,
    /// Negative prompt (what to avoid)
    #[serde(default)]
    pub negative_prompt: Option<String>,
    /// Generation parameters
    #[serde(default)]
    pub parameters: GenerationParams,
}

impl GenerationRequest {
    /// Create a new generation request
    pub fn new(generation_type: GenerationType, prompt: impl Into<String>) -> Self {
        Self {
            generation_type,
            prompt: prompt.into(),
            negative_prompt: None,
            parameters: GenerationParams::default(),
        }
    }

    /// Builder: set negative prompt
    pub fn with_negative_prompt(mut self, negative: impl Into<String>) -> Self {
        self.negative_prompt = Some(negative.into());
        self
    }

    /// Builder: set parameters
    pub fn with_params(mut self, params: GenerationParams) -> Self {
        self.parameters = params;
        self
    }

    /// Builder: set size
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.parameters.width = Some(width);
        self.parameters.height = Some(height);
        self
    }

    /// Builder: set style
    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        self.parameters.style = Some(style.into());
        self
    }
}

/// The actual generated content
#[derive(Debug, Clone)]
pub enum GenerationData {
    /// Raw bytes (for small files that can be pasted)
    Bytes(Vec<u8>),
    /// URL to download (for large files or when provider returns URL)
    Url(String),
    /// Local file path (when saved to disk)
    LocalPath(PathBuf),
}

impl GenerationData {
    /// Get size in bytes (if known)
    pub fn size_bytes(&self) -> Option<usize> {
        match self {
            Self::Bytes(data) => Some(data.len()),
            Self::Url(_) | Self::LocalPath(_) => None,
        }
    }

    /// Check if this is local data (bytes or path)
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Bytes(_) | Self::LocalPath(_))
    }
}

/// Metadata about the generation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationMetadata {
    /// Model used for generation
    #[serde(default)]
    pub model: Option<String>,
    /// Seed used (for reproducibility)
    #[serde(default)]
    pub seed: Option<i64>,
    /// Revised prompt (if provider modified it)
    #[serde(default)]
    pub revised_prompt: Option<String>,
    /// Generation time in milliseconds
    #[serde(default)]
    pub generation_time_ms: Option<u64>,
}

/// Output from a generation request
#[derive(Debug, Clone)]
pub struct GenerationOutput {
    /// Type of media generated
    pub media_type: GenerationType,
    /// MIME type of the output
    pub mime_type: String,
    /// The actual content
    pub data: GenerationData,
    /// Metadata about the generation
    pub metadata: GenerationMetadata,
}

impl GenerationOutput {
    /// Get file extension based on MIME type
    pub fn extension(&self) -> &str {
        match self.mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" | "image/jpg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            "video/mp4" => "mp4",
            "video/webm" => "webm",
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/wav" => "wav",
            "audio/ogg" => "ogg",
            _ => self.media_type.default_extension(),
        }
    }

    /// Get size in bytes (if known)
    pub fn size_bytes(&self) -> Option<usize> {
        self.data.size_bytes()
    }
}

/// Progress information for async generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProgress {
    /// Task ID
    pub task_id: String,
    /// Progress (0.0 - 1.0)
    pub progress: f32,
    /// Status string (e.g., "processing", "completed", "failed")
    pub status: String,
    /// Optional message
    #[serde(default)]
    pub message: Option<String>,
}

impl GenerationProgress {
    /// Create a new progress update
    pub fn new(task_id: impl Into<String>, progress: f32, status: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            progress,
            status: status.into(),
            message: None,
        }
    }

    /// Check if generation is complete
    pub fn is_complete(&self) -> bool {
        self.status == "completed" || self.status == "succeeded"
    }

    /// Check if generation failed
    pub fn is_failed(&self) -> bool {
        self.status == "failed" || self.status == "error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_type_display() {
        assert_eq!(GenerationType::Image.display_name(), "Image");
        assert_eq!(GenerationType::Video.display_name(), "Video");
        assert_eq!(GenerationType::Audio.display_name(), "Audio");
        assert_eq!(GenerationType::Speech.display_name(), "Speech");
    }

    #[test]
    fn test_generation_type_extension() {
        assert_eq!(GenerationType::Image.default_extension(), "png");
        assert_eq!(GenerationType::Video.default_extension(), "mp4");
    }

    #[test]
    fn test_generation_params_builder() {
        let params = GenerationParams::new()
            .with_size(1024, 1024)
            .with_style("vivid")
            .with_quality("hd")
            .with_seed(12345);

        assert_eq!(params.width, Some(1024));
        assert_eq!(params.height, Some(1024));
        assert_eq!(params.style, Some("vivid".to_string()));
        assert_eq!(params.quality, Some("hd".to_string()));
        assert_eq!(params.seed, Some(12345));
    }

    #[test]
    fn test_generation_params_merge() {
        let base = GenerationParams::new()
            .with_size(512, 512)
            .with_style("natural");

        let override_params = GenerationParams::new()
            .with_width(1024)
            .with_quality("hd");

        let merged = base.merge(&override_params);

        // Width overridden
        assert_eq!(merged.width, Some(1024));
        // Height from base
        assert_eq!(merged.height, Some(512));
        // Style from base
        assert_eq!(merged.style, Some("natural".to_string()));
        // Quality from override
        assert_eq!(merged.quality, Some("hd".to_string()));
    }

    #[test]
    fn test_generation_request_builder() {
        let request = GenerationRequest::new(GenerationType::Image, "a cat")
            .with_negative_prompt("blurry")
            .with_size(1024, 1024)
            .with_style("vivid");

        assert_eq!(request.generation_type, GenerationType::Image);
        assert_eq!(request.prompt, "a cat");
        assert_eq!(request.negative_prompt, Some("blurry".to_string()));
        assert_eq!(request.parameters.width, Some(1024));
        assert_eq!(request.parameters.style, Some("vivid".to_string()));
    }

    #[test]
    fn test_generation_output_extension() {
        let output = GenerationOutput {
            media_type: GenerationType::Image,
            mime_type: "image/jpeg".to_string(),
            data: GenerationData::Bytes(vec![]),
            metadata: GenerationMetadata::default(),
        };

        assert_eq!(output.extension(), "jpg");
    }

    #[test]
    fn test_generation_progress() {
        let progress = GenerationProgress::new("task-123", 0.5, "processing");
        assert!(!progress.is_complete());
        assert!(!progress.is_failed());

        let completed = GenerationProgress::new("task-123", 1.0, "completed");
        assert!(completed.is_complete());
    }

    #[test]
    fn test_generation_data_size() {
        let bytes = GenerationData::Bytes(vec![0u8; 1000]);
        assert_eq!(bytes.size_bytes(), Some(1000));

        let url = GenerationData::Url("https://example.com/image.png".to_string());
        assert_eq!(url.size_bytes(), None);
    }
}
```

**Step 2: Run tests**

Run: `cd Aether/core && cargo test generation::types`
Expected: All tests pass

**Step 3: Commit types**

```bash
git add Aether/core/src/generation/types.rs
git commit -m "feat(generation): add core types (GenerationType, Params, Request, Output)"
```

---

## Task 3: Create Error Types

**Files:**
- Create: `Aether/core/src/generation/error.rs`

**Step 1: Write the error module**

```rust
// Aether/core/src/generation/error.rs

//! Error types for media generation
//!
//! This module defines generation-specific errors that can occur during
//! media generation operations.

use std::time::Duration;
use thiserror::Error;

/// Errors that can occur during media generation
#[derive(Debug, Clone, Error)]
pub enum GenerationError {
    // === Retryable errors ===
    /// Rate limited by provider
    #[error("Rate limited{}", .retry_after.map(|d| format!(", retry after {:?}", d)).unwrap_or_default())]
    RateLimited {
        /// Time to wait before retry
        retry_after: Option<Duration>,
    },

    /// Service temporarily unavailable
    #[error("Service unavailable: {message}")]
    ServiceUnavailable { message: String },

    /// Request timed out
    #[error("Generation timed out after {duration:?}")]
    Timeout { duration: Duration },

    /// Network error
    #[error("Network error: {message}")]
    NetworkError { message: String },

    // === Content errors (need user action) ===
    /// Content filtered by provider's safety system
    #[error("Content filtered: {reason}")]
    ContentFiltered {
        reason: String,
        suggestion: Option<String>,
    },

    /// Prompt too long
    #[error("Prompt too long (max {max_length} characters)")]
    PromptTooLong { max_length: usize },

    /// Unsupported format requested
    #[error("Unsupported format '{requested}', supported: {}", supported.join(", "))]
    UnsupportedFormat {
        requested: String,
        supported: Vec<String>,
    },

    /// Unsupported generation type for this provider
    #[error("Provider does not support {generation_type} generation")]
    UnsupportedType { generation_type: String },

    // === Config/Auth errors ===
    /// Invalid API key
    #[error("Invalid API key for provider '{provider}'")]
    InvalidApiKey { provider: String },

    /// Quota exceeded
    #[error("Quota exceeded for provider '{provider}'")]
    QuotaExceeded { provider: String },

    /// Provider not found
    #[error("Provider '{name}' not found")]
    ProviderNotFound { name: String },

    /// Invalid configuration
    #[error("Invalid configuration: {message}")]
    InvalidConfig { message: String },

    // === Unrecoverable errors ===
    /// Internal provider error
    #[error("Provider error: {message}")]
    ProviderError { message: String },

    /// Internal error
    #[error("Internal error: {message}")]
    InternalError { message: String },
}

impl GenerationError {
    // === Constructor helpers ===

    /// Create a rate limited error
    pub fn rate_limited(retry_after: Option<Duration>) -> Self {
        Self::RateLimited { retry_after }
    }

    /// Create a service unavailable error
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::ServiceUnavailable {
            message: message.into(),
        }
    }

    /// Create a timeout error
    pub fn timeout(duration: Duration) -> Self {
        Self::Timeout { duration }
    }

    /// Create a network error
    pub fn network(message: impl Into<String>) -> Self {
        Self::NetworkError {
            message: message.into(),
        }
    }

    /// Create a content filtered error
    pub fn content_filtered(reason: impl Into<String>) -> Self {
        Self::ContentFiltered {
            reason: reason.into(),
            suggestion: None,
        }
    }

    /// Create a content filtered error with suggestion
    pub fn content_filtered_with_suggestion(
        reason: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self::ContentFiltered {
            reason: reason.into(),
            suggestion: Some(suggestion.into()),
        }
    }

    /// Create a prompt too long error
    pub fn prompt_too_long(max_length: usize) -> Self {
        Self::PromptTooLong { max_length }
    }

    /// Create an unsupported format error
    pub fn unsupported_format(requested: impl Into<String>, supported: Vec<String>) -> Self {
        Self::UnsupportedFormat {
            requested: requested.into(),
            supported,
        }
    }

    /// Create an unsupported type error
    pub fn unsupported_type(generation_type: impl Into<String>) -> Self {
        Self::UnsupportedType {
            generation_type: generation_type.into(),
        }
    }

    /// Create an invalid API key error
    pub fn invalid_api_key(provider: impl Into<String>) -> Self {
        Self::InvalidApiKey {
            provider: provider.into(),
        }
    }

    /// Create a quota exceeded error
    pub fn quota_exceeded(provider: impl Into<String>) -> Self {
        Self::QuotaExceeded {
            provider: provider.into(),
        }
    }

    /// Create a provider not found error
    pub fn provider_not_found(name: impl Into<String>) -> Self {
        Self::ProviderNotFound { name: name.into() }
    }

    /// Create an invalid config error
    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::InvalidConfig {
            message: message.into(),
        }
    }

    /// Create a provider error
    pub fn provider_error(message: impl Into<String>) -> Self {
        Self::ProviderError {
            message: message.into(),
        }
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::InternalError {
            message: message.into(),
        }
    }

    // === Classification helpers ===

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::ServiceUnavailable { .. }
                | Self::Timeout { .. }
                | Self::NetworkError { .. }
        )
    }

    /// Check if error needs user action
    pub fn needs_user_action(&self) -> bool {
        matches!(
            self,
            Self::ContentFiltered { .. }
                | Self::PromptTooLong { .. }
                | Self::QuotaExceeded { .. }
        )
    }

    /// Check if error should trigger fallback to another provider
    pub fn should_fallback(&self) -> bool {
        matches!(
            self,
            Self::ServiceUnavailable { .. }
                | Self::Timeout { .. }
                | Self::QuotaExceeded { .. }
                | Self::InvalidApiKey { .. }
                | Self::UnsupportedType { .. }
        )
    }

    /// Get suggestion for the error (if any)
    pub fn suggestion(&self) -> Option<&str> {
        match self {
            Self::ContentFiltered { suggestion, .. } => suggestion.as_deref(),
            Self::RateLimited { retry_after } => {
                if retry_after.is_some() {
                    Some("Wait for the rate limit to reset")
                } else {
                    Some("Wait a moment and try again")
                }
            }
            Self::QuotaExceeded { .. } => Some("Check your account billing or upgrade your plan"),
            Self::InvalidApiKey { .. } => Some("Verify your API key in Settings → Providers"),
            Self::PromptTooLong { .. } => Some("Shorten your prompt and try again"),
            _ => None,
        }
    }
}

// Conversion to AetherError for compatibility
impl From<GenerationError> for crate::error::AetherError {
    fn from(err: GenerationError) -> Self {
        match err {
            GenerationError::RateLimited { .. } => crate::error::AetherError::RateLimitError {
                message: err.to_string(),
                suggestion: err.suggestion().map(|s| s.to_string()),
            },
            GenerationError::NetworkError { message } => crate::error::AetherError::NetworkError {
                message,
                suggestion: Some("Check your internet connection".to_string()),
            },
            GenerationError::InvalidApiKey { provider } => {
                crate::error::AetherError::AuthenticationError {
                    message: format!("Invalid API key for {}", provider),
                    provider,
                    suggestion: Some("Verify your API key in Settings".to_string()),
                }
            }
            GenerationError::Timeout { duration } => crate::error::AetherError::Timeout {
                suggestion: Some(format!(
                    "Request timed out after {:?}. Try again.",
                    duration
                )),
            },
            GenerationError::InvalidConfig { message } => {
                crate::error::AetherError::InvalidConfig {
                    message,
                    suggestion: Some("Check your configuration".to_string()),
                }
            }
            _ => crate::error::AetherError::ProviderError {
                message: err.to_string(),
                suggestion: err.suggestion().map(|s| s.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_is_retryable() {
        assert!(GenerationError::rate_limited(None).is_retryable());
        assert!(GenerationError::timeout(Duration::from_secs(30)).is_retryable());
        assert!(GenerationError::network("failed").is_retryable());
        assert!(GenerationError::service_unavailable("down").is_retryable());

        assert!(!GenerationError::content_filtered("nsfw").is_retryable());
        assert!(!GenerationError::invalid_api_key("test").is_retryable());
    }

    #[test]
    fn test_error_needs_user_action() {
        assert!(GenerationError::content_filtered("nsfw").needs_user_action());
        assert!(GenerationError::prompt_too_long(1000).needs_user_action());
        assert!(GenerationError::quota_exceeded("dalle").needs_user_action());

        assert!(!GenerationError::network("failed").needs_user_action());
    }

    #[test]
    fn test_error_should_fallback() {
        assert!(GenerationError::service_unavailable("down").should_fallback());
        assert!(GenerationError::timeout(Duration::from_secs(30)).should_fallback());
        assert!(GenerationError::quota_exceeded("dalle").should_fallback());
        assert!(GenerationError::invalid_api_key("test").should_fallback());

        assert!(!GenerationError::content_filtered("nsfw").should_fallback());
        assert!(!GenerationError::network("failed").should_fallback());
    }

    #[test]
    fn test_error_suggestion() {
        let err = GenerationError::content_filtered_with_suggestion("nsfw", "Try a different prompt");
        assert_eq!(err.suggestion(), Some("Try a different prompt"));

        let err = GenerationError::rate_limited(Some(Duration::from_secs(60)));
        assert!(err.suggestion().is_some());
    }

    #[test]
    fn test_error_display() {
        let err = GenerationError::rate_limited(Some(Duration::from_secs(60)));
        assert!(err.to_string().contains("Rate limited"));
        assert!(err.to_string().contains("60"));

        let err = GenerationError::content_filtered("NSFW content detected");
        assert!(err.to_string().contains("Content filtered"));
        assert!(err.to_string().contains("NSFW"));
    }

    #[test]
    fn test_conversion_to_aether_error() {
        let err = GenerationError::invalid_api_key("openai");
        let aether_err: crate::error::AetherError = err.into();
        assert!(matches!(aether_err, crate::error::AetherError::AuthenticationError { .. }));
    }
}
```

**Step 2: Run tests**

Run: `cd Aether/core && cargo test generation::error`
Expected: All tests pass

**Step 3: Commit error types**

```bash
git add Aether/core/src/generation/error.rs
git commit -m "feat(generation): add GenerationError type with classification helpers"
```

---

## Task 4: Create Registry

**Files:**
- Create: `Aether/core/src/generation/registry.rs`

**Step 1: Write the registry module**

```rust
// Aether/core/src/generation/registry.rs

//! Provider Registry for managing generation providers
//!
//! This module provides a registry to store and retrieve generation providers by name.
//! Similar to ProviderRegistry but for GenerationProvider.

use crate::error::{AetherError, Result};
use crate::generation::{GenerationProvider, GenerationType};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry for managing generation providers
///
/// # Example
///
/// ```rust
/// use aethecore::generation::{GenerationProviderRegistry, MockGenerationProvider};
/// use std::sync::Arc;
///
/// let mut registry = GenerationProviderRegistry::new();
///
/// // Register a provider
/// let provider = Arc::new(MockGenerationProvider::new("dalle3"));
/// registry.register("dalle3".to_string(), provider).unwrap();
///
/// // Retrieve a provider
/// let provider = registry.get("dalle3").unwrap();
/// assert_eq!(provider.name(), "dalle3");
/// ```
pub struct GenerationProviderRegistry {
    providers: HashMap<String, Arc<dyn GenerationProvider>>,
}

impl GenerationProviderRegistry {
    /// Create a new empty provider registry
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider with a unique name
    ///
    /// # Arguments
    ///
    /// * `name` - Unique identifier for the provider
    /// * `provider` - Arc-wrapped provider implementation
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Provider registered successfully
    /// * `Err(AetherError::InvalidConfig)` - Provider name already exists
    pub fn register(
        &mut self,
        name: String,
        provider: Arc<dyn GenerationProvider>,
    ) -> Result<()> {
        if self.providers.contains_key(&name) {
            return Err(AetherError::invalid_config(format!(
                "Generation provider '{}' is already registered",
                name
            )));
        }
        self.providers.insert(name, provider);
        Ok(())
    }

    /// Get a provider by name
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name to look up
    ///
    /// # Returns
    ///
    /// * `Some(Arc<dyn GenerationProvider>)` - Provider found
    /// * `None` - Provider not found
    pub fn get(&self, name: &str) -> Option<Arc<dyn GenerationProvider>> {
        self.providers.get(name).cloned()
    }

    /// Get a provider by name, returning error if not found
    pub fn get_or_err(&self, name: &str) -> Result<Arc<dyn GenerationProvider>> {
        self.get(name).ok_or_else(|| {
            AetherError::invalid_config(format!("Generation provider '{}' not found", name))
        })
    }

    /// Get all registered provider names in sorted order
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Check if a provider is registered
    pub fn contains(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get the number of registered providers
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Get all providers that support a specific generation type
    pub fn providers_for_type(&self, gen_type: GenerationType) -> Vec<Arc<dyn GenerationProvider>> {
        self.providers
            .values()
            .filter(|p| p.supported_types().contains(&gen_type))
            .cloned()
            .collect()
    }

    /// Get provider names that support a specific generation type
    pub fn names_for_type(&self, gen_type: GenerationType) -> Vec<String> {
        self.providers
            .iter()
            .filter(|(_, p)| p.supported_types().contains(&gen_type))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Remove a provider from the registry
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn GenerationProvider>> {
        self.providers.remove(name)
    }

    /// Clear all providers
    pub fn clear(&mut self) {
        self.providers.clear();
    }
}

impl Default for GenerationProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::MockGenerationProvider;

    #[test]
    fn test_registry_new() {
        let registry = GenerationProviderRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_register() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = Arc::new(MockGenerationProvider::new("dalle3"));

        let result = registry.register("dalle3".to_string(), provider);
        assert!(result.is_ok());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_register_duplicate() {
        let mut registry = GenerationProviderRegistry::new();
        let provider1 = Arc::new(MockGenerationProvider::new("dalle3"));
        let provider2 = Arc::new(MockGenerationProvider::new("dalle3-v2"));

        registry.register("dalle3".to_string(), provider1).unwrap();

        let result = registry.register("dalle3".to_string(), provider2);
        assert!(result.is_err());

        if let Err(AetherError::InvalidConfig { message, .. }) = result {
            assert!(message.contains("already registered"));
        } else {
            panic!("Expected InvalidConfig error");
        }
    }

    #[test]
    fn test_registry_get() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = Arc::new(MockGenerationProvider::new("dalle3"));

        registry.register("dalle3".to_string(), provider).unwrap();

        let retrieved = registry.get("dalle3").unwrap();
        assert_eq!(retrieved.name(), "dalle3");
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = GenerationProviderRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_get_or_err() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = Arc::new(MockGenerationProvider::new("dalle3"));
        registry.register("dalle3".to_string(), provider).unwrap();

        assert!(registry.get_or_err("dalle3").is_ok());
        assert!(registry.get_or_err("nonexistent").is_err());
    }

    #[test]
    fn test_registry_contains() {
        let mut registry = GenerationProviderRegistry::new();
        let provider = Arc::new(MockGenerationProvider::new("dalle3"));

        registry.register("dalle3".to_string(), provider).unwrap();

        assert!(registry.contains("dalle3"));
        assert!(!registry.contains("stability"));
    }

    #[test]
    fn test_registry_names() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("stability".to_string(), Arc::new(MockGenerationProvider::new("stability")))
            .unwrap();
        registry
            .register("dalle3".to_string(), Arc::new(MockGenerationProvider::new("dalle3")))
            .unwrap();
        registry
            .register("replicate".to_string(), Arc::new(MockGenerationProvider::new("replicate")))
            .unwrap();

        let names = registry.names();
        assert_eq!(names, vec!["dalle3", "replicate", "stability"]);
    }

    #[test]
    fn test_registry_providers_for_type() {
        let mut registry = GenerationProviderRegistry::new();

        registry
            .register("mock1".to_string(), Arc::new(MockGenerationProvider::new("mock1")))
            .unwrap();
        registry
            .register("mock2".to_string(), Arc::new(MockGenerationProvider::new("mock2")))
            .unwrap();

        let image_providers = registry.providers_for_type(GenerationType::Image);
        assert_eq!(image_providers.len(), 2);
    }

    #[test]
    fn test_registry_remove() {
        let mut registry = GenerationProviderRegistry::new();
        registry
            .register("dalle3".to_string(), Arc::new(MockGenerationProvider::new("dalle3")))
            .unwrap();

        assert!(registry.contains("dalle3"));
        let removed = registry.remove("dalle3");
        assert!(removed.is_some());
        assert!(!registry.contains("dalle3"));
    }

    #[test]
    fn test_registry_clear() {
        let mut registry = GenerationProviderRegistry::new();
        registry
            .register("dalle3".to_string(), Arc::new(MockGenerationProvider::new("dalle3")))
            .unwrap();
        registry
            .register("stability".to_string(), Arc::new(MockGenerationProvider::new("stability")))
            .unwrap();

        assert_eq!(registry.len(), 2);
        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_default() {
        let registry = GenerationProviderRegistry::default();
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_registry_provider_usage() {
        use crate::generation::{GenerationRequest, GenerationType};

        let mut registry = GenerationProviderRegistry::new();
        let provider = Arc::new(MockGenerationProvider::new("test"));

        registry.register("test".to_string(), provider).unwrap();

        let provider = registry.get("test").unwrap();
        let request = GenerationRequest::new(GenerationType::Image, "test prompt");
        let output = provider.generate(request).await.unwrap();
        assert_eq!(output.media_type, GenerationType::Image);
    }
}
```

**Step 2: Run tests**

Run: `cd Aether/core && cargo test generation::registry`
Expected: All tests pass

**Step 3: Commit registry**

```bash
git add Aether/core/src/generation/registry.rs
git commit -m "feat(generation): add GenerationProviderRegistry"
```

---

## Task 5: Add Module to lib.rs and Run Full Tests

**Files:**
- Modify: `Aether/core/src/lib.rs`

**Step 1: Add generation module to lib.rs**

Add after line 85 (after `pub mod cowork_ffi;`):

```rust
pub mod generation; // NEW: Media generation providers (image/video/audio)
```

Add to re-exports section (around line 170):

```rust
// Generation exports
pub use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationParams,
    GenerationProgress, GenerationProvider, GenerationProviderRegistry, GenerationRequest,
    GenerationType, MockGenerationProvider,
};
```

**Step 2: Run full test suite**

Run: `cd Aether/core && cargo test`
Expected: All tests pass (including new generation tests)

**Step 3: Run clippy**

Run: `cd Aether/core && cargo clippy -- -D warnings`
Expected: No warnings

**Step 4: Commit integration**

```bash
git add Aether/core/src/lib.rs
git commit -m "feat(generation): export generation module from lib.rs"
```

---

## Task 6: Create Configuration Types

**Files:**
- Create: `Aether/core/src/config/types/generation.rs`
- Modify: `Aether/core/src/config/types/mod.rs`

**Step 1: Write generation config types**

```rust
// Aether/core/src/config/types/generation.rs

//! Generation provider configuration types
//!
//! Contains configuration for media generation providers:
//! - GenerationConfig: Global generation settings
//! - GenerationProviderConfig: Individual provider settings

use crate::generation::GenerationType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Global generation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    /// Default provider for image generation
    #[serde(default)]
    pub default_image_provider: Option<String>,

    /// Default provider for video generation
    #[serde(default)]
    pub default_video_provider: Option<String>,

    /// Default provider for audio generation
    #[serde(default)]
    pub default_audio_provider: Option<String>,

    /// Default provider for speech synthesis
    #[serde(default)]
    pub default_speech_provider: Option<String>,

    /// Directory to save generated files
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,

    /// Max file size (MB) for auto-paste (larger files saved to disk)
    #[serde(default = "default_auto_paste_threshold_mb")]
    pub auto_paste_threshold_mb: u32,

    /// Threshold (seconds) for switching to background execution
    #[serde(default = "default_background_task_threshold_seconds")]
    pub background_task_threshold_seconds: u32,

    /// Enable smart routing based on prompt analysis
    #[serde(default)]
    pub smart_routing_enabled: bool,

    /// Configured providers
    #[serde(default)]
    pub providers: HashMap<String, GenerationProviderConfig>,
}

fn default_output_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| PathBuf::from("~/Downloads"))
        .join("Aether")
        .join("generated")
}

fn default_auto_paste_threshold_mb() -> u32 {
    5 // 5 MB
}

fn default_background_task_threshold_seconds() -> u32 {
    30 // 30 seconds
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            default_image_provider: None,
            default_video_provider: None,
            default_audio_provider: None,
            default_speech_provider: None,
            output_dir: default_output_dir(),
            auto_paste_threshold_mb: default_auto_paste_threshold_mb(),
            background_task_threshold_seconds: default_background_task_threshold_seconds(),
            smart_routing_enabled: false,
            providers: HashMap::new(),
        }
    }
}

impl GenerationConfig {
    /// Get default provider for a generation type
    pub fn default_provider_for(&self, gen_type: GenerationType) -> Option<&str> {
        match gen_type {
            GenerationType::Image => self.default_image_provider.as_deref(),
            GenerationType::Video => self.default_video_provider.as_deref(),
            GenerationType::Audio => self.default_audio_provider.as_deref(),
            GenerationType::Speech => self.default_speech_provider.as_deref(),
        }
    }

    /// Check if a provider is enabled
    pub fn is_provider_enabled(&self, name: &str) -> bool {
        self.providers
            .get(name)
            .map(|p| p.enabled)
            .unwrap_or(false)
    }

    /// Get enabled providers for a generation type
    pub fn enabled_providers_for(&self, gen_type: GenerationType) -> Vec<&str> {
        self.providers
            .iter()
            .filter(|(_, config)| config.enabled && config.capabilities.contains(&gen_type))
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

/// Individual generation provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderConfig {
    /// Provider type: "openai", "stability", "replicate", "banana", "openai_compat"
    pub provider_type: String,

    /// API key (or reference to keychain)
    #[serde(default)]
    pub api_key: Option<String>,

    /// Base URL for API endpoint
    #[serde(default)]
    pub base_url: Option<String>,

    /// Default model name
    #[serde(default)]
    pub model: Option<String>,

    /// Whether this provider is enabled
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,

    /// Provider brand color for UI
    #[serde(default = "default_provider_color")]
    pub color: String,

    /// Supported generation types
    #[serde(default = "default_capabilities")]
    pub capabilities: Vec<GenerationType>,

    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Default generation parameters
    #[serde(default)]
    pub defaults: GenerationDefaults,

    /// Model mappings (for Replicate-style providers)
    #[serde(default)]
    pub models: HashMap<String, String>,
}

fn default_provider_enabled() -> bool {
    false
}

fn default_provider_color() -> String {
    "#808080".to_string()
}

fn default_capabilities() -> Vec<GenerationType> {
    vec![GenerationType::Image]
}

fn default_timeout_seconds() -> u64 {
    120 // 2 minutes for generation (longer than chat)
}

impl Default for GenerationProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: "openai".to_string(),
            api_key: None,
            base_url: None,
            model: None,
            enabled: false,
            color: default_provider_color(),
            capabilities: default_capabilities(),
            timeout_seconds: default_timeout_seconds(),
            defaults: GenerationDefaults::default(),
            models: HashMap::new(),
        }
    }
}

impl GenerationProviderConfig {
    /// Create a test configuration
    pub fn test_config(provider_type: impl Into<String>) -> Self {
        Self {
            provider_type: provider_type.into(),
            api_key: Some("test-key".to_string()),
            enabled: true,
            ..Default::default()
        }
    }

    /// Check if provider supports a generation type
    pub fn supports(&self, gen_type: GenerationType) -> bool {
        self.capabilities.contains(&gen_type)
    }

    /// Get effective base URL
    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }
}

/// Default parameters for generation requests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationDefaults {
    // Image defaults
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub style: Option<String>,

    // Video defaults
    #[serde(default)]
    pub duration_seconds: Option<f32>,
    #[serde(default)]
    pub fps: Option<u32>,

    // Audio/Speech defaults
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub speed: Option<f32>,

    // Common
    #[serde(default)]
    pub num_outputs: Option<u32>,
}

impl GenerationDefaults {
    /// Convert to GenerationParams
    pub fn to_params(&self) -> crate::generation::GenerationParams {
        let mut params = crate::generation::GenerationParams::default();

        // Parse size string like "1024x1024"
        if let Some(ref size) = self.size {
            if let Some((w, h)) = size.split_once('x') {
                params.width = w.parse().ok();
                params.height = h.parse().ok();
            }
        }

        params.quality = self.quality.clone();
        params.style = self.style.clone();
        params.duration_seconds = self.duration_seconds;
        params.fps = self.fps;
        params.voice_id = self.voice.clone();
        params.speed = self.speed;
        params.num_outputs = self.num_outputs;

        params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_config_default() {
        let config = GenerationConfig::default();
        assert!(config.default_image_provider.is_none());
        assert_eq!(config.auto_paste_threshold_mb, 5);
        assert_eq!(config.background_task_threshold_seconds, 30);
    }

    #[test]
    fn test_generation_config_default_provider_for() {
        let mut config = GenerationConfig::default();
        config.default_image_provider = Some("dalle3".to_string());
        config.default_video_provider = Some("stability".to_string());

        assert_eq!(
            config.default_provider_for(GenerationType::Image),
            Some("dalle3")
        );
        assert_eq!(
            config.default_provider_for(GenerationType::Video),
            Some("stability")
        );
        assert_eq!(config.default_provider_for(GenerationType::Audio), None);
    }

    #[test]
    fn test_provider_config_default() {
        let config = GenerationProviderConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.provider_type, "openai");
        assert!(config.capabilities.contains(&GenerationType::Image));
    }

    #[test]
    fn test_provider_config_test() {
        let config = GenerationProviderConfig::test_config("stability");
        assert!(config.enabled);
        assert_eq!(config.provider_type, "stability");
        assert!(config.api_key.is_some());
    }

    #[test]
    fn test_provider_config_supports() {
        let mut config = GenerationProviderConfig::default();
        config.capabilities = vec![GenerationType::Image, GenerationType::Video];

        assert!(config.supports(GenerationType::Image));
        assert!(config.supports(GenerationType::Video));
        assert!(!config.supports(GenerationType::Audio));
    }

    #[test]
    fn test_generation_defaults_to_params() {
        let defaults = GenerationDefaults {
            size: Some("1024x1024".to_string()),
            quality: Some("hd".to_string()),
            style: Some("vivid".to_string()),
            ..Default::default()
        };

        let params = defaults.to_params();
        assert_eq!(params.width, Some(1024));
        assert_eq!(params.height, Some(1024));
        assert_eq!(params.quality, Some("hd".to_string()));
        assert_eq!(params.style, Some("vivid".to_string()));
    }

    #[test]
    fn test_enabled_providers_for() {
        let mut config = GenerationConfig::default();
        config.providers.insert(
            "dalle3".to_string(),
            GenerationProviderConfig {
                enabled: true,
                capabilities: vec![GenerationType::Image],
                ..Default::default()
            },
        );
        config.providers.insert(
            "stability".to_string(),
            GenerationProviderConfig {
                enabled: true,
                capabilities: vec![GenerationType::Image, GenerationType::Video],
                ..Default::default()
            },
        );
        config.providers.insert(
            "disabled".to_string(),
            GenerationProviderConfig {
                enabled: false,
                capabilities: vec![GenerationType::Image],
                ..Default::default()
            },
        );

        let image_providers = config.enabled_providers_for(GenerationType::Image);
        assert_eq!(image_providers.len(), 2);
        assert!(image_providers.contains(&"dalle3"));
        assert!(image_providers.contains(&"stability"));

        let video_providers = config.enabled_providers_for(GenerationType::Video);
        assert_eq!(video_providers.len(), 1);
        assert!(video_providers.contains(&"stability"));
    }
}
```

**Step 2: Add to config/types/mod.rs**

Find the config types mod.rs and add:

```rust
pub mod generation;
pub use generation::{GenerationConfig, GenerationDefaults, GenerationProviderConfig};
```

**Step 3: Run tests**

Run: `cd Aether/core && cargo test config::types::generation`
Expected: All tests pass

**Step 4: Commit config types**

```bash
git add Aether/core/src/config/types/generation.rs Aether/core/src/config/types/mod.rs
git commit -m "feat(generation): add GenerationConfig and GenerationProviderConfig"
```

---

## Task 7: Final Integration and Full Test

**Files:**
- Modify: `Aether/core/src/lib.rs` (add config exports)
- Modify: `Aether/core/src/config/mod.rs` (re-export generation config)

**Step 1: Add config exports to lib.rs**

Find the config re-exports section and add:

```rust
pub use crate::config::{GenerationConfig, GenerationDefaults, GenerationProviderConfig};
```

**Step 2: Run full test suite**

Run: `cd Aether/core && cargo test`
Expected: All tests pass

**Step 3: Run clippy**

Run: `cd Aether/core && cargo clippy -- -D warnings`
Expected: No warnings

**Step 4: Build release**

Run: `cd Aether/core && cargo build --release`
Expected: Build succeeds

**Step 5: Final commit**

```bash
git add -A
git commit -m "feat(generation): complete Phase 1 - core framework

- GenerationType enum (Image/Video/Audio/Speech)
- GenerationParams with builder pattern and merge
- GenerationRequest/Output types
- GenerationError with classification helpers
- GenerationProvider trait (async generate, progress)
- GenerationProviderRegistry
- GenerationConfig and GenerationProviderConfig
- MockGenerationProvider for testing
- Full test coverage"
```

---

## Summary

**Phase 1 delivers:**

1. ✅ `GenerationType` enum with display helpers
2. ✅ `GenerationParams` with builder pattern and merge
3. ✅ `GenerationRequest` and `GenerationOutput` types
4. ✅ `GenerationData` enum (Bytes/Url/LocalPath)
5. ✅ `GenerationError` with classification (retryable/user-action/fallback)
6. ✅ `GenerationProvider` trait (async generate, estimate_duration, check_progress)
7. ✅ `GenerationProviderRegistry` (register, get, filter by type)
8. ✅ `GenerationConfig` and `GenerationProviderConfig`
9. ✅ `MockGenerationProvider` for testing
10. ✅ Full test coverage

**Next Phase:** Implement OpenAI provider (DALL·E 3 + TTS) and openai_compat.

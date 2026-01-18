/// Type definitions for the media generation module
///
/// This module defines all the core types used for media generation operations
/// including images, videos, audio, and speech synthesis.
///
/// # Core Types
///
/// - `GenerationType`: Enum representing the type of media to generate
/// - `GenerationParams`: Parameter superset with builder pattern
/// - `GenerationRequest`: Complete generation request
/// - `GenerationOutput`: Generation result with metadata
/// - `GenerationData`: The actual generated content (bytes, URL, or file path)
/// - `GenerationMetadata`: Additional information about the generation
/// - `GenerationProgress`: Progress tracking for long-running generations
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Type of media generation operation
///
/// Each variant represents a different category of generative AI capability.
///
/// # Example
///
/// ```rust
/// use aethecore::generation::GenerationType;
///
/// let gen_type = GenerationType::Image;
/// assert!(gen_type.supports_style());
/// assert!(!gen_type.supports_voice());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationType {
    /// Image generation (DALL-E, Stable Diffusion, Midjourney, etc.)
    Image,
    /// Video generation (Runway, Pika, Sora, etc.)
    Video,
    /// Audio/music generation (Suno, Udio, etc.)
    Audio,
    /// Text-to-speech synthesis (ElevenLabs, OpenAI TTS, etc.)
    Speech,
}

impl GenerationType {
    /// Check if this generation type supports style parameters
    ///
    /// # Returns
    ///
    /// `true` for Image and Video which typically support style options
    pub fn supports_style(&self) -> bool {
        matches!(self, GenerationType::Image | GenerationType::Video)
    }

    /// Check if this generation type supports voice parameters
    ///
    /// # Returns
    ///
    /// `true` for Speech which requires voice selection
    pub fn supports_voice(&self) -> bool {
        matches!(self, GenerationType::Speech)
    }

    /// Check if this generation type typically produces long-running operations
    ///
    /// # Returns
    ///
    /// `true` for Video and Audio which often require async polling
    pub fn is_long_running(&self) -> bool {
        matches!(self, GenerationType::Video | GenerationType::Audio)
    }

    /// Get a human-readable name for this generation type
    pub fn display_name(&self) -> &'static str {
        match self {
            GenerationType::Image => "Image",
            GenerationType::Video => "Video",
            GenerationType::Audio => "Audio",
            GenerationType::Speech => "Speech",
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
/// This is a superset of all possible parameters across different generation types.
/// Use the builder pattern for convenient construction.
///
/// # Example
///
/// ```rust
/// use aethecore::generation::GenerationParams;
///
/// let params = GenerationParams::builder()
///     .width(1024)
///     .height(1024)
///     .quality("hd")
///     .style("vivid")
///     .build();
///
/// assert_eq!(params.width, Some(1024));
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationParams {
    // === Image/Video parameters ===
    /// Width in pixels
    pub width: Option<u32>,
    /// Height in pixels
    pub height: Option<u32>,
    /// Aspect ratio (e.g., "16:9", "1:1")
    pub aspect_ratio: Option<String>,
    /// Quality level (e.g., "standard", "hd")
    pub quality: Option<String>,
    /// Style preset (e.g., "vivid", "natural")
    pub style: Option<String>,
    /// Number of outputs to generate
    pub n: Option<u32>,
    /// Random seed for reproducibility
    pub seed: Option<i64>,
    /// Output format (e.g., "png", "webp", "mp4")
    pub format: Option<String>,

    // === Video-specific parameters ===
    /// Video duration in seconds
    pub duration_seconds: Option<f32>,
    /// Frames per second
    pub fps: Option<u32>,

    // === Audio/Speech parameters ===
    /// Voice ID or name for TTS
    pub voice: Option<String>,
    /// Speaking speed (0.5 to 2.0)
    pub speed: Option<f32>,
    /// Language code (e.g., "en", "zh")
    pub language: Option<String>,

    // === Common parameters ===
    /// Model name/version to use
    pub model: Option<String>,
    /// Negative prompt (what to avoid)
    pub negative_prompt: Option<String>,
    /// Guidance scale / CFG scale
    pub guidance_scale: Option<f32>,
    /// Number of inference steps
    pub steps: Option<u32>,

    // === Reference inputs ===
    /// Reference image URL or base64
    pub reference_image: Option<String>,
    /// Reference audio URL or base64
    pub reference_audio: Option<String>,

    // === Provider-specific parameters ===
    /// Additional provider-specific parameters
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl GenerationParams {
    /// Create a new empty GenerationParams
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a builder for GenerationParams
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::GenerationParams;
    ///
    /// let params = GenerationParams::builder()
    ///     .width(512)
    ///     .height(512)
    ///     .build();
    /// ```
    pub fn builder() -> GenerationParamsBuilder {
        GenerationParamsBuilder::default()
    }

    /// Merge another GenerationParams into this one
    ///
    /// Values from `other` will override values in `self` if they are `Some`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::GenerationParams;
    ///
    /// let mut base = GenerationParams::builder()
    ///     .width(512)
    ///     .quality("standard")
    ///     .build();
    ///
    /// let override_params = GenerationParams::builder()
    ///     .width(1024)
    ///     .style("vivid")
    ///     .build();
    ///
    /// base.merge(override_params);
    ///
    /// assert_eq!(base.width, Some(1024)); // Overridden
    /// assert_eq!(base.quality, Some("standard".to_string())); // Kept
    /// assert_eq!(base.style, Some("vivid".to_string())); // Added
    /// ```
    pub fn merge(&mut self, other: GenerationParams) {
        if other.width.is_some() {
            self.width = other.width;
        }
        if other.height.is_some() {
            self.height = other.height;
        }
        if other.aspect_ratio.is_some() {
            self.aspect_ratio = other.aspect_ratio;
        }
        if other.quality.is_some() {
            self.quality = other.quality;
        }
        if other.style.is_some() {
            self.style = other.style;
        }
        if other.n.is_some() {
            self.n = other.n;
        }
        if other.seed.is_some() {
            self.seed = other.seed;
        }
        if other.format.is_some() {
            self.format = other.format;
        }
        if other.duration_seconds.is_some() {
            self.duration_seconds = other.duration_seconds;
        }
        if other.fps.is_some() {
            self.fps = other.fps;
        }
        if other.voice.is_some() {
            self.voice = other.voice;
        }
        if other.speed.is_some() {
            self.speed = other.speed;
        }
        if other.language.is_some() {
            self.language = other.language;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.negative_prompt.is_some() {
            self.negative_prompt = other.negative_prompt;
        }
        if other.guidance_scale.is_some() {
            self.guidance_scale = other.guidance_scale;
        }
        if other.steps.is_some() {
            self.steps = other.steps;
        }
        if other.reference_image.is_some() {
            self.reference_image = other.reference_image;
        }
        if other.reference_audio.is_some() {
            self.reference_audio = other.reference_audio;
        }
        // Merge extra parameters
        for (key, value) in other.extra {
            self.extra.insert(key, value);
        }
    }

    /// Create a merged copy without modifying the original
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::GenerationParams;
    ///
    /// let base = GenerationParams::builder().width(512).build();
    /// let other = GenerationParams::builder().height(512).build();
    ///
    /// let merged = base.merged_with(other);
    ///
    /// assert_eq!(merged.width, Some(512));
    /// assert_eq!(merged.height, Some(512));
    /// ```
    pub fn merged_with(&self, other: GenerationParams) -> GenerationParams {
        let mut result = self.clone();
        result.merge(other);
        result
    }
}

/// Builder for GenerationParams
///
/// Provides a fluent interface for constructing GenerationParams.
#[derive(Debug, Default)]
pub struct GenerationParamsBuilder {
    params: GenerationParams,
}

impl GenerationParamsBuilder {
    /// Set the width in pixels
    pub fn width(mut self, width: u32) -> Self {
        self.params.width = Some(width);
        self
    }

    /// Set the height in pixels
    pub fn height(mut self, height: u32) -> Self {
        self.params.height = Some(height);
        self
    }

    /// Set the aspect ratio
    pub fn aspect_ratio<S: Into<String>>(mut self, ratio: S) -> Self {
        self.params.aspect_ratio = Some(ratio.into());
        self
    }

    /// Set the quality level
    pub fn quality<S: Into<String>>(mut self, quality: S) -> Self {
        self.params.quality = Some(quality.into());
        self
    }

    /// Set the style preset
    pub fn style<S: Into<String>>(mut self, style: S) -> Self {
        self.params.style = Some(style.into());
        self
    }

    /// Set the number of outputs to generate
    pub fn n(mut self, n: u32) -> Self {
        self.params.n = Some(n);
        self
    }

    /// Set the random seed
    pub fn seed(mut self, seed: i64) -> Self {
        self.params.seed = Some(seed);
        self
    }

    /// Set the output format
    pub fn format<S: Into<String>>(mut self, format: S) -> Self {
        self.params.format = Some(format.into());
        self
    }

    /// Set the video duration in seconds
    pub fn duration_seconds(mut self, duration: f32) -> Self {
        self.params.duration_seconds = Some(duration);
        self
    }

    /// Set the frames per second
    pub fn fps(mut self, fps: u32) -> Self {
        self.params.fps = Some(fps);
        self
    }

    /// Set the voice for TTS
    pub fn voice<S: Into<String>>(mut self, voice: S) -> Self {
        self.params.voice = Some(voice.into());
        self
    }

    /// Set the speaking speed
    pub fn speed(mut self, speed: f32) -> Self {
        self.params.speed = Some(speed);
        self
    }

    /// Set the language code
    pub fn language<S: Into<String>>(mut self, language: S) -> Self {
        self.params.language = Some(language.into());
        self
    }

    /// Set the model name
    pub fn model<S: Into<String>>(mut self, model: S) -> Self {
        self.params.model = Some(model.into());
        self
    }

    /// Set the negative prompt
    pub fn negative_prompt<S: Into<String>>(mut self, prompt: S) -> Self {
        self.params.negative_prompt = Some(prompt.into());
        self
    }

    /// Set the guidance scale
    pub fn guidance_scale(mut self, scale: f32) -> Self {
        self.params.guidance_scale = Some(scale);
        self
    }

    /// Set the number of inference steps
    pub fn steps(mut self, steps: u32) -> Self {
        self.params.steps = Some(steps);
        self
    }

    /// Set the reference image
    pub fn reference_image<S: Into<String>>(mut self, image: S) -> Self {
        self.params.reference_image = Some(image.into());
        self
    }

    /// Set the reference audio
    pub fn reference_audio<S: Into<String>>(mut self, audio: S) -> Self {
        self.params.reference_audio = Some(audio.into());
        self
    }

    /// Add a custom extra parameter
    pub fn extra<S: Into<String>>(mut self, key: S, value: serde_json::Value) -> Self {
        self.params.extra.insert(key.into(), value);
        self
    }

    /// Build the GenerationParams
    pub fn build(self) -> GenerationParams {
        self.params
    }
}

/// A complete generation request
///
/// Contains all information needed to execute a generation operation.
///
/// # Example
///
/// ```rust
/// use aethecore::generation::{GenerationRequest, GenerationType, GenerationParams};
///
/// let request = GenerationRequest::new(
///     GenerationType::Image,
///     "A beautiful sunset over mountains",
/// )
/// .with_params(GenerationParams::builder().width(1024).height(1024).build());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationRequest {
    /// Type of generation to perform
    pub generation_type: GenerationType,
    /// The prompt/input text
    pub prompt: String,
    /// Generation parameters
    pub params: GenerationParams,
    /// Optional request ID for tracking
    pub request_id: Option<String>,
    /// Optional user ID for tracking
    pub user_id: Option<String>,
}

impl GenerationRequest {
    /// Create a new generation request
    ///
    /// # Arguments
    ///
    /// * `generation_type` - Type of media to generate
    /// * `prompt` - The input prompt text
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::{GenerationRequest, GenerationType};
    ///
    /// let request = GenerationRequest::new(
    ///     GenerationType::Image,
    ///     "A cat wearing a hat",
    /// );
    /// ```
    pub fn new<S: Into<String>>(generation_type: GenerationType, prompt: S) -> Self {
        Self {
            generation_type,
            prompt: prompt.into(),
            params: GenerationParams::default(),
            request_id: None,
            user_id: None,
        }
    }

    /// Add parameters to the request
    pub fn with_params(mut self, params: GenerationParams) -> Self {
        self.params = params;
        self
    }

    /// Set the request ID
    pub fn with_request_id<S: Into<String>>(mut self, id: S) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// Set the user ID
    pub fn with_user_id<S: Into<String>>(mut self, id: S) -> Self {
        self.user_id = Some(id.into());
        self
    }

    /// Create an image generation request
    pub fn image<S: Into<String>>(prompt: S) -> Self {
        Self::new(GenerationType::Image, prompt)
    }

    /// Create a video generation request
    pub fn video<S: Into<String>>(prompt: S) -> Self {
        Self::new(GenerationType::Video, prompt)
    }

    /// Create an audio generation request
    pub fn audio<S: Into<String>>(prompt: S) -> Self {
        Self::new(GenerationType::Audio, prompt)
    }

    /// Create a speech generation request
    pub fn speech<S: Into<String>>(text: S) -> Self {
        Self::new(GenerationType::Speech, text)
    }
}

/// The generated content data
///
/// Represents the actual output of a generation operation.
/// Can be raw bytes, a URL, or a local file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum GenerationData {
    /// Raw binary data (e.g., PNG bytes)
    Bytes(Vec<u8>),
    /// URL to the generated content
    Url(String),
    /// Path to a local file
    LocalPath(String),
}

impl GenerationData {
    /// Create a Bytes variant
    pub fn bytes(data: Vec<u8>) -> Self {
        GenerationData::Bytes(data)
    }

    /// Create a URL variant
    pub fn url<S: Into<String>>(url: S) -> Self {
        GenerationData::Url(url.into())
    }

    /// Create a LocalPath variant
    pub fn local_path<S: Into<String>>(path: S) -> Self {
        GenerationData::LocalPath(path.into())
    }

    /// Check if this is raw bytes
    pub fn is_bytes(&self) -> bool {
        matches!(self, GenerationData::Bytes(_))
    }

    /// Check if this is a URL
    pub fn is_url(&self) -> bool {
        matches!(self, GenerationData::Url(_))
    }

    /// Check if this is a local path
    pub fn is_local_path(&self) -> bool {
        matches!(self, GenerationData::LocalPath(_))
    }

    /// Get the URL if this is a URL variant
    pub fn as_url(&self) -> Option<&str> {
        match self {
            GenerationData::Url(url) => Some(url),
            _ => None,
        }
    }

    /// Get the bytes if this is a Bytes variant
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            GenerationData::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Get the local path if this is a LocalPath variant
    pub fn as_local_path(&self) -> Option<&str> {
        match self {
            GenerationData::LocalPath(path) => Some(path),
            _ => None,
        }
    }
}

/// Metadata about a generation operation
///
/// Contains additional information about the generation result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationMetadata {
    /// Provider that performed the generation
    pub provider: Option<String>,
    /// Model used for generation
    pub model: Option<String>,
    /// Time taken for generation
    pub duration: Option<Duration>,
    /// Seed used (if applicable)
    pub seed: Option<i64>,
    /// Revised prompt (if the provider modified it)
    pub revised_prompt: Option<String>,
    /// Content type / MIME type
    pub content_type: Option<String>,
    /// File size in bytes
    pub size_bytes: Option<u64>,
    /// Width in pixels (for images/videos)
    pub width: Option<u32>,
    /// Height in pixels (for images/videos)
    pub height: Option<u32>,
    /// Duration in seconds (for videos/audio)
    pub duration_seconds: Option<f32>,
    /// Additional provider-specific metadata
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl GenerationMetadata {
    /// Create new empty metadata
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the provider name
    pub fn with_provider<S: Into<String>>(mut self, provider: S) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Set the model name
    pub fn with_model<S: Into<String>>(mut self, model: S) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the generation duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Set the seed
    pub fn with_seed(mut self, seed: i64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set the revised prompt
    pub fn with_revised_prompt<S: Into<String>>(mut self, prompt: S) -> Self {
        self.revised_prompt = Some(prompt.into());
        self
    }

    /// Set the content type
    pub fn with_content_type<S: Into<String>>(mut self, content_type: S) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the file size
    pub fn with_size_bytes(mut self, size: u64) -> Self {
        self.size_bytes = Some(size);
        self
    }

    /// Set dimensions
    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Set duration in seconds
    pub fn with_duration_seconds(mut self, seconds: f32) -> Self {
        self.duration_seconds = Some(seconds);
        self
    }
}

/// The complete output of a generation operation
///
/// Contains the generated content, metadata, and any additional outputs.
///
/// # Example
///
/// ```rust
/// use aethecore::generation::{GenerationOutput, GenerationType, GenerationData, GenerationMetadata};
///
/// let output = GenerationOutput::new(
///     GenerationType::Image,
///     GenerationData::url("https://example.com/image.png"),
/// );
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationOutput {
    /// Type of generation that was performed
    pub generation_type: GenerationType,
    /// The primary generated content
    pub data: GenerationData,
    /// Additional outputs (for n > 1)
    pub additional_outputs: Vec<GenerationData>,
    /// Metadata about the generation
    pub metadata: GenerationMetadata,
    /// Original request ID if provided
    pub request_id: Option<String>,
}

impl GenerationOutput {
    /// Create a new generation output
    ///
    /// # Arguments
    ///
    /// * `generation_type` - Type of media that was generated
    /// * `data` - The generated content
    pub fn new(generation_type: GenerationType, data: GenerationData) -> Self {
        Self {
            generation_type,
            data,
            additional_outputs: Vec::new(),
            metadata: GenerationMetadata::default(),
            request_id: None,
        }
    }

    /// Add metadata to the output
    pub fn with_metadata(mut self, metadata: GenerationMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Add additional outputs
    pub fn with_additional_outputs(mut self, outputs: Vec<GenerationData>) -> Self {
        self.additional_outputs = outputs;
        self
    }

    /// Set the request ID
    pub fn with_request_id<S: Into<String>>(mut self, id: S) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// Get the total number of outputs (primary + additional)
    pub fn output_count(&self) -> usize {
        1 + self.additional_outputs.len()
    }

    /// Iterate over all outputs (primary first, then additional)
    pub fn all_outputs(&self) -> impl Iterator<Item = &GenerationData> {
        std::iter::once(&self.data).chain(self.additional_outputs.iter())
    }
}

/// Progress information for long-running generation operations
///
/// Used for video and audio generation which may take significant time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProgress {
    /// Current progress percentage (0-100)
    pub percentage: f32,
    /// Current step/phase description
    pub step: String,
    /// Estimated time remaining
    pub eta: Option<Duration>,
    /// Whether the operation is complete
    pub is_complete: bool,
    /// Optional preview URL
    pub preview_url: Option<String>,
}

impl GenerationProgress {
    /// Create a new progress indicator
    ///
    /// # Arguments
    ///
    /// * `percentage` - Progress from 0 to 100
    /// * `step` - Description of current step
    pub fn new<S: Into<String>>(percentage: f32, step: S) -> Self {
        Self {
            percentage: percentage.clamp(0.0, 100.0),
            step: step.into(),
            eta: None,
            is_complete: percentage >= 100.0,
            preview_url: None,
        }
    }

    /// Create a progress indicator for a started operation
    pub fn started<S: Into<String>>(step: S) -> Self {
        Self::new(0.0, step)
    }

    /// Create a progress indicator for a completed operation
    pub fn completed() -> Self {
        Self {
            percentage: 100.0,
            step: "Complete".to_string(),
            eta: None,
            is_complete: true,
            preview_url: None,
        }
    }

    /// Set the ETA
    pub fn with_eta(mut self, eta: Duration) -> Self {
        self.eta = Some(eta);
        self
    }

    /// Set a preview URL
    pub fn with_preview<S: Into<String>>(mut self, url: S) -> Self {
        self.preview_url = Some(url.into());
        self
    }
}

impl Default for GenerationProgress {
    fn default() -> Self {
        Self::new(0.0, "Starting")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === GenerationType tests ===

    #[test]
    fn test_generation_type_supports_style() {
        assert!(GenerationType::Image.supports_style());
        assert!(GenerationType::Video.supports_style());
        assert!(!GenerationType::Audio.supports_style());
        assert!(!GenerationType::Speech.supports_style());
    }

    #[test]
    fn test_generation_type_supports_voice() {
        assert!(!GenerationType::Image.supports_voice());
        assert!(!GenerationType::Video.supports_voice());
        assert!(!GenerationType::Audio.supports_voice());
        assert!(GenerationType::Speech.supports_voice());
    }

    #[test]
    fn test_generation_type_is_long_running() {
        assert!(!GenerationType::Image.is_long_running());
        assert!(GenerationType::Video.is_long_running());
        assert!(GenerationType::Audio.is_long_running());
        assert!(!GenerationType::Speech.is_long_running());
    }

    #[test]
    fn test_generation_type_display() {
        assert_eq!(GenerationType::Image.to_string(), "Image");
        assert_eq!(GenerationType::Video.to_string(), "Video");
        assert_eq!(GenerationType::Audio.to_string(), "Audio");
        assert_eq!(GenerationType::Speech.to_string(), "Speech");
    }

    #[test]
    fn test_generation_type_serialization() {
        let json = serde_json::to_string(&GenerationType::Image).unwrap();
        assert_eq!(json, "\"image\"");

        let parsed: GenerationType = serde_json::from_str("\"video\"").unwrap();
        assert_eq!(parsed, GenerationType::Video);
    }

    // === GenerationParams tests ===

    #[test]
    fn test_generation_params_builder() {
        let params = GenerationParams::builder()
            .width(1024)
            .height(768)
            .quality("hd")
            .style("vivid")
            .n(2)
            .seed(12345)
            .build();

        assert_eq!(params.width, Some(1024));
        assert_eq!(params.height, Some(768));
        assert_eq!(params.quality, Some("hd".to_string()));
        assert_eq!(params.style, Some("vivid".to_string()));
        assert_eq!(params.n, Some(2));
        assert_eq!(params.seed, Some(12345));
    }

    #[test]
    fn test_generation_params_merge() {
        let mut base = GenerationParams::builder()
            .width(512)
            .quality("standard")
            .model("dall-e-3")
            .build();

        let override_params = GenerationParams::builder()
            .width(1024)
            .style("vivid")
            .build();

        base.merge(override_params);

        assert_eq!(base.width, Some(1024)); // Overridden
        assert_eq!(base.quality, Some("standard".to_string())); // Kept
        assert_eq!(base.style, Some("vivid".to_string())); // Added
        assert_eq!(base.model, Some("dall-e-3".to_string())); // Kept
    }

    #[test]
    fn test_generation_params_merged_with() {
        let base = GenerationParams::builder()
            .width(512)
            .quality("standard")
            .build();

        let other = GenerationParams::builder()
            .height(512)
            .style("vivid")
            .build();

        let merged = base.merged_with(other);

        // Original unchanged
        assert_eq!(base.height, None);
        assert_eq!(base.style, None);

        // Merged has both
        assert_eq!(merged.width, Some(512));
        assert_eq!(merged.height, Some(512));
        assert_eq!(merged.quality, Some("standard".to_string()));
        assert_eq!(merged.style, Some("vivid".to_string()));
    }

    #[test]
    fn test_generation_params_extra() {
        let params = GenerationParams::builder()
            .extra("custom_key", serde_json::json!("custom_value"))
            .extra("numeric", serde_json::json!(42))
            .build();

        assert_eq!(
            params.extra.get("custom_key"),
            Some(&serde_json::json!("custom_value"))
        );
        assert_eq!(params.extra.get("numeric"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_generation_params_serialization() {
        let params = GenerationParams::builder()
            .width(1024)
            .height(1024)
            .quality("hd")
            .build();

        let json = serde_json::to_string(&params).unwrap();
        let parsed: GenerationParams = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.width, Some(1024));
        assert_eq!(parsed.height, Some(1024));
        assert_eq!(parsed.quality, Some("hd".to_string()));
    }

    // === GenerationRequest tests ===

    #[test]
    fn test_generation_request_new() {
        let request = GenerationRequest::new(GenerationType::Image, "A cat");

        assert_eq!(request.generation_type, GenerationType::Image);
        assert_eq!(request.prompt, "A cat");
        assert!(request.request_id.is_none());
        assert!(request.user_id.is_none());
    }

    #[test]
    fn test_generation_request_convenience_constructors() {
        let image = GenerationRequest::image("prompt");
        assert_eq!(image.generation_type, GenerationType::Image);

        let video = GenerationRequest::video("prompt");
        assert_eq!(video.generation_type, GenerationType::Video);

        let audio = GenerationRequest::audio("prompt");
        assert_eq!(audio.generation_type, GenerationType::Audio);

        let speech = GenerationRequest::speech("prompt");
        assert_eq!(speech.generation_type, GenerationType::Speech);
    }

    #[test]
    fn test_generation_request_with_params() {
        let params = GenerationParams::builder().width(1024).build();
        let request = GenerationRequest::image("A sunset")
            .with_params(params)
            .with_request_id("req-123")
            .with_user_id("user-456");

        assert_eq!(request.params.width, Some(1024));
        assert_eq!(request.request_id, Some("req-123".to_string()));
        assert_eq!(request.user_id, Some("user-456".to_string()));
    }

    // === GenerationData tests ===

    #[test]
    fn test_generation_data_bytes() {
        let data = GenerationData::bytes(vec![1, 2, 3, 4]);

        assert!(data.is_bytes());
        assert!(!data.is_url());
        assert!(!data.is_local_path());
        assert_eq!(data.as_bytes(), Some(&[1, 2, 3, 4][..]));
        assert_eq!(data.as_url(), None);
    }

    #[test]
    fn test_generation_data_url() {
        let data = GenerationData::url("https://example.com/image.png");

        assert!(!data.is_bytes());
        assert!(data.is_url());
        assert!(!data.is_local_path());
        assert_eq!(data.as_url(), Some("https://example.com/image.png"));
        assert_eq!(data.as_bytes(), None);
    }

    #[test]
    fn test_generation_data_local_path() {
        let data = GenerationData::local_path("/tmp/image.png");

        assert!(!data.is_bytes());
        assert!(!data.is_url());
        assert!(data.is_local_path());
        assert_eq!(data.as_local_path(), Some("/tmp/image.png"));
    }

    #[test]
    fn test_generation_data_serialization() {
        let data = GenerationData::url("https://example.com/image.png");
        let json = serde_json::to_string(&data).unwrap();

        assert!(json.contains("Url"));
        assert!(json.contains("https://example.com/image.png"));

        let parsed: GenerationData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_url(), Some("https://example.com/image.png"));
    }

    // === GenerationMetadata tests ===

    #[test]
    fn test_generation_metadata_builder() {
        let metadata = GenerationMetadata::new()
            .with_provider("openai")
            .with_model("dall-e-3")
            .with_duration(Duration::from_secs(5))
            .with_seed(12345)
            .with_content_type("image/png")
            .with_size_bytes(102400)
            .with_dimensions(1024, 1024);

        assert_eq!(metadata.provider, Some("openai".to_string()));
        assert_eq!(metadata.model, Some("dall-e-3".to_string()));
        assert_eq!(metadata.duration, Some(Duration::from_secs(5)));
        assert_eq!(metadata.seed, Some(12345));
        assert_eq!(metadata.content_type, Some("image/png".to_string()));
        assert_eq!(metadata.size_bytes, Some(102400));
        assert_eq!(metadata.width, Some(1024));
        assert_eq!(metadata.height, Some(1024));
    }

    #[test]
    fn test_generation_metadata_revised_prompt() {
        let metadata =
            GenerationMetadata::new().with_revised_prompt("An enhanced description of a cat");

        assert_eq!(
            metadata.revised_prompt,
            Some("An enhanced description of a cat".to_string())
        );
    }

    // === GenerationOutput tests ===

    #[test]
    fn test_generation_output_new() {
        let data = GenerationData::url("https://example.com/image.png");
        let output = GenerationOutput::new(GenerationType::Image, data);

        assert_eq!(output.generation_type, GenerationType::Image);
        assert!(output.data.is_url());
        assert_eq!(output.output_count(), 1);
        assert!(output.additional_outputs.is_empty());
    }

    #[test]
    fn test_generation_output_with_additional() {
        let primary = GenerationData::url("https://example.com/image1.png");
        let additional = vec![
            GenerationData::url("https://example.com/image2.png"),
            GenerationData::url("https://example.com/image3.png"),
        ];

        let output = GenerationOutput::new(GenerationType::Image, primary)
            .with_additional_outputs(additional);

        assert_eq!(output.output_count(), 3);

        let all: Vec<_> = output.all_outputs().collect();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_generation_output_with_metadata() {
        let data = GenerationData::url("https://example.com/image.png");
        let metadata = GenerationMetadata::new()
            .with_provider("openai")
            .with_model("dall-e-3");

        let output = GenerationOutput::new(GenerationType::Image, data)
            .with_metadata(metadata)
            .with_request_id("req-123");

        assert_eq!(output.metadata.provider, Some("openai".to_string()));
        assert_eq!(output.metadata.model, Some("dall-e-3".to_string()));
        assert_eq!(output.request_id, Some("req-123".to_string()));
    }

    // === GenerationProgress tests ===

    #[test]
    fn test_generation_progress_new() {
        let progress = GenerationProgress::new(50.0, "Processing");

        assert_eq!(progress.percentage, 50.0);
        assert_eq!(progress.step, "Processing");
        assert!(!progress.is_complete);
    }

    #[test]
    fn test_generation_progress_clamps() {
        let low = GenerationProgress::new(-10.0, "Start");
        assert_eq!(low.percentage, 0.0);

        let high = GenerationProgress::new(150.0, "End");
        assert_eq!(high.percentage, 100.0);
        assert!(high.is_complete);
    }

    #[test]
    fn test_generation_progress_started() {
        let progress = GenerationProgress::started("Initializing");

        assert_eq!(progress.percentage, 0.0);
        assert_eq!(progress.step, "Initializing");
        assert!(!progress.is_complete);
    }

    #[test]
    fn test_generation_progress_completed() {
        let progress = GenerationProgress::completed();

        assert_eq!(progress.percentage, 100.0);
        assert!(progress.is_complete);
    }

    #[test]
    fn test_generation_progress_with_eta() {
        let progress =
            GenerationProgress::new(50.0, "Processing").with_eta(Duration::from_secs(30));

        assert_eq!(progress.eta, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_generation_progress_with_preview() {
        let progress = GenerationProgress::new(75.0, "Rendering")
            .with_preview("https://example.com/preview.png");

        assert_eq!(
            progress.preview_url,
            Some("https://example.com/preview.png".to_string())
        );
    }

    #[test]
    fn test_generation_progress_default() {
        let progress = GenerationProgress::default();

        assert_eq!(progress.percentage, 0.0);
        assert_eq!(progress.step, "Starting");
        assert!(!progress.is_complete);
    }
}

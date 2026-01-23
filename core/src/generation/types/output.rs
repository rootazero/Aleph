/// Generation output types
///
/// Contains the generated content data, metadata, and complete output structure.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

use super::generation_type::GenerationType;

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

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
use serde::{Deserialize, Serialize};

use super::generation_type::GenerationType;
use super::params::GenerationParams;

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

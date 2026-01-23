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
use serde::{Deserialize, Serialize};

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

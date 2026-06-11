/// Type of media generation operation
///
/// Each variant represents a different category of generative AI capability.
///
/// # Example
///
/// ```rust
/// use alephcore::generation::GenerationType;
///
/// let gen_type = GenerationType::Image;
/// assert!(gen_type.supports_style());
/// assert!(!gen_type.supports_voice());
/// ```
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationType {
    /// Image generation (DALL-E, Stable Diffusion, Midjourney, etc.)
    Image,
    /// Video generation (Runway, Pika, Sora, etc.)
    Video,
    /// Audio/music generation (Suno, Udio, etc.)
    Audio,
    /// Text-to-speech synthesis (`ElevenLabs`, `OpenAI` TTS, etc.)
    Speech,
    /// Speech-to-text transcription (Whisper, etc.)
    Transcription,
}

impl GenerationType {
    /// Check if this generation type supports style parameters
    ///
    /// # Returns
    ///
    /// `true` for Image and Video which typically support style options
    #[must_use]
    pub const fn supports_style(&self) -> bool {
        matches!(self, Self::Image | Self::Video)
    }

    /// Check if this generation type supports voice parameters
    ///
    /// # Returns
    ///
    /// `true` for Speech which requires voice selection
    #[must_use]
    pub const fn supports_voice(&self) -> bool {
        matches!(self, Self::Speech)
    }

    /// Check if this generation type typically produces long-running operations
    ///
    /// # Returns
    ///
    /// `true` for Video and Audio which often require async polling
    #[must_use]
    pub const fn is_long_running(&self) -> bool {
        matches!(self, Self::Video | Self::Audio)
    }

    /// Get a human-readable name for this generation type
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Speech => "Speech",
            Self::Transcription => "Transcription",
        }
    }
}

impl std::fmt::Display for GenerationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

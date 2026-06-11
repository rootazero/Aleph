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
    /// Text-to-speech synthesis (`ElevenLabs`, `OpenAI` TTS, etc.)
    Speech,
    /// Speech-to-text / transcription (Whisper, etc.)
    Transcription,
}

impl GenerationType {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Speech => "speech",
            Self::Transcription => "transcription",
        }
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Speech => "Speech",
            Self::Transcription => "STT",
        }
    }

    #[must_use]
    pub const fn icon(&self) -> &'static str {
        match self {
            Self::Image => "🖼️",
            Self::Video => "🎬",
            Self::Audio => "🎵",
            Self::Speech => "🗣️",
            Self::Transcription => "🎤",
        }
    }
}

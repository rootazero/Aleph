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
    /// Speech-to-text / transcription (Whisper, etc.)
    Transcription,
}

impl GenerationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            GenerationType::Image => "image",
            GenerationType::Video => "video",
            GenerationType::Audio => "audio",
            GenerationType::Speech => "speech",
            GenerationType::Transcription => "transcription",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            GenerationType::Image => "Image",
            GenerationType::Video => "Video",
            GenerationType::Audio => "Audio",
            GenerationType::Speech => "Speech",
            GenerationType::Transcription => "Transcription",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            GenerationType::Image => "🖼️",
            GenerationType::Video => "🎬",
            GenerationType::Audio => "🎵",
            GenerationType::Speech => "🗣️",
            GenerationType::Transcription => "🎤",
        }
    }
}

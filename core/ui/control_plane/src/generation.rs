use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    pub fn as_str(&self) -> &'static str {
        match self {
            GenerationType::Image => "image",
            GenerationType::Video => "video",
            GenerationType::Audio => "audio",
            GenerationType::Speech => "speech",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            GenerationType::Image => "Image",
            GenerationType::Video => "Video",
            GenerationType::Audio => "Audio",
            GenerationType::Speech => "Speech",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            GenerationType::Image => "🖼️",
            GenerationType::Video => "🎬",
            GenerationType::Audio => "🎵",
            GenerationType::Speech => "🗣️",
        }
    }
}

use crate::generation::GenerationType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetProvider {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub provider_type: String,
    pub capabilities: Vec<GenerationType>,
    pub default_model: String,
    pub description: String,
    pub base_url: Option<String>,
    pub is_unsupported: bool,
}

pub struct PresetProviders;

impl PresetProviders {
    // Image Providers
    pub fn image_providers() -> Vec<PresetProvider> {
        vec![
            PresetProvider {
                id: "openai-dalle".to_string(),
                name: "OpenAI DALL-E".to_string(),
                icon: "🎨".to_string(),
                color: "#10a37f".to_string(),
                provider_type: "openai".to_string(),
                capabilities: vec![GenerationType::Image],
                default_model: "dall-e-3".to_string(),
                description: "OpenAI's DALL-E image generation models".to_string(),
                base_url: Some("https://api.openai.com".to_string()),
                is_unsupported: false,
            },
            PresetProvider {
                id: "stability-ai".to_string(),
                name: "Stability AI".to_string(),
                icon: "✨".to_string(),
                color: "#8B5CF6".to_string(),
                provider_type: "stability".to_string(),
                capabilities: vec![GenerationType::Image],
                default_model: "stable-diffusion-xl-1024-v1-0".to_string(),
                description: "Stable Diffusion models via Stability AI".to_string(),
                base_url: Some("https://api.stability.ai".to_string()),
                is_unsupported: false,
            },
            PresetProvider {
                id: "google-imagen".to_string(),
                name: "Google Imagen".to_string(),
                icon: "📷".to_string(),
                color: "#4285F4".to_string(),
                provider_type: "google".to_string(),
                capabilities: vec![GenerationType::Image],
                default_model: "imagen-3.0-generate-002".to_string(),
                description: "Google's Imagen image generation via Gemini API".to_string(),
                base_url: None,
                is_unsupported: false,
            },
            PresetProvider {
                id: "replicate".to_string(),
                name: "Replicate".to_string(),
                icon: "🔄".to_string(),
                color: "#F97316".to_string(),
                provider_type: "replicate".to_string(),
                capabilities: vec![GenerationType::Image],
                default_model: "black-forest-labs/flux-schnell".to_string(),
                description: "Run open-source models on Replicate".to_string(),
                base_url: Some("https://api.replicate.com".to_string()),
                is_unsupported: false,
            },
        ]
    }

    // Video Providers
    pub fn video_providers() -> Vec<PresetProvider> {
        vec![
            PresetProvider {
                id: "google-veo".to_string(),
                name: "Google Veo".to_string(),
                icon: "🎬".to_string(),
                color: "#4285F4".to_string(),
                provider_type: "google_veo".to_string(),
                capabilities: vec![GenerationType::Video],
                default_model: "veo-2.0-generate-001".to_string(),
                description: "Google's Veo video generation".to_string(),
                base_url: None,
                is_unsupported: false,
            },
            PresetProvider {
                id: "runway".to_string(),
                name: "Runway".to_string(),
                icon: "▶️".to_string(),
                color: "#00D4AA".to_string(),
                provider_type: "runway".to_string(),
                capabilities: vec![GenerationType::Video],
                default_model: "gen-3".to_string(),
                description: "Runway Gen-3 video generation".to_string(),
                base_url: Some("https://api.runwayml.com/v1".to_string()),
                is_unsupported: true,
            },
            PresetProvider {
                id: "pika".to_string(),
                name: "Pika".to_string(),
                icon: "🔍".to_string(),
                color: "#FF6B6B".to_string(),
                provider_type: "pika".to_string(),
                capabilities: vec![GenerationType::Video],
                default_model: "pika-1.0".to_string(),
                description: "Pika video generation".to_string(),
                base_url: Some("https://api.pika.art/v1".to_string()),
                is_unsupported: true,
            },
        ]
    }

    // Audio Providers
    pub fn audio_providers() -> Vec<PresetProvider> {
        vec![
            PresetProvider {
                id: "openai-tts".to_string(),
                name: "OpenAI TTS".to_string(),
                icon: "🗣️".to_string(),
                color: "#10a37f".to_string(),
                provider_type: "openai_tts".to_string(),
                capabilities: vec![GenerationType::Speech],
                default_model: "tts-1-hd".to_string(),
                description: "OpenAI text-to-speech models".to_string(),
                base_url: Some("https://api.openai.com".to_string()),
                is_unsupported: false,
            },
            PresetProvider {
                id: "elevenlabs".to_string(),
                name: "ElevenLabs".to_string(),
                icon: "🔊".to_string(),
                color: "#000000".to_string(),
                provider_type: "elevenlabs".to_string(),
                capabilities: vec![GenerationType::Speech, GenerationType::Audio],
                default_model: "eleven_multilingual_v2".to_string(),
                description: "ElevenLabs voice synthesis".to_string(),
                base_url: Some("https://api.elevenlabs.io".to_string()),
                is_unsupported: false,
            },
        ]
    }

    // Get all providers
    pub fn all() -> Vec<PresetProvider> {
        let mut all = Vec::new();
        all.extend(Self::image_providers());
        all.extend(Self::video_providers());
        all.extend(Self::audio_providers());
        all
    }

    // Get providers by category
    pub fn by_category(category: GenerationType) -> Vec<PresetProvider> {
        match category {
            GenerationType::Image => Self::image_providers(),
            GenerationType::Video => Self::video_providers(),
            GenerationType::Audio | GenerationType::Speech => Self::audio_providers(),
        }
    }

    // Find preset by ID
    pub fn find(id: &str) -> Option<PresetProvider> {
        Self::all().into_iter().find(|p| p.id == id)
    }
}

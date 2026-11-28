//! Generation provider implementations
//!
//! This module contains concrete implementations of the `GenerationProvider` trait
//! for various AI service providers.
//!
//! # Available Providers
//!
//! The factory (`create_provider`) builds one provider per canonical type:
//!
//! - `OpenAiImageProvider` - DALL-E 3 image generation
//! - `OpenAiTtsProvider` - `OpenAI` Text-to-Speech
//! - `OpenAiWhisperProvider` - `OpenAI` speech-to-text
//! - `DeepgramSttProvider` - Deepgram speech-to-text
//! - `DeepgramTtsProvider` - Deepgram Text-to-Speech
//! - `AzureSpeechProvider` - `Azure` Speech
//! - `SunoProvider` - Suno music generation
//! - `BflProvider` - Black Forest Labs image generation
//! - `CartesiaProvider` - Cartesia Text-to-Speech
//! - `MinimaxTtsProvider` - MiniMax Text-to-Speech
//! - `VolcengineTtsProvider` - Volcengine Text-to-Speech
//! - `OpenAiCompatProvider` - Generic OpenAI-compatible API for third-party proxies
//! - `StabilityImageProvider` - Stability AI (Stable Diffusion XL) image generation
//! - `GoogleImagenProvider` - Google Imagen 3 image generation
//! - `GoogleVeoProvider` - Google Veo 2/3 video generation
//! - `ReplicateProvider` - Replicate API for Flux, SDXL, `MusicGen`, and more
//! - `ElevenLabsProvider` - `ElevenLabs` high-quality Text-to-Speech
//! - `FalProvider` - fal.ai queue API (image / video / music)
//! - `MidjourneyProvider` - `T8Star` Midjourney API proxy for high-quality image generation
//!
//! The spelling a config uses to select one of them — and the aliases each
//! one answers to — lives in `PROVIDER_TYPE_ALIASES` inside `factory.rs`.
//!
//! # Factory Function
//!
//! Use `create_provider()` to create providers from configuration:
//!
//! ```rust,ignore
//! use alephcore::config::GenerationProviderConfig;
//! use alephcore::generation::providers::create_provider;
//!
//! let config = GenerationProviderConfig {
//!     provider_type: "openai".to_string(),
//!     api_key: Some("sk-xxx".to_string()),
//!     model: Some("dall-e-3".to_string()),
//!     ..Default::default()
//! };
//!
//! let provider = create_provider("dalle", &config, alephcore::generation::GenerationType::Image)?;
//! ```

pub mod azure_speech;
pub mod bfl;
pub mod cartesia;
pub mod deepgram_stt;
pub mod deepgram_tts;
pub mod elevenlabs;
mod factory;
pub mod fal;
pub mod google_imagen;
pub mod google_veo;
pub(crate) mod http;
pub mod midjourney;
pub mod minimax_tts;
pub mod openai_compat;
pub mod openai_image;
pub mod openai_tts;
pub mod openai_whisper;
pub mod replicate;
pub mod stability;
pub mod suno;
pub mod url_normalize;
pub mod volcengine_tts;

#[cfg(test)]
mod tests;

pub use azure_speech::AzureSpeechProvider;
pub use bfl::BflProvider;
pub use cartesia::CartesiaProvider;
pub use deepgram_stt::DeepgramSttProvider;
pub use deepgram_tts::DeepgramTtsProvider;
pub use elevenlabs::ElevenLabsProvider;
pub use factory::create_provider;
pub(crate) use factory::canonical_provider_type;
pub use fal::FalProvider;
pub use google_imagen::GoogleImagenProvider;
pub use google_veo::GoogleVeoProvider;
pub use midjourney::{MidjourneyMode, MidjourneyProvider};
pub use minimax_tts::MinimaxTtsProvider;
pub use openai_compat::OpenAiCompatProvider;
pub use openai_image::OpenAiImageProvider;
pub use openai_tts::OpenAiTtsProvider;
pub use openai_whisper::OpenAiWhisperProvider;
pub use replicate::ReplicateProvider;
pub use stability::StabilityImageProvider;
pub use suno::SunoProvider;
pub use volcengine_tts::VolcengineTtsProvider;

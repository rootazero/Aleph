//! Generation provider implementations
//!
//! This module contains concrete implementations of the `GenerationProvider` trait
//! for various AI service providers.
//!
//! # Available Providers
//!
//! - `OpenAiImageProvider` - DALL-E 3 image generation
//! - `OpenAiTtsProvider` - OpenAI Text-to-Speech
//! - `OpenAiCompatProvider` - Generic OpenAI-compatible API for third-party proxies
//! - `StabilityImageProvider` - Stability AI (Stable Diffusion XL) image generation
//! - `GoogleImagenProvider` - Google Imagen 3 image generation
//! - `GoogleVeoProvider` - Google Veo 2/3 video generation
//! - `ReplicateProvider` - Replicate API for Flux, SDXL, MusicGen, and more
//! - `ElevenLabsProvider` - ElevenLabs high-quality Text-to-Speech
//! - `MidjourneyProvider` - T8Star Midjourney API proxy for high-quality image generation
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
//! let provider = create_provider("dalle", &config)?;
//! ```

pub mod elevenlabs;
mod factory;
pub mod google_imagen;
pub mod google_veo;
pub mod midjourney;
pub mod openai_compat;
pub mod openai_image;
pub mod openai_tts;
pub mod replicate;
pub mod stability;
pub mod url_normalize;

#[cfg(test)]
mod tests;

pub use elevenlabs::ElevenLabsProvider;
pub use factory::create_provider;
pub use google_imagen::GoogleImagenProvider;
pub use google_veo::GoogleVeoProvider;
pub use midjourney::{MidjourneyMode, MidjourneyProvider, MidjourneyProviderBuilder};
pub use openai_compat::{OpenAiCompatProvider, OpenAiCompatProviderBuilder};
pub use openai_image::OpenAiImageProvider;
pub use openai_tts::OpenAiTtsProvider;
pub use replicate::{ReplicateProvider, ReplicateProviderBuilder};
pub use stability::StabilityImageProvider;

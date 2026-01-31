//! Generation defaults configuration
//!
//! Contains the GenerationDefaults struct for default generation parameters.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// GenerationDefaults
// =============================================================================

/// Default parameters for generation requests
///
/// These defaults are applied to generation requests when
/// the corresponding parameter is not explicitly specified.
///
/// # Example TOML
/// ```toml
/// [generation.providers.dalle.defaults]
/// width = 1024
/// height = 1024
/// quality = "hd"
/// style = "vivid"
/// n = 1
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GenerationDefaults {
    // === Image/Video parameters ===
    /// Default width in pixels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Default height in pixels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Default aspect ratio (e.g., "16:9", "1:1")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,

    /// Default quality level (e.g., "standard", "hd")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    /// Default style preset (e.g., "vivid", "natural")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    /// Default number of outputs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,

    /// Default output format (e.g., "png", "webp", "mp4")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    // === Video-specific parameters ===
    /// Default video duration in seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f32>,

    /// Default frames per second
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    // === Audio/Speech parameters ===
    /// Default voice ID or name for TTS
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,

    /// Default speaking speed (0.5 to 2.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Default language code (e.g., "en", "zh")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    // === Common parameters ===
    /// Default guidance scale / CFG scale
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance_scale: Option<f32>,

    /// Default number of inference steps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
}

impl GenerationDefaults {
    /// Create new empty defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate the defaults
    pub fn validate(&self, provider_name: &str) -> Result<(), String> {
        // Validate width/height are reasonable
        if let Some(width) = self.width {
            if width == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.width must be greater than 0",
                    provider_name
                ));
            }
            if width > 8192 {
                tracing::warn!(
                    provider = provider_name,
                    width = width,
                    "Default width is very large (>8192)"
                );
            }
        }

        if let Some(height) = self.height {
            if height == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.height must be greater than 0",
                    provider_name
                ));
            }
            if height > 8192 {
                tracing::warn!(
                    provider = provider_name,
                    height = height,
                    "Default height is very large (>8192)"
                );
            }
        }

        // Validate n
        if let Some(n) = self.n {
            if n == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.n must be greater than 0",
                    provider_name
                ));
            }
            if n > 10 {
                tracing::warn!(
                    provider = provider_name,
                    n = n,
                    "Default n is high (>10), may be expensive"
                );
            }
        }

        // Validate speed is in range
        if let Some(speed) = self.speed {
            if !(0.25..=4.0).contains(&speed) {
                return Err(format!(
                    "generation.providers.{}.defaults.speed must be between 0.25 and 4.0, got {}",
                    provider_name, speed
                ));
            }
        }

        // Validate fps
        if let Some(fps) = self.fps {
            if fps == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.fps must be greater than 0",
                    provider_name
                ));
            }
            if fps > 120 {
                tracing::warn!(
                    provider = provider_name,
                    fps = fps,
                    "Default fps is very high (>120)"
                );
            }
        }

        // Validate duration_seconds
        if let Some(duration) = self.duration_seconds {
            if duration <= 0.0 {
                return Err(format!(
                    "generation.providers.{}.defaults.duration_seconds must be greater than 0",
                    provider_name
                ));
            }
        }

        // Validate guidance_scale
        if let Some(scale) = self.guidance_scale {
            if scale < 0.0 {
                return Err(format!(
                    "generation.providers.{}.defaults.guidance_scale must be >= 0, got {}",
                    provider_name, scale
                ));
            }
            if scale > 30.0 {
                tracing::warn!(
                    provider = provider_name,
                    guidance_scale = scale,
                    "Default guidance_scale is very high (>30)"
                );
            }
        }

        // Validate steps
        if let Some(steps) = self.steps {
            if steps == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.steps must be greater than 0",
                    provider_name
                ));
            }
            if steps > 150 {
                tracing::warn!(
                    provider = provider_name,
                    steps = steps,
                    "Default steps is high (>150), generation will be slow"
                );
            }
        }

        Ok(())
    }

    /// Convert to GenerationParams from the generation module
    pub fn to_params(&self) -> crate::generation::GenerationParams {
        let mut builder = crate::generation::GenerationParams::builder();

        if let Some(width) = self.width {
            builder = builder.width(width);
        }
        if let Some(height) = self.height {
            builder = builder.height(height);
        }
        if let Some(ref ratio) = self.aspect_ratio {
            builder = builder.aspect_ratio(ratio.clone());
        }
        if let Some(ref quality) = self.quality {
            builder = builder.quality(quality.clone());
        }
        if let Some(ref style) = self.style {
            builder = builder.style(style.clone());
        }
        if let Some(n) = self.n {
            builder = builder.n(n);
        }
        if let Some(ref format) = self.format {
            builder = builder.format(format.clone());
        }
        if let Some(duration) = self.duration_seconds {
            builder = builder.duration_seconds(duration);
        }
        if let Some(fps) = self.fps {
            builder = builder.fps(fps);
        }
        if let Some(ref voice) = self.voice {
            builder = builder.voice(voice.clone());
        }
        if let Some(speed) = self.speed {
            builder = builder.speed(speed);
        }
        if let Some(ref language) = self.language {
            builder = builder.language(language.clone());
        }
        if let Some(scale) = self.guidance_scale {
            builder = builder.guidance_scale(scale);
        }
        if let Some(steps) = self.steps {
            builder = builder.steps(steps);
        }

        builder.build()
    }
}

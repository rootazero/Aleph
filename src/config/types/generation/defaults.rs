//! Generation defaults configuration
//!
//! Contains the `GenerationDefaults` struct for default generation parameters.

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
        // These fields are persisted and validated, but no production request
        // path consumes them. Rejecting them keeps the configuration honest
        // until request assembly applies them through `to_params`.
        if self.width.is_some()
            || self.height.is_some()
            || self.aspect_ratio.is_some()
            || self.quality.is_some()
            || self.style.is_some()
            || self.n.is_some()
            || self.format.is_some()
            || self.duration_seconds.is_some()
            || self.fps.is_some()
            || self.speed.is_some()
            || self.language.is_some()
            || self.guidance_scale.is_some()
            || self.steps.is_some()
        {
            return Err(format!(
                "generation.providers.{provider_name}.defaults contains parameters that are not applied to generation requests"
            ));
        }

        Ok(())
    }

    /// Convert to `GenerationParams` from the generation module
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

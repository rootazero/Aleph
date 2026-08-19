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
        // Positive integer fields: must be > 0, warn past a sanity threshold
        validate_count(
            self.width,
            provider_name,
            "width",
            8192,
            "Default width is very large (>8192)",
        )?;
        validate_count(
            self.height,
            provider_name,
            "height",
            8192,
            "Default height is very large (>8192)",
        )?;
        validate_count(
            self.n,
            provider_name,
            "n",
            10,
            "Default n is high (>10), may be expensive",
        )?;
        validate_count(
            self.fps,
            provider_name,
            "fps",
            120,
            "Default fps is very high (>120)",
        )?;
        validate_count(
            self.steps,
            provider_name,
            "steps",
            150,
            "Default steps is high (>150), generation will be slow",
        )?;

        // Validate speed is in range
        if let Some(speed) = self.speed {
            if !(0.25..=4.0).contains(&speed) {
                return Err(format!(
                    "generation.providers.{provider_name}.defaults.speed must be between 0.25 and 4.0, got {speed}"
                ));
            }
        }

        // Validate duration_seconds
        if let Some(duration) = self.duration_seconds {
            if duration <= 0.0 {
                return Err(format!(
                    "generation.providers.{provider_name}.defaults.duration_seconds must be greater than 0"
                ));
            }
        }

        // Validate guidance_scale
        if let Some(scale) = self.guidance_scale {
            if scale < 0.0 {
                return Err(format!(
                    "generation.providers.{provider_name}.defaults.guidance_scale must be >= 0, got {scale}"
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

/// Shared shape for the positive-integer default fields: the value must be
/// greater than 0, and values past `warn_over` are legal but suspicious.
fn validate_count(
    value: Option<u32>,
    provider_name: &str,
    field: &str,
    warn_over: u32,
    warn_msg: &str,
) -> Result<(), String> {
    if let Some(v) = value {
        if v == 0 {
            return Err(format!(
                "generation.providers.{provider_name}.defaults.{field} must be greater than 0"
            ));
        }
        if v > warn_over {
            tracing::warn!(
                provider = provider_name,
                field = field,
                value = v,
                "{warn_msg}"
            );
        }
    }
    Ok(())
}

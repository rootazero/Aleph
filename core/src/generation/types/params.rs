/// Parameters for generation requests
///
/// This is a superset of all possible parameters across different generation types.
/// Use the builder pattern for convenient construction.
///
/// # Example
///
/// ```rust
/// use alephcore::generation::GenerationParams;
///
/// let params = GenerationParams::builder()
///     .width(1024)
///     .height(1024)
///     .quality("hd")
///     .style("vivid")
///     .build();
///
/// assert_eq!(params.width, Some(1024));
/// ```
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationParams {
    // === Image/Video parameters ===
    /// Width in pixels
    pub width: Option<u32>,
    /// Height in pixels
    pub height: Option<u32>,
    /// Aspect ratio (e.g., "16:9", "1:1")
    pub aspect_ratio: Option<String>,
    /// Quality level (e.g., "standard", "hd")
    pub quality: Option<String>,
    /// Style preset (e.g., "vivid", "natural")
    pub style: Option<String>,
    /// Number of outputs to generate
    pub n: Option<u32>,
    /// Random seed for reproducibility
    pub seed: Option<i64>,
    /// Output format (e.g., "png", "webp", "mp4")
    pub format: Option<String>,

    // === Video-specific parameters ===
    /// Video duration in seconds
    pub duration_seconds: Option<f32>,
    /// Frames per second
    pub fps: Option<u32>,

    // === Audio/Speech parameters ===
    /// Voice ID or name for TTS
    pub voice: Option<String>,
    /// Speaking speed (0.5 to 2.0)
    pub speed: Option<f32>,
    /// Language code (e.g., "en", "zh")
    pub language: Option<String>,

    // === Common parameters ===
    /// Model name/version to use
    pub model: Option<String>,
    /// Negative prompt (what to avoid)
    pub negative_prompt: Option<String>,
    /// Guidance scale / CFG scale
    pub guidance_scale: Option<f32>,
    /// Number of inference steps
    pub steps: Option<u32>,

    // === Reference inputs ===
    /// Reference image URL or base64
    pub reference_image: Option<String>,
    /// Reference audio URL or base64
    pub reference_audio: Option<String>,

    // === Provider-specific parameters ===
    /// Additional provider-specific parameters
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl GenerationParams {
    /// Create a new empty GenerationParams
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a builder for GenerationParams
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationParams;
    ///
    /// let params = GenerationParams::builder()
    ///     .width(512)
    ///     .height(512)
    ///     .build();
    /// ```
    pub fn builder() -> GenerationParamsBuilder {
        GenerationParamsBuilder::default()
    }

    /// Merge another GenerationParams into this one
    ///
    /// Values from `other` will override values in `self` if they are `Some`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationParams;
    ///
    /// let mut base = GenerationParams::builder()
    ///     .width(512)
    ///     .quality("standard")
    ///     .build();
    ///
    /// let override_params = GenerationParams::builder()
    ///     .width(1024)
    ///     .style("vivid")
    ///     .build();
    ///
    /// base.merge(override_params);
    ///
    /// assert_eq!(base.width, Some(1024)); // Overridden
    /// assert_eq!(base.quality, Some("standard".to_string())); // Kept
    /// assert_eq!(base.style, Some("vivid".to_string())); // Added
    /// ```
    pub fn merge(&mut self, other: GenerationParams) {
        if other.width.is_some() {
            self.width = other.width;
        }
        if other.height.is_some() {
            self.height = other.height;
        }
        if other.aspect_ratio.is_some() {
            self.aspect_ratio = other.aspect_ratio;
        }
        if other.quality.is_some() {
            self.quality = other.quality;
        }
        if other.style.is_some() {
            self.style = other.style;
        }
        if other.n.is_some() {
            self.n = other.n;
        }
        if other.seed.is_some() {
            self.seed = other.seed;
        }
        if other.format.is_some() {
            self.format = other.format;
        }
        if other.duration_seconds.is_some() {
            self.duration_seconds = other.duration_seconds;
        }
        if other.fps.is_some() {
            self.fps = other.fps;
        }
        if other.voice.is_some() {
            self.voice = other.voice;
        }
        if other.speed.is_some() {
            self.speed = other.speed;
        }
        if other.language.is_some() {
            self.language = other.language;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
        if other.negative_prompt.is_some() {
            self.negative_prompt = other.negative_prompt;
        }
        if other.guidance_scale.is_some() {
            self.guidance_scale = other.guidance_scale;
        }
        if other.steps.is_some() {
            self.steps = other.steps;
        }
        if other.reference_image.is_some() {
            self.reference_image = other.reference_image;
        }
        if other.reference_audio.is_some() {
            self.reference_audio = other.reference_audio;
        }
        // Merge extra parameters
        for (key, value) in other.extra {
            self.extra.insert(key, value);
        }
    }

    /// Create a merged copy without modifying the original
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::GenerationParams;
    ///
    /// let base = GenerationParams::builder().width(512).build();
    /// let other = GenerationParams::builder().height(512).build();
    ///
    /// let merged = base.merged_with(other);
    ///
    /// assert_eq!(merged.width, Some(512));
    /// assert_eq!(merged.height, Some(512));
    /// ```
    pub fn merged_with(&self, other: GenerationParams) -> GenerationParams {
        let mut result = self.clone();
        result.merge(other);
        result
    }
}

/// Builder for GenerationParams
///
/// Provides a fluent interface for constructing GenerationParams.
#[derive(Debug, Default)]
pub struct GenerationParamsBuilder {
    params: GenerationParams,
}

impl GenerationParamsBuilder {
    /// Set the width in pixels
    pub fn width(mut self, width: u32) -> Self {
        self.params.width = Some(width);
        self
    }

    /// Set the height in pixels
    pub fn height(mut self, height: u32) -> Self {
        self.params.height = Some(height);
        self
    }

    /// Set the aspect ratio
    pub fn aspect_ratio<S: Into<String>>(mut self, ratio: S) -> Self {
        self.params.aspect_ratio = Some(ratio.into());
        self
    }

    /// Set the quality level
    pub fn quality<S: Into<String>>(mut self, quality: S) -> Self {
        self.params.quality = Some(quality.into());
        self
    }

    /// Set the style preset
    pub fn style<S: Into<String>>(mut self, style: S) -> Self {
        self.params.style = Some(style.into());
        self
    }

    /// Set the number of outputs to generate
    pub fn n(mut self, n: u32) -> Self {
        self.params.n = Some(n);
        self
    }

    /// Set the random seed
    pub fn seed(mut self, seed: i64) -> Self {
        self.params.seed = Some(seed);
        self
    }

    /// Set the output format
    pub fn format<S: Into<String>>(mut self, format: S) -> Self {
        self.params.format = Some(format.into());
        self
    }

    /// Set the video duration in seconds
    pub fn duration_seconds(mut self, duration: f32) -> Self {
        self.params.duration_seconds = Some(duration);
        self
    }

    /// Set the frames per second
    pub fn fps(mut self, fps: u32) -> Self {
        self.params.fps = Some(fps);
        self
    }

    /// Set the voice for TTS
    pub fn voice<S: Into<String>>(mut self, voice: S) -> Self {
        self.params.voice = Some(voice.into());
        self
    }

    /// Set the speaking speed
    pub fn speed(mut self, speed: f32) -> Self {
        self.params.speed = Some(speed);
        self
    }

    /// Set the language code
    pub fn language<S: Into<String>>(mut self, language: S) -> Self {
        self.params.language = Some(language.into());
        self
    }

    /// Set the model name
    pub fn model<S: Into<String>>(mut self, model: S) -> Self {
        self.params.model = Some(model.into());
        self
    }

    /// Set the negative prompt
    pub fn negative_prompt<S: Into<String>>(mut self, prompt: S) -> Self {
        self.params.negative_prompt = Some(prompt.into());
        self
    }

    /// Set the guidance scale
    pub fn guidance_scale(mut self, scale: f32) -> Self {
        self.params.guidance_scale = Some(scale);
        self
    }

    /// Set the number of inference steps
    pub fn steps(mut self, steps: u32) -> Self {
        self.params.steps = Some(steps);
        self
    }

    /// Set the reference image
    pub fn reference_image<S: Into<String>>(mut self, image: S) -> Self {
        self.params.reference_image = Some(image.into());
        self
    }

    /// Set the reference audio
    pub fn reference_audio<S: Into<String>>(mut self, audio: S) -> Self {
        self.params.reference_audio = Some(audio.into());
        self
    }

    /// Add a custom extra parameter
    pub fn extra<S: Into<String>>(mut self, key: S, value: serde_json::Value) -> Self {
        self.params.extra.insert(key.into(), value);
        self
    }

    /// Build the GenerationParams
    pub fn build(self) -> GenerationParams {
        self.params
    }
}

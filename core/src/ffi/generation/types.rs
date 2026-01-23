//! FFI-safe type definitions for generation operations
//!
//! This module contains all FFI-safe type conversions between core generation types
//! and their FFI representations.

use crate::generation::{
    GenerationData, GenerationMetadata, GenerationOutput, GenerationParams, GenerationProgress,
    GenerationType,
};

// ============================================================================
// FFI-Safe Type Definitions
// ============================================================================

/// FFI-safe generation type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationTypeFFI {
    /// Image generation (DALL-E, Stable Diffusion, etc.)
    Image,
    /// Video generation (Runway, Pika, etc.)
    Video,
    /// Audio/music generation (Suno, MusicGen, etc.)
    Audio,
    /// Text-to-speech synthesis (ElevenLabs, OpenAI TTS, etc.)
    Speech,
}

impl From<GenerationType> for GenerationTypeFFI {
    fn from(t: GenerationType) -> Self {
        match t {
            GenerationType::Image => GenerationTypeFFI::Image,
            GenerationType::Video => GenerationTypeFFI::Video,
            GenerationType::Audio => GenerationTypeFFI::Audio,
            GenerationType::Speech => GenerationTypeFFI::Speech,
        }
    }
}

impl From<GenerationTypeFFI> for GenerationType {
    fn from(t: GenerationTypeFFI) -> Self {
        match t {
            GenerationTypeFFI::Image => GenerationType::Image,
            GenerationTypeFFI::Video => GenerationType::Video,
            GenerationTypeFFI::Audio => GenerationType::Audio,
            GenerationTypeFFI::Speech => GenerationType::Speech,
        }
    }
}

/// FFI-safe generation parameters
#[derive(Debug, Clone, Default)]
pub struct GenerationParamsFFI {
    // Image/Video parameters
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub aspect_ratio: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub n: Option<u32>,
    pub seed: Option<i64>,
    pub format: Option<String>,

    // Video-specific
    pub duration_seconds: Option<f32>,
    pub fps: Option<u32>,

    // Audio/Speech parameters
    pub voice: Option<String>,
    pub speed: Option<f32>,
    pub language: Option<String>,

    // Common parameters
    pub model: Option<String>,
    pub negative_prompt: Option<String>,
    pub guidance_scale: Option<f32>,
    pub steps: Option<u32>,

    // Reference inputs
    pub reference_image: Option<String>,
    pub reference_audio: Option<String>,

    // Image editing - mask for inpainting (transparent areas = edit regions)
    pub mask: Option<String>,
}

impl From<GenerationParamsFFI> for GenerationParams {
    fn from(p: GenerationParamsFFI) -> Self {
        let mut extra = std::collections::HashMap::new();

        // Add mask to extra params if provided
        if let Some(mask) = &p.mask {
            extra.insert("mask".to_string(), serde_json::json!(mask));
        }

        GenerationParams {
            width: p.width,
            height: p.height,
            aspect_ratio: p.aspect_ratio,
            quality: p.quality,
            style: p.style,
            n: p.n,
            seed: p.seed,
            format: p.format,
            duration_seconds: p.duration_seconds,
            fps: p.fps,
            voice: p.voice,
            speed: p.speed,
            language: p.language,
            model: p.model,
            negative_prompt: p.negative_prompt,
            guidance_scale: p.guidance_scale,
            steps: p.steps,
            reference_image: p.reference_image,
            reference_audio: p.reference_audio,
            extra,
        }
    }
}

impl From<GenerationParams> for GenerationParamsFFI {
    fn from(p: GenerationParams) -> Self {
        // Extract mask from extra params
        let mask = p
            .extra
            .get("mask")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        GenerationParamsFFI {
            width: p.width,
            height: p.height,
            aspect_ratio: p.aspect_ratio,
            quality: p.quality,
            style: p.style,
            n: p.n,
            seed: p.seed,
            format: p.format,
            duration_seconds: p.duration_seconds,
            fps: p.fps,
            voice: p.voice,
            speed: p.speed,
            language: p.language,
            model: p.model,
            negative_prompt: p.negative_prompt,
            guidance_scale: p.guidance_scale,
            steps: p.steps,
            reference_image: p.reference_image,
            reference_audio: p.reference_audio,
            mask,
        }
    }
}

/// FFI-safe generation data representation
#[derive(Debug, Clone)]
pub enum GenerationDataTypeFFI {
    /// Raw binary data
    Bytes,
    /// URL to the generated content
    Url,
    /// Path to a local file
    LocalPath,
}

/// FFI-safe generation data
#[derive(Debug, Clone)]
pub struct GenerationDataFFI {
    /// Type of data
    pub data_type: GenerationDataTypeFFI,
    /// Raw bytes (if data_type is Bytes)
    pub bytes: Option<Vec<u8>>,
    /// URL string (if data_type is Url)
    pub url: Option<String>,
    /// Local file path (if data_type is LocalPath)
    pub local_path: Option<String>,
}

impl From<GenerationData> for GenerationDataFFI {
    fn from(data: GenerationData) -> Self {
        match data {
            GenerationData::Bytes(bytes) => GenerationDataFFI {
                data_type: GenerationDataTypeFFI::Bytes,
                bytes: Some(bytes),
                url: None,
                local_path: None,
            },
            GenerationData::Url(url) => GenerationDataFFI {
                data_type: GenerationDataTypeFFI::Url,
                bytes: None,
                url: Some(url),
                local_path: None,
            },
            GenerationData::LocalPath(path) => GenerationDataFFI {
                data_type: GenerationDataTypeFFI::LocalPath,
                bytes: None,
                url: None,
                local_path: Some(path),
            },
        }
    }
}

/// FFI-safe generation metadata
#[derive(Debug, Clone, Default)]
pub struct GenerationMetadataFFI {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub duration_ms: Option<u64>,
    pub seed: Option<i64>,
    pub revised_prompt: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<f32>,
}

impl From<GenerationMetadata> for GenerationMetadataFFI {
    fn from(m: GenerationMetadata) -> Self {
        GenerationMetadataFFI {
            provider: m.provider,
            model: m.model,
            duration_ms: m.duration.map(|d| d.as_millis() as u64),
            seed: m.seed,
            revised_prompt: m.revised_prompt,
            content_type: m.content_type,
            size_bytes: m.size_bytes,
            width: m.width,
            height: m.height,
            duration_seconds: m.duration_seconds,
        }
    }
}

/// FFI-safe generation output
#[derive(Debug, Clone)]
pub struct GenerationOutputFFI {
    pub generation_type: GenerationTypeFFI,
    pub data: GenerationDataFFI,
    pub additional_outputs: Vec<GenerationDataFFI>,
    pub metadata: GenerationMetadataFFI,
    pub request_id: Option<String>,
}

impl From<GenerationOutput> for GenerationOutputFFI {
    fn from(output: GenerationOutput) -> Self {
        GenerationOutputFFI {
            generation_type: output.generation_type.into(),
            data: output.data.into(),
            additional_outputs: output
                .additional_outputs
                .into_iter()
                .map(|d| d.into())
                .collect(),
            metadata: output.metadata.into(),
            request_id: output.request_id,
        }
    }
}

/// FFI-safe generation progress
#[derive(Debug, Clone)]
pub struct GenerationProgressFFI {
    pub percentage: f32,
    pub step: String,
    pub eta_ms: Option<u64>,
    pub is_complete: bool,
    pub preview_url: Option<String>,
}

impl From<GenerationProgress> for GenerationProgressFFI {
    fn from(p: GenerationProgress) -> Self {
        GenerationProgressFFI {
            percentage: p.percentage,
            step: p.step,
            eta_ms: p.eta.map(|d| d.as_millis() as u64),
            is_complete: p.is_complete,
            preview_url: p.preview_url,
        }
    }
}

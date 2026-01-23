//! Constants for Replicate API Provider
//!
//! This module contains API endpoints, timeouts, polling configuration,
//! and built-in model mappings.

/// Default API endpoint for Replicate
pub const DEFAULT_ENDPOINT: &str = "https://api.replicate.com";

/// Default timeout for generation requests (5 minutes)
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Polling interval between status checks (1 second)
pub const POLL_INTERVAL_MS: u64 = 1000;

/// Maximum number of polling attempts (5 minutes at 1 second intervals)
pub const MAX_POLL_ATTEMPTS: u32 = 300;

// === Built-in Model Mappings ===

/// Flux Schnell - fast image generation
pub const MODEL_FLUX_SCHNELL: &str = "black-forest-labs/flux-schnell";

/// Stable Diffusion XL - high-quality image generation
pub const MODEL_SDXL: &str =
    "stability-ai/sdxl:39ed52f2a78e934b3ba6e2a89f5b1c712de7dfea535525255b1aa35c5565e08b";

/// Meta MusicGen - audio/music generation
pub const MODEL_MUSICGEN: &str =
    "meta/musicgen:b05b1dff1d8c6dc63d14b0cdb42135378dcb87f6373b0d3d341ede46e59e2b38";

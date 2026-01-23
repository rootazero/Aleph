//! Constants for Google Veo Video Generation Provider

/// Default API endpoint for Google AI (Gemini API)
pub const DEFAULT_ENDPOINT: &str = "https://generativelanguage.googleapis.com";

/// Default model for video generation
pub const DEFAULT_MODEL: &str = "veo-2.0-generate-001";

/// Default timeout for the entire video generation process (10 minutes)
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Polling interval in seconds
pub const POLL_INTERVAL_SECS: u64 = 10;

/// Maximum number of poll attempts (60 * 10s = 10 minutes)
pub const MAX_POLL_ATTEMPTS: u32 = 60;

/// Default video duration in seconds
pub const DEFAULT_DURATION_SECS: u32 = 8;

/// Default aspect ratio
pub const DEFAULT_ASPECT_RATIO: &str = "16:9";

/// Default resolution
pub const DEFAULT_RESOLUTION: &str = "720p";

/// Available aspect ratios for Veo
pub const ASPECT_RATIOS: &[&str] = &["16:9", "9:16"];

/// Available resolutions for Veo 3
pub const RESOLUTIONS: &[&str] = &["720p", "1080p", "4k"];

/// Available durations for Veo 3 (in seconds)
pub const VEO3_DURATIONS: &[u32] = &[4, 6, 8];

/// Available durations for Veo 2 (in seconds, range 5-8)
pub const VEO2_DURATION_RANGE: (u32, u32) = (5, 8);

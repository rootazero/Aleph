//! Video configuration types
//!
//! Contains video transcript extraction configuration:
//! - VideoConfig: YouTube and other video platform settings

use serde::{Deserialize, Serialize};

// =============================================================================
// VideoConfig
// =============================================================================

/// Video transcript extraction configuration
///
/// Enables extracting transcripts from video platforms (currently YouTube)
/// and injecting them into the AI context for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    /// Enable video transcript extraction
    #[serde(default = "default_video_enabled")]
    pub enabled: bool,

    /// Enable YouTube transcript extraction
    #[serde(default = "default_youtube_transcript")]
    pub youtube_transcript: bool,

    /// Preferred language for transcripts (ISO 639-1 code, e.g., "en", "zh")
    #[serde(default = "default_preferred_language")]
    pub preferred_language: String,

    /// Maximum transcript length in characters (0 = no limit)
    #[serde(default = "default_max_transcript_length")]
    pub max_transcript_length: usize,
}

pub fn default_video_enabled() -> bool {
    true
}

pub fn default_youtube_transcript() -> bool {
    true
}

pub fn default_preferred_language() -> String {
    // Try to detect system language, fallback to English
    // Check LANG environment variable (e.g., "zh_CN.UTF-8", "en_US.UTF-8")
    if let Ok(lang) = std::env::var("LANG") {
        let lang_lower = lang.to_lowercase();
        if lang_lower.starts_with("zh") {
            return "zh".to_string();
        } else if lang_lower.starts_with("ja") {
            return "ja".to_string();
        } else if lang_lower.starts_with("ko") {
            return "ko".to_string();
        } else if lang_lower.starts_with("de") {
            return "de".to_string();
        } else if lang_lower.starts_with("fr") {
            return "fr".to_string();
        } else if lang_lower.starts_with("es") {
            return "es".to_string();
        } else if lang_lower.starts_with("pt") {
            return "pt".to_string();
        } else if lang_lower.starts_with("ru") {
            return "ru".to_string();
        }
        // Extract language code from LANG (first 2 chars before underscore)
        if let Some(code) = lang.split('_').next() {
            if code.len() >= 2 {
                return code[..2].to_lowercase();
            }
        }
    }

    // Also check LC_ALL and LC_MESSAGES
    for var in ["LC_ALL", "LC_MESSAGES"] {
        if let Ok(lang) = std::env::var(var) {
            if !lang.is_empty() && lang != "C" && lang != "POSIX" {
                if let Some(code) = lang.split('_').next() {
                    if code.len() >= 2 {
                        return code[..2].to_lowercase();
                    }
                }
            }
        }
    }

    "en".to_string()
}

pub fn default_max_transcript_length() -> usize {
    50000 // ~12,500 words, roughly 25-30 minutes of video
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            enabled: default_video_enabled(),
            youtube_transcript: default_youtube_transcript(),
            preferred_language: default_preferred_language(),
            max_transcript_length: default_max_transcript_length(),
        }
    }
}

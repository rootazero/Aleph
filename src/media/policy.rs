//! Size and lifecycle policy for media processing.

use std::path::PathBuf;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::error::MediaError;
use super::types::MediaType;

/// Size and lifecycle policy for media processing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MediaPolicy {
    /// Maximum image file size in bytes (default: 20 MB).
    #[serde(default = "default_max_image_bytes")]
    pub max_image_bytes: u64,

    /// Maximum audio file size in bytes (default: 100 MB).
    #[serde(default = "default_max_audio_bytes")]
    pub max_audio_bytes: u64,

    /// Maximum video duration in seconds (default: 1800 = 30 min).
    #[serde(default = "default_max_video_duration")]
    pub max_video_duration: u64,

    /// Maximum document pages (default: 200).
    #[serde(default = "default_max_document_pages")]
    pub max_document_pages: u32,

    /// Temporary file directory.
    #[serde(default = "default_temp_dir")]
    pub temp_dir: PathBuf,

    /// Temp file TTL in seconds (default: 3600 = 1 hour).
    #[serde(default = "default_temp_ttl_secs")]
    pub temp_ttl_secs: u64,
}

fn default_max_image_bytes() -> u64 {
    20 * 1024 * 1024
}
fn default_max_audio_bytes() -> u64 {
    100 * 1024 * 1024
}
fn default_max_video_duration() -> u64 {
    1800
}
fn default_max_document_pages() -> u32 {
    200
}
fn default_temp_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aleph")
        .join("media_temp")
}
fn default_temp_ttl_secs() -> u64 {
    3600
}

impl Default for MediaPolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: default_max_image_bytes(),
            max_audio_bytes: default_max_audio_bytes(),
            max_video_duration: default_max_video_duration(),
            max_document_pages: default_max_document_pages(),
            temp_dir: default_temp_dir(),
            temp_ttl_secs: default_temp_ttl_secs(),
        }
    }
}

impl MediaPolicy {
    /// Temp file TTL as Duration.
    pub fn temp_ttl(&self) -> Duration {
        Duration::from_secs(self.temp_ttl_secs)
    }

    /// Validate file size against policy for the given media type.
    pub fn check_size(
        &self,
        media_type: &MediaType,
        file_size_bytes: u64,
    ) -> Result<(), MediaError> {
        match media_type {
            MediaType::Image { .. } => {
                if file_size_bytes > self.max_image_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Image size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_image_bytes
                        ),
                    });
                }
            }
            MediaType::Audio { .. } => {
                if file_size_bytes > self.max_audio_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Audio size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_audio_bytes
                        ),
                    });
                }
            }
            MediaType::Video { duration_secs, .. } => {
                if let Some(dur) = duration_secs {
                    if *dur > self.max_video_duration as f64 {
                        return Err(MediaError::SizeLimitExceeded {
                            message: format!(
                                "Video duration {:.0}s exceeds limit of {}s",
                                dur, self.max_video_duration
                            ),
                        });
                    }
                }
            }
            MediaType::Document { pages, .. } => {
                if let Some(p) = pages {
                    if *p > self.max_document_pages {
                        return Err(MediaError::SizeLimitExceeded {
                            message: format!(
                                "Document has {} pages, exceeds limit of {}",
                                p, self.max_document_pages
                            ),
                        });
                    }
                }
            }
            MediaType::Unknown => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::*;

    #[test]
    fn default_policy_values() {
        let p = MediaPolicy::default();
        assert_eq!(p.max_image_bytes, 20 * 1024 * 1024);
        assert_eq!(p.max_audio_bytes, 100 * 1024 * 1024);
        assert_eq!(p.max_video_duration, 1800);
        assert_eq!(p.max_document_pages, 200);
        assert_eq!(p.temp_ttl(), Duration::from_secs(3600));
    }

    #[test]
    fn check_size_image_ok() {
        let p = MediaPolicy::default();
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        assert!(p.check_size(&mt, 1024).is_ok());
    }

    #[test]
    fn check_size_image_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        assert!(p.check_size(&mt, 21 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_size_audio_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        };
        assert!(p.check_size(&mt, 101 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_size_video_duration_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Video {
            format: VideoFormat::Mp4,
            duration_secs: Some(2000.0),
        };
        assert!(p.check_size(&mt, 0).is_err());
    }

    #[test]
    fn check_size_document_pages_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Document {
            format: DocFormat::Pdf,
            pages: Some(300),
        };
        assert!(p.check_size(&mt, 0).is_err());
    }

    #[test]
    fn check_size_unknown_always_ok() {
        let p = MediaPolicy::default();
        assert!(p.check_size(&MediaType::Unknown, u64::MAX).is_ok());
    }
}

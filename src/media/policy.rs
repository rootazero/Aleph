//! Size and lifecycle policy for media processing.

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

    /// Maximum video file size in bytes (default: 500 MB).
    #[serde(default = "default_max_video_bytes")]
    pub max_video_bytes: u64,

    /// Maximum document file size in bytes (default: 50 MB).
    #[serde(default = "default_max_document_bytes")]
    pub max_document_bytes: u64,

    /// Maximum document pages (default: 200).
    #[serde(default = "default_max_document_pages")]
    pub max_document_pages: u32,
}

const fn default_max_image_bytes() -> u64 {
    20 * 1024 * 1024
}
const fn default_max_audio_bytes() -> u64 {
    100 * 1024 * 1024
}
const fn default_max_video_duration() -> u64 {
    1800
}
const fn default_max_video_bytes() -> u64 {
    500 * 1024 * 1024
}
const fn default_max_document_bytes() -> u64 {
    50 * 1024 * 1024
}
const fn default_max_document_pages() -> u32 {
    200
}

impl Default for MediaPolicy {
    fn default() -> Self {
        Self {
            max_image_bytes: default_max_image_bytes(),
            max_audio_bytes: default_max_audio_bytes(),
            max_video_duration: default_max_video_duration(),
            max_video_bytes: default_max_video_bytes(),
            max_document_bytes: default_max_document_bytes(),
            max_document_pages: default_max_document_pages(),
        }
    }
}

impl MediaPolicy {
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
                if file_size_bytes > self.max_video_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Video file size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_video_bytes
                        ),
                    });
                }
                if let Some(dur) = duration_secs {
                    if !dur.is_finite() || *dur > self.max_video_duration as f64 {
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
                if file_size_bytes > self.max_document_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Document file size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_document_bytes
                        ),
                    });
                }
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
            MediaType::Unknown => {
                // SECURITY (P1): previously this branch hard-coded a 100 MB
                // ceiling regardless of operator config, and the ceiling was
                // HIGHER than the image (20 MB) / document (50 MB) caps. A
                // caller could declare `media_type: MediaType::Unknown` to
                // claim a higher quota. Read the operator-configured ceiling
                // instead.
                if file_size_bytes > self.max_unknown_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Unknown media type size {file_size_bytes} bytes exceeds configured limit of {} bytes",
                            self.max_unknown_bytes
                        ),
                    });
                }
            }
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
        assert_eq!(p.max_video_bytes, 500 * 1024 * 1024);
        assert_eq!(p.max_document_bytes, 50 * 1024 * 1024);
        assert_eq!(p.max_document_pages, 200);
    }

    #[test]
    fn check_size_image_ok() {
        let p = MediaPolicy::default();
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
        };
        assert!(p.check_size(&mt, 1024).is_ok());
    }

    #[test]
    fn check_size_image_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
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
        assert!(p.check_size(&mt, 1024).is_err());
    }

    #[test]
    fn check_size_video_file_size_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Video {
            format: VideoFormat::Mp4,
            duration_secs: Some(60.0),
        };
        assert!(p.check_size(&mt, 501 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_size_document_pages_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Document {
            format: DocFormat::Pdf,
            pages: Some(300),
        };
        assert!(p.check_size(&mt, 1024).is_err());
    }

    #[test]
    fn check_size_document_file_size_exceeds() {
        let p = MediaPolicy::default();
        let mt = MediaType::Document {
            format: DocFormat::Pdf,
            pages: Some(10),
        };
        assert!(p.check_size(&mt, 51 * 1024 * 1024).is_err());
    }

    #[test]
    fn check_size_unknown_within_default_limit() {
        let p = MediaPolicy::default();
        assert!(p.check_size(&MediaType::Unknown, 1024).is_ok());
    }

    #[test]
    fn check_size_unknown_exceeds_default_limit() {
        let p = MediaPolicy::default();
        assert!(p
            .check_size(&MediaType::Unknown, 101 * 1024 * 1024)
            .is_err());
    }
}

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

    /// Maximum file size in bytes for media whose type could not be detected
    /// (default: 20 MB, matching the image cap).
    ///
    /// The previous default was 100 MB, which was *higher* than the image
    /// (20 MB) and document (50 MB) per-type caps. A caller declaring
    /// `MediaType::Unknown` could therefore claim a larger quota than a
    /// declared image — closing that gap is a default-policy decision, not
    /// an operator one. Operators can still raise the value explicitly via
    /// config; the `debug_assert!` in `MediaPolicy::new` pins the invariant
    /// that the unknown cap must not exceed the smallest per-type cap.
    #[serde(default = "default_max_unknown_bytes")]
    pub max_unknown_bytes: u64,
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
const fn default_max_unknown_bytes() -> u64 {
    // MED-08: lower the default from 100 MB to 20 MB so a caller declaring
    // `MediaType::Unknown` cannot claim a larger quota than a declared image
    // (which is also 20 MB). The image cap is the smallest per-type cap and
    // a sensible lower bound for "we have no idea what this is, refuse
    // anything we'd also refuse for an image".
    20 * 1024 * 1024
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
            max_unknown_bytes: default_max_unknown_bytes(),
        }
    }
}

impl MediaPolicy {
    /// Construct a `MediaPolicy` and assert that the unknown-type cap is no
    /// wider than the smallest per-type cap. `MediaType::Unknown` is the
    /// escape hatch for "we don't know what it is", and the safe default is
    /// that not-knowing cannot grant more quota than the most restrictive
    /// known type. This constructor is the supported way to build a
    /// non-default policy; `Default::default` already passes the assertion
    /// in tree, so existing callers are unaffected.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_image_bytes: u64,
        max_audio_bytes: u64,
        max_video_duration: u64,
        max_video_bytes: u64,
        max_document_bytes: u64,
        max_document_pages: u32,
        max_unknown_bytes: u64,
    ) -> Self {
        debug_assert!(
            max_unknown_bytes <= max_image_bytes,
            "max_unknown_bytes ({max_unknown_bytes}) must be <= \
             max_image_bytes ({max_image_bytes}); otherwise a caller declaring \
             MediaType::Unknown can claim a larger quota than a declared image",
        );
        Self {
            max_image_bytes,
            max_audio_bytes,
            max_video_duration,
            max_video_bytes,
            max_document_bytes,
            max_document_pages,
            max_unknown_bytes,
        }
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
                if file_size_bytes > self.max_video_bytes {
                    return Err(MediaError::SizeLimitExceeded {
                        message: format!(
                            "Video file size {} bytes exceeds limit of {} bytes",
                            file_size_bytes, self.max_video_bytes
                        ),
                    });
                }
                if let Some(dur) = duration_secs {
                    // SECURITY (P1 MED-06): the previous predicate allowed
                    // negative finite values to slip through (only NaN and
                    // +inf were rejected alongside the upper bound). A
                    // hostile caller could declare `duration_secs: -3600.0`
                    // and bypass the duration cap. Reject any non-positive
                    // finite duration up front.
                    if *dur < 0.0 || !dur.is_finite() || *dur > self.max_video_duration as f64 {
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

    /// Regression test for MED-06: a negative finite `duration_secs` was
    /// previously accepted because the predicate only rejected non-finite
    /// values and values above `max_video_duration`. -1.0 is finite and
    /// below the upper bound, but it is not a valid duration and must be
    /// rejected.
    #[test]
    fn check_size_video_negative_duration_rejected() {
        let p = MediaPolicy::default();
        let mt = MediaType::Video {
            format: VideoFormat::Mp4,
            duration_secs: Some(-1.0),
        };
        assert!(
            p.check_size(&mt, 1024).is_err(),
            "negative duration must be rejected"
        );
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

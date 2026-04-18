//! Media attachment types for the `_media` tool output convention.

use crate::sync_primitives::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Media item declared by tool output via `_media` convention.
/// Any tool can include `_media: Vec<MediaItem>` in its output JSON
/// to request media delivery to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Remote URL, local file path, or data URL of the media
    pub url: String,
    /// Media type: "image", "video", "audio", "file"
    pub media_type: String,
    /// MIME type (auto-detected from URL/Content-Type if absent)
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Optional filename
    #[serde(default)]
    pub filename: Option<String>,
}

/// Shared media buffer between StreamCallback and ReplyEmitter.
pub type PendingMedia = Arc<Mutex<Vec<MediaItem>>>;

/// Maximum number of media items allowed per run.
pub const MAX_MEDIA_PER_RUN: usize = 10;

/// Detect MIME type from URL extension, with a fallback default based on media_type.
pub fn detect_mime(url: &str, media_type: &str) -> String {
    // Strip query string and fragment before checking extension
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split('#').next().unwrap_or(path);
    // Try URL extension first
    if let Some(ext) = path.rsplit('.').next() {
        match ext.to_lowercase().as_str() {
            "png" => return "image/png".to_string(),
            "jpg" | "jpeg" => return "image/jpeg".to_string(),
            "gif" => return "image/gif".to_string(),
            "webp" => return "image/webp".to_string(),
            "mp4" => return "video/mp4".to_string(),
            "webm" => return "video/webm".to_string(),
            "mp3" => return "audio/mpeg".to_string(),
            "wav" => return "audio/wav".to_string(),
            "ogg" => return "audio/ogg".to_string(),
            "flac" => return "audio/flac".to_string(),
            _ => {}
        }
    }
    // Fallback by media_type
    match media_type {
        "image" => "image/png".to_string(),
        "video" => "video/mp4".to_string(),
        "audio" => "audio/mpeg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_mime_from_extension() {
        assert_eq!(
            detect_mime("https://example.com/photo.png", "image"),
            "image/png"
        );
        assert_eq!(
            detect_mime("https://example.com/video.mp4", "video"),
            "video/mp4"
        );
        assert_eq!(
            detect_mime("https://example.com/song.mp3", "audio"),
            "audio/mpeg"
        );
    }

    #[test]
    fn test_detect_mime_with_query_params() {
        assert_eq!(
            detect_mime("https://example.com/img.png?token=abc", "image"),
            "image/png"
        );
        assert_eq!(
            detect_mime("https://example.com/video.mp4?v=1.0", "video"),
            "video/mp4"
        );
        assert_eq!(
            detect_mime("https://cdn.example.com/file.jpg#frag", "image"),
            "image/jpeg"
        );
    }

    #[test]
    fn test_detect_mime_fallback() {
        assert_eq!(
            detect_mime("https://example.com/file?token=abc", "image"),
            "image/png"
        );
        assert_eq!(
            detect_mime("https://example.com/file?token=abc", "video"),
            "video/mp4"
        );
        assert_eq!(
            detect_mime("https://example.com/file?token=abc", "audio"),
            "audio/mpeg"
        );
        assert_eq!(
            detect_mime("https://example.com/file", "file"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_media_item_deserialization() {
        let json = r#"{"url":"https://example.com/img.png","media_type":"image"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.url, "https://example.com/img.png");
        assert_eq!(item.media_type, "image");
        assert!(item.mime_type.is_none());
        assert!(item.filename.is_none());
    }

    #[test]
    fn test_media_item_with_all_fields() {
        let json = r#"{"url":"https://example.com/img.png","media_type":"image","mime_type":"image/png","filename":"photo.png"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.mime_type.unwrap(), "image/png");
        assert_eq!(item.filename.unwrap(), "photo.png");
    }
}

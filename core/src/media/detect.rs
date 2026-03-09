//! Media format detection via magic bytes and file extension.

use super::error::MediaError;
use super::types::{AudioFormat, DocFormat, MediaImageFormat, MediaType, VideoFormat};

/// Detect media type from file extension.
pub fn detect_by_extension(ext: &str) -> Result<MediaType, MediaError> {
    let ext_lower = ext.to_ascii_lowercase();
    let ext_clean = ext_lower.trim_start_matches('.');

    match ext_clean {
        // Images
        "png" => Ok(MediaType::Image { format: MediaImageFormat::Png, width: None, height: None }),
        "jpg" | "jpeg" => Ok(MediaType::Image { format: MediaImageFormat::Jpeg, width: None, height: None }),
        "webp" => Ok(MediaType::Image { format: MediaImageFormat::WebP, width: None, height: None }),
        "gif" => Ok(MediaType::Image { format: MediaImageFormat::Gif, width: None, height: None }),
        "svg" => Ok(MediaType::Image { format: MediaImageFormat::Svg, width: None, height: None }),
        "heic" | "heif" => Ok(MediaType::Image { format: MediaImageFormat::Heic, width: None, height: None }),
        // Audio
        "mp3" => Ok(MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None }),
        "wav" => Ok(MediaType::Audio { format: AudioFormat::Wav, duration_secs: None }),
        "ogg" => Ok(MediaType::Audio { format: AudioFormat::Ogg, duration_secs: None }),
        "flac" => Ok(MediaType::Audio { format: AudioFormat::Flac, duration_secs: None }),
        "m4a" => Ok(MediaType::Audio { format: AudioFormat::M4a, duration_secs: None }),
        // Video
        "mp4" => Ok(MediaType::Video { format: VideoFormat::Mp4, duration_secs: None }),
        "webm" => Ok(MediaType::Video { format: VideoFormat::WebM, duration_secs: None }),
        "mov" => Ok(MediaType::Video { format: VideoFormat::Mov, duration_secs: None }),
        // Documents
        "pdf" => Ok(MediaType::Document { format: DocFormat::Pdf, pages: None }),
        "docx" => Ok(MediaType::Document { format: DocFormat::Docx, pages: None }),
        "xlsx" => Ok(MediaType::Document { format: DocFormat::Xlsx, pages: None }),
        "txt" => Ok(MediaType::Document { format: DocFormat::Txt, pages: None }),
        "md" | "markdown" => Ok(MediaType::Document { format: DocFormat::Markdown, pages: None }),
        "html" | "htm" => Ok(MediaType::Document { format: DocFormat::Html, pages: None }),
        _ => Err(MediaError::UnsupportedFormat(ext_clean.to_string())),
    }
}

/// Detect media type from file magic bytes (first 16 bytes).
pub fn detect_by_magic(bytes: &[u8]) -> MediaType {
    if bytes.len() < 4 {
        return MediaType::Unknown;
    }

    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return MediaType::Image { format: MediaImageFormat::Png, width: None, height: None };
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return MediaType::Image { format: MediaImageFormat::Jpeg, width: None, height: None };
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF8") {
        return MediaType::Image { format: MediaImageFormat::Gif, width: None, height: None };
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return MediaType::Image { format: MediaImageFormat::WebP, width: None, height: None };
    }
    // PDF: %PDF
    if bytes.starts_with(b"%PDF") {
        return MediaType::Document { format: DocFormat::Pdf, pages: None };
    }
    // ZIP-based (DOCX/XLSX): PK\x03\x04
    if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        return MediaType::Document { format: DocFormat::Docx, pages: None };
    }
    // MP3: ID3 tag or sync word
    if bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0) {
        return MediaType::Audio { format: AudioFormat::Mp3, duration_secs: None };
    }
    // WAV: RIFF....WAVE
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return MediaType::Audio { format: AudioFormat::Wav, duration_secs: None };
    }
    // OGG: OggS
    if bytes.starts_with(b"OggS") {
        return MediaType::Audio { format: AudioFormat::Ogg, duration_secs: None };
    }
    // FLAC: fLaC
    if bytes.starts_with(b"fLaC") {
        return MediaType::Audio { format: AudioFormat::Flac, duration_secs: None };
    }
    // ftyp-based containers (MP4/MOV/M4A)
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"M4A " || brand == b"M4B " {
            return MediaType::Audio { format: AudioFormat::M4a, duration_secs: None };
        }
        if brand == b"qt  " {
            return MediaType::Video { format: VideoFormat::Mov, duration_secs: None };
        }
        return MediaType::Video { format: VideoFormat::Mp4, duration_secs: None };
    }
    // WebM: EBML header
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return MediaType::Video { format: VideoFormat::WebM, duration_secs: None };
    }

    MediaType::Unknown
}

/// Detect from file path: try magic bytes first, fall back to extension.
pub fn detect_from_path(path: &std::path::Path) -> Result<MediaType, MediaError> {
    if path.exists() {
        if let Ok(bytes) = std::fs::read(path).map(|b| b.into_iter().take(16).collect::<Vec<_>>()) {
            let magic_result = detect_by_magic(&bytes);
            if magic_result != MediaType::Unknown {
                return Ok(magic_result);
            }
        }
    }

    path.extension()
        .and_then(|e| e.to_str())
        .map(detect_by_extension)
        .unwrap_or(Err(MediaError::DetectionFailed(format!(
            "Cannot determine media type for: {}",
            path.display(),
        ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_image_extensions() {
        assert!(matches!(detect_by_extension("png").unwrap(), MediaType::Image { format: MediaImageFormat::Png, .. }));
        assert!(matches!(detect_by_extension("JPG").unwrap(), MediaType::Image { format: MediaImageFormat::Jpeg, .. }));
        assert!(matches!(detect_by_extension(".jpeg").unwrap(), MediaType::Image { format: MediaImageFormat::Jpeg, .. }));
        assert!(matches!(detect_by_extension("webp").unwrap(), MediaType::Image { format: MediaImageFormat::WebP, .. }));
        assert!(matches!(detect_by_extension("gif").unwrap(), MediaType::Image { format: MediaImageFormat::Gif, .. }));
        assert!(matches!(detect_by_extension("heic").unwrap(), MediaType::Image { format: MediaImageFormat::Heic, .. }));
        assert!(matches!(detect_by_extension("heif").unwrap(), MediaType::Image { format: MediaImageFormat::Heic, .. }));
    }

    #[test]
    fn detect_audio_extensions() {
        assert!(matches!(detect_by_extension("mp3").unwrap(), MediaType::Audio { format: AudioFormat::Mp3, .. }));
        assert!(matches!(detect_by_extension("wav").unwrap(), MediaType::Audio { format: AudioFormat::Wav, .. }));
        assert!(matches!(detect_by_extension("flac").unwrap(), MediaType::Audio { format: AudioFormat::Flac, .. }));
        assert!(matches!(detect_by_extension("m4a").unwrap(), MediaType::Audio { format: AudioFormat::M4a, .. }));
    }

    #[test]
    fn detect_video_extensions() {
        assert!(matches!(detect_by_extension("mp4").unwrap(), MediaType::Video { format: VideoFormat::Mp4, .. }));
        assert!(matches!(detect_by_extension("webm").unwrap(), MediaType::Video { format: VideoFormat::WebM, .. }));
        assert!(matches!(detect_by_extension("mov").unwrap(), MediaType::Video { format: VideoFormat::Mov, .. }));
    }

    #[test]
    fn detect_document_extensions() {
        assert!(matches!(detect_by_extension("pdf").unwrap(), MediaType::Document { format: DocFormat::Pdf, .. }));
        assert!(matches!(detect_by_extension("md").unwrap(), MediaType::Document { format: DocFormat::Markdown, .. }));
        assert!(matches!(detect_by_extension("html").unwrap(), MediaType::Document { format: DocFormat::Html, .. }));
    }

    #[test]
    fn detect_unknown_extension() {
        assert!(detect_by_extension("xyz").is_err());
    }

    #[test]
    fn detect_magic_png() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Image { format: MediaImageFormat::Png, .. }));
    }

    #[test]
    fn detect_magic_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Image { format: MediaImageFormat::Jpeg, .. }));
    }

    #[test]
    fn detect_magic_pdf() {
        let bytes = b"%PDF-1.4 rest of header";
        assert!(matches!(detect_by_magic(bytes), MediaType::Document { format: DocFormat::Pdf, .. }));
    }

    #[test]
    fn detect_magic_wav() {
        let bytes = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        assert!(matches!(detect_by_magic(bytes), MediaType::Audio { format: AudioFormat::Wav, .. }));
    }

    #[test]
    fn detect_magic_webp() {
        let bytes = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert!(matches!(detect_by_magic(bytes), MediaType::Image { format: MediaImageFormat::WebP, .. }));
    }

    #[test]
    fn detect_magic_mp4() {
        let bytes = [0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 0, 0];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Video { format: VideoFormat::Mp4, .. }));
    }

    #[test]
    fn detect_magic_too_short() {
        assert!(matches!(detect_by_magic(&[0x89, 0x50]), MediaType::Unknown));
    }

    #[test]
    fn detect_magic_unknown() {
        let bytes = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Unknown));
    }
}

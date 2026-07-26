//! Media format detection via magic bytes and file extension.

use super::error::MediaError;
use super::types::{AudioFormat, DocFormat, MediaImageFormat, MediaType, VideoFormat};

/// Detect media type from file extension.
pub fn detect_by_extension(ext: &str) -> Result<MediaType, MediaError> {
    let ext_lower = ext.to_ascii_lowercase();
    let ext_clean = ext_lower.trim_start_matches('.');

    if let Some(ty) = detect_image_extension(ext_clean) {
        return Ok(ty);
    }
    if let Some(ty) = detect_audio_extension(ext_clean) {
        return Ok(ty);
    }
    if let Some(ty) = detect_video_extension(ext_clean) {
        return Ok(ty);
    }
    if let Some(ty) = detect_document_extension(ext_clean) {
        return Ok(ty);
    }
    Err(MediaError::UnsupportedFormat(ext_clean.to_string()))
}

fn detect_image_extension(ext: &str) -> Option<MediaType> {
    let format = match ext {
        "png" => MediaImageFormat::Png,
        "jpg" | "jpeg" => MediaImageFormat::Jpeg,
        "webp" => MediaImageFormat::WebP,
        "gif" => MediaImageFormat::Gif,
        "svg" => MediaImageFormat::Svg,
        "heic" | "heif" => MediaImageFormat::Heic,
        _ => return None,
    };
    Some(MediaType::Image {
        format,
        width: None,
        height: None,
    })
}

fn detect_audio_extension(ext: &str) -> Option<MediaType> {
    let format = match ext {
        "mp3" => AudioFormat::Mp3,
        "wav" => AudioFormat::Wav,
        "ogg" => AudioFormat::Ogg,
        "flac" => AudioFormat::Flac,
        "m4a" => AudioFormat::M4a,
        _ => return None,
    };
    Some(MediaType::Audio {
        format,
        duration_secs: None,
    })
}

fn detect_video_extension(ext: &str) -> Option<MediaType> {
    let format = match ext {
        "mp4" => VideoFormat::Mp4,
        "webm" => VideoFormat::WebM,
        "mov" => VideoFormat::Mov,
        _ => return None,
    };
    Some(MediaType::Video {
        format,
        duration_secs: None,
    })
}

fn detect_document_extension(ext: &str) -> Option<MediaType> {
    let format = match ext {
        "pdf" => DocFormat::Pdf,
        "docx" => DocFormat::Docx,
        "xlsx" => DocFormat::Xlsx,
        "txt" => DocFormat::Txt,
        "md" | "markdown" => DocFormat::Markdown,
        "html" | "htm" => DocFormat::Html,
        _ => return None,
    };
    Some(MediaType::Document { format, pages: None })
}

/// Detect media type from file magic bytes (first 16 bytes).
#[must_use]
pub fn detect_by_magic(bytes: &[u8]) -> MediaType {
    if bytes.len() < 4 {
        return MediaType::Unknown;
    }

    detect_image_magic(bytes)
        .or_else(|| detect_document_magic(bytes))
        .or_else(|| detect_audio_magic(bytes))
        .or_else(|| detect_ftyp_magic(bytes))
        .or_else(|| detect_video_magic(bytes))
        .unwrap_or(MediaType::Unknown)
}

fn detect_image_magic(bytes: &[u8]) -> Option<MediaType> {
    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some(MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        });
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MediaType::Image {
            format: MediaImageFormat::Jpeg,
            width: None,
            height: None,
        });
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF8") {
        return Some(MediaType::Image {
            format: MediaImageFormat::Gif,
            width: None,
            height: None,
        });
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(MediaType::Image {
            format: MediaImageFormat::WebP,
            width: None,
            height: None,
        });
    }
    None
}

fn detect_document_magic(bytes: &[u8]) -> Option<MediaType> {
    // PDF: %PDF
    if bytes.starts_with(b"%PDF") {
        return Some(MediaType::Document {
            format: DocFormat::Pdf,
            pages: None,
        });
    }
    // ZIP-based (DOCX/XLSX/PPTX/JAR/etc): PK\x03\x04
    // Cannot distinguish DOCX from XLSX/PPTX/ZIP by magic bytes alone;
    // precise detection requires parsing the ZIP's [Content_Types].xml.
    // Default to Docx as the most common document format.
    if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        return Some(MediaType::Document {
            format: DocFormat::Docx,
            pages: None,
        });
    }
    None
}

fn detect_audio_magic(bytes: &[u8]) -> Option<MediaType> {
    // MP3: ID3 tag or sync word
    if bytes.starts_with(b"ID3") || (bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0) {
        return Some(MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        });
    }
    // WAV: RIFF....WAVE
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return Some(MediaType::Audio {
            format: AudioFormat::Wav,
            duration_secs: None,
        });
    }
    // OGG: OggS
    if bytes.starts_with(b"OggS") {
        return Some(MediaType::Audio {
            format: AudioFormat::Ogg,
            duration_secs: None,
        });
    }
    // FLAC: fLaC
    if bytes.starts_with(b"fLaC") {
        return Some(MediaType::Audio {
            format: AudioFormat::Flac,
            duration_secs: None,
        });
    }
    None
}

fn detect_ftyp_magic(bytes: &[u8]) -> Option<MediaType> {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return None;
    }
    let brand = &bytes[8..12];
    if brand == b"M4A " || brand == b"M4B " {
        return Some(MediaType::Audio {
            format: AudioFormat::M4a,
            duration_secs: None,
        });
    }
    if brand == b"qt  " {
        return Some(MediaType::Video {
            format: VideoFormat::Mov,
            duration_secs: None,
        });
    }
    // HEIC/HEIF image formats
    if brand == b"heic" || brand == b"mif1" || brand == b"msf1" || brand == b"heix" {
        return Some(MediaType::Image {
            format: MediaImageFormat::Heic,
            width: None,
            height: None,
        });
    }
    Some(MediaType::Video {
        format: VideoFormat::Mp4,
        duration_secs: None,
    })
}

fn detect_video_magic(bytes: &[u8]) -> Option<MediaType> {
    // WebM: EBML header
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(MediaType::Video {
            format: VideoFormat::WebM,
            duration_secs: None,
        });
    }
    None
}

/// Detect from file path: try magic bytes first, fall back to extension.
pub async fn detect_from_path(path: &std::path::Path) -> Result<MediaType, MediaError> {
    // Try magic bytes first — open may fail if path doesn't exist or is not readable;
    // we simply fall back to extension detection in that case.
    if let Ok(mut f) = tokio::fs::File::open(path).await {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 16];
        if let Ok(()) = f.read_exact(&mut buf).await {
            if buf.len() >= 4 {
                let magic_result = detect_by_magic(&buf);
                if magic_result != MediaType::Unknown {
                    return Ok(magic_result);
                }
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
        assert!(matches!(
            detect_by_extension("png").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::Png,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("JPG").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::Jpeg,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension(".jpeg").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::Jpeg,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("webp").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::WebP,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("gif").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::Gif,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("heic").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::Heic,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("heif").unwrap(),
            MediaType::Image {
                format: MediaImageFormat::Heic,
                ..
            }
        ));
    }

    #[test]
    fn detect_audio_extensions() {
        assert!(matches!(
            detect_by_extension("mp3").unwrap(),
            MediaType::Audio {
                format: AudioFormat::Mp3,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("wav").unwrap(),
            MediaType::Audio {
                format: AudioFormat::Wav,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("flac").unwrap(),
            MediaType::Audio {
                format: AudioFormat::Flac,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("m4a").unwrap(),
            MediaType::Audio {
                format: AudioFormat::M4a,
                ..
            }
        ));
    }

    #[test]
    fn detect_video_extensions() {
        assert!(matches!(
            detect_by_extension("mp4").unwrap(),
            MediaType::Video {
                format: VideoFormat::Mp4,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("webm").unwrap(),
            MediaType::Video {
                format: VideoFormat::WebM,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("mov").unwrap(),
            MediaType::Video {
                format: VideoFormat::Mov,
                ..
            }
        ));
    }

    #[test]
    fn detect_document_extensions() {
        assert!(matches!(
            detect_by_extension("pdf").unwrap(),
            MediaType::Document {
                format: DocFormat::Pdf,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("md").unwrap(),
            MediaType::Document {
                format: DocFormat::Markdown,
                ..
            }
        ));
        assert!(matches!(
            detect_by_extension("html").unwrap(),
            MediaType::Document {
                format: DocFormat::Html,
                ..
            }
        ));
    }

    #[test]
    fn detect_unknown_extension() {
        assert!(detect_by_extension("xyz").is_err());
    }

    #[test]
    fn detect_magic_png() {
        let bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert!(matches!(
            detect_by_magic(&bytes),
            MediaType::Image {
                format: MediaImageFormat::Png,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(
            detect_by_magic(&bytes),
            MediaType::Image {
                format: MediaImageFormat::Jpeg,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_pdf() {
        let bytes = b"%PDF-1.4 rest of header";
        assert!(matches!(
            detect_by_magic(bytes),
            MediaType::Document {
                format: DocFormat::Pdf,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_wav() {
        let bytes = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        assert!(matches!(
            detect_by_magic(bytes),
            MediaType::Audio {
                format: AudioFormat::Wav,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_webp() {
        let bytes = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert!(matches!(
            detect_by_magic(bytes),
            MediaType::Image {
                format: MediaImageFormat::WebP,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_mp4() {
        let bytes = [
            0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 0, 0,
        ];
        assert!(matches!(
            detect_by_magic(&bytes),
            MediaType::Video {
                format: VideoFormat::Mp4,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_too_short() {
        assert!(matches!(detect_by_magic(&[0x89, 0x50]), MediaType::Unknown));
    }

    #[tokio::test]
    async fn detect_from_path_reads_offset8_signature() {
        // WebP's "WEBP" marker lives at byte offset 8.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("img.bin"); // misleading extension on purpose
        std::fs::write(&file, b"RIFF\x00\x00\x00\x00WEBPVP8 ").unwrap();
        assert!(matches!(
            detect_from_path(&file).await.unwrap(),
            MediaType::Image {
                format: MediaImageFormat::WebP,
                ..
            }
        ));
    }

    #[test]
    fn detect_magic_unknown() {
        let bytes = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
            0x0E, 0x0F,
        ];
        assert!(matches!(detect_by_magic(&bytes), MediaType::Unknown));
    }
}

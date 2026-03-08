//! Structural pattern detector for file paths, URLs, and context signals.
//!
//! Language-agnostic detection of structural patterns in user input:
//! - File system paths (Unix and Windows)
//! - URLs (HTTP/HTTPS/FTP)
//! - Context signals from UI environment (selected file, clipboard)

use once_cell::sync::Lazy;
use regex::Regex;

use crate::intent::types::{DetectionLayer, ExecuteMetadata, IntentResult};

/// Context signals from the UI environment.
#[derive(Debug, Clone, Default)]
pub struct StructuralContext {
    pub selected_file: Option<String>,
    pub clipboard_type: Option<String>,
}

/// Path extraction pattern.
/// Matches Unix paths (/path or ~/path) and Windows paths (C:\path).
static PATH_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"['"]?([/~][A-Za-z0-9_./-]+|[A-Za-z]:\\[A-Za-z0-9_.\\/]+)['"]?"#).unwrap()
});

/// URL extraction pattern.
/// Matches http://, https://, ftp://, ftps:// URLs.
static URL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(https?://[^\s,;'")\]}>]+|ftps?://[^\s,;'")\]}>]+)"#).unwrap()
});

/// Image file extensions for context signal detection.
const IMAGE_EXTENSIONS: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tiff", ".svg", ".heic",
];

/// Language-agnostic detector for structural patterns in user input.
///
/// Detects file paths, URLs, and UI context signals without relying
/// on any natural language keywords.
pub struct StructuralDetector;

impl StructuralDetector {
    pub fn new() -> Self {
        Self
    }

    /// Main detection entry point.
    ///
    /// Checks context signals first, then extracts URLs and paths from input.
    /// Returns `Some(IntentResult::Execute)` if any structural signal is found.
    pub fn detect(&self, input: &str, context: &StructuralContext) -> Option<IntentResult> {
        // 1. Check context signals first (highest priority)
        if let Some(result) = self.check_context(context) {
            return Some(result);
        }

        // 2. Extract URL from input
        let url = self.extract_url(input);

        // 3. Extract path, excluding any URL region
        let path = if let Some(ref u) = url {
            self.extract_path_excluding_url(input, u)
        } else {
            self.extract_path(input)
        };

        // 4. Build result if we found anything
        if url.is_some() || path.is_some() {
            return Some(IntentResult::Execute {
                confidence: 0.7,
                metadata: ExecuteMetadata {
                    detected_path: path,
                    detected_url: url,
                    layer: DetectionLayer::L1,
                    ..Default::default()
                },
            });
        }

        // 5. No structural signal
        None
    }

    /// Check UI context signals for structural intent.
    pub fn check_context(&self, context: &StructuralContext) -> Option<IntentResult> {
        if let Some(ref file) = context.selected_file {
            let lower = file.to_lowercase();
            let is_image = IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext));
            let hint = if is_image {
                format!("image_file:{}", file)
            } else {
                format!("file:{}", file)
            };
            return Some(IntentResult::Execute {
                confidence: 0.85,
                metadata: ExecuteMetadata {
                    context_hint: Some(hint),
                    layer: DetectionLayer::L1,
                    ..Default::default()
                },
            });
        }

        if let Some(ref clip_type) = context.clipboard_type {
            if clip_type == "image" {
                return Some(IntentResult::Execute {
                    confidence: 0.8,
                    metadata: ExecuteMetadata {
                        context_hint: Some("clipboard:image".to_string()),
                        layer: DetectionLayer::L1,
                        ..Default::default()
                    },
                });
            }
        }

        None
    }

    /// Extract the first URL from input.
    pub fn extract_url(&self, input: &str) -> Option<String> {
        URL_PATTERN
            .captures(input)
            .map(|c| c[1].to_string())
    }

    /// Extract the first file path from input.
    pub fn extract_path(&self, input: &str) -> Option<String> {
        PATH_PATTERN.captures(input).and_then(|c| {
            let path = c[1].to_string();
            // Filter out bare "/" — require at least 2 chars
            if path.len() < 2 {
                None
            } else {
                Some(path)
            }
        })
    }

    /// Extract a file path from input, ignoring any region that overlaps with the given URL.
    pub fn extract_path_excluding_url(&self, input: &str, url: &str) -> Option<String> {
        let cleaned = input.replace(url, " ");
        // Re-run path extraction on the cleaned input
        PATH_PATTERN.captures(&cleaned).and_then(|c| {
            let path = c[1].to_string();
            if path.len() < 2 {
                None
            } else {
                Some(path)
            }
        })
    }
}

impl Default for StructuralDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_unix_path() {
        let detector = StructuralDetector::new();
        let result = detector
            .detect("please read /home/user/file.txt", &StructuralContext::default())
            .unwrap();
        match result {
            IntentResult::Execute {
                metadata,
                ..
            } => {
                assert_eq!(
                    metadata.detected_path,
                    Some("/home/user/file.txt".to_string())
                );
                assert_eq!(metadata.layer, DetectionLayer::L1);
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn detect_home_path() {
        let detector = StructuralDetector::new();
        let result = detector
            .detect("organize ~/Downloads", &StructuralContext::default())
            .unwrap();
        match result {
            IntentResult::Execute { metadata, .. } => {
                assert_eq!(metadata.detected_path, Some("~/Downloads".to_string()));
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn detect_windows_path() {
        let detector = StructuralDetector::new();
        let result = detector
            .detect(
                "open C:\\Users\\test\\file.txt",
                &StructuralContext::default(),
            )
            .unwrap();
        match result {
            IntentResult::Execute { metadata, .. } => {
                assert!(metadata.detected_path.is_some());
                assert!(metadata.detected_path.unwrap().contains("Users"));
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn detect_url() {
        let detector = StructuralDetector::new();
        let result = detector
            .detect(
                "fetch https://example.com/page",
                &StructuralContext::default(),
            )
            .unwrap();
        match result {
            IntentResult::Execute { metadata, .. } => {
                assert_eq!(
                    metadata.detected_url,
                    Some("https://example.com/page".to_string())
                );
                assert!(metadata.detected_path.is_none());
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn detect_context_selected_image() {
        let detector = StructuralDetector::new();
        let ctx = StructuralContext {
            selected_file: Some("photo.jpg".to_string()),
            ..Default::default()
        };
        let result = detector.detect("", &ctx).unwrap();
        match result {
            IntentResult::Execute { metadata, .. } => {
                assert_eq!(
                    metadata.context_hint,
                    Some("image_file:photo.jpg".to_string())
                );
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn detect_context_selected_file() {
        let detector = StructuralDetector::new();
        let ctx = StructuralContext {
            selected_file: Some("report.pdf".to_string()),
            ..Default::default()
        };
        let result = detector.detect("", &ctx).unwrap();
        match result {
            IntentResult::Execute { metadata, .. } => {
                assert_eq!(
                    metadata.context_hint,
                    Some("file:report.pdf".to_string())
                );
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn detect_context_clipboard_image() {
        let detector = StructuralDetector::new();
        let ctx = StructuralContext {
            selected_file: None,
            clipboard_type: Some("image".to_string()),
        };
        let result = detector.detect("", &ctx).unwrap();
        match result {
            IntentResult::Execute {
                confidence,
                metadata,
            } => {
                assert!((confidence - 0.8).abs() < f32::EPSILON);
                assert_eq!(
                    metadata.context_hint,
                    Some("clipboard:image".to_string())
                );
            }
            _ => panic!("expected Execute"),
        }
    }

    #[test]
    fn no_structural_signal() {
        let detector = StructuralDetector::new();
        let result = detector.detect(
            "what is quantum computing",
            &StructuralContext::default(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn no_detect_bare_slash() {
        let detector = StructuralDetector::new();
        let result = detector.detect("just a / character", &StructuralContext::default());
        assert!(result.is_none());
    }

    #[test]
    fn url_takes_priority_over_path() {
        let detector = StructuralDetector::new();
        let result = detector
            .detect(
                "check https://example.com/path/to/page",
                &StructuralContext::default(),
            )
            .unwrap();
        match result {
            IntentResult::Execute { metadata, .. } => {
                assert!(metadata.detected_url.is_some());
                // The URL's path component should NOT be extracted as a file path
                assert!(metadata.detected_path.is_none());
            }
            _ => panic!("expected Execute"),
        }
    }
}

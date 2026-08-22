//! Document processor — text extraction for plain text and Markdown.
//!
//! Handles TXT and MD natively. PDF and DOCX/XLSX are deferred to plugins (P4).

use async_trait::async_trait;

use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::{DocFormat, MediaInput, MediaOutput, MediaType};

/// Document provider for plain text formats (TXT, Markdown, HTML).
///
/// For formats requiring heavy parsing (PDF, DOCX, XLSX), this provider
/// returns `UnsupportedFormat` — those should be handled by plugin providers.
pub struct TextDocumentProvider;

#[async_trait]
impl MediaProvider for TextDocumentProvider {
    fn name(&self) -> &str {
        "text-document"
    }

    fn priority(&self) -> u8 {
        10
    }

    fn supported_types(&self) -> Vec<MediaType> {
        vec![
            MediaType::Document {
                format: DocFormat::Txt,
                pages: None,
            },
            MediaType::Document {
                format: DocFormat::Markdown,
                pages: None,
            },
            MediaType::Document {
                format: DocFormat::Html,
                pages: None,
            },
        ]
    }

    fn supports(&self, media_type: &MediaType) -> bool {
        matches!(
            media_type,
            MediaType::Document {
                format: DocFormat::Txt,
                ..
            } | MediaType::Document {
                format: DocFormat::Markdown,
                ..
            } | MediaType::Document {
                format: DocFormat::Html,
                ..
            }
        )
    }

    async fn process(
        &self,
        input: &MediaInput,
        _media_type: &MediaType,
        _prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        // Shared size cap across all input variants so a hostile caller
        // cannot route around the FilePath branch's check by sending the
        // same payload as Base64.
        const MAX_TEXT_FILE_BYTES: u64 = 10 * 1024 * 1024;
        match input {
            MediaInput::FilePath { path } => {
                let meta =
                    tokio::fs::metadata(path)
                        .await
                        .map_err(|e| MediaError::ProviderError {
                            provider: "text-document".into(),
                            message: format!(
                                "Failed to read metadata for {}: {}",
                                path.display(),
                                e
                            ),
                        })?;
                if meta.len() > MAX_TEXT_FILE_BYTES {
                    return Err(MediaError::ProviderError {
                        provider: "text-document".into(),
                        message: format!(
                            "File {} is too large ({} bytes > {} bytes limit)",
                            path.display(),
                            meta.len(),
                            MAX_TEXT_FILE_BYTES
                        ),
                    });
                }
                let content = tokio::fs::read_to_string(path).await.map_err(|e| {
                    MediaError::ProviderError {
                        provider: "text-document".into(),
                        message: format!("Failed to read {}: {}", path.display(), e),
                    }
                })?;
                Ok(MediaOutput::Text { text: content })
            }
            MediaInput::Base64 { data, .. } => {
                use base64::Engine;
                // Pre-decode OOM guard: the existing MAX_TEXT_FILE_BYTES cap
                // only protects the FilePath branch. A multi-MB base64 string
                // here would peak at ~0.75× its size after decode before any
                // size check could refuse it. Apply the same 4/3 worst-case
                // expansion used in media/cache.rs::decode_data_url.
                let approx = data.len().saturating_mul(3) / 4;
                if approx as u64 > MAX_TEXT_FILE_BYTES {
                    return Err(MediaError::ProviderError {
                        provider: "text-document".into(),
                        message: format!(
                            "Base64 payload exceeds size cap before decode \
                             (encoded={}, approx decoded > {})",
                            data.len(),
                            MAX_TEXT_FILE_BYTES
                        ),
                    });
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| MediaError::ProviderError {
                        provider: "text-document".into(),
                        message: format!("Base64 decode error: {e}"),
                    })?;
                let text = String::from_utf8(bytes).map_err(|e| MediaError::ProviderError {
                    provider: "text-document".into(),
                    message: format!("UTF-8 decode error: {e}"),
                })?;
                Ok(MediaOutput::Text { text })
            }
            MediaInput::Url { .. } => Err(MediaError::ProviderError {
                provider: "text-document".into(),
                message: "URL input not supported for text documents; use web_fetch tool first"
                    .into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::MediaImageFormat;

    #[test]
    fn supports_text_formats() {
        let p = TextDocumentProvider;
        assert!(p.supports(&MediaType::Document {
            format: DocFormat::Txt,
            pages: None,
        }));
        assert!(p.supports(&MediaType::Document {
            format: DocFormat::Markdown,
            pages: None,
        }));
        assert!(p.supports(&MediaType::Document {
            format: DocFormat::Html,
            pages: None,
        }));
        assert!(!p.supports(&MediaType::Document {
            format: DocFormat::Pdf,
            pages: None,
        }));
        assert!(!p.supports(&MediaType::Document {
            format: DocFormat::Docx,
            pages: None,
        }));
        assert!(!p.supports(&MediaType::Image {
            format: MediaImageFormat::Png,
        }));
    }

    #[tokio::test]
    async fn read_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "Hello, world!").await.unwrap();

        let p = TextDocumentProvider;
        let input = MediaInput::FilePath { path: file_path };
        let mt = MediaType::Document {
            format: DocFormat::Txt,
            pages: None,
        };
        let result = p.process(&input, &mt, None).await.unwrap();
        match result {
            MediaOutput::Text { text } => assert_eq!(text, "Hello, world!"),
            _ => panic!("Expected Text output"),
        }
    }

    #[tokio::test]
    async fn read_base64_text() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode("Test content");
        let p = TextDocumentProvider;
        let input = MediaInput::Base64 {
            data: encoded,
            media_type: MediaType::Document {
                format: DocFormat::Txt,
                pages: None,
            },
        };
        let mt = MediaType::Document {
            format: DocFormat::Txt,
            pages: None,
        };
        let result = p.process(&input, &mt, None).await.unwrap();
        match result {
            MediaOutput::Text { text } => assert_eq!(text, "Test content"),
            _ => panic!("Expected Text output"),
        }
    }

    #[tokio::test]
    async fn url_input_not_supported() {
        let p = TextDocumentProvider;
        let input = MediaInput::Url {
            url: "https://example.com/file.txt".into(),
        };
        let mt = MediaType::Document {
            format: DocFormat::Txt,
            pages: None,
        };
        let err = p.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::ProviderError { .. }));
    }
}
